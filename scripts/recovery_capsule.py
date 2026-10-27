#!/usr/bin/env python3
"""Encrypt a recovery directory, or validate and privately publish a capsule.

Uses GnuPG's AES256/CFB+MDC format, with no password in argv/environment or
shared agent cache. No network, database or Docker operations are performed.
"""

import argparse
import ctypes
import datetime as dt
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import secrets
import selectors
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import time

FORMAT = "stateset-freight-recovery-capsule"
MANIFEST = "capsule-manifest.json"
DEFAULT_BYTES = 8 * 1024**3
DEFAULT_FILES = 100000


class CapsuleError(ValueError):
    pass


def require(condition, message):
    if not condition:
        raise CapsuleError(message)


def sha(path):
    value = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for chunk in iter(lambda: stream.read(1048576), b""):
            value.update(chunk)
    return value.hexdigest()


def sync_directory(path):
    descriptor = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def private_key(path):
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(descriptor, "rb") as stream:
        info = os.fstat(stream.fileno())
        require(
            stat.S_ISREG(info.st_mode)
            and info.st_uid == os.getuid()
            and stat.S_IMODE(info.st_mode) & 0o077 == 0,
            "Key must be an owned private regular file",
        )
        value = stream.read(4098)
    if value.endswith(b"\n"):
        value = value[:-1]
    require(
        32 <= len(value) <= 4096
        and not any(byte in value for byte in [b"\n", b"\r", b"\0"]),
        "Key must contain one 32–4096 byte line",
    )
    return value


def destination(path):
    path = Path(path).absolute()
    require(path.parent.is_dir(), "Output parent must already exist")
    require(not os.path.lexists(path), "Refusing an existing output")
    return path.parent.resolve() / path.name


def limits(max_bytes, max_files):
    require(type(max_bytes) is int and max_bytes > 0, "Invalid byte limit")
    require(type(max_files) is int and 0 < max_files <= 1000000, "Invalid file limit")


def file_name(value):
    require(
        isinstance(value, str) and value and "\\" not in value and "\0" not in value,
        "Invalid archive name",
    )
    path = PurePosixPath(value)
    require(
        not path.is_absolute()
        and str(path) == value
        and all(part not in [".", ".."] for part in path.parts),
        "Unsafe archive name",
    )
    return path


def json_unique(pairs):
    output = {}
    for key, value in pairs:
        require(key not in output, "Duplicate manifest key")
        output[key] = value
    return output


def crypt(operation, source, target, key, max_output, timeout=180):
    """Bound output and diagnostics; publish nothing until GPG integrity succeeds."""
    require(operation in ["encrypt", "decrypt"], "Invalid crypto operation")
    require(
        shutil.which("gpg") and shutil.which("gpgconf"),
        "GnuPG and gpgconf are required",
    )
    with tempfile.TemporaryDirectory(prefix="freight-capsule-gpg-") as home:
        read_fd, write_fd = os.pipe()
        process = None
        try:
            deadline = time.monotonic() + timeout
            command = [
                "gpg",
                "--no-options",
                "--homedir",
                home,
                "--batch",
                "--no-tty",
                "--no-symkey-cache",
                "--rfc4880",
                "--pinentry-mode",
                "loopback",
                "--passphrase-fd",
                str(read_fd),
                "--status-fd",
                "2",
                "--output",
                "-",
            ]
            if operation == "encrypt":
                command += [
                    "--symmetric",
                    "--cipher-algo",
                    "AES256",
                    "--compress-algo",
                    "none",
                    "--s2k-mode",
                    "3",
                    "--s2k-count",
                    "65011712",
                ]
            else:
                command += ["--decrypt"]
            command += ["--", str(source)]
            process = subprocess.Popen(
                command,
                pass_fds=(read_fd,),
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                env={"PATH": os.defpath, "LC_ALL": "C"},
            )
            os.close(read_fd)
            read_fd = None
            os.set_blocking(write_fd, False)
            key_bytes = memoryview(key + b"\n")
            key_offset = 0
            diagnostics = bytearray()
            count = 0
            with selectors.DefaultSelector() as selector, Path(target).open(
                "xb"
            ) as stream:
                os.chmod(target, 0o600)
                selector.register(process.stdout, selectors.EVENT_READ, "output")
                selector.register(process.stderr, selectors.EVENT_READ, "status")
                selector.register(write_fd, selectors.EVENT_WRITE, "key")
                while selector.get_map():
                    require(time.monotonic() < deadline, "GnuPG operation timed out")
                    for item, _ in selector.select(
                        timeout=min(1, max(0, deadline - time.monotonic()))
                    ):
                        if item.data == "key":
                            # A valid maximum-length key can exceed a small
                            # pipe's capacity. Feed it while draining GPG's
                            # output/status, under the same operation deadline.
                            try:
                                written = os.write(item.fd, key_bytes[key_offset:])
                            except BlockingIOError:
                                continue
                            require(written > 0, "GnuPG key transfer failed")
                            key_offset += written
                            if key_offset == len(key_bytes):
                                selector.unregister(write_fd)
                                os.close(write_fd)
                                write_fd = None
                            continue
                        raw = os.read(item.fd, 65536)
                        if not raw:
                            selector.unregister(item.fileobj)
                        elif item.data == "output":
                            count += len(raw)
                            require(
                                count <= max_output, "Capsule exceeds output byte limit"
                            )
                            stream.write(raw)
                        else:
                            diagnostics.extend(raw)
                            require(
                                len(diagnostics) <= 1048576,
                                "GnuPG diagnostic limit exceeded",
                            )
                code = process.wait(timeout=max(0.01, deadline - time.monotonic()))
                stream.flush()
                os.fsync(stream.fileno())
            statuses = {
                line.split()[1]
                for line in bytes(diagnostics).splitlines()
                if line.startswith(b"[GNUPG:] ") and len(line.split()) >= 2
            }
            required = (
                {b"END_ENCRYPTION"}
                if operation == "encrypt"
                else {b"DECRYPTION_OKAY", b"GOODMDC"}
            )
            require(
                code == 0 and required.issubset(statuses),
                "GnuPG encryption or integrity verification failed",
            )
            if operation == "decrypt":
                require(
                    any(
                        line.split()[1:4] == [b"DECRYPTION_INFO", b"2", b"9"]
                        and line.split()[4:] in [[], [b"0"]]
                        for line in bytes(diagnostics).splitlines()
                        if line.startswith(b"[GNUPG:] ")
                    ),
                    "Capsule requires AES256 and modification detection",
                )
        finally:
            if read_fd is not None:
                os.close(read_fd)
            if write_fd is not None:
                os.close(write_fd)
            if process is not None:
                if process.poll() is None:
                    process.kill()
                process.wait(timeout=10)
                process.stdout.close()
                process.stderr.close()
            subprocess.run(
                ["gpgconf", "--homedir", home, "--kill", "gpg-agent"],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                timeout=10,
                check=False,
            )


def create(source, output, key_file, max_bytes=DEFAULT_BYTES, max_files=DEFAULT_FILES):
    limits(max_bytes, max_files)
    source = Path(source).absolute()
    require(
        not source.is_symlink() and source.is_dir(), "Source must be a real directory"
    )
    source = source.resolve()
    output = destination(output)
    key_file = Path(key_file).absolute()
    for path in [output, key_file.resolve()]:
        require(
            source != path and source not in path.parents,
            "Output and key must be outside the source",
        )
    key = private_key(key_file)
    files = {}
    total = 0
    for path in sorted(source.rglob("*")):
        info = path.lstat()
        require(not stat.S_ISLNK(info.st_mode), "Source links are not supported")
        if stat.S_ISDIR(info.st_mode):
            continue
        require(stat.S_ISREG(info.st_mode), "Source contains a nonregular file")
        name = str(path.relative_to(source))
        file_name(name)
        total += info.st_size
        require(
            total <= max_bytes and len(files) < max_files,
            "Source exceeds capsule limits",
        )
        files[name] = {
            "sha256": sha(path),
            "bytes": info.st_size,
            "mode": 0o600 | (info.st_mode & 0o100),
        }
    require(files, "Source is empty")
    manifest = {
        "format": FORMAT,
        "version": 1,
        "created_utc": dt.datetime.now(dt.timezone.utc).isoformat(),
        "files": files,
    }
    raw_manifest = (
        json.dumps(manifest, sort_keys=True, separators=(",", ":")) + "\n"
    ).encode()
    with tempfile.TemporaryDirectory(
        prefix=".freight-capsule-", dir=output.parent
    ) as folder:
        archive = Path(folder) / "payload.tar"
        with tarfile.open(archive, "w", format=tarfile.USTAR_FORMAT) as stream:
            header = tarfile.TarInfo(MANIFEST)
            header.size, header.mode = len(raw_manifest), 0o600
            stream.addfile(header, io.BytesIO(raw_manifest))
            for name, expected in files.items():
                path = source / name
                descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
                with os.fdopen(descriptor, "rb") as value:
                    info = os.fstat(value.fileno())
                    require(
                        stat.S_ISREG(info.st_mode)
                        and info.st_size == expected["bytes"],
                        "Source changed while packing",
                    )
                    header = tarfile.TarInfo("payload/" + name)
                    header.size, header.mode = info.st_size, expected["mode"]
                    stream.addfile(header, value)
                require(sha(path) == expected["sha256"], "Source changed while packing")
        # Validate the actual retained bytes before encrypting, including the
        # opened-descriptor content if a same-size file was replaced mid-copy.
        validate_archive(archive, None, max_bytes, max_files)
        encrypted = Path(folder) / "capsule.gpg"
        crypt("encrypt", archive, encrypted, key, archive.stat().st_size + 1048576)
        os.link(encrypted, output)
        sync_directory(output.parent)
    return {
        "format": FORMAT,
        "version": 1,
        "files": len(files),
        "plaintext_bytes": total,
        "capsule_sha256": sha(output),
        "capsule_bytes": output.stat().st_size,
    }


def validate_archive(archive, target, max_bytes, max_files):
    with tarfile.open(archive, "r:") as stream:
        entry = stream.next()
        require(
            entry is not None
            and entry.name == MANIFEST
            and entry.isfile()
            and entry.size <= min(64 * 1024**2, max_files * 1024 + 4096),
            "Invalid capsule manifest",
        )
        manifest = json.loads(
            stream.extractfile(entry).read(), object_pairs_hook=json_unique
        )
        require(
            isinstance(manifest, dict)
            and manifest.get("format") == FORMAT
            and type(manifest.get("version")) is int
            and manifest["version"] == 1,
            "Unsupported capsule format",
        )
        files = manifest.get("files")
        require(
            isinstance(files, dict) and 0 < len(files) <= max_files,
            "Invalid capsule file count",
        )
        total = 0
        for name, value in files.items():
            path = file_name(name)
            require(
                isinstance(value, dict) and set(value) == {"sha256", "bytes", "mode"},
                "Invalid file metadata",
            )
            require(
                type(value["bytes"]) is int
                and value["bytes"] >= 0
                and type(value["mode"]) is int
                and value["mode"] in [0o600, 0o700],
                "Invalid file size or permissions",
            )
            require(
                isinstance(value["sha256"], str) and re_full_sha(value["sha256"]),
                "Invalid file hash",
            )
            require(
                not any(
                    str(parent) in files
                    for parent in path.parents
                    if str(parent) != "."
                ),
                "Conflicting file paths",
            )
            total += value["bytes"]
        require(total <= max_bytes, "Capsule exceeds plaintext byte limit")
        seen = set()
        while True:
            entry = stream.next()
            if entry is None:
                break
            require(
                entry.isfile() and entry.name.startswith("payload/"),
                "Unsupported archive member",
            )
            name = entry.name[len("payload/") :]
            file_name(name)
            require(
                name in files and name not in seen,
                "Unexpected or duplicate archive member",
            )
            expected = files[name]
            require(
                entry.size == expected["bytes"], "Archive size differs from manifest"
            )
            value = hashlib.sha256()
            destination_file = None
            try:
                if target is not None:
                    path = target / name
                    directory = target
                    for part in PurePosixPath(name).parts[:-1]:
                        directory = directory / part
                        directory.mkdir(mode=0o700, exist_ok=True)
                    destination_file = path.open("xb")
                    os.chmod(path, expected["mode"])
                with stream.extractfile(entry) as incoming:
                    for chunk in iter(lambda: incoming.read(1048576), b""):
                        value.update(chunk)
                        if destination_file is not None:
                            destination_file.write(chunk)
                require(
                    value.hexdigest() == expected["sha256"],
                    "Archive hash differs from manifest",
                )
            finally:
                if destination_file is not None:
                    destination_file.flush()
                    os.fsync(destination_file.fileno())
                    destination_file.close()
            seen.add(name)
        require(seen == set(files), "Capsule is missing a declared file")
        require(
            Path(archive).stat().st_size - stream.offset >= 1024,
            "Missing archive terminator",
        )
        stream.fileobj.seek(stream.offset)
        for raw in iter(lambda: stream.fileobj.read(1048576), b""):
            require(not any(raw), "Unexpected data after archive terminator")
    return {"files": len(files), "plaintext_bytes": total}


def re_full_sha(value):
    return len(value) == 64 and all(
        character in "0123456789abcdef" for character in value
    )


def rename_directory_new(source, output):
    # Linux renameat2 supplies atomic no-replace semantics for directories.
    # Refuse unsupported platforms instead of replacing an existing directory.
    library = ctypes.CDLL(None, use_errno=True)
    function = getattr(library, "renameat2", None)
    require(
        function is not None, "Atomic directory publication requires Linux renameat2"
    )
    function.argtypes = [
        ctypes.c_int,
        ctypes.c_char_p,
        ctypes.c_int,
        ctypes.c_char_p,
        ctypes.c_uint,
    ]
    function.restype = ctypes.c_int
    if function(-100, os.fsencode(source), -100, os.fsencode(output), 1) != 0:
        error = ctypes.get_errno()
        raise OSError(error, "Refusing to replace restore destination")


def restore(
    capsule, output, key_file, max_bytes=DEFAULT_BYTES, max_files=DEFAULT_FILES
):
    limits(max_bytes, max_files)
    output = destination(output)
    key = private_key(key_file)
    with tempfile.TemporaryDirectory(
        prefix=".freight-capsule-", dir=output.parent
    ) as folder:
        archive = Path(folder) / "payload.tar"
        # The raw archive bound includes tar headers/padding and bounded JSON.
        crypt(
            "decrypt",
            Path(capsule).resolve(),
            archive,
            key,
            max_bytes + max_files * 2048 + 10485760,
        )
        extracted = Path(folder) / "validated"
        extracted.mkdir(mode=0o700)
        result = validate_archive(archive, extracted, max_bytes, max_files)
        for path in sorted(extracted.rglob("*"), reverse=True):
            if path.is_dir():
                sync_directory(path)
        sync_directory(extracted)
        rename_directory_new(extracted, output)
        sync_directory(output.parent)
    return {"format": FORMAT, "version": 1, **result, "capsule_sha256": sha(capsule)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="operation", required=True)
    generate = commands.add_parser(
        "keygen", help="Write a new separate private recovery key"
    )
    generate.add_argument("--output", type=Path, required=True)
    for operation, input_name in [("create", "source"), ("restore", "capsule")]:
        command = commands.add_parser(operation)
        command.add_argument("--" + input_name, type=Path, required=True)
        command.add_argument("--output", type=Path, required=True)
        command.add_argument("--key-file", type=Path, required=True)
        command.add_argument("--max-bytes", type=int, default=DEFAULT_BYTES)
        command.add_argument("--max-files", type=int, default=DEFAULT_FILES)
    args = parser.parse_args()
    os.umask(0o077)
    try:
        if args.operation == "keygen":
            path = destination(args.output)
            with path.open("xb") as stream:
                stream.write(secrets.token_urlsafe(48).encode() + b"\n")
                stream.flush()
                os.fsync(stream.fileno())
            os.chmod(path, 0o600)
            sync_directory(path.parent)
            result = {"result": "private_key_created", "key_contents_logged": False}
        else:
            inputs = args.source if args.operation == "create" else args.capsule
            result = globals()[args.operation](
                inputs, args.output, args.key_file, args.max_bytes, args.max_files
            )
        print(json.dumps({"result": "pass", **result}, sort_keys=True))
        return 0
    except (
        CapsuleError,
        OSError,
        ValueError,
        tarfile.TarError,
        subprocess.SubprocessError,
    ):
        # Archive and GPG errors may contain sensitive names. Keep them private.
        print(
            "Recovery capsule operation failed; no unvalidated restore was published.",
            file=sys.stderr,
        )
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
