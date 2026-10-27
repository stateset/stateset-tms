"""Real OpenSSL/filesystem controls; no Docker, network or deployment secrets."""

import hashlib
import importlib.util
import json
import os
import re
from pathlib import Path
import shutil
import stat
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent
VALIDATE = ROOT / "scripts/validate-postgres-tls.sh"
INSTALL = ROOT / "scripts/install-postgres-tls.sh"


class QualificationSubnet(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        spec = importlib.util.spec_from_file_location(
            'qualification_subnet_roles', ROOT / 'scripts/qualify-authenticated-roles.py')
        cls.roles = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(cls.roles)

    def test_small_private_network_outside_existing_allocations_is_accepted(self):
        self.assertEqual(self.roles.private_docker_subnet(
            '10.252.240.0/24', ['172.16.0.0/12', '192.168.0.0/16', 'fd00::/64']),
            '10.252.240.0/24')

    def test_overlap_with_equal_parent_or_child_docker_network_is_refused(self):
        for existing in ['10.252.240.0/24', '10.252.0.0/16', '10.252.240.64/26']:
            with self.subTest(existing=existing), self.assertRaisesRegex(ValueError, 'overlaps'):
                self.roles.private_docker_subnet('10.252.240.0/24', [existing])

    def test_public_link_local_documentation_and_ipv6_networks_are_refused(self):
        for network in ['8.8.8.0/24', '169.254.10.0/24', '192.0.2.0/24', 'fd00::/64']:
            with self.subTest(network=network), self.assertRaises(ValueError):
                self.roles.private_docker_subnet(network, [])

    def test_noncanonical_and_oversized_or_too_small_networks_are_refused(self):
        for network in ['10.1.0.1/24', '10.1.0.0/16', '10.1.0.0/29', 'not-a-network']:
            with self.subTest(network=network), self.assertRaises(ValueError):
                self.roles.private_docker_subnet(network, [])

    def test_all_three_rfc1918_ranges_and_supported_prefix_limits(self):
        for network in ['10.1.1.0/28', '172.16.1.0/24', '192.168.1.0/27']:
            with self.subTest(network=network):
                self.assertEqual(self.roles.private_docker_subnet(network, []), network)


class RuntimeRoleSchemaFence(unittest.TestCase):
    def test_role_job_requires_the_complete_current_migration_ledger(self):
        versions = sorted(int(p.name.split('_', 1)[0]) for p in
                          (ROOT / 'migrations').glob('[0-9]*_*.sql'))
        self.assertEqual(versions, list(range(1, max(versions) + 1)))
        text = (ROOT / 'scripts/configure-runtime-role.sql').read_text()
        fence = re.search(r'count\(\*\) <> (\d+) OR min\(version\) <> (\d+) OR max\(version\) <> (\d+)', text)
        self.assertIsNotNone(fence, 'Retain an exact migration-ledger fence')
        self.assertEqual(tuple(map(int, fence.groups())),
                         (len(versions), min(versions), max(versions)))


class PostgresTLS(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temp = tempfile.TemporaryDirectory(prefix="freight-tls-certificates-")
        cls.pki = Path(cls.temp.name)
        cls.env = {"PATH": os.defpath, "LC_ALL": "C"}
        cls.commands = 0
        artifact = os.environ.get("FREIGHT_TLS_TEST_ARTIFACTS")
        cls.artifact = Path(artifact) if artifact else None
        if cls.artifact:
            cls.artifact.mkdir(mode=0o700)

        def openssl(*args):
            result = subprocess.run(
                ["openssl", *map(str, args)],
                cwd=cls.pki,
                env=cls.env,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                timeout=30,
            )
            if result.returncode:
                raise RuntimeError(result.stderr)

        cls.openssl = staticmethod(openssl)
        (cls.pki / "ca.cnf").write_text(
            "[req]\ndistinguished_name=dn\nx509_extensions=ca\n[dn]\n[ca]\nbasicConstraints=critical,CA:true\nkeyUsage=critical,keyCertSign,cRLSign\nsubjectKeyIdentifier=hash\n"
        )
        for name in ["ca", "other-ca"]:
            openssl(
                "req",
                "-new",
                "-x509",
                "-newkey",
                "rsa:2048",
                "-nodes",
                "-days",
                "3",
                "-config",
                "ca.cnf",
                "-subj",
                "/CN=Owned-" + name,
                "-keyout",
                name + ".key",
                "-out",
                name + ".crt",
            )
        for name in ["leaf", "next", "intermediate"]:
            openssl(
                "req",
                "-new",
                "-newkey",
                "rsa:2048",
                "-nodes",
                "-subj",
                "/CN=postgres",
                "-keyout",
                name + ".key",
                "-out",
                name + ".csr",
            )
        ext = {
            "server": "basicConstraints=critical,CA:false\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectAltName=DNS:postgres\n",
            "wrong-host": "basicConstraints=critical,CA:false\nextendedKeyUsage=serverAuth\nsubjectAltName=DNS:other.example.test\n",
            "client": "basicConstraints=critical,CA:false\nextendedKeyUsage=clientAuth\nsubjectAltName=DNS:postgres\n",
            "no-san": "basicConstraints=critical,CA:false\nextendedKeyUsage=serverAuth\n",
            "intermediate": "basicConstraints=critical,CA:true,pathlen:0\nkeyUsage=critical,keyCertSign,cRLSign\nsubjectKeyIdentifier=hash\nauthorityKeyIdentifier=keyid,issuer\n",
        }
        for name, value in ext.items():
            (cls.pki / (name + ".ext")).write_text(value)
        for name, csr, extensions in [
            ("server", "leaf", "server"),
            ("next", "next", "server"),
            ("wrong-host", "leaf", "wrong-host"),
            ("client", "leaf", "client"),
            ("no-san", "leaf", "no-san"),
            ("intermediate", "intermediate", "intermediate"),
        ]:
            openssl(
                "x509",
                "-req",
                "-in",
                csr + ".csr",
                "-CA",
                "ca.crt",
                "-CAkey",
                "ca.key",
                "-CAcreateserial",
                "-days",
                "2",
                "-extfile",
                extensions + ".ext",
                "-out",
                name + ".crt",
            )
        openssl(
            "x509",
            "-req",
            "-in",
            "leaf.csr",
            "-CA",
            "intermediate.crt",
            "-CAkey",
            "intermediate.key",
            "-CAcreateserial",
            "-days",
            "2",
            "-extfile",
            "server.ext",
            "-out",
            "intermediate-leaf.crt",
        )
        (cls.pki / "chain.crt").write_bytes(
            (cls.pki / "intermediate-leaf.crt").read_bytes()
            + (cls.pki / "intermediate.crt").read_bytes()
        )
        (cls.pki / "index").write_text("")
        (cls.pki / "serial").write_text("1000\n")
        (cls.pki / "issuer.cnf").write_text(
            "[ca]\ndefault_ca=issuer\n[issuer]\ndatabase=index\nserial=serial\nnew_certs_dir=.\ncertificate=ca.crt\nprivate_key=ca.key\ndefault_md=sha256\npolicy=policy\nunique_subject=no\n[policy]\ncommonName=supplied\n[server]\nbasicConstraints=critical,CA:false\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectAltName=DNS:postgres\n"
        )
        for name, start, end in [
            ("expired", "20200101000000Z", "20200102000000Z"),
            ("future", "20350101000000Z", "20350102000000Z"),
        ]:
            openssl(
                "ca",
                "-batch",
                "-config",
                "issuer.cnf",
                "-extensions",
                "server",
                "-in",
                "leaf.csr",
                "-startdate",
                start,
                "-enddate",
                end,
                "-out",
                name + ".crt",
            )
        openssl(
            "pkey",
            "-in",
            "leaf.key",
            "-aes-256-cbc",
            "-passout",
            "pass:owned-test-password",
            "-out",
            "encrypted.key",
        )
        (cls.pki / "corrupt.crt").write_text("not a certificate\n")
        if cls.artifact:
            public = cls.artifact / "public-certificates"
            public.mkdir()
            for file in cls.pki.glob("*.crt"):
                shutil.copyfile(file, public / file.name)

    @classmethod
    def tearDownClass(cls):
        cls.temp.cleanup()

    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix="freight-tls-publication-")
        self.addCleanup(self.directory.cleanup)
        self.destination = Path(self.directory.name) / "tls"
        self.destination.mkdir()

    def invoke(
        self,
        *,
        installer=False,
        cert="server.crt",
        key="leaf.key",
        ca="ca.crt",
        hostname="postgres",
        env=None,
        extra=None,
    ):
        def path(name):
            return Path(name) if Path(name).is_absolute() else self.pki / name

        if installer:
            command = [
                "bash",
                str(INSTALL),
                "--cert",
                str(path(cert)),
                "--key",
                str(path(key)),
                "--ca",
                str(path(ca)),
                "--hostname",
                hostname,
                "--destination",
                str(self.destination),
                "--owner",
                f"{os.getuid()}:{os.getgid()}",
                *(extra or []),
            ]
        else:
            command = [
                "bash",
                str(VALIDATE),
                str(path(cert)),
                str(path(key)),
                str(path(ca)),
                hostname,
            ]
        result = subprocess.run(
            command,
            env=env or self.env,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            timeout=15,
        )
        type(self).commands += 1
        if self.artifact:
            item = {
                "test": self.id(),
                "installer": installer,
                "certificate": Path(cert).name,
                "key": Path(key).name,
                "ca": Path(ca).name,
                "hostname": hostname,
                "exit_code": result.returncode,
                "stdout": result.stdout,
                "stderr": result.stderr,
            }
            with (self.artifact / f"invocation-{self.commands:03d}.json").open(
                "x"
            ) as out:
                json.dump(item, out, indent=2)
                out.write("\n")
        return result

    def accepted(self, **args):
        result = self.invoke(**args)
        self.assertEqual(result.returncode, 0, result.stderr)

    def refused(self, **args):
        result = self.invoke(**args)
        self.assertNotEqual(result.returncode, 0, result.stdout)
        return result

    def snapshot(self):
        result = {}
        for path in sorted(self.destination.rglob("*")):
            info = path.lstat()
            item = {
                "mode": stat.S_IMODE(info.st_mode),
                "uid": info.st_uid,
                "gid": info.st_gid,
                "mtime_ns": info.st_mtime_ns,
            }
            if path.is_symlink():
                item["target"] = os.readlink(path)
            elif path.is_file():
                item["sha256"] = hashlib.sha256(path.read_bytes()).hexdigest()
            result[str(path.relative_to(self.destination))] = item
        return result

    def test_valid_server_chain_and_key(self):
        self.accepted()

    def test_complete_intermediate_chain(self):
        self.accepted(cert="chain.crt")

    def test_incomplete_intermediate_chain_refuses(self):
        self.refused(cert="intermediate-leaf.crt")

    def test_untrusted_authority_refuses(self):
        self.refused(ca="other-ca.crt")

    def test_wrong_hostname_refuses(self):
        self.refused(cert="wrong-host.crt")

    def test_common_name_without_dns_san_refuses(self):
        self.refused(cert="no-san.crt")

    def test_expired_certificate_refuses(self):
        self.refused(cert="expired.crt")

    def test_not_yet_valid_certificate_refuses(self):
        self.refused(cert="future.crt")

    def test_client_only_purpose_refuses(self):
        self.refused(cert="client.crt")

    def test_mismatched_private_key_refuses(self):
        self.refused(key="next.key")

    def test_corrupt_certificate_refuses(self):
        self.refused(cert="corrupt.crt")

    def test_encrypted_private_key_refuses_without_prompt(self):
        self.refused(key="encrypted.key")

    def test_symlink_inputs_refuse(self):
        for argument, filename in [
            ("cert", "server.crt"),
            ("key", "leaf.key"),
            ("ca", "ca.crt"),
        ]:
            with self.subTest(argument=argument):
                link = Path(self.directory.name) / (argument + "-link")
                link.symlink_to(self.pki / filename)
                self.refused(**{argument: str(link)})

    def test_published_generation_has_private_key_and_expected_owner(self):
        self.accepted(installer=True)
        current = self.destination / "current"
        self.assertTrue(current.is_symlink())
        target = current.resolve()
        self.assertEqual(target.parent, self.destination)
        for name, mode in [
            ("server.crt", 0o644),
            ("ca.crt", 0o644),
            ("server.key", 0o600),
        ]:
            info = (target / name).stat()
            self.assertEqual(stat.S_IMODE(info.st_mode), mode)
            self.assertEqual((info.st_uid, info.st_gid), (os.getuid(), os.getgid()))
        self.assertEqual(stat.S_IMODE(target.stat().st_mode), 0o700)
        self.assertEqual(
            (target / "server.key").read_bytes(), (self.pki / "leaf.key").read_bytes()
        )

    def test_rotation_publishes_complete_new_generation_and_retains_previous(self):
        self.accepted(installer=True)
        previous = (self.destination / "current").resolve()
        original = (previous / "server.key").read_bytes()
        self.accepted(installer=True, cert="next.crt", key="next.key")
        current = (self.destination / "current").resolve()
        self.assertNotEqual(current, previous)
        self.assertEqual((previous / "server.key").read_bytes(), original)
        self.assertEqual(
            (current / "server.key").read_bytes(), (self.pki / "next.key").read_bytes()
        )
        self.assertEqual(
            (current / "server.crt").read_bytes(), (self.pki / "next.crt").read_bytes()
        )

    def test_rejected_rotation_retains_exact_previous_publication(self):
        self.accepted(installer=True)
        before = self.snapshot()
        for args in [
            {"key": "next.key"},
            {"cert": "expired.crt"},
            {"cert": "wrong-host.crt"},
            {"ca": "other-ca.crt"},
        ]:
            with self.subTest(args=args):
                self.refused(installer=True, **args)
                self.assertEqual(self.snapshot(), before)

    def test_owner_failure_cleans_staging_and_preserves_previous(self):
        self.accepted(installer=True)
        before = self.snapshot()
        self.refused(
            installer=True,
            cert="next.crt",
            key="next.key",
            extra=["--owner", "owned-no-such-tls-user-20261018"],
        )
        self.assertEqual(self.snapshot(), before)

    def test_unmanaged_current_directory_refuses(self):
        (self.destination / "current").mkdir()
        (self.destination / "current/owned-canary").write_text("retain original")
        before = self.snapshot()
        self.refused(installer=True)
        self.assertEqual(self.snapshot(), before)

    def test_escaping_publication_symlink_refuses(self):
        (self.destination / "current").symlink_to("../outside")
        before = self.snapshot()
        self.refused(installer=True)
        self.assertEqual(self.snapshot(), before)

    def test_symlink_destination_refuses(self):
        original = self.destination
        self.destination = Path(self.directory.name) / "tls-link"
        self.destination.symlink_to(original, target_is_directory=True)
        self.refused(installer=True)
        self.assertEqual(list(original.iterdir()), [])

    def test_changed_source_key_after_validation_cannot_publish_mixed_pair(self):
        self.accepted(installer=True)
        before = self.snapshot()
        key = Path(self.directory.name) / "raced.key"
        shutil.copyfile(self.pki / "leaf.key", key)
        wrapper = Path(self.directory.name) / "commands"
        wrapper.mkdir()
        # Change only this owned key after the certificate copy. The production
        # installer must catch the real mixed staged pair before publication.
        command = wrapper / "install"
        command.write_text(
            '#!/usr/bin/env bash\nset -e\n/usr/bin/install "$@"\ncase "${!#}" in */server.crt) /bin/cp -- "$OWNED_REPLACEMENT_KEY" "$OWNED_RACED_KEY" ;; esac\n'
        )
        command.chmod(0o755)
        env = {
            **self.env,
            "PATH": str(wrapper) + ":" + os.defpath,
            "OWNED_REPLACEMENT_KEY": str(self.pki / "next.key"),
            "OWNED_RACED_KEY": str(key),
        }
        self.refused(installer=True, key=str(key), env=env)
        self.assertEqual(self.snapshot(), before)

    def test_signal_after_atomic_publication_retains_complete_generation(self):
        wrapper = Path(self.directory.name) / "commands"
        wrapper.mkdir()
        command = wrapper / "mv"
        # This child signals its own installer parent after the actual rename.
        command.write_text(
            '#!/usr/bin/env bash\nset -e\n/usr/bin/mv "$@"\nkill -TERM "$PPID"\n'
        )
        command.chmod(0o755)
        result = self.refused(
            installer=True, env={**self.env, "PATH": str(wrapper) + ":" + os.defpath}
        )
        self.assertEqual(result.returncode, 143)
        current = (self.destination / "current").resolve()
        self.assertTrue(current.is_dir())
        self.assertEqual(
            (current / "server.key").read_bytes(), (self.pki / "leaf.key").read_bytes()
        )
        self.assertEqual(
            (current / "server.crt").read_bytes(),
            (self.pki / "server.crt").read_bytes(),
        )
        self.assertFalse(list(self.destination.glob(".current-*")))


if __name__ == "__main__":
    unittest.main()
