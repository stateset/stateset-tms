"""Exercise package-specific release decisions without a network or compiler."""
import argparse
import contextlib
import datetime as dt
import importlib.util
import io
import json
import os
from pathlib import Path
import shutil
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location('production_audit', Path(__file__).with_name('audit-production.py'))
AUDIT = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(AUDIT)


def finding(name='client-only', version='1.2.3'):
    return {'package': {'name': name, 'version': version}, 'advisory': {'id': 'RUSTSEC-OWNED-FIXTURE'}}


class ProductionAudit(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix='freight-audit-unit-')
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        (self.root/'crates/client').mkdir(parents=True)
        (self.root/'Cargo.toml').write_text('# owned manifest\n')
        (self.root/'crates/client/Cargo.toml').write_text('# owned client\n')
        (self.root/'Cargo.lock').write_text('# owned lock\n')
        self.calls = []
        self.tree = None
        self.audit_findings = [finding()]
        self.settings = dict(ignore=[], target_arch=None, target_os=None, severity=None)
        self.database = '1'*40
        self.dirty = ''
        self.change_lock = False
        self.change_database = False
        self.git_reads = 0
        self.bad_count = False
        self.metadata_root = None
        self.metadata_status = 0
        self.metadata_missing_selected = False
        self.change_path_manifest = False
        self.change_selected_manifest = False
        self.path_manifest = self.root/'path-peer/Cargo.toml'
        self.path_manifest.parent.mkdir()
        self.path_manifest.write_text('# owned mutable path dependency\n')
        for name in ('integration-contracts', 'runtime-contracts'):
            directory = self.root/name
            directory.mkdir()
            (directory/'Cargo.toml').write_text('# owned helper manifest\n')
            (directory/'Cargo.lock').write_text('# owned helper lock\n')

    def run_audit(self, *arguments):
        output = self.root/'result.json'
        artifacts = self.root/'artifacts'
        artifacts.mkdir()

        def check_output(command, **_):
            if command == ['rustc', '-vV']:
                return 'rustc 1.90.0\nhost: x86_64-unknown-linux-gnu\n'
            if 'rev-parse' in command:
                self.git_reads += 1
                return (('2'*40) if self.change_database and self.git_reads > 1 else self.database)+'\n'
            if 'status' in command:
                return self.dirty
            raise AssertionError('Unexpected command: '+repr(command))

        def run(command, cwd, stdout, stderr):
            self.calls.append(command)
            self.assertEqual(cwd, self.root)
            if command[:2] == ['cargo', 'metadata']:
                selected = Path(command[command.index('--manifest-path')+1])
                manifests = [self.path_manifest] if self.metadata_missing_selected else [selected, self.path_manifest]
                stdout.write(json.dumps({'workspace_root': str(self.metadata_root or selected.parent),
                    'packages': [{'source': None, 'manifest_path': str(p)} for p in manifests]}).encode())
                if self.change_selected_manifest:
                    selected.write_text('# changed during metadata resolution\n')
                return SimpleNamespace(returncode=self.metadata_status)
            if command[:2] == ['cargo', 'tree']:
                package = command[command.index('--package')+1]
                tree = self.tree if self.tree is not None else package+' v0.1.2 (/fixture)\nclient-only v1.2.3\nsafe v0.2.0\n'
                stdout.write(tree.encode())
                return SimpleNamespace(returncode=0)
            self.assertEqual(command[:2], ['cargo', 'audit'])
            raw = dict(settings=self.settings, vulnerabilities=dict(count=len(self.audit_findings)+(1 if self.bad_count else 0), found=bool(self.audit_findings), list=self.audit_findings), database={'last-commit': None}, warnings={})
            stdout.write(json.dumps(raw).encode())
            if self.change_lock:
                Path(command[command.index('--file')+1]).write_text('# changed during audit\n')
            if self.change_path_manifest:
                self.path_manifest.write_text('# changed during audit\n')
            return SimpleNamespace(returncode=int(bool(self.audit_findings)))

        with patch.object(AUDIT, 'ROOT', self.root), patch.object(AUDIT.tempfile, 'mkdtemp', return_value=str(artifacts)), patch.object(AUDIT.subprocess, 'check_output', side_effect=check_output), patch.object(AUDIT.subprocess, 'run', side_effect=run), patch('sys.argv', ['audit-production.py', '--output', str(output), *arguments]), contextlib.redirect_stdout(io.StringIO()):
            code = AUDIT.main()
        report = json.loads(output.read_text())
        self.assertEqual(Path(report['harness_archive']['path']).read_bytes(), Path(AUDIT.__file__).read_bytes())
        return code, report

    def test_default_root_stays_server_and_keeps_inactive_findings_visible(self):
        self.tree = 'stateset-freight v0.1.2 (/fixture)\nserver-only v2.0.0\n'
        code, report = self.run_audit()
        self.assertEqual(code, 0)
        self.assertEqual(report['package'], 'stateset-freight')
        self.assertEqual(report['package_version'], '0.1.2')
        self.assertEqual(report['active_vulnerabilities'], [])
        self.assertEqual(report['inactive_lockfile_advisories'], self.audit_findings)
        self.assertEqual(report['advisories_ignored'], [])

    def test_client_only_vulnerability_fails_tui_gate(self):
        code, report = self.run_audit('--package', 'stateset-freight-tui')
        self.assertEqual(code, 1)
        self.assertEqual(report['package'], 'stateset-freight-tui')
        self.assertEqual(report['active_vulnerabilities'], self.audit_findings)
        self.assertEqual(report['inactive_lockfile_advisories'], [])

    def test_helper_uses_its_own_workspace_lock_and_path_bindings(self):
        self.audit_findings = []
        code, report = self.run_audit('--manifest-path', 'runtime-contracts/Cargo.toml', '--package', 'stateset-freight-runtime-wire',
                                      '--target', 'x86_64-unknown-linux-gnu', '--features', 'owned-feature', '--no-default-features')
        self.assertEqual(code, 0)
        manifest = str(self.root/'runtime-contracts/Cargo.toml')
        self.assertEqual(report['lockfile']['path'], str(self.root/'runtime-contracts/Cargo.lock'))
        self.assertEqual({p['path'] for p in report['manifests']}, {manifest, str(self.path_manifest)})
        metadata, tree, audit = self.calls
        for command in (metadata, tree):
            self.assertEqual(command[command.index('--manifest-path')+1], manifest)
            self.assertEqual(command[command.index('--features')+1], 'owned-feature')
            self.assertIn('--no-default-features', command)
            self.assertIn('--locked', command)
            self.assertIn('--offline', command)
        self.assertEqual(audit[audit.index('--file')+1], str(self.root/'runtime-contracts/Cargo.lock'))

    def test_helper_active_vulnerability_fails_without_suppression(self):
        code, report = self.run_audit('--manifest-path', 'integration-contracts/Cargo.toml', '--package', 'stateset-freight-contracts')
        self.assertEqual(code, 1)
        self.assertEqual(report['active_vulnerabilities'], self.audit_findings)
        self.assertEqual(report['advisories_ignored'], [])

    def test_helper_cannot_accidentally_audit_api_graph(self):
        self.tree = 'stateset-freight v0.1.2 (/fixture)\nclient-only v1.2.3\n'
        code, report = self.run_audit('--manifest-path', 'runtime-contracts/Cargo.toml', '--package', 'stateset-freight-runtime-wire')
        self.assertEqual(code, 1)
        self.assertIn('first dependency graph node', report['error'])
        self.assertEqual(len(self.calls), 2)

    def test_different_resolved_workspace_is_refused(self):
        self.metadata_root = self.root
        code, report = self.run_audit('--manifest-path', 'runtime-contracts/Cargo.toml', '--package', 'stateset-freight-runtime-wire')
        self.assertEqual(code, 1)
        self.assertIn('different workspace root', report['error'])
        self.assertEqual(len(self.calls), 1)

    def test_missing_selected_manifest_is_refused(self):
        self.metadata_missing_selected = True
        code, report = self.run_audit('--manifest-path', 'runtime-contracts/Cargo.toml', '--package', 'stateset-freight-runtime-wire')
        self.assertEqual(code, 1)
        self.assertIn('absent from Cargo metadata', report['error'])

    def test_metadata_failure_does_not_run_advisory_audit(self):
        self.metadata_status = 101
        code, report = self.run_audit('--manifest-path', 'runtime-contracts/Cargo.toml', '--package', 'stateset-freight-runtime-wire')
        self.assertEqual(code, 1)
        self.assertIn('Cannot resolve the selected workspace', report['error'])
        self.assertEqual(len(self.calls), 1)

    def test_helper_path_dependency_mutation_is_refused(self):
        self.audit_findings = []
        self.change_path_manifest = True
        code, report = self.run_audit('--manifest-path', 'runtime-contracts/Cargo.toml', '--package', 'stateset-freight-runtime-wire')
        self.assertEqual(code, 1)
        self.assertIn('Build manifests changed', report['error'])

    def test_helper_lock_mutation_is_refused(self):
        self.audit_findings = []
        self.change_lock = True
        code, report = self.run_audit('--manifest-path', 'integration-contracts/Cargo.toml', '--package', 'stateset-freight-contracts')
        self.assertEqual(code, 1)
        self.assertIn('Lockfile changed', report['error'])

    def test_manifest_change_during_metadata_resolution_is_refused(self):
        self.change_selected_manifest = True
        code, report = self.run_audit('--manifest-path', 'runtime-contracts/Cargo.toml', '--package', 'stateset-freight-runtime-wire')
        self.assertEqual(code, 1)
        self.assertIn('Workspace manifest changed', report['error'])

    def test_unregistered_manifest_is_refused_before_any_cargo_call(self):
        with self.assertRaisesRegex(RuntimeError, 'explicit repository qualification'):
            self.run_audit('--manifest-path', str(self.path_manifest), '--package', 'stateset-freight-runtime-wire')
        self.assertEqual(self.calls, [])

    def test_server_only_finding_is_retained_by_healthy_client(self):
        self.audit_findings = [finding('server-only', '2.0.0')]
        code, report = self.run_audit('--package', 'stateset-freight-tui')
        self.assertEqual(code, 0)
        self.assertEqual(report['inactive_lockfile_advisories'], self.audit_findings)
        self.assertIn({'name': 'client-only', 'version': '1.2.3'}, report['active_packages'])
        self.assertIn({'name': 'safe', 'version': '0.2.0'}, report['active_packages'])

    def test_server_graph_cannot_stand_in_for_client(self):
        self.tree = 'stateset-freight v0.1.2 (/fixture)\nclient-only v1.2.3\n'
        code, report = self.run_audit('--package', 'stateset-freight-tui')
        self.assertEqual(code, 1)
        self.assertIn('Selected production root', report['error'])
        self.assertEqual(len(self.calls), 1)

    def test_selected_name_as_nested_dependency_cannot_stand_in_for_root(self):
        self.tree = 'wrong-root v1.0.0\nstateset-freight-tui v0.1.2 (/fixture)\n'
        code, report = self.run_audit('--package', 'stateset-freight-tui')
        self.assertEqual(code, 1)
        self.assertIn('first dependency graph node', report['error'])

    def test_empty_graph_is_refused_before_advisory_execution(self):
        self.tree = ''
        code, report = self.run_audit('--package', 'stateset-freight-tui')
        self.assertEqual(code, 1)
        self.assertNotIn('raw_lockfile_audit', report)

    def test_declared_target_features_and_offline_revision_are_bound(self):
        self.audit_findings = []
        code, report = self.run_audit('--package', 'stateset-freight-tui', '--target', 'aarch64-apple-darwin', '--features', 'explicit-feature', '--no-default-features', '--no-fetch', '--db', str(self.root/'db'))
        self.assertEqual(code, 0)
        tree = self.calls[0]
        for flag, value in [('--package', 'stateset-freight-tui'), ('--target', 'aarch64-apple-darwin'), ('--features', 'explicit-feature'), ('--edges', 'normal,build')]:
            self.assertEqual(tree[tree.index(flag)+1], value)
        self.assertIn('--locked', tree)
        self.assertIn('--offline', tree)
        self.assertIn('--no-default-features', tree)
        self.assertNotIn('--workspace', tree)
        self.assertIn('--no-fetch', self.calls[1])
        self.assertEqual(report['advisory_database']['resolved_git_commit'], self.database)
        self.assertFalse(report['default_features'])

    def test_unrelated_version_is_not_misclassified_as_active(self):
        self.audit_findings = [finding(version='1.2.4')]
        code, report = self.run_audit('--package', 'stateset-freight-tui')
        self.assertEqual(code, 0)
        self.assertEqual(report['inactive_lockfile_advisories'], self.audit_findings)

    def test_suppressed_advisories_refuse_a_passing_report(self):
        self.settings['ignore'] = ['RUSTSEC-OWNED-FIXTURE']
        code, report = self.run_audit('--package', 'stateset-freight-tui')
        self.assertEqual(code, 1)
        self.assertIn('filters or suppresses', report['error'])

    def test_incomplete_raw_report_is_refused(self):
        self.bad_count = True
        code, report = self.run_audit('--package', 'stateset-freight-tui')
        self.assertEqual(code, 1)
        self.assertIn('Incomplete RustSec', report['error'])

    def test_lockfile_mutation_during_audit_is_refused(self):
        self.change_lock = True
        code, report = self.run_audit('--package', 'stateset-freight-tui')
        self.assertEqual(code, 1)
        self.assertIn('Lockfile changed', report['error'])

    def test_offline_database_mutation_is_refused(self):
        self.change_database = True
        code, report = self.run_audit('--package', 'stateset-freight-tui', '--no-fetch')
        self.assertEqual(code, 1)
        self.assertIn('database changed', report['error'])

    def test_dirty_advisory_database_is_refused(self):
        self.dirty = ' M crates/owned.md\n'
        code, report = self.run_audit('--package', 'stateset-freight-tui')
        self.assertEqual(code, 1)
        self.assertIn('local changes', report['error'])

    def test_package_selector_cannot_be_a_pattern_path_or_option(self):
        for name in ['*', 'stateset-freight@0.1.2', '../crate', '--workspace', '', 'a b', 'a\nb']:
            with self.subTest(name=name), self.assertRaises(argparse.ArgumentTypeError):
                AUDIT.package_name(name)

    def test_existing_evidence_is_not_overwritten_or_audited_again(self):
        output = self.root/'previous.json'
        original = b'{"retained": "preceding execution"}\n'
        output.write_bytes(original)
        with patch('sys.argv', ['audit-production.py', '--package', 'stateset-freight-tui', '--output', str(output)]), patch.object(AUDIT.subprocess, 'run') as command, self.assertRaises(RuntimeError):
            AUDIT.main()
        command.assert_not_called()
        self.assertEqual(output.read_bytes(), original)

    def except_finding(self, **changes):
        today = dt.datetime.now(dt.timezone.utc).date()
        entry = dict(advisory='RUSTSEC-2026-0001', package='stateset-freight-runtime-wire', crate='client-only', version='1.2.3',
                     approved=today.isoformat(), expires=(today+dt.timedelta(days=30)).isoformat(), reason='Owned fixture exception')
        entry.update(changes)
        (self.root/'scripts').mkdir(exist_ok=True)
        (self.root/'scripts/advisory-exceptions.json').write_text(json.dumps({'exceptions': [entry]}))
        self.audit_findings = [dict(finding(), advisory={'id': 'RUSTSEC-2026-0001'})]
        return entry

    def wire(self):
        return self.run_audit('--manifest-path', 'runtime-contracts/Cargo.toml', '--package', 'stateset-freight-runtime-wire')

    def test_helper_exception_applies_to_its_exact_finding_and_stays_visible(self):
        entry = self.except_finding()
        code, report = self.wire()
        self.assertEqual(code, 0)
        self.assertEqual(report['active_vulnerabilities'], [])
        self.assertEqual(report['excepted_vulnerabilities'], [{'exception': entry, 'finding': self.audit_findings[0]}])
        self.assertEqual(report['advisory_exceptions']['path'], str(self.root/'scripts/advisory-exceptions.json'))
        self.assertEqual(report['advisories_ignored'], [])

    def test_expired_exception_fails_before_any_cargo_call(self):
        yesterday = dt.datetime.now(dt.timezone.utc).date()-dt.timedelta(days=1)
        self.except_finding(approved=(yesterday-dt.timedelta(days=10)).isoformat(), expires=yesterday.isoformat())
        code, report = self.wire()
        self.assertEqual(code, 1)
        self.assertIn('expired', report['error'])
        self.assertEqual(self.calls, [])

    def test_unused_exception_fails_once_the_advisory_is_fixed(self):
        self.except_finding()
        self.audit_findings = []
        code, report = self.wire()
        self.assertEqual(code, 1)
        self.assertIn('unused', report['error'])

    def test_exception_for_another_version_does_not_apply(self):
        self.except_finding(version='1.2.4')
        code, report = self.wire()
        self.assertEqual(code, 1)
        self.assertEqual(len(report['active_vulnerabilities']), 1)
        self.assertEqual(report['excepted_vulnerabilities'], [])

    def test_application_graphs_cannot_carry_exceptions(self):
        for package in ('stateset-freight', 'stateset-freight-tui'):
            with self.subTest(package=package):
                self.except_finding(package=package)
                code, report = self.run_audit('--package', package) if package == 'stateset-freight-tui' else self.run_audit()
                self.assertEqual(code, 1)
                self.assertIn('Application graphs', report['error'])
                (self.root/'result.json').unlink()
                shutil.rmtree(self.root/'artifacts')

    def test_malformed_or_open_ended_exceptions_are_refused(self):
        today = dt.datetime.now(dt.timezone.utc).date()
        for changes in (dict(expires=(today+dt.timedelta(days=91)).isoformat()), dict(reason=' '), dict(advisory='GHSA-xxxx'), dict(note='extra field')):
            with self.subTest(changes=changes):
                self.except_finding(**changes)
                code, report = self.wire()
                self.assertEqual(code, 1)
                self.assertEqual(self.calls, [])
                (self.root/'result.json').unlink()
                shutil.rmtree(self.root/'artifacts')


if __name__ == '__main__':
    unittest.main()
