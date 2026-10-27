#!/usr/bin/env python3
"""Verify a pinned source archive and create a local Git repository with one root commit."""
import argparse
import gzip
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tarfile

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location('alpha_export', Path(__file__).with_name('prepare-source-alpha.py'))
alpha = importlib.util.module_from_spec(spec)
spec.loader.exec_module(alpha)
MAX_EXPANDED = 64 * 1024 * 1024
MAX_FILE = 16 * 1024 * 1024
MAX_FILES = 4096


def sha(data):
    return hashlib.sha256(data).hexdigest()


def file_sha(path):
    digest = hashlib.sha256()
    with path.open('rb') as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b''):
            digest.update(block)
    return digest.hexdigest()


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError('Duplicate manifest field')
        result[key] = value
    return result


class BoundedReader:
    def __init__(self, source):
        self.source = source
        self.remaining = MAX_EXPANDED

    def read(self, size=-1):
        requested = self.remaining + 1 if size < 0 else min(size, self.remaining + 1)
        data = self.source.read(requested)
        self.remaining -= len(data)
        if self.remaining < 0:
            raise ValueError('Expanded archive exceeds the review limit')
        return data


def verify_archive(path, expected_sha256):
    if not re.fullmatch(r'[a-f0-9]{64}', expected_sha256):
        raise ValueError('A complete lowercase SHA-256 pin is required')
    if path.is_symlink() or not path.is_file() or file_sha(path) != expected_sha256:
        raise ValueError('Source archive differs from the reviewed SHA-256 pin')
    files, modes = {}, {}
    with path.open('rb') as source, gzip.GzipFile(fileobj=source) as uncompressed:
        limited = BoundedReader(uncompressed)
        with tarfile.open(fileobj=limited, mode='r|') as archive:
            for member in archive:
                prefix = 'stateset-freight-source-alpha/'
                if not member.isfile() or not member.name.startswith(prefix):
                    raise ValueError('Only regular source files under the alpha root are allowed')
                name = alpha.safe_path(member.name[len(prefix):])
                if name in files or len(files) >= MAX_FILES:
                    raise ValueError('Duplicate or excessive archive members')
                if not 0 <= member.size <= MAX_FILE or member.mode not in {0o644, 0o755}:
                    raise ValueError('Unexpected source file size or mode')
                if member.uid or member.gid or member.mtime or member.uname or member.gname:
                    raise ValueError('Source archive metadata must be normalized')
                data = archive.extractfile(member).read()
                if len(data) != member.size:
                    raise ValueError('Truncated source archive member')
                alpha.validate_blob(name, data)
                files[name] = data
                modes[name] = member.mode
            # Read buffered padding and the gzip trailer too. Hidden concatenated
            # payloads must not escape the manifest merely because TAR hit EOF.
            while True:
                block = archive.fileobj.read(65536)
                if not block:
                    break
                if block.strip(b'\0'):
                    raise ValueError('Unexpected data after the source TAR')
    if alpha.MANIFEST not in files:
        raise ValueError('Source manifest is missing')
    manifest = json.loads(files[alpha.MANIFEST], object_pairs_hook=unique_object)
    if not isinstance(manifest, dict):
        raise ValueError('Unsupported source candidate manifest')
    records = manifest.get('files')
    if (type(manifest.get('schema')) is not int or manifest['schema'] != 1
            or manifest.get('manifest_excludes_itself') is not True
            or manifest.get('publication_cleared') is not False
            or manifest.get('distribution_compliance_attested') is not False
            or not re.fullmatch(r'(?:[a-f0-9]{40}|[a-f0-9]{64})', str(manifest.get('input_commit', '')))
            or not isinstance(records, dict) or not records):
        raise ValueError('Unsupported source candidate manifest')
    if set(files) != set(records) | {alpha.MANIFEST}:
        raise ValueError('Manifest does not bind the exact archive file set')
    for name, record in records.items():
        alpha.safe_path(name)
        if (not isinstance(record, dict) or type(record.get('bytes')) is not int or type(record.get('executable')) is not bool
                or record['bytes'] != len(files[name]) or record.get('sha256') != sha(files[name])
                or modes[name] != (0o755 if record['executable'] else 0o644)):
            raise ValueError('Source file hash, size or mode differs from the manifest: ' + name)
    if any(name.startswith('.github/workflows/') and name != '.github/workflows/community.yml' for name in files):
        raise ValueError('Only the reviewed community workflow may be staged')
    ignored = files.get('.gitleaksignore', b'').decode('utf-8')
    if any(line.strip() and not line.lstrip().startswith('#') for line in ignored.splitlines()):
        raise ValueError('Source candidate must not carry historical scanner fingerprints')
    return manifest, files, modes


def stage(archive, expected_sha256, output, report, author_name, author_email):
    archive, output, report = Path(archive), Path(output).absolute(), Path(report).absolute()
    if output.exists() or output.is_symlink() or report.exists() or report.is_symlink():
        raise ValueError('Staging directory and private report must be new paths')
    if output == report or output in report.parents or output in archive.absolute().parents:
        raise ValueError('Archive and report must stay outside the staging directory')
    if any(part.casefold() == '.git' for part in output.parts):
        raise ValueError('Stage outside Git metadata directories')
    if (not author_name.strip() or any(c in author_name for c in '\r\n<>')
            or not re.fullmatch(r'[^@\s<>]+@[^@\s<>]+\.[^@\s<>]+', author_email)):
        raise ValueError('Provide an explicit public commit author name and email')
    manifest, files, modes = verify_archive(archive, expected_sha256)
    env = alpha.git_environment()
    env.update(GIT_CONFIG_NOSYSTEM='1', GIT_CONFIG_GLOBAL=os.devnull, GIT_TERMINAL_PROMPT='0',
               GIT_AUTHOR_NAME=author_name, GIT_AUTHOR_EMAIL=author_email,
               GIT_COMMITTER_NAME=author_name, GIT_COMMITTER_EMAIL=author_email)

    def git(*args):
        return subprocess.check_output(['git', '-C', str(output), *args], env=env, stderr=subprocess.PIPE)

    created = False
    try:
        output.mkdir(mode=0o700)
        created = True
        for name, data in files.items():
            alpha.write_file(output, name, data, modes[name] == 0o755)
        git('init', '--quiet', '--template=')
        git('symbolic-ref', 'HEAD', 'refs/heads/main')
        git('config', 'core.hooksPath', os.devnull)
        git('config', 'core.autocrlf', 'false')
        git('config', 'commit.gpgSign', 'false')
        git('add', '--force', '--all')
        git('commit', '--quiet', '-m', 'StateSet Freight developer alpha source snapshot')
        commit, tree = alpha.tree_at(output, 'HEAD')
        if set(tree) != set(files) or git('rev-list', '--all', '--count').strip() != b'1' or git('remote').strip():
            raise ValueError('Staging must contain exactly one root commit and no remotes')
        stored = alpha.read_inputs(output, tree, sorted(tree))
        if any(stored[name] != files[name] or (tree[name][0] == '100755') != (modes[name] == 0o755) for name in files):
            raise ValueError('Git conversion changed source bytes or modes')
        git('fsck', '--full', '--strict')
        result = {'schema': 1, 'archive_sha256': expected_sha256,
                  'manifest_sha256': sha(files[alpha.MANIFEST]), 'source_input_commit': manifest['input_commit'],
                  'root_commit': commit, 'tree': git('rev-parse', 'HEAD^{tree}').decode().strip(),
                  'files': len(files), 'reachable_commits': 1, 'remotes': [],
                  'author': {'name': author_name, 'email': author_email},
                  'publication_cleared': False, 'distribution_compliance_attested': False}
        descriptor = os.open(str(report), os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(descriptor, 'wb') as handle:
            handle.write(alpha.encoded(result))
        return result
    except BaseException:
        if created:
            shutil.rmtree(str(output))
        raise


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--archive', type=Path, required=True)
    parser.add_argument('--expected-sha256', required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--report', type=Path, required=True)
    parser.add_argument('--author-name', required=True)
    parser.add_argument('--author-email', required=True)
    args = parser.parse_args()
    try:
        result = stage(args.archive, args.expected_sha256, args.output, args.report, args.author_name, args.author_email)
    except (ValueError, OSError, KeyError, TypeError, EOFError, tarfile.TarError, subprocess.CalledProcessError) as error:
        raise SystemExit('Source staging failed: ' + str(error))
    print('Staged {} files in one root commit {}; no remote or publication clearance'.format(result['files'], result['root_commit']))


if __name__ == '__main__':
    main()
