"""Ownership and failure boundaries for the optional native runtime backend."""

import os
from pathlib import Path
import signal
import shutil
import subprocess
import tempfile
import unittest
from types import SimpleNamespace
from unittest.mock import Mock, patch

from native_postgres import NativePostgres
from runtime_contracts import Runtime, fingerprint


class NativePostgresTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="freight-native-test-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.bin = self.root / 'bin'
        self.bin.mkdir()
        for name in ('initdb', 'postgres'):
            path = self.bin / name
            path.write_text('#!/bin/sh\nexit 99\n')
            path.chmod(0o700)
        self.owned = self.root / 'owned'
        self.owned.mkdir(mode=0o700)
        self.pg = NativePostgres(self.owned, self.bin, 15432)

    def test_refuses_shared_and_symlinked_directories(self):
        self.owned.chmod(0o755)
        with self.assertRaisesRegex(ValueError, 'private owned'):
            NativePostgres(self.owned, self.bin, 15432)
        self.owned.chmod(0o700)
        link = self.root / 'link'
        link.symlink_to(self.owned)
        with self.assertRaisesRegex(ValueError, 'symlink'):
            NativePostgres(link, self.bin, 15432)

    def test_runtime_fingerprint_detects_a_changed_vendored_dependency(self):
        dependency = self.owned / 'vendor/dependency/src/lib.rs'
        dependency.parent.mkdir(parents=True)
        dependency.write_text('pub const FIXED: bool = true;\n')
        original = fingerprint(self.owned)
        dependency.write_text('pub const FIXED: bool = false;\n')
        self.assertNotEqual(original['sha256'], fingerprint(self.owned)['sha256'])

    def test_refuses_privileged_invalid_and_boolean_ports(self):
        for port in (True, '15432', 1023, 65536):
            with self.subTest(port=port), self.assertRaises(ValueError):
                NativePostgres(self.owned, self.bin, port)

    def test_refuses_missing_or_nonexecutable_tools(self):
        (self.bin / 'initdb').chmod(0o600)
        with self.assertRaisesRegex(ValueError, 'executable'):
            NativePostgres(self.owned, self.bin, 15432)

    def test_never_adopts_existing_cluster_or_dangling_socket(self):
        for path in (self.pg.data, self.pg.socket):
            path.symlink_to(self.root / 'absent')
            with self.assertRaisesRegex(ValueError, 'never reuses'):
                self.pg.start()
            path.unlink()

    def test_refuses_root_before_invoking_tools(self):
        with patch('native_postgres.os.geteuid', return_value=0), \
                patch('native_postgres.subprocess.run') as run:
            with self.assertRaisesRegex(ValueError, 'non-root'):
                self.pg.start()
            run.assert_not_called()

    def test_refuses_other_postgres_major_before_initialization(self):
        with patch('native_postgres.os.geteuid', return_value=1000), \
                patch('native_postgres.subprocess.run', return_value=SimpleNamespace(
                    stdout='postgres (PostgreSQL) 17.2\n')) as run:
            with self.assertRaisesRegex(ValueError, 'PostgreSQL 16'):
                self.pg.start()
            self.assertEqual(run.call_count, 1)
            self.assertFalse(self.pg.data.exists())

    def start_fake(self, initdb_version='16'):
        def run(command, **kwargs):
            self.assertEqual(kwargs['env'], {'PATH': os.defpath, 'LC_ALL': 'C'})
            if command[-1] == '--version':
                return SimpleNamespace(stdout='postgres (PostgreSQL) 16.15\n')
            self.pg.data.mkdir(mode=0o700)
            (self.pg.data / 'PG_VERSION').write_text(initdb_version)
            return SimpleNamespace(returncode=0)
        with patch('native_postgres.os.geteuid', return_value=1000), \
                patch('native_postgres.subprocess.run', side_effect=run), \
                patch('native_postgres.subprocess.Popen') as popen:
            self.pg.start()
        return popen

    def test_initialization_checks_major_and_never_starts_mismatched_server(self):
        with self.assertRaisesRegex(RuntimeError, 'versions differ'):
            self.start_fake('15')
        self.assertIsNone(self.pg.process)

    def test_explicit_loopback_and_clean_environment(self):
        with patch.dict(os.environ, {'PGHOST': 'production.invalid', 'PGPORT': '5432'}):
            popen = self.start_fake()
        cfg = (self.pg.data / 'postgresql.auto.conf').read_text()
        self.assertIn("listen_addresses = '127.0.0.1'", cfg)
        self.assertIn('port = 15432', cfg)
        self.assertNotIn('production.invalid', cfg)
        self.assertEqual(popen.call_args.kwargs['env'], {'PATH': os.defpath, 'LC_ALL': 'C'})
        self.assertEqual(popen.call_args.args[0], [str(self.bin / 'postgres'), '-D', str(self.pg.data)])
        with self.assertRaisesRegex(ValueError, 'never reuses'):
            self.pg.start()
        self.assertFalse(self.pg.metadata()['container_qualified'])
        self.assertFalse(self.pg.metadata()['resource_limits_enforced'])
        self.assertFalse(self.pg.metadata()['tls_qualified'])

    def test_identity_requires_live_owned_process_data_and_pid(self):
        self.pg.data.mkdir()
        (self.pg.data / 'postmaster.pid').write_text('12345\n')
        self.pg.process = Mock(pid=12345, poll=Mock(return_value=None))
        self.pg.assert_identity(str(self.pg.data))
        with self.assertRaisesRegex(RuntimeError, 'unowned'):
            self.pg.assert_identity(str(self.root))
        (self.pg.data / 'postmaster.pid').write_text('54321\n')
        with self.assertRaisesRegex(RuntimeError, 'identity changed'):
            self.pg.assert_identity(str(self.pg.data))
        self.pg.process.poll.return_value = 1
        with self.assertRaisesRegex(RuntimeError, 'not running'):
            self.pg.assert_identity(str(self.pg.data))

    def test_clean_stop_uses_only_owned_process_and_checks_port(self):
        proc = Mock(returncode=0, poll=Mock(return_value=None))
        self.pg.process = proc
        with patch('native_postgres.socket.socket') as sock:
            sock.return_value.__enter__.return_value.connect_ex.return_value = 111
            result = self.pg.stop()
        proc.send_signal.assert_called_once_with(signal.SIGINT)
        proc.kill.assert_not_called()
        self.assertTrue(result['port_closed'])
        self.assertIsNone(self.pg.process)

    def test_forced_stop_fails_and_retains_original_data(self):
        self.pg.data.mkdir()
        marker = self.pg.data / 'original'
        marker.write_bytes(b'retained')
        proc = Mock(returncode=0, poll=Mock(return_value=None))
        proc.wait.side_effect = [subprocess.TimeoutExpired('postgres', 30), None]
        self.pg.process = proc
        with self.assertRaisesRegex(RuntimeError, 'did not stop cleanly'):
            self.pg.stop()
        self.assertEqual([call.args[0] for call in proc.send_signal.call_args_list],
                         [signal.SIGINT, signal.SIGQUIT])
        self.assertEqual(marker.read_bytes(), b'retained')

    def test_native_identity_error_is_not_retried_or_followed_by_database_writes(self):
        # Attestation must fail immediately even if another server accepts the role.
        runtime = Runtime(SimpleNamespace(postgres_bin=self.bin))
        self.addCleanup(lambda: shutil.rmtree(runtime.directory))
        runtime.sql = Mock(side_effect=['freight_runtime', '/unowned'])
        with patch('runtime_contracts.NativePostgres', autospec=True) as pg:
            pg.return_value.process = Mock(poll=Mock(return_value=None))
            pg.return_value.assert_identity.side_effect = RuntimeError('unowned')
            with self.assertRaisesRegex(RuntimeError, 'unowned'):
                runtime.postgres()
        self.assertEqual(runtime.sql.call_count, 2)

    def test_cleanup_attempts_all_children_and_native_postgres_after_a_child_error(self):
        runtime = Runtime(SimpleNamespace())
        self.addCleanup(lambda: shutil.rmtree(runtime.directory))
        runtime.processes = {'sync': Mock(), 'edi': Mock()}
        runtime.native_postgres = Mock()
        runtime.stop = Mock(side_effect=[RuntimeError('child stop'), None])
        with self.assertRaisesRegex(RuntimeError, 'child stop'):
            runtime.cleanup()
        self.assertEqual(runtime.stop.call_count, 2)
        runtime.native_postgres.stop.assert_called_once()


if __name__ == '__main__':
    unittest.main()
