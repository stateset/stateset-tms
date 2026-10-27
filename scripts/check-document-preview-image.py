#!/usr/bin/env python3
"""Build the current production Dockerfile and qualify its PDF fault behavior.

The standalone qualifier can inspect any supplied local candidate. This gate
always builds the current checkout and binds the image to unchanged build inputs.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import traceback
import uuid

from preview_image_artifacts import bundle
from runtime_contracts import ROOT, fingerprint, require


def source():
    return dict(application=fingerprint(ROOT), docker_inputs={name: hashlib.sha256(
        (ROOT / name).read_bytes()).hexdigest() for name in ['Dockerfile', '.dockerignore']},
        qualification_inputs={name: hashlib.sha256((ROOT / name).read_bytes()).hexdigest()
            for name in ['scripts/check-document-preview-image.py',
                         'scripts/preview_image_artifacts.py',
                         'scripts/qualify-document-previews.py',
                         'scripts/document_preview_runtime.py',
                         'scripts/runtime_contracts.py',
                         'tests/fixtures/documents/preview-text.pdf']})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, help='Fresh artifact directory; never reused or overwritten')
    args = parser.parse_args()
    if args.output is None:
        directory = Path(tempfile.mkdtemp(prefix='freight-preview-image-check-'))
    else:
        require(not args.output.is_symlink() and not args.output.exists(),
                'Preserve image diagnostics; choose a fresh output directory')
        args.output.mkdir(mode=0o700)
        directory = args.output.resolve()
    directory.chmod(0o700)
    execute(directory)


def execute(directory):
    report = dict(schema_version=1, result='failure', artifacts=str(directory),
                  phases={name: 'not_started' for name in ['build', 'qualification', 'verification']},
                  scope='Current complete production Dockerfile build and finite preview '
                        'fault/overload checks; not a full release matrix, sustained capacity '
                        'or independent security acceptance.')
    try:
        cli = directory / 'docker-client'
        cli.mkdir(mode=0o700)
        docker = shutil.which('docker')
        require(docker, 'Docker CLI is required for the production image gate')
        prefix = [docker, '--host', 'unix:///var/run/docker.sock', '--config', str(cli)]
        environment = {'PATH': os.environ['PATH'], 'LC_ALL': 'C'}
        before = source()
        (directory / 'source-before.json').write_text(json.dumps(before, indent=2, sort_keys=True) + '\n')
        tag = 'freight-preview-release-check:' + uuid.uuid4().hex
        commands = [prefix + ['build', '--progress=plain', '--label',
                              'com.stateset.freight.preview-build=' + tag, '--tag', tag, str(ROOT)],
                    [sys.executable, str(ROOT / 'scripts/qualify-document-previews.py'),
                     '--image', tag, '--output', str(directory / 'qualification.json')]]
        report.update(application_source=before, image_tag=tag, commands=commands)
        for name, command in zip(['build', 'qualification'], commands):
            report['phases'][name] = 'running'
            with (directory / (name + '.log')).open('x') as stream:
                subprocess.run(command, cwd=str(ROOT), env=environment, check=True,
                               stdout=stream, stderr=subprocess.STDOUT)
            report['phases'][name] = 'pass'
        report['phases']['verification'] = 'running'
        require(source() == before, 'Build inputs changed during image qualification')
        qualification = json.loads((directory / 'qualification.json').read_text())
        require(qualification['result'] == 'pass', 'Image fault qualification failed')
        report['phases']['verification'] = 'pass'
        report.update(result='pass', image_id=qualification['binding']['image'])
    except BaseException as error:
        for name, state in report['phases'].items():
            if state == 'running':
                report['phases'][name] = 'failure'
        report['error'] = str(error)
        (directory / 'failure.log').write_text(traceback.format_exc())
        raise
    finally:
        with (directory / 'result.json').open('x') as stream:
            json.dump(report, stream, indent=2, sort_keys=True)
            stream.write('\n')
        print('Production image preview qualification:', directory / 'result.json', flush=True)
        archive = bundle(directory)
        print('Downloadable diagnostics:', archive, flush=True)


if __name__ == '__main__':
    main()
