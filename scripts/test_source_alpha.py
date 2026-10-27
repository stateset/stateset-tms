"""Source export boundaries must hold independently of a private working tree."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest
from unittest import mock

spec = importlib.util.spec_from_file_location('source_alpha', Path(__file__).with_name('prepare-source-alpha.py'))
alpha = importlib.util.module_from_spec(spec)
spec.loader.exec_module(alpha)


class SourceAlphaTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.base = Path(self.temporary.name)
        self.root = self.base / 'checkout'
        self.root.mkdir()
        self.run_git('init', '-q')
        self.run_git('config', 'user.name', 'Synthetic Test')
        self.run_git('config', 'user.email', 'fixture@example.invalid')
        self.files = {
            'README.md': b'# Fixture\n\n[Private trace](docs/evidence/trace.json)\n\n[Public guide](docs/guide.md)\n',
            'docs/guide.md': b'# Guide\n\nPublic instructions.\n',
            'src/lib.rs': b'// owned synthetic source\n',
            'scripts/launch': b'#!/bin/sh\nexit 0\n',
            '.github/workflows/community.yml': b'      - run: python3 scripts/check-open-source.py\n      - run: python3 -m unittest discover -s scripts -p test_publication_tools.py -v\n',
            'mcp/catalog.json': b'{}\n', 'mcp/guides.json': b'{}\n',
            'mcp/scripts/catalog.mjs': b'import {readFileSync,writeFileSync} from "node:fs"; writeFileSync("mcp/guides.json",JSON.stringify({guide:readFileSync("docs/guide.md","utf8")}));\n',
            'publication/source-alpha/public.md': b'# Public alpha\n',
        }
        fixture_path = 'web/tests/fixtures/journal/old.mjs'
        self.files[fixture_path] = b'export const version = 1;\n'
        self.files['web/tests/fixtures/journal/manifest.json'] = alpha.encoded({'files': {
            'old.mjs': {'sha256': alpha.digest(self.files[fixture_path]), 'qualified_source': 'web/lib/old.mjs',
                        'qualification': 'docs/evidence/trace.json', 'qualification_sha256': '0' * 64}}})
        self.policy = {'schema': 1, 'files': sorted(list(self.files) + [alpha.POLICY]),
                       'overlays': {'docs/alpha-status.md': 'publication/source-alpha/public.md'}}
        for name, data in self.files.items():
            self.write(name, data)
        self.write('docs/evidence/trace.json', b'{"private_synthetic_trace": true}')
        self.write('.env', b'SYNTHETIC_PRIVATE_CONFIGURATION=true\n')
        (self.root / 'scripts/launch').chmod(0o755)
        self.commit()

    def write(self, name, data):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)

    def run_git(self, *args):
        return subprocess.check_output(['git', '-C', str(self.root), *args], stderr=subprocess.PIPE)

    def commit(self):
        self.write(alpha.POLICY, alpha.encoded(self.policy))
        self.run_git('add', '--all')
        self.run_git('commit', '-qm', 'Synthetic export fixture')

    def export(self, name='candidate', archive=None, ref='HEAD'):
        return alpha.prepare(self.root, ref, self.base / name, archive)

    def test_immutable_tree_omissions_hashes_modes_and_receipts(self):
        self.write('src/lib.rs', b'// uncommitted bytes must stay private\n')
        self.write('untracked.txt', b'private synthetic untracked data')
        manifest = self.export()
        output = self.base / 'candidate'
        self.assertEqual((output / 'src/lib.rs').read_bytes(), self.files['src/lib.rs'])
        self.assertFalse((output / '.git').exists())
        self.assertFalse((output / '.env').exists())
        self.assertFalse((output / 'untracked.txt').exists())
        self.assertFalse((output / 'docs/evidence').exists())
        self.assertNotIn('Private trace', (output / 'README.md').read_text())
        self.assertIn('Public guide', (output / 'README.md').read_text())
        self.assertTrue(os.access(str(output / 'scripts/launch'), os.X_OK))
        self.assertEqual(output.stat().st_mode & 0o777, 0o700)
        self.assertFalse(manifest['publication_cleared'])
        self.assertNotIn(alpha.MANIFEST, manifest['files'])
        actual = {p.relative_to(output).as_posix() for p in output.rglob('*') if p.is_file()}
        self.assertEqual(actual, set(manifest['files']) | {alpha.MANIFEST})
        for name, record in manifest['files'].items():
            self.assertEqual(record['sha256'], alpha.digest((output / name).read_bytes()))
        journal = json.loads((output / 'web/tests/fixtures/journal/manifest.json').read_text())
        entry = journal['files']['old.mjs']
        receipt = (output / entry['qualification']).read_bytes()
        self.assertEqual(alpha.digest(receipt), entry['qualification_sha256'])
        self.assertEqual(json.loads(receipt)['runnable_sources_sha256'][entry['qualified_source']], entry['sha256'])
        self.assertIn('--source-alpha', (output / '.github/workflows/community.yml').read_text())
        self.assertIn('Public instructions', (output / 'mcp/guides.json').read_text())

    def test_full_python_discovery_is_preserved_without_duplicate_exporter_step(self):
        path = self.root / '.github/workflows/community.yml'
        path.write_text("      - run: python3 scripts/check-open-source.py\n"
                        "      - run: python3 -m unittest discover -s scripts -p 'test_*.py' -v\n")
        self.run_git('add', '.github/workflows/community.yml')
        self.run_git('commit', '-qm', 'Use complete test discovery')
        self.export('all-python-tests')
        output = self.base / 'all-python-tests'
        workflow = (output / '.github/workflows/community.yml').read_text()
        self.assertIn("-p 'test_*.py'", workflow)
        self.assertEqual(workflow.count('unittest discover'), 1)
        self.assertIn('--source-alpha', workflow)

    def test_tracked_but_omitted_rust_include_asset_refuses_before_output(self):
        self.write('src/lib.rs', b'const QUERY: &str = include_str!("search.sql");\n')
        self.write('src/search.sql', b'SELECT 1;\n')
        self.commit()
        with self.assertRaisesRegex(ValueError, 'Rust include input is not allowlisted'):
            self.export('missing-query')
        self.assertFalse((self.base / 'missing-query').exists())

    def test_omitted_rust_module_refuses_before_output_even_without_literal_include(self):
        self.write('src/lib.rs', b'mod renderer;\n')
        self.write('src/renderer.rs', b'// reviewed synthetic module\n')
        self.commit()
        archive = self.base / 'missing-module.tar.gz'
        with self.assertRaisesRegex(ValueError, 'First-party source is not allowlisted: src/renderer.rs'):
            self.export('missing-module', archive)
        self.assertFalse((self.base / 'missing-module').exists())
        self.assertFalse(archive.exists())

    def test_omitted_python_helper_or_test_cannot_silently_shrink_hosted_checks(self):
        self.write('scripts/helper.py', b'def value(): return 7\n')
        self.write('scripts/test_helper.py', b'import helper\n')
        self.commit()
        with self.assertRaisesRegex(ValueError, 'First-party source is not allowlisted'):
            self.export('missing-python')
        self.assertFalse((self.base / 'missing-python').exists())
        self.policy['files'].append('scripts/helper.py')
        self.commit()
        with self.assertRaisesRegex(ValueError, 'scripts/test_helper.py'):
            self.export('missing-python-test')
        self.assertFalse((self.base / 'missing-python-test').exists())

    def test_explicit_source_review_retains_every_new_module_helper_and_test_byte(self):
        additions = {'src/renderer.rs': b'// reviewed module\n',
                     'scripts/helper.py': b'def value(): return 7\n',
                     'scripts/test_helper.py': b'import helper\n'}
        for name, data in additions.items():
            self.write(name, data)
            self.policy['files'].append(name)
        self.commit()
        self.export('complete-surface')
        for name, data in additions.items():
            self.assertEqual((self.base / 'complete-surface' / name).read_bytes(), data)

    def test_graphics_export_requires_all_demo_and_real_api_checks(self):
        name = 'scripts/check-tui-graphics.py'
        self.write(name, b'# owned synthetic graphics check\n')
        self.policy['files'].append(name)
        workflow = self.files['.github/workflows/community.yml']
        commands = [
            b'      - run: python3 scripts/check-tui-graphics.py target/debug/freight-tui\n',
            b'      - run: python3 scripts/check-tui-graphics.py target/debug/freight-tui --api-binary target/debug/stateset-freight\n',
            b'      - run: python3 scripts/check-tui-graphics.py target/debug/freight-tui --api-binary target/debug/stateset-freight --pdf\n',
        ]
        for number in range(3):
            self.write('.github/workflows/community.yml', workflow + b''.join(commands[:number]))
            self.commit()
            with self.assertRaisesRegex(ValueError, 'Graphics source requires'):
                self.export('missing-graphics-' + str(number))
            self.assertFalse((self.base / ('missing-graphics-' + str(number))).exists())
        self.write('.github/workflows/community.yml', workflow + b''.join(commands))
        self.commit()
        self.export('complete-graphics')

    def test_plain_and_raw_string_byte_and_source_includes_preserve_assets(self):
        self.write('src/lib.rs', b'''const QUERY: &str = include_str!("./search.sql");
const BYTES: &[u8] = include_bytes!(r#"search.sql"#);
include!(r"../src/extra.rs");
''')
        self.write('src/search.sql', b'SELECT 1;\n')
        self.write('src/extra.rs', b'// owned included source\n')
        self.policy['files'] += ['src/search.sql', 'src/extra.rs']
        self.commit()
        self.export('complete-includes')
        self.assertEqual((self.base / 'complete-includes/src/search.sql').read_bytes(), b'SELECT 1;\n')

    def test_rust_include_cannot_escape_the_export(self):
        self.write('src/lib.rs', b'const QUERY: &str = include_str!("../../outside.sql");\n')
        self.commit()
        with self.assertRaisesRegex(ValueError, 'Rust include escapes export'):
            self.export('escaped-include')
        self.assertFalse((self.base / 'escaped-include').exists())

    def test_repeated_archives_are_identical_and_regular_source_only(self):
        first, second = self.base / 'a.tar.gz', self.base / 'b.tar.gz'
        self.export('a', first); self.export('b', second)
        self.assertEqual(first.read_bytes(), second.read_bytes())
        self.assertEqual(first.stat().st_mode & 0o777, 0o600)
        with tarfile.open(str(first)) as archive:
            for member in archive.getmembers():
                self.assertTrue(member.isfile())
                self.assertEqual(member.uid, 0)
                self.assertEqual(member.mtime, 0)
                self.assertTrue(member.name.startswith('stateset-freight-source-alpha/'))

    def test_never_overwrites_directory_or_archive(self):
        self.export()
        with self.assertRaisesRegex(ValueError, 'already exist'):
            self.export()
        archive = self.base / 'existing.tar.gz'
        archive.write_bytes(b'existing artifact')
        with self.assertRaisesRegex(ValueError, 'new path'):
            self.export('other', archive)
        self.assertEqual(archive.read_bytes(), b'existing artifact')
        self.assertFalse((self.base / 'other').exists())

    def test_forbidden_paths_and_unsafe_components(self):
        for name in ['../private', '/private', 'docs/evidence/log.json', '.env', 'target/api',
                     'web/node_modules/a', 'x/../y', 'x\\y', '.git/config', '.Git/config', 'trace.log']:
            with self.subTest(name=name), self.assertRaises(ValueError):
                alpha.safe_path(name)

    def test_missing_or_duplicate_allowlist_entries_are_refused(self):
        for extra in ['missing.rs', 'src/lib.rs']:
            self.policy['files'].append(extra)
            self.commit()
            with self.subTest(extra=extra), self.assertRaises(ValueError):
                self.export()
            self.policy['files'].pop()

    def test_tracked_symlink_cannot_export_private_file_contents(self):
        (self.root / 'src/lib.rs').unlink()
        (self.root / 'src/lib.rs').symlink_to('../.env')
        self.commit()
        with self.assertRaisesRegex(ValueError, 'regular Git files'):
            self.export()
        self.assertFalse((self.base / 'candidate').exists())

    def test_broken_overlay_link_fails_and_cleans_only_new_output(self):
        self.write('publication/source-alpha/public.md', b'[Broken](missing.md)\n')
        self.commit()
        sentinel = self.base / 'keep'
        sentinel.write_text('keep')
        with self.assertRaisesRegex(ValueError, 'Broken export'):
            self.export()
        self.assertFalse((self.base / 'candidate').exists())
        self.assertEqual(sentinel.read_text(), 'keep')

    def test_unknown_documentation_link_is_not_silently_removed(self):
        self.write('README.md', b'[Unknown](missing.md)\n')
        self.commit()
        with self.assertRaisesRegex(ValueError, 'Unresolved source'):
            self.export()

    def test_fixture_hash_drift_is_refused(self):
        self.write('web/tests/fixtures/journal/old.mjs', b'changed bytes')
        self.commit()
        with self.assertRaisesRegex(ValueError, 'fixture hash mismatch'):
            self.export()

    def test_document_templates_cannot_replace_business_source(self):
        self.policy['overlays']['src/lib.rs'] = 'publication/source-alpha/public.md'
        self.commit()
        with self.assertRaisesRegex(ValueError, 'Only Markdown'):
            self.export()

    def test_binary_and_lfs_inputs_need_explicit_review(self):
        for data in [b'\x7fELF\0unreviewed binary', b'version https://git-lfs.github.com/spec/v1\noid sha256:synthetic\n']:
            self.write('src/lib.rs', data)
            self.commit()
            with self.subTest(data=data), self.assertRaisesRegex(ValueError, 'binary|LFS'):
                self.export()

    def test_candidate_and_archive_must_stay_outside_checkout(self):
        with self.assertRaisesRegex(ValueError, 'outside the input'):
            alpha.prepare(self.root, 'HEAD', self.root / 'candidate')
        with self.assertRaisesRegex(ValueError, 'outside the input'):
            self.export(archive=self.root / 'candidate.tar.gz')

    def test_explicit_old_commit_ignores_later_commits(self):
        original = self.run_git('rev-parse', 'HEAD').decode().strip()
        self.write('src/lib.rs', b'// newer source\n')
        self.commit()
        result = self.export(ref=original)
        self.assertEqual(result['input_commit'], original)
        self.assertEqual((self.base / 'candidate/src/lib.rs').read_bytes(), self.files['src/lib.rs'])

    def test_ambient_git_variables_cannot_redirect_the_input_checkout(self):
        with mock.patch.dict(os.environ, {'GIT_DIR': str(self.base / 'wrong.git'),
                                         'GIT_WORK_TREE': str(self.base / 'wrong-tree')}):
            result = self.export()
        self.assertEqual(result['input_commit'], self.run_git('rev-parse', 'HEAD').decode().strip())


if __name__ == '__main__':
    unittest.main()
