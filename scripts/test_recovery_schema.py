"""Exact frozen-ledger and inherited-check controls without Docker or SQL."""

import copy
import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import subprocess
import sys
import unittest

spec = importlib.util.spec_from_file_location(
    'recovery_schema', Path(__file__).with_name('qualify-restricted-recovery.py'))
recovery = importlib.util.module_from_spec(spec)
spec.loader.exec_module(recovery)


class ExactRecoveryLedger(unittest.TestCase):
    def test_optimized_interpreter_keeps_schema_refusals(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            rows = self.fixture(root, 3)
            code = '''import json, sys
sys.path.insert(0, sys.argv[1])
from migration_ledger import verify_migration_ledger
root, rows = sys.argv[2], json.loads(sys.argv[3])
verify_migration_ledger(root, rows)
damaged = [dict(item) for item in rows]
damaged[1]['checksum'] = '0' * 64
for bad in [rows[:-1], rows + [rows[-1]], list(reversed(rows)), damaged]:
    try:
        verify_migration_ledger(root, bad)
    except AssertionError:
        continue
    raise SystemExit('Optimized interpreter accepted a corrupt ledger')
'''
            result = subprocess.run([sys.executable, '-O', '-c', code,
                str(Path(__file__).resolve().parent), str(root), json.dumps(rows)],
                capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)

    def fixture(self, root, count):
        directory = root / 'migrations'
        directory.mkdir()
        rows = []
        for number in range(1, count + 1):
            content = ('-- original owned migration %d\n' % number).encode()
            (directory / ('%03d_owned.sql' % number)).write_bytes(content)
            rows.append({'version': number, 'checksum': hashlib.sha256(content).hexdigest(),
                         'applied_at': '2026-10-20T00:00:00Z'})
        return rows

    def test_complete_frozen_ledgers_work_at_preceding_current_and_next_sizes(self):
        for count in [28, 31, 32]:
            with self.subTest(count=count), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                rows = self.fixture(root, count)
                before = copy.deepcopy(rows)
                result = recovery.verify_migration_ledger(root, rows)
                self.assertEqual(len(result), count)
                self.assertEqual(rows, before, 'Verification must retain original timestamps')

    def test_current_application_requires_every_actual_migration(self):
        root = Path(__file__).resolve().parent.parent
        rows = [{'version': int(path.name.split('_', 1)[0]),
                 'checksum': hashlib.sha256(path.read_bytes()).hexdigest()}
                for path in sorted((root / 'migrations').glob('*.sql'))]
        self.assertEqual(recovery.verify_migration_ledger(root, rows), rows)
        with self.assertRaisesRegex(AssertionError, 'exact migration ledger'):
            recovery.verify_migration_ledger(root, rows[:28])

    def test_missing_extra_reordered_duplicate_and_changed_installed_rows_refuse(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            rows = self.fixture(root, 3)
            changed = copy.deepcopy(rows)
            changed[1]['checksum'] = '0' * 64
            for bad in [rows[:-1], rows[1:], rows + [rows[-1]], list(reversed(rows)), changed]:
                with self.subTest(rows=bad), self.assertRaisesRegex(AssertionError, 'exact migration ledger'):
                    recovery.verify_migration_ledger(root, bad)

    def test_empty_gapped_and_duplicate_source_versions_refuse(self):
        for scenario in ['empty', 'gap', 'duplicate']:
            with self.subTest(scenario=scenario), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                rows = self.fixture(root, 0 if scenario == 'empty' else 3)
                if scenario == 'gap':
                    (root / 'migrations/002_owned.sql').unlink()
                if scenario == 'duplicate':
                    (root / 'migrations/001_another.sql').write_text('-- duplicate version\n')
                with self.assertRaisesRegex(AssertionError, 'nonempty, unique and contiguous'):
                    recovery.verify_migration_ledger(root, rows)

    def test_modified_source_bytes_and_malformed_or_symlinked_inputs_refuse(self):
        for scenario in ['changed', 'malformed', 'symlink']:
            with self.subTest(scenario=scenario), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                rows = self.fixture(root, 1)
                path = root / 'migrations/001_owned.sql'
                if scenario == 'changed':
                    path.write_text('-- changed after original installation\n')
                elif scenario == 'malformed':
                    path.rename(root / 'migrations/unknown.sql')
                else:
                    path.unlink()
                    outside = root / 'outside.sql'
                    outside.write_text('-- external canary\n')
                    path.symlink_to(outside)
                with self.assertRaises(AssertionError):
                    recovery.verify_migration_ledger(root, rows)


class RecoveryInventory(unittest.TestCase):
    def test_public_inventory_retains_all_58_baseline_assertions(self):
        inventory = recovery.required_recovery_checks()
        self.assertEqual(sum(inventory.values()), 58)
        self.assertEqual(inventory[recovery.SCHEMA_CHECK], 2)
        self.assertNotIn(recovery.LEGACY_SCHEMA_CHECK, inventory)

    def test_empty_duplicate_invalid_counts_or_unknown_schema_refuse(self):
        valid = {'schema': 1, 'checks': [{'name': 'owned assertion', 'count': 1}]}
        for value in [
            dict(valid, schema=2), dict(valid, schema=True), dict(valid, checks=[]),
            dict(valid, checks=valid['checks'] * 2),
            dict(valid, checks=[{'name': 'owned assertion', 'count': 0}]),
            dict(valid, checks=[{'name': 'owned assertion', 'count': True}]),
            dict(valid, checks=[{'count': 1}]),
        ]:
            with self.subTest(value=value), tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / 'inventory.json'
                path.write_text(json.dumps(value))
                with self.assertRaises(AssertionError):
                    recovery.required_recovery_checks(path)

    def test_only_exact_legacy_schema_label_maps_to_stable_semantic_id(self):
        before = [{'name': recovery.LEGACY_SCHEMA_CHECK}] * 2 + [{'name': 'original receipt replay'}]
        after = [{'name': recovery.SCHEMA_CHECK}] * 2 + [{'name': 'original receipt replay'}]
        self.assertEqual(recovery.recovery_check_inventory(before), recovery.recovery_check_inventory(after))

    def test_missing_generation_other_assertion_or_similar_label_stays_missing(self):
        before = [{'name': recovery.LEGACY_SCHEMA_CHECK}] * 2 + [{'name': 'original receipt replay'}]
        for after in [
            [{'name': recovery.SCHEMA_CHECK}, {'name': 'original receipt replay'}],
            [{'name': recovery.SCHEMA_CHECK}] * 2,
            [{'name': recovery.LEGACY_SCHEMA_CHECK.replace('28', '31')}] * 2 + [{'name': 'original receipt replay'}],
        ]:
            with self.subTest(after=after):
                self.assertTrue(recovery.recovery_check_inventory(before) - recovery.recovery_check_inventory(after))
