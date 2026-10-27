"""Publication tools must preserve notices and never silently clear review gaps."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest import mock


def load(name):
    path = Path(__file__).parent / (name.replace('_', '-') + '.py')
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


notices = load('collect_third_party_notices')
history = load('review_publication_history')


class NoticeTests(unittest.TestCase):
    def test_nested_license_directory_and_native_supplements_are_retained(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); package = root / 'native'; package.mkdir()
            (package / 'LICENSES').mkdir(); (package / 'LICENSES/MIT.txt').write_text('owned license text')
            (package / 'README.md').write_text('Bundled components require separate terms review')
            (package / 'versions.json').write_text('{"vips":"owned"}')
            record = notices.collect_package(package, root / 'output', {'name': '@img/sharp-libvips-owned', 'license': 'LGPL-3.0-or-later'})
            self.assertEqual([x['path'] for x in record['notices']], ['LICENSES/MIT.txt'])
            self.assertEqual({x['path'] for x in record['supplements']}, {'README.md', 'versions.json'})
            self.assertIn('bundled_native_libraries_require_separate_review', record['review_reasons'])
            self.assertIn('license_requires_distribution_review', record['review_reasons'])

    def test_missing_text_is_a_review_gap_even_with_license_metadata(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            record = notices.collect_package(root, root / 'output', {'name': 'owned', 'license': 'MIT'})
            self.assertEqual(record['review_reasons'], ['missing_license_text'])

    def test_symlinked_notice_file_and_directory_are_refused(self):
        for directory in [False, True]:
            with self.subTest(directory=directory), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary); package = root / 'package'; package.mkdir()
                external = root / 'private'; external.mkdir() if directory else external.write_text('private synthetic material')
                (package / ('LICENSES' if directory else 'LICENSE')).symlink_to(external, target_is_directory=directory)
                with self.assertRaisesRegex(ValueError, 'symlink|regular files'):
                    notices.notice_files(package)

    def make_npm(self, root, optional=False):
        package = root / 'web'; package.mkdir()
        location = 'node_modules/owned'
        locked = {'version': '1.0.0', 'license': 'MIT', 'optional': optional}
        (package / 'package-lock.json').write_text(json.dumps({'packages': {'': {}, location: locked}}))
        return package / location, locked

    def test_absent_optional_packages_are_visible_and_required_absence_fails(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); self.make_npm(root, optional=True)
            records, absent = notices.npm_records(root, 'web', root / 'output')
            self.assertEqual(records, []); self.assertEqual(len(absent), 1)
            data = json.loads((root / 'web/package-lock.json').read_text())
            data['packages']['node_modules/owned']['optional'] = False
            (root / 'web/package-lock.json').write_text(json.dumps(data))
            with self.assertRaisesRegex(ValueError, 'required locked npm package'):
                notices.npm_records(root, 'web', root / 'output')

    def test_installed_name_version_license_and_symlink_drift_are_refused(self):
        for field, wrong in [('name', 'different'), ('version', '2.0.0'), ('license', 'GPL-3.0-only')]:
            with self.subTest(field=field), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary); package, locked = self.make_npm(root)
                package.mkdir(parents=True); data = dict(locked, name='owned'); data[field] = wrong
                (package / 'package.json').write_text(json.dumps(data))
                with self.assertRaisesRegex(ValueError, 'differs'):
                    notices.npm_records(root, 'web', root / 'output')
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); package, locked = self.make_npm(root)
            package.parent.mkdir(); external = root / 'external'; external.mkdir(); package.symlink_to(external)
            with self.assertRaisesRegex(ValueError, 'symlinked npm package'):
                notices.npm_records(root, 'web', root / 'output')

    def test_unsafe_lock_paths_are_refused(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); (root / 'web').mkdir()
            (root / 'web/package-lock.json').write_text(json.dumps({'packages': {'../private': {'version': '1'}}}))
            with self.assertRaisesRegex(ValueError, 'unsafe npm lockfile'):
                notices.npm_records(root, 'web', root / 'output')

    def test_review_gate_retains_manifest_and_never_overwrites(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / 'Cargo.lock').write_text('owned')
            for folder in ['web', 'mcp']:
                (root / folder).mkdir(); (root / folder / 'package-lock.json').write_text('{"packages":{}}')
            package = root / 'dependency'; package.mkdir()
            metadata = {'workspace_members': ['owned'], 'resolve': {'nodes': [{'id': 'owned', 'dependencies': []}]},
                        'packages': [{'id': 'owned', 'name': 'owned', 'version': '1', 'license': None,
                                      'manifest_path': str(package / 'Cargo.toml')}]}
            for name in ['LICENSE', 'LICENSE-MIT', 'LICENSE-APACHE']:
                (root / name).write_text('owned license')
            output = root / 'bundle'
            args = ['collect', '--target', 'owned-target', '--output', str(output), '--fail-on-review']
            with mock.patch.object(notices, 'ROOT', root), mock.patch('sys.argv', args), \
                 mock.patch.object(notices.subprocess, 'check_output', return_value=json.dumps(metadata).encode()):
                self.assertEqual(notices.main(), 2)
                data = json.loads((output / 'manifest.json').read_text())
                self.assertFalse(data['distribution_compliance_attested'])
                self.assertEqual(data['distribution_review_required'][0]['reasons'], ['missing_license_expression'])
                with self.assertRaises(FileExistsError):
                    notices.main()
            self.assertTrue((output / 'manifest.json').exists())


class UpstreamTests(unittest.TestCase):
    def fixture(self, root, role='license', expression='MIT'):
        package = root / 'package'; package.mkdir()
        (package / 'package.json').write_text('owned synthetic release metadata')
        material = root / 'third-party/LICENSE.txt'; material.parent.mkdir()
        material.write_text('owned synthetic upstream license text')
        entry = {'name': 'owned', 'version': '1', 'license': expression,
                 'release_integrity': 'sha512-owned-release',
                 'registry_url': 'https://registry.example/owned/1',
                 'package_files_sha256': {'package.json': notices.hash_file(package / 'package.json')},
                 'materials': [{'path': 'third-party/LICENSE.txt', 'role': role,
                                'source_url': 'https://upstream.example/pinned/LICENSE',
                                'sha256': notices.hash_file(material)}]}
        (root / 'third-party/npm-notices.json').write_text(json.dumps({'schema_version': 1, 'entries': [entry]}))
        record = {'name': 'owned', 'version': '1', 'license': expression, 'ecosystem': 'npm'}
        return package, material, entry, record

    def test_pinned_license_resolves_text_gap_and_copies_provenance(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); package, material, entry, record = self.fixture(root)
            result = notices.collect_package(package, root / 'bundle', record,
                     upstream=(root, entry['release_integrity'], notices.load_catalog(root)))
            self.assertNotIn('missing_license_text', result['review_reasons'])
            self.assertEqual(result['notice_origin'], 'package files')
            self.assertEqual(result['notices'], [])
            self.assertEqual((root / 'bundle/upstream/LICENSE.txt').read_bytes(), material.read_bytes())
            self.assertEqual(result['upstream_release_binding']['package_files_verified'], 1)

    def test_recipe_license_and_attribution_do_not_substitute_for_package_license(self):
        for role in ['build_recipe_license', 'attribution', 'reference_license']:
            with self.subTest(role=role), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary); package, material, entry, record = self.fixture(root, role, 'LGPL-3.0-or-later')
                result = notices.collect_package(package, root / 'bundle', record,
                         upstream=(root, entry['release_integrity'], notices.load_catalog(root)))
                self.assertIn('missing_license_text', result['review_reasons'])
                self.assertIn('license_requires_distribution_review', result['review_reasons'])

    def test_release_file_and_integrity_drift_are_refused(self):
        for kind in ['integrity', 'installed', 'material']:
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary); package, material, entry, record = self.fixture(root)
                catalog = notices.load_catalog(root)
                integrity = entry['release_integrity']
                if kind == 'integrity': integrity = 'sha512-different-release'
                if kind == 'installed': (package / 'package.json').write_text('modified')
                if kind == 'material': material.write_text('modified')
                with self.assertRaisesRegex(ValueError, 'binding|changed'):
                    notices.collect_package(package, root / 'bundle', record, upstream=(root, integrity, catalog))

    def test_unsafe_symlinked_and_modified_catalog_materials_are_refused(self):
        for kind in ['traversal', 'symlink', 'modified', 'duplicate', 'role', 'source']:
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary); package, material, entry, record = self.fixture(root)
                if kind == 'traversal': entry['materials'][0]['path'] = '../private'
                if kind == 'symlink':
                    material.unlink(); material.symlink_to(package / 'package.json')
                if kind == 'modified': material.write_text('different bytes')
                if kind == 'role': entry['materials'][0]['role'] = 'clear-all-reviews'
                if kind == 'source': entry['materials'][0]['source_url'] = 'https://user:owned-secret@upstream.example/LICENSE'
                entries = [entry, entry] if kind == 'duplicate' else [entry]
                (root / 'third-party/npm-notices.json').write_text(json.dumps({'schema_version': 1, 'entries': entries}))
                with self.assertRaises(ValueError): notices.load_catalog(root)

    def test_native_magic_and_filename_require_review_even_with_pinned_mit_text(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); package, material, entry, record = self.fixture(root)
            (package / 'engine.node').write_bytes(b'\x7fELF-owned')
            (package / 'unnamed-engine').write_bytes(b'\x00asm-owned')
            (package / 'library.so.2').write_bytes(b'owned shared library fixture')
            (package / 'source.js').write_text('owned JavaScript')
            result = notices.collect_package(package, root / 'bundle', record,
                     upstream=(root, entry['release_integrity'], notices.load_catalog(root)))
            self.assertIn('native_artifacts_require_distribution_review', result['review_reasons'])
            self.assertNotIn('missing_license_text', result['review_reasons'])
            self.assertEqual({x['path'] for x in result['native_artifacts']}, {'engine.node', 'unnamed-engine', 'library.so.2'})
            self.assertFalse((root / 'bundle/engine.node').exists())

    def test_native_scan_refuses_file_and_directory_symlinks(self):
        for directory in [False, True]:
            with self.subTest(directory=directory), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary); package = root / 'package'; package.mkdir()
                external = root / 'private'; external.mkdir() if directory else external.write_bytes(b'\x7fELF-owned')
                (package / ('lib' if directory else 'engine.node')).symlink_to(external, target_is_directory=directory)
                with self.assertRaisesRegex(ValueError, 'symlink|regular'): notices.native_artifacts(package)

    def test_component_version_drift_and_missing_copyright_remain_visible(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); package, material, entry, record = self.fixture(root)
            (package / 'versions.json').write_text('{"vips":"2"}')
            entry['native_component_versions'] = {'vips': '1'}
            catalog = {('owned', '1'): entry}
            with self.assertRaisesRegex(ValueError, 'component versions'):
                notices.collect_package(package, root / 'bundle', record,
                         upstream=(root, entry['release_integrity'], catalog))
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); package, material, entry, record = self.fixture(root)
            entry['materials'] = []; entry['copyright_statement'] = 'not supplied; no holder inferred'
            result = notices.collect_package(package, root / 'bundle', record,
                     upstream=(root, entry['release_integrity'], {('owned', '1'): entry}))
            self.assertIn('missing_license_text', result['review_reasons'])
            self.assertEqual(result['copyright_statement'], entry['copyright_statement'])


class HistoryTests(unittest.TestCase):
    def test_redacted_pointers_exclude_documentation_endpoints(self):
        data = b'user@customer-company.com\n/home/fixture/private/\nhttps://fixture:owned-pass@db.internal/\n127.0.0.1 192.0.2.1\n'
        result = history.classify(data)
        self.assertEqual(result['email_address'], [1, 3])
        self.assertEqual(result['personal_home_path'], [2])
        self.assertEqual(result['credential_url'], [3])
        self.assertEqual(result['internal_hostname'], [3])
        self.assertNotIn('ipv4_address', result)
        self.assertNotIn('owned-pass', json.dumps(result))
        self.assertNotIn('email_address', history.classify(b'a@example.com b@carrier.example security@stateset.io'))

    def test_long_packed_text_preserves_later_review_pointers(self):
        result = history.classify(b'A' * (1024 * 1024) + b'\nperson@customer-company.com\n')
        self.assertEqual(result['email_address'], [2])

    def test_private_review_output_is_exclusive_and_owner_readable_only(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / 'private-review.json'
            result = {'commits': 1, 'unique_blobs': 1, 'review_candidate_blobs': 0}
            with mock.patch.object(history, 'audit', return_value=result), \
                 mock.patch('sys.argv', ['review', '--output', str(output)]):
                history.main()
                self.assertEqual(output.stat().st_mode & 0o777, 0o600)
                with self.assertRaises(FileExistsError):
                    history.main()
            self.assertEqual(json.loads(output.read_text()), result)

    def test_deleted_history_binary_keys_and_oversized_blobs_remain_reviewable(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            subprocess.run(['git', 'init', '-q', str(root)], check=True)
            environment = dict(os.environ, GIT_AUTHOR_NAME='Owned fixture', GIT_AUTHOR_EMAIL='fixture@example.com',
                               GIT_COMMITTER_NAME='Owned fixture', GIT_COMMITTER_EMAIL='fixture@example.com')
            def run(*args):
                subprocess.run(['git', '-C', str(root), *args], env=environment, check=True, capture_output=True)
            (root / 'removed.txt').write_text('person@customer-company.com\n')
            (root / 'key.der').write_bytes(b'\x00owned public test key')
            run('add', '.'); run('commit', '-qm', 'owned first')
            (root / 'removed.txt').unlink(); (root / 'large.txt').write_text('x' * 256)
            run('add', '-A'); run('commit', '-qm', 'owned second')
            result = history.audit(root, max_blob_bytes=128)
            self.assertEqual(result['commits'], 2)
            deleted = next(x for x in result['candidates'] if 'removed.txt' in x['paths'])
            self.assertFalse(deleted['present_at_head']); self.assertIn('email_address', deleted['review_categories'])
            key = next(x for x in result['candidates'] if 'key.der' in x['paths'])
            self.assertIn('binary_requires_manual_review', key['review_categories'])
            self.assertIn('key_or_certificate_file', key['review_categories'])
            self.assertTrue(any('oversize_uninspected' in x['review_categories'] for x in result['candidates']))
            self.assertFalse(result['publication_cleared'])
            self.assertNotIn('person@customer-company.com', json.dumps(result))


if __name__ == '__main__':
    unittest.main()
