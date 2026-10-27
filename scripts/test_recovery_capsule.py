#!/usr/bin/env python3
"""Real GnuPG and hostile archive checks for the portable recovery boundary."""

import hashlib
import io
import json
import os
from pathlib import Path
import secrets
import shutil
import stat
import subprocess
import tarfile
import tempfile
import unittest

import recovery_capsule as capsule


class RecoveryCapsule(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if not shutil.which("gpg") or not shutil.which("gpgconf"):
            raise RuntimeError("These security checks require actual GnuPG/gpgconf")

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="freight-capsule-test-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.source = self.root / "source"
        self.source.mkdir(mode=0o700)
        (self.source / "private").mkdir(mode=0o700)
        (self.source / "private/server.key").write_bytes(
            b"synthetic-private-key\0\xff\r\n"
        )
        (self.source / "database.dump").write_bytes(bytes(range(256)) * 37)
        (self.source / "wire").write_bytes(b"#!/bin/sh\necho synthetic\n")
        (self.source / "wire").chmod(0o755)
        self.key = self.root / "separate.key"
        self.key.write_bytes(secrets.token_urlsafe(48).encode() + b"\n")
        self.key.chmod(0o600)
        self.encrypted = self.root / "capsule.gpg"
        self.output = self.root / "restored"

    def make(self):
        return capsule.create(self.source, self.encrypted, self.key, 1048576, 100)

    def restore(self, **kwargs):
        return capsule.restore(
            self.encrypted,
            self.output,
            self.key,
            kwargs.get("max_bytes", 1048576),
            kwargs.get("max_files", 100),
        )

    def refused(self, operation):
        with self.assertRaises(
            (capsule.CapsuleError, OSError, ValueError, tarfile.TarError)
        ):
            operation()
        self.assertFalse(os.path.lexists(self.output))
        self.assertFalse(list(self.root.glob(".freight-capsule-*")))

    def test_real_round_trip_restores_all_bytes_and_private_modes(self):
        result = self.make()
        self.assertEqual(result["files"], 3)
        self.assertEqual(stat.S_IMODE(self.encrypted.stat().st_mode), 0o600)
        self.restore()
        for path in self.source.rglob("*"):
            target = self.output / path.relative_to(self.source)
            if path.is_file():
                self.assertEqual(path.read_bytes(), target.read_bytes())
                self.assertEqual(
                    stat.S_IMODE(target.stat().st_mode),
                    0o700 if path.name == "wire" else 0o600,
                )
            else:
                self.assertEqual(stat.S_IMODE(target.stat().st_mode), 0o700)
        self.assertEqual(stat.S_IMODE(self.output.stat().st_mode), 0o700)
        for marker in [b"synthetic-private-key", self.key.read_bytes().strip()]:
            self.assertNotIn(marker, self.encrypted.read_bytes())

    def test_wrong_key_after_correct_restore_does_not_use_agent_cache(self):
        self.make()
        self.restore()
        shutil.rmtree(self.output)
        self.key.write_bytes(secrets.token_urlsafe(48).encode())
        self.refused(self.restore)

    def test_ciphertext_tamper_refuses_even_after_partial_decryption(self):
        self.make()
        data = bytearray(self.encrypted.read_bytes())
        data[-8] ^= 1
        self.encrypted.write_bytes(data)
        self.refused(self.restore)

    def test_truncated_ciphertext_never_publishes(self):
        self.make()
        self.encrypted.write_bytes(self.encrypted.read_bytes()[:-20])
        self.refused(self.restore)

    def test_unencrypted_archive_is_not_a_valid_capsule(self):
        self.encrypted.write_bytes(b"not an encrypted capsule")
        self.refused(self.restore)

    def test_restore_refuses_existing_output_and_keeps_its_bytes(self):
        self.make()
        self.output.mkdir()
        sentinel = self.output / "keep"
        sentinel.write_bytes(b"original")
        with self.assertRaises(capsule.CapsuleError):
            self.restore()
        self.assertEqual(sentinel.read_bytes(), b"original")

    def test_atomic_directory_publish_refuses_raced_empty_directory(self):
        staging = self.root / "staging"
        staging.mkdir()
        (staging / "payload").write_bytes(b"candidate")
        self.output.mkdir()
        with self.assertRaises(OSError):
            capsule.rename_directory_new(staging, self.output)
        self.assertTrue(staging.is_dir())
        self.assertEqual(list(self.output.iterdir()), [])

    def test_create_refuses_existing_capsule(self):
        self.encrypted.write_bytes(b"original encrypted artifact")
        with self.assertRaises(capsule.CapsuleError):
            self.make()
        self.assertEqual(self.encrypted.read_bytes(), b"original encrypted artifact")

    def test_source_link_and_special_file_are_refused_without_capsule(self):
        path = self.source / "linked"
        path.symlink_to(self.key)
        with self.assertRaises(capsule.CapsuleError):
            self.make()
        path.unlink()
        os.mkfifo(path)
        with self.assertRaises(capsule.CapsuleError):
            self.make()
        self.assertFalse(self.encrypted.exists())

    def test_key_requires_private_regular_file_and_stays_outside_payload(self):
        self.key.chmod(0o644)
        with self.assertRaises(capsule.CapsuleError):
            self.make()
        self.key.chmod(0o600)
        link = self.root / "key-link"
        link.symlink_to(self.key)
        with self.assertRaises(OSError):
            capsule.create(self.source, self.encrypted, link)
        internal = self.source / "misplaced.key"
        internal.write_bytes(self.key.read_bytes())
        internal.chmod(0o600)
        with self.assertRaises(capsule.CapsuleError):
            capsule.create(self.source, self.encrypted, internal)

    def test_size_and_file_limits_refuse_before_publication(self):
        with self.assertRaises(capsule.CapsuleError):
            capsule.create(self.source, self.encrypted, self.key, 5, 100)
        with self.assertRaises(capsule.CapsuleError):
            capsule.create(self.source, self.encrypted, self.key, 1048576, 1)
        self.make()
        self.refused(lambda: self.restore(max_bytes=5))
        self.refused(lambda: self.restore(max_files=1))

    def hostile(self, manifest, entries):
        archive = self.root / "hostile.tar"
        with tarfile.open(archive, "w", format=tarfile.USTAR_FORMAT) as stream:
            header = tarfile.TarInfo(capsule.MANIFEST)
            data = (
                manifest
                if isinstance(manifest, bytes)
                else json.dumps(manifest).encode()
            )
            header.size = len(data)
            stream.addfile(header, io.BytesIO(data))
            for name, data, kind in entries:
                header = tarfile.TarInfo(name)
                header.type = kind
                header.linkname = "../../outside"
                header.size = len(data)
                stream.addfile(header, io.BytesIO(data))
        capsule.crypt(
            "encrypt", archive, self.encrypted, capsule.private_key(self.key), 1048576
        )

    def manifest(self, names=("safe",)):
        return {
            "format": capsule.FORMAT,
            "version": 1,
            "files": {
                name: {
                    "sha256": hashlib.sha256(b"ok").hexdigest(),
                    "bytes": 2,
                    "mode": 0o600,
                }
                for name in names
            },
        }

    def test_path_traversal_is_rejected_inside_valid_encryption(self):
        self.hostile(
            self.manifest(("../../outside",)),
            [("payload/../../outside", b"ok", tarfile.REGTYPE)],
        )
        self.refused(self.restore)
        self.assertFalse((self.root.parent / "outside").exists())

    def test_symlink_member_cannot_redirect_restored_writes(self):
        self.hostile(self.manifest(), [("payload/safe", b"", tarfile.SYMTYPE)])
        self.refused(self.restore)

    def test_hardlink_member_is_refused(self):
        self.hostile(self.manifest(), [("payload/safe", b"", tarfile.LNKTYPE)])
        self.refused(self.restore)

    def test_duplicate_archive_member_is_refused(self):
        self.hostile(self.manifest(), [("payload/safe", b"ok", tarfile.REGTYPE)] * 2)
        self.refused(self.restore)

    def test_manifest_hash_mismatch_is_refused(self):
        self.hostile(self.manifest(), [("payload/safe", b"NO", tarfile.REGTYPE)])
        self.refused(self.restore)

    def test_missing_declared_file_is_refused(self):
        self.hostile(
            self.manifest(("safe", "missing")),
            [("payload/safe", b"ok", tarfile.REGTYPE)],
        )
        self.refused(self.restore)

    def test_extra_undeclared_file_is_refused(self):
        self.hostile(
            self.manifest(),
            [
                ("payload/safe", b"ok", tarfile.REGTYPE),
                ("payload/extra", b"ok", tarfile.REGTYPE),
            ],
        )
        self.refused(self.restore)

    def test_duplicate_json_manifest_keys_are_refused(self):
        raw = (
            json.dumps(self.manifest())
            .replace('"version": 1', '"version": 1, "version": 1')
            .encode()
        )
        self.hostile(raw, [("payload/safe", b"ok", tarfile.REGTYPE)])
        self.refused(self.restore)

    def test_invalid_manifest_type_is_refused(self):
        self.hostile(b"[]", [])
        self.refused(self.restore)

    def test_conflicting_parent_file_names_are_refused(self):
        self.hostile(
            self.manifest(("safe", "safe/child")),
            [("payload/safe", b"ok", tarfile.REGTYPE)],
        )
        self.refused(self.restore)

    def test_cli_generates_private_key_without_logging_it(self):
        path = self.root / "generated.key"
        result = subprocess.run(
            [
                os.sys.executable,
                str(Path(capsule.__file__)),
                "keygen",
                "--output",
                str(path),
            ],
            capture_output=True,
            check=True,
        )
        self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o600)
        self.assertNotIn(path.read_bytes().strip(), result.stdout + result.stderr)
        capsule.private_key(path)

    def limited_pipe_child(self, mode):
        code = """import fcntl,os,sys
from pathlib import Path
sys.path.insert(0,str(Path(sys.argv[1]).parent))
import recovery_capsule as c
root=Path(sys.argv[2]);mode=sys.argv[3]
original_pipe=os.pipe
def small_pipe():
 r,w=original_pipe()
 fcntl.fcntl(w,getattr(fcntl,'F_SETPIPE_SZ',1031),4096)
 assert fcntl.fcntl(w,getattr(fcntl,'F_GETPIPE_SZ',1032))==4096
 return r,w
os.pipe=small_pipe
key=root/'large.key';key.write_bytes(b'k'*4096);key.chmod(0o600)
output=root/'limited-restored';encrypted=root/'limited.gpg'
if mode=='roundtrip':
 c.create(root/'source',encrypted,key,1048576,100)
 c.restore(encrypted,output,key,1048576,100)
 for path in (root/'source').rglob('*'):
  if path.is_file():assert path.read_bytes()==(output/path.relative_to(root/'source')).read_bytes()
else:
 encrypted.write_bytes(b'not an OpenPGP message')
 try:c.restore(encrypted,output,key,1048576,100)
 except (c.CapsuleError,OSError,ValueError):pass
 else:raise AssertionError('Malformed input accepted')
 assert not output.exists()
assert not list(root.glob('.freight-capsule-*'))
"""
        result = subprocess.run(
            [os.sys.executable, "-c", code, capsule.__file__, str(self.root), mode],
            capture_output=True,
            timeout=15,
        )
        self.assertEqual(result.returncode, 0, result.stderr.decode())

    def test_maximum_key_round_trip_with_actual_four_kib_pipes(self):
        self.limited_pipe_child("roundtrip")

    def test_maximum_key_and_bad_input_do_not_deadlock_small_pipes(self):
        self.limited_pipe_child("malformed")


if __name__ == "__main__":
    unittest.main()
