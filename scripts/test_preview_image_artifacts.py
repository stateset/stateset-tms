"""Retained image-gate evidence must distinguish failure and preserve exact bytes."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch

from preview_image_artifacts import bundle

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location('image_gate', ROOT / 'scripts/check-document-preview-image.py')
GATE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GATE)


def contents(directory):
    archive = directory / 'diagnostics.tar.gz'
    digest = (directory / 'diagnostics.sha256').read_text().split()[0]
    if hashlib.sha256(archive.read_bytes()).hexdigest() != digest:
        raise AssertionError('Archive digest mismatch')
    with tarfile.open(str(archive), 'r:gz') as stream:
        members = stream.getmembers()
        if any(not member.isfile() or member.name.startswith('/') or '..' in Path(member.name).parts
               for member in members):
            raise AssertionError('Unsafe archive member')
        result = {member.name: stream.extractfile(member).read() for member in members}
    manifest = json.loads(result.pop('diagnostics-manifest.json'))
    if len(manifest['files']) != len(result):
        raise AssertionError('Inventory mismatch')
    for row in manifest['files']:
        raw = result[row['path']]
        if len(raw) != row['bytes'] or hashlib.sha256(raw).hexdigest() != row['sha256']:
            raise AssertionError('Member digest mismatch')
    return result


class ImageArtifacts(unittest.TestCase):
    def test_exact_raw_bytes_survive_and_docker_state_old_runs_and_ambient_files_are_excluded(self):
        with tempfile.TemporaryDirectory() as parent:
            directory = Path(parent) / 'run'
            directory.mkdir()
            wanted = {'result.json': b'{"result":"failure"}\n', 'build.log': b'raw\r\n\x00\xff\n',
                      'freight-preview-qualification-current/api.log': b'raw API trace\r\n',
                      'freight-preview-qualification-current/runtime-policy.json': b'{"filesystem_isolation":false}\n',
                      'freight-preview-qualification-current/renderer.log': b'isolated renderer trace\r\n',
                      'freight-preview-qualification-current/renderer-binding.json': b'{"network":"none"}\n',
                      'freight-preview-qualification-current/renderer-resource-final.json': b'{"oom_kill":0}\n',
                      'freight-preview-qualification-current/renderer-cleanup.json': b'{"container_removed":true}\n',
                      'freight-preview-qualification-current/cleanup.json': b'{"container_removed":true}\n'}
            excluded = ['docker-client/config.json', 'freight-preview-qualification-current/docker-client/config.json',
                        'freight-preview-qualification-current/unknown.json', '.env', 'unrelated/build.log']
            for name, raw in list(wanted.items()) + [(name, b'never export') for name in excluded]:
                path = directory / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(raw)
            (Path(parent) / 'old-run.log').write_bytes(b'never export old run')
            bundle(directory)
            self.assertEqual(contents(directory), wanted)
            for name, raw in wanted.items():
                self.assertEqual((directory / name).read_bytes(), raw)
            with self.assertRaises(RuntimeError):
                bundle(directory)
            self.assertEqual(contents(directory), wanted)

    def test_selected_links_directories_and_special_files_are_refused_without_following_them(self):
        for kind in ['file_link', 'runtime_link', 'renderer_file_link', 'directory', 'fifo', 'dangling']:
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as parent:
                directory = Path(parent) / 'run'
                directory.mkdir()
                outside = Path(parent) / 'private'
                outside.write_bytes(b'private contents')
                target = directory / 'build.log'
                if kind == 'runtime_link':
                    (directory / 'freight-preview-qualification-foreign').symlink_to(Path(parent), target_is_directory=True)
                elif kind == 'renderer_file_link':
                    runtime = directory / 'freight-preview-qualification-owned'
                    runtime.mkdir()
                    (runtime / 'renderer.log').symlink_to(outside)
                elif kind == 'directory':
                    target.mkdir()
                elif kind == 'fifo':
                    os.mkfifo(str(target))
                else:
                    target.symlink_to(outside if kind == 'file_link' else Path(parent) / 'missing')
                with self.assertRaises(RuntimeError):
                    bundle(directory)
                self.assertFalse((directory / 'diagnostics.tar.gz').exists())
                self.assertEqual(outside.read_bytes(), b'private contents')

    def test_existing_output_files_and_symlinks_are_refused_before_any_build(self):
        with tempfile.TemporaryDirectory() as parent:
            root = Path(parent)
            directory = root / 'previous'
            directory.mkdir()
            original = directory / 'result.json'
            original.write_bytes(b'previous evidence\n')
            link = root / 'linked'
            link.symlink_to(directory, target_is_directory=True)
            file = root / 'file'
            file.write_bytes(b'original file')
            for path in [directory, link, file]:
                result = subprocess.run([sys.executable, str(ROOT / 'scripts/check-document-preview-image.py'),
                                         '--output', str(path)], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(b'Preserve image diagnostics', result.stderr)
            self.assertEqual(original.read_bytes(), b'previous evidence\n')
            self.assertEqual(file.read_bytes(), b'original file')
            self.assertEqual(sorted(path.name for path in root.iterdir()), ['file', 'linked', 'previous'])

    def test_failed_build_preserves_diagnostics_and_never_starts_qualification(self):
        with tempfile.TemporaryDirectory() as parent:
            directory = Path(parent)
            calls = []

            def fail(command, **options):
                calls.append(command)
                options['stdout'].write('synthetic build failure\n')
                self.assertEqual(options['env'].keys(), {'PATH', 'LC_ALL'})
                self.assertIn('--host', command)
                self.assertIn('unix:///var/run/docker.sock', command)
                raise subprocess.CalledProcessError(19, command)

            with patch.object(GATE.shutil, 'which', return_value='/owned/docker'), \
                 patch.object(GATE.subprocess, 'run', side_effect=fail):
                with self.assertRaises(subprocess.CalledProcessError):
                    GATE.execute(directory)
            self.assertEqual(len(calls), 1)
            files = contents(directory)
            report = json.loads(files['result.json'])
            self.assertEqual(report['result'], 'failure')
            self.assertEqual(report['phases'], dict(build='failure', qualification='not_started', verification='not_started'))
            self.assertEqual(files['build.log'], b'synthetic build failure\n')
            self.assertIn(b'CalledProcessError', files['failure.log'])

    def test_qualification_failure_and_mutated_inputs_never_become_a_pass(self):
        for cause in ['qualifier_exit', 'qualifier_failed_report', 'source_changed', 'pass']:
            with self.subTest(cause=cause), tempfile.TemporaryDirectory() as parent:
                directory = Path(parent)
                calls = []

                def simulate(command, **options):
                    calls.append(command)
                    options['stdout'].write('synthetic stage\n')
                    if len(calls) == 2:
                        runtime = directory / 'freight-preview-qualification-owned'
                        runtime.mkdir()
                        (runtime / 'cleanup.json').write_text('{"container_removed":true}\n')
                        (directory / 'qualification.json').write_text(json.dumps(dict(
                            result='failure' if cause != 'pass' else 'pass', binding=dict(image='sha256:synthetic'))))
                        if cause == 'qualifier_exit':
                            raise subprocess.CalledProcessError(23, command)

                before = {'frozen': 'source'}
                after = {'mutated': 'source'} if cause == 'source_changed' else before
                with patch.object(GATE.shutil, 'which', return_value='/owned/docker'), \
                     patch.object(GATE, 'source', side_effect=[before, after]), \
                     patch.object(GATE.subprocess, 'run', side_effect=simulate):
                    if cause == 'pass':
                        GATE.execute(directory)
                    else:
                        with self.assertRaises((subprocess.CalledProcessError, AssertionError)):
                            GATE.execute(directory)
                self.assertEqual(len(calls), 2)
                self.assertIn('build', calls[0])
                self.assertEqual(calls[1][-4], '--image')
                files = contents(directory)
                report = json.loads(files['result.json'])
                self.assertEqual(report['result'], 'pass' if cause == 'pass' else 'failure')
                self.assertIn('freight-preview-qualification-owned/cleanup.json', files)
                if cause == 'qualifier_exit':
                    self.assertEqual(report['phases']['qualification'], 'failure')
                elif cause != 'pass':
                    self.assertEqual(report['phases']['verification'], 'failure')

    def test_missing_docker_is_a_retained_failure_not_an_empty_success(self):
        with tempfile.TemporaryDirectory() as parent:
            directory = Path(parent)
            with patch.object(GATE.shutil, 'which', return_value=None):
                with self.assertRaises(AssertionError):
                    GATE.execute(directory)
            report = json.loads(contents(directory)['result.json'])
            self.assertEqual(report['result'], 'failure')
            self.assertTrue(all(state == 'not_started' for state in report['phases'].values()))


if __name__ == '__main__':
    unittest.main()
