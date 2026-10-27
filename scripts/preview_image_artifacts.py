"""Package only this image gate's diagnostics, never Docker client state or old runs."""
import hashlib
import io
import json
from pathlib import Path
import stat
import tarfile

TOP_LEVEL = {'source-before.json', 'result.json', 'build.log', 'qualification.log',
             'qualification.json', 'failure.log'}
RUNTIME_FILES = {'plan.json', 'fixture-binding.json', 'runtime-binding.json',
                 'runtime-policy.json', 'resource-final.json', 'cleanup.json', 'api.log', 'failure.log',
                 'renderer-binding.json', 'renderer-resource-final.json',
                 'renderer-cleanup.json', 'renderer.log'}


def bundle(directory):
    directory = Path(directory)
    if directory.is_symlink() or not directory.is_dir():
        raise RuntimeError('Diagnostics root must be an owned directory')
    for name in ['diagnostics.tar.gz', 'diagnostics.sha256']:
        if (directory / name).exists() or (directory / name).is_symlink():
            raise RuntimeError('Preserve existing diagnostics bundle')
    selected = []
    for entry in sorted(directory.iterdir()):
        if entry.name in TOP_LEVEL:
            selected.append(entry)
        elif entry.name.startswith('freight-preview-qualification-'):
            if entry.is_symlink() or not entry.is_dir():
                raise RuntimeError('Invalid preview diagnostics directory')
            selected.extend(entry / name for name in sorted(RUNTIME_FILES)
                            if (entry / name).exists() or (entry / name).is_symlink())
    records = []
    contents = []
    for path in selected:
        # Never follow links, recurse into Docker configuration, or accept special files.
        if not stat.S_ISREG(path.lstat().st_mode):
            raise RuntimeError('Diagnostics must contain only regular files: ' + path.name)
        raw = path.read_bytes()
        name = path.relative_to(directory).as_posix()
        records.append(dict(path=name, bytes=len(raw), sha256=hashlib.sha256(raw).hexdigest()))
        contents.append((name, raw))
    manifest = dict(schema_version=1, files=records,
                    scope='Exact retained image-build and preview diagnostics only; '
                          'result.json determines gate outcome. No Docker client settings, '
                          'other runner directories or prior runs are collected.')
    contents.append(('diagnostics-manifest.json',
                     (json.dumps(manifest, indent=2, sort_keys=True) + '\n').encode()))
    archive = directory / 'diagnostics.tar.gz'
    with archive.open('xb') as stream:
        with tarfile.open(fileobj=stream, mode='w:gz') as output:
            for name, raw in contents:
                info = tarfile.TarInfo(name)
                info.size = len(raw)
                info.mode = 0o600
                output.addfile(info, io.BytesIO(raw))
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    with (directory / 'diagnostics.sha256').open('x') as stream:
        stream.write(digest + '  diagnostics.tar.gz\n')
    return archive
