"""Exercise release-gate host overload decisions with owned load fixtures."""
import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).with_name('host-load.py')
SPEC = importlib.util.spec_from_file_location('host_load', SCRIPT)
HOST = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(HOST)


class HostLoad(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix='freight-host-load-unit-')
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.loadavg = self.root/'loadavg'
        self.record = self.root/'record.json'
        patcher = patch.dict(os.environ, {}, clear=False)
        patcher.start()
        self.addCleanup(patcher.stop)
        for variable in ('FREIGHT_RELEASE_START_LOAD_PER_CPU', 'FREIGHT_RELEASE_FAILURE_LOAD_PER_CPU'):
            os.environ.pop(variable, None)

    def load(self, one, five):
        self.loadavg.write_text(f'{one} {five} 1.00 3/900 12345\n')

    def run_main(self, *argv):
        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr):
            status = HOST.main([*argv, '--loadavg', str(self.loadavg), '--cpus', '8'])
        return status, stderr.getvalue()

    def recorded(self, peak, cpus=8):
        self.record.write_text(json.dumps({'cpus': cpus, 'samples': 3, 'peak_load1': peak,
                                           'peak_at': '2026-10-27T16:22:00Z'}))

    def test_preflight_starts_only_on_a_quiet_host(self):
        self.load(30, 8.0)
        self.assertEqual(self.run_main('preflight'), (0, ''))
        self.load(1, 8.1)
        status, message = self.run_main('preflight')
        self.assertEqual(status, HOST.OVERLOADED)
        self.assertIn('5-minute load 8.1 on 8 CPUs, limit 8.0', message)
        os.environ['FREIGHT_RELEASE_START_LOAD_PER_CPU'] = '1.5'
        self.assertEqual(self.run_main('preflight'), (0, ''))
        self.assertEqual(self.run_main('preflight', '--max-load-per-cpu', '1'), (HOST.OVERLOADED, message))

    def test_invalid_limits_are_refused(self):
        self.load(1, 1)
        for value in ('0', '-1', 'many', 'nan'):
            with self.subTest(value=value):
                os.environ['FREIGHT_RELEASE_START_LOAD_PER_CPU'] = value
                with self.assertRaises(SystemExit):
                    self.run_main('preflight')

    def test_success_is_never_relabelled(self):
        self.recorded(90.0)
        self.assertEqual(self.run_main('classify', '--record', str(self.record), '--status', '0'), (0, ''))

    def test_failure_during_overload_stays_a_failure_with_its_cause_named(self):
        self.recorded(90.0)
        status, message = self.run_main('classify', '--record', str(self.record), '--status', '1')
        self.assertEqual(status, HOST.OVERLOADED)
        self.assertNotEqual(status, 0)
        self.assertIn('peaked at 90.0 on 8 CPUs at 2026-10-27T16:22:00Z, above 16.0', message)
        self.assertIn('not a product verdict', message)

    def test_failure_on_a_quiet_host_keeps_its_status(self):
        self.recorded(16.0)
        status, message = self.run_main('classify', '--record', str(self.record), '--status', '3')
        self.assertEqual(status, 3)
        self.assertIn('within limits', message)
        os.environ['FREIGHT_RELEASE_FAILURE_LOAD_PER_CPU'] = '1'
        self.assertEqual(self.run_main('classify', '--record', str(self.record), '--status', '3')[0],
                         HOST.OVERLOADED)

    def test_missing_or_malformed_records_keep_the_original_failure(self):
        status, message = self.run_main('classify', '--record', str(self.record), '--status', '2')
        self.assertEqual((status, 'not recorded' in message), (2, True))
        for text in ('{', '{"cpus": 8}', '{"cpus": "x", "peak_load1": 1}', '[]'):
            with self.subTest(text=text):
                self.record.write_text(text)
                self.assertEqual(self.run_main('classify', '--record', str(self.record), '--status', '2')[0], 2)

    def test_monitor_records_the_peak_until_terminated(self):
        self.load(3.5, 1)
        monitor = subprocess.Popen(
            [sys.executable, str(SCRIPT), 'monitor', '--loadavg', str(self.loadavg), '--cpus', '8',
             '--output', str(self.record), '--interval', '0.02'])
        self.addCleanup(monitor.kill)

        def samples():
            try:
                return json.loads(self.record.read_text())['samples']
            except (OSError, ValueError):
                return 0

        def wait_for(count):
            deadline = time.monotonic() + 20
            while samples() < count:
                self.assertLess(time.monotonic(), deadline, 'monitor stopped sampling')
                time.sleep(0.02)

        wait_for(1)
        self.load(41.25, 1)
        wait_for(samples() + 3)
        self.load(2.0, 1)
        wait_for(samples() + 3)
        monitor.terminate()
        self.assertEqual(monitor.wait(timeout=20), 0)
        record = json.loads(self.record.read_text())
        self.assertEqual((record['cpus'], record['peak_load1']), (8, 41.25))
        self.assertRegex(record['peak_at'], r'^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ$')
        self.assertFalse(self.record.with_name(self.record.name + '.partial').exists())


if __name__ == '__main__':
    unittest.main()
