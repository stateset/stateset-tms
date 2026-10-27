"""Archive pins, exact manifests and fresh Git history protect source publication."""
import gzip
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest
from unittest import mock

spec = importlib.util.spec_from_file_location('alpha_stage', Path(__file__).with_name('stage-source-alpha.py'))
stage = importlib.util.module_from_spec(spec)
spec.loader.exec_module(stage)


class StageTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.archive = self.root / 'source.tar.gz'
        self.output = self.root / 'staged'
        self.report = self.root / 'review.json'
        self.files = {'README.md': b'# Synthetic source\n', 'scripts/launch': b'#!/bin/sh\nexit 0\n',
                      '.gitleaksignore': b'# No historical exemptions\n'}
        records = {name: {'sha256': stage.sha(data), 'bytes': len(data), 'executable': name == 'scripts/launch'}
                   for name, data in self.files.items()}
        self.files['SOURCE_ALPHA.json'] = stage.alpha.encoded({
            'schema': 1, 'input_commit': 'a' * 40, 'manifest_excludes_itself': True,
            'publication_cleared': False, 'distribution_compliance_attested': False, 'files': records})
        self.write_archive()

    def write_archive(self, extra=None, mode_overrides=None):
        with self.archive.open('wb') as handle, gzip.GzipFile(filename='', fileobj=handle, mode='wb', mtime=0) as compressed:
            with tarfile.open(fileobj=compressed, mode='w', format=tarfile.PAX_FORMAT) as bundle:
                for name, data in sorted(self.files.items()):
                    info = tarfile.TarInfo('stateset-freight-source-alpha/' + name)
                    info.size = len(data); info.mode = 0o755 if name == 'scripts/launch' else 0o644
                    info.mode = (mode_overrides or {}).get(name, info.mode)
                    bundle.addfile(info, io.BytesIO(data))
                if extra is not None:
                    bundle.addfile(extra, io.BytesIO(b'x' * extra.size) if extra.isfile() else None)

    def verify(self):
        return stage.verify_archive(self.archive, stage.file_sha(self.archive))

    def prepare(self):
        return stage.stage(self.archive, stage.file_sha(self.archive), self.output, self.report,
                           'Synthetic Maintainer', 'fixture@example.invalid')

    def test_fresh_git_history_preserves_all_bytes_modes_and_explicit_public_identity(self):
        hook = self.root / 'hooks'; hook.mkdir()
        (hook / 'pre-commit').write_text('#!/bin/sh\ntouch "' + str(self.root / 'hook-ran') + '"\n')
        (hook / 'pre-commit').chmod(0o755)
        configuration = self.root / 'global.gitconfig'
        configuration.write_text('[core]\n hooksPath = ' + str(hook) + '\n')
        with mock.patch.dict(os.environ, {'GIT_DIR': str(self.root / 'wrong.git'),
                                         'GIT_WORK_TREE': str(self.root / 'wrong-tree'),
                                         'GIT_CONFIG_GLOBAL': str(configuration)}):
            result = self.prepare()
        self.assertFalse((self.root / 'hook-ran').exists())
        self.assertEqual(result['reachable_commits'], 1)
        self.assertEqual(result['remotes'], [])
        self.assertFalse(result['publication_cleared'])
        self.assertEqual(self.output.stat().st_mode & 0o777, 0o700)
        self.assertEqual(self.report.stat().st_mode & 0o777, 0o600)
        self.assertEqual(stage.alpha.git(self.output, 'log', '--format=%P').strip(), b'')
        self.assertEqual(stage.alpha.git(self.output, 'log', '--format=%ae').strip(), b'fixture@example.invalid')
        self.assertFalse(stage.alpha.git(self.output, 'status', '--porcelain').strip())
        for name, data in self.files.items():
            self.assertEqual(stage.alpha.git(self.output, 'show', 'HEAD:' + name), data)
        self.assertTrue(os.access(str(self.output / 'scripts/launch'), os.X_OK))
        self.assertFalse((self.output / '.git/objects/info/alternates').exists())

    def test_wrong_archive_pin_and_source_hash_drift_are_refused_before_staging(self):
        with self.assertRaisesRegex(ValueError, 'SHA-256 pin'):
            stage.verify_archive(self.archive, '0' * 64)
        self.files['README.md'] = b'changed bytes\n'; self.write_archive()
        with self.assertRaisesRegex(ValueError, 'differs from the manifest'):
            self.prepare()
        self.assertFalse(self.output.exists())
        self.assertFalse(self.report.exists())

    def test_missing_extra_or_duplicate_members_are_refused(self):
        original = dict(self.files)
        for action in ['missing', 'extra', 'duplicate']:
            with self.subTest(action=action):
                self.files = dict(original)
                if action == 'missing': del self.files['README.md']
                if action == 'extra': self.files['unexpected.txt'] = b'unreviewed'
                extra = tarfile.TarInfo('stateset-freight-source-alpha/README.md') if action == 'duplicate' else None
                self.write_archive(extra)
                with self.assertRaisesRegex(ValueError, 'exact archive|Duplicate'):
                    self.verify()

    def test_symlinks_hardlinks_directories_and_unsafe_names_are_refused(self):
        for kind in [tarfile.SYMTYPE, tarfile.LNKTYPE, tarfile.DIRTYPE]:
            with self.subTest(kind=kind):
                info = tarfile.TarInfo('stateset-freight-source-alpha/link'); info.type = kind; info.linkname = '../private'
                self.write_archive(info)
                with self.assertRaisesRegex(ValueError, 'regular source'):
                    self.verify()
        for name in ['../private', '.git/config', '.Git/config', '.env', '/absolute', 'nested/../../private']:
            with self.subTest(name=name):
                self.write_archive(tarfile.TarInfo('stateset-freight-source-alpha/' + name))
                with self.assertRaises(ValueError): self.verify()

    def test_duplicate_manifest_keys_and_unbound_executable_modes_are_refused(self):
        original = self.files['SOURCE_ALPHA.json']
        self.files['SOURCE_ALPHA.json'] = b'{"schema":1,"schema":1}'
        self.write_archive()
        with self.assertRaisesRegex(ValueError, 'Duplicate manifest'):
            self.verify()
        self.files['SOURCE_ALPHA.json'] = original
        self.write_archive(mode_overrides={'README.md': 0o755})
        with self.assertRaisesRegex(ValueError, 'mode differs'):
            self.verify()

    def test_hidden_trailing_payload_and_expansion_limit_are_refused(self):
        with self.archive.open('ab') as handle:
            handle.write(gzip.compress(b'unreviewed trailing payload'))
        with self.assertRaisesRegex(ValueError, 'Unexpected data after'):
            self.verify()
        self.write_archive()
        with mock.patch.object(stage, 'MAX_EXPANDED', 512), self.assertRaisesRegex(ValueError, 'Expanded archive'):
            self.verify()

    def test_malformed_manifest_shapes_fail_before_output_creation(self):
        original = json.loads(self.files['SOURCE_ALPHA.json'])
        broken_record = json.loads(json.dumps(original)); broken_record['files']['README.md'] = None
        for manifest in [[], dict(original, schema=True), dict(original, input_commit='not-a-commit'), broken_record]:
            with self.subTest(manifest=manifest):
                self.files['SOURCE_ALPHA.json'] = stage.alpha.encoded(manifest); self.write_archive()
                with self.assertRaises(ValueError): self.prepare()
                self.assertFalse(self.output.exists())

    def test_no_existing_directory_report_or_symlink_is_overwritten(self):
        self.prepare()
        original = self.report.read_bytes()
        with self.assertRaisesRegex(ValueError, 'new paths'): self.prepare()
        self.assertEqual(self.report.read_bytes(), original)
        other = self.root / 'other'; other.mkdir(); (other / 'keep').write_text('keep')
        linked = self.root / 'linked'; linked.symlink_to(other, target_is_directory=True)
        with self.assertRaisesRegex(ValueError, 'new paths'):
            stage.stage(self.archive, stage.file_sha(self.archive), linked, self.root / 'another.json', 'Fixture', 'fixture@example.invalid')
        self.assertEqual((other / 'keep').read_text(), 'keep')

    def test_nested_parent_history_is_not_copied_and_internal_reports_are_refused(self):
        with self.assertRaisesRegex(ValueError, 'outside the staging'):
            stage.stage(self.archive, stage.file_sha(self.archive), self.output, self.output / 'report.json', 'Fixture', 'fixture@example.invalid')
        checkout = self.root / 'private'; checkout.mkdir()
        stage.alpha.git(checkout, 'init', '--quiet', '--template=')
        stage.alpha.git(checkout, '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
                        '-c', 'commit.gpgSign=false', 'commit', '--quiet', '--allow-empty', '-m', 'Private parent fixture')
        parent_commit = stage.alpha.git(checkout, 'rev-parse', 'HEAD').decode().strip()
        with self.assertRaisesRegex(ValueError, 'Git metadata'):
            stage.stage(self.archive, stage.file_sha(self.archive), checkout / '.git/public', self.report, 'Fixture', 'fixture@example.invalid')
        result = stage.stage(self.archive, stage.file_sha(self.archive), checkout / 'public', self.report, 'Fixture', 'fixture@example.invalid')
        self.assertEqual(result['reachable_commits'], 1)
        self.assertEqual(stage.alpha.git(checkout, 'rev-parse', 'HEAD').decode().strip(), parent_commit)
        with self.assertRaises(subprocess.CalledProcessError):
            stage.alpha.git(checkout / 'public', 'cat-file', '-e', parent_commit)

    def test_private_workflows_and_historical_fingerprint_bypasses_are_refused(self):
        for name, data in [('.github/workflows/check.yml', b'self-hosted'), ('.gitleaksignore', b'old-fingerprint\n')]:
            original = dict(self.files)
            self.files[name] = data
            manifest = json.loads(self.files['SOURCE_ALPHA.json'])
            manifest['files'][name] = {'sha256': stage.sha(data), 'bytes': len(data), 'executable': False}
            self.files['SOURCE_ALPHA.json'] = stage.alpha.encoded(manifest)
            self.write_archive()
            with self.subTest(name=name), self.assertRaisesRegex(ValueError, 'workflow|fingerprints'):
                self.verify()
            self.files = original


if __name__ == '__main__':
    unittest.main()
