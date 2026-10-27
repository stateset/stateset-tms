#!/usr/bin/env python3
"""Prepare a private, deterministic source-alpha candidate from an immutable Git tree."""
import argparse
import gzip
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import posixpath
import re
import shutil
import subprocess
import tarfile
from urllib.parse import unquote, urlsplit

POLICY = 'publication/source-alpha.json'
MANIFEST = 'SOURCE_ALPHA.json'
LINK = re.compile(r'!?\[[^\]\n]*\]\(([^)\s]+)\)')
FORBIDDEN_PARTS = {'.git', '.aws', '.agents', '.codex', 'node_modules', 'target', '.next', '.next-dev'}
PUBLIC_BINARY_FIXTURES = {'tests/fixtures/identity/key-a.der', 'tests/fixtures/identity/key-b.der'}


def digest(data):
    return hashlib.sha256(data).hexdigest()


def encoded(value):
    return (json.dumps(value, sort_keys=True, indent=2) + '\n').encode()


def safe_path(name):
    path = PurePosixPath(name)
    if (not isinstance(name, str) or not name or '\\' in name or path.is_absolute()
            or path.as_posix() != name or any(p in {'.', '..'} for p in name.split('/'))
            or any(p.casefold() in FORBIDDEN_PARTS for p in path.parts)
            or any(ord(c) < 32 for c in name)):
        raise ValueError('Unsafe export path')
    if (name.startswith(('docs/evidence/', 'docs/screenshots/', 'docs/releases/'))
            or (path.name.startswith('.env') and name != '.env.example')
            or path.suffix in {'.log', '.dump', '.png', '.gz', '.zip'}):
        raise ValueError('Private/generated artifact cannot enter source export: ' + name)
    return name


def git_environment():
    env = {key: value for key, value in os.environ.items() if not key.startswith('GIT_')}
    env.update(GIT_CONFIG_NOSYSTEM='1', GIT_CONFIG_GLOBAL=os.devnull, GIT_TERMINAL_PROMPT='0')
    return env


def git(repository, *args):
    return subprocess.check_output(['git', '-C', str(repository), *args], env=git_environment(), stderr=subprocess.PIPE)


def tree_at(repository, ref):
    commit = git(repository, 'rev-parse', '--verify', ref + '^{commit}').decode().strip()
    tree = {}
    for record in git(repository, 'ls-tree', '-rz', commit).split(b'\0'):
        if not record:
            continue
        metadata, path = record.split(b'\t', 1)
        mode, kind, oid = metadata.decode().split()
        tree[path.decode('utf-8')] = (mode, kind, oid)
    return commit, tree


def read_blob(repository, tree, name):
    mode, kind, oid = tree[name]
    if kind != 'blob' or mode not in {'100644', '100755'}:
        raise ValueError('Export inputs must be regular Git files: ' + name)
    size = int(git(repository, 'cat-file', '-s', oid))
    if size > 16 * 1024 * 1024:
        raise ValueError('Export input exceeds review limit: ' + name)
    return validate_blob(name, git(repository, 'cat-file', 'blob', oid))


def validate_blob(name, data):
    if data.startswith(b'version https://git-lfs.github.com/spec/v1'):
        raise ValueError('LFS content needs a separate review: ' + name)
    if b'\0' in data and name not in PUBLIC_BINARY_FIXTURES:
        raise ValueError('Unreviewed binary input: ' + name)
    return data


def read_inputs(repository, tree, names):
    for name in names:
        mode, kind, _ = tree[name]
        if kind != 'blob' or mode not in {'100644', '100755'}:
            raise ValueError('Export inputs must be regular Git files: ' + name)
    batch = subprocess.Popen(['git', '-C', str(repository), 'cat-file', '--batch'],
                             stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                             env=git_environment())
    inputs = {}
    try:
        for name in names:
            oid = tree[name][2]
            batch.stdin.write((oid + '\n').encode()); batch.stdin.flush()
            header = batch.stdout.readline().decode().split()
            if len(header) != 3 or header[:2] != [oid, 'blob']:
                raise ValueError('Unexpected Git object response')
            size = int(header[2])
            if size > 16 * 1024 * 1024:
                raise ValueError('Export input exceeds review limit: ' + name)
            data = batch.stdout.read(size)
            if len(data) != size or batch.stdout.read(1) != b'\n':
                raise ValueError('Git object framing changed')
            inputs[name] = validate_blob(name, data)
        batch.stdin.close()
        if batch.wait(timeout=10) != 0:
            raise ValueError('Git object inspection failed')
    finally:
        if batch.poll() is None:
            batch.kill(); batch.wait()
        if not batch.stdin.closed:
            batch.stdin.close()
        batch.stdout.close(); batch.stderr.close()
    return inputs


def local_target(name, link):
    parsed = urlsplit(link.strip('<>'))
    if parsed.scheme or parsed.netloc or not parsed.path:
        return None
    target = posixpath.normpath(posixpath.join(posixpath.dirname(name), unquote(parsed.path)))
    if target.startswith('../') or target.startswith('/'):
        raise ValueError('Documentation link escapes export: ' + name)
    return target


def validate_literal_rust_includes(inputs):
    """Refuse missing literal include assets before writing an export.

    Computed macro paths and Rust module discovery still require a compiler
    check of the exported tree; this guard handles ordinary quoted/raw paths.
    """
    plain = re.compile(r'\binclude(?:_str|_bytes)?!\s*\(\s*"([^"\\\r\n]+)"\s*,?\s*\)')
    raw = re.compile(r'\binclude(?:_str|_bytes)?!\s*\(\s*r(#{0,255})"([^\r\n]*?)"\1\s*,?\s*\)')
    roots = ('src/', 'crates/', 'tests/', 'examples/', 'runtime-contracts/', 'integration-contracts/')
    for name, data in inputs.items():
        if not name.endswith('.rs') or not (name == 'build.rs' or name.startswith(roots)):
            continue
        text = data.decode('utf-8')
        paths = plain.findall(text) + [match[1] for match in raw.findall(text)]
        for path in paths:
            target = posixpath.normpath(posixpath.join(posixpath.dirname(name), path))
            if target.startswith(('../', '/')) or target == '..':
                raise ValueError('Rust include escapes export: ' + name)
            if target not in inputs:
                raise ValueError('Rust include input is not allowlisted: ' + name + ' -> ' + target)


def validate_first_party_source_inventory(inputs, tracked):
    """Keep the complete default application and Python check surface.

    A stale allowlist must not omit a new Rust module or silently shrink the
    hosted Python test suite. New paths require explicit policy review rather
    than being discovered and copied into the export automatically.
    """
    missing = []
    for name in tracked:
        rust = name.startswith(('src/', 'crates/', 'tests/', 'examples/')) and name.endswith(('.rs', '.sql'))
        python = name.startswith('scripts/') and name.endswith('.py')
        if (rust or python) and name not in inputs:
            missing.append(name)
    if missing:
        raise ValueError('First-party source is not allowlisted: ' + ', '.join(sorted(missing)))


def validate_graphics_checks(inputs):
    if 'scripts/check-tui-graphics.py' not in inputs:
        return
    workflow = inputs['.github/workflows/community.yml'].decode('utf-8')
    required = [
        'python3 scripts/check-tui-graphics.py target/debug/freight-tui',
        'python3 scripts/check-tui-graphics.py target/debug/freight-tui --api-binary target/debug/stateset-freight',
        'python3 scripts/check-tui-graphics.py target/debug/freight-tui --api-binary target/debug/stateset-freight --pdf',
    ]
    commands = {line.strip()[len('- run: '):] for line in workflow.splitlines()
                if line.strip().startswith('- run: ')}
    if not set(required) <= commands:
        raise ValueError('Graphics source requires demo and actual API PNG/PDF community checks')


def clean_markdown(name, data, included, tracked):
    """Drop whole prose blocks citing omitted artifacts; never fabricate evidence."""
    retained, removed = [], []
    fenced = False
    for block in data.decode('utf-8').split('\n\n'):
        was_fenced = fenced
        for line in block.splitlines():
            if re.match(r'^\s*(```|~~~)', line):
                fenced = not fenced
        missing = []
        if not was_fenced and not block.lstrip().startswith(('```', '~~~')):
            for match in LINK.finditer(block):
                target = local_target(name, match.group(1))
                if target and target not in included:
                    if target not in tracked:
                        raise ValueError('Unresolved source documentation link: ' + name + ' -> ' + target)
                    missing.append(target)
        if missing:
            removed.append({'sha256': digest(block.encode()), 'omitted_references': len(missing)})
        else:
            retained.append(block)
    return '\n\n'.join(retained).encode(), removed


def rebind_journal(inputs):
    name = 'web/tests/fixtures/journal/manifest.json'
    manifest = json.loads(inputs[name])
    receipts = {}
    for filename, entry in manifest['files'].items():
        fixture = 'web/tests/fixtures/journal/' + safe_path(filename)
        if digest(inputs[fixture]) != entry['sha256']:
            raise ValueError('Journal fixture hash mismatch')
        receipt_name = 'web/tests/fixtures/journal/receipts/' + filename + '.json'
        receipt = encoded({
            'scope': 'Source-only synthetic fixture byte binding; no historical execution attested',
            'runnable_sources_sha256': {entry['qualified_source']: entry['sha256']},
        })
        receipts[receipt_name] = receipt
        entry['qualification'] = receipt_name
        entry['qualification_sha256'] = digest(receipt)
    manifest['scope'] = 'Synthetic browser journal fixtures; byte bindings only, private historical reports omitted'
    return encoded(manifest), receipts


def write_file(root, name, data, executable=False):
    path = root / name
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open('xb') as handle:
        handle.write(data)
    path.chmod(0o755 if executable else 0o644)


def archive_tree(root, handle):
    with gzip.GzipFile(filename='', fileobj=handle, mode='wb', mtime=0) as compressed:
        with tarfile.open(fileobj=compressed, mode='w', format=tarfile.PAX_FORMAT) as bundle:
            for path in sorted(p for p in root.rglob('*') if p.is_file()):
                data = path.read_bytes()
                info = tarfile.TarInfo('stateset-freight-source-alpha/' + path.relative_to(root).as_posix())
                info.size = len(data)
                info.mode = 0o755 if path.stat().st_mode & 0o111 else 0o644
                info.mtime = info.uid = info.gid = 0
                info.uname = info.gname = ''
                bundle.addfile(info, io.BytesIO(data))


def prepare(repository, ref, output, archive=None):
    repository = Path(repository).resolve()
    output = Path(output).absolute()
    if output.exists() or output.is_symlink():
        raise ValueError('Output must not already exist')
    if repository == output.resolve() or repository in output.resolve().parents:
        raise ValueError('Keep the private export outside the input checkout')
    if archive is not None:
        archive = Path(archive).absolute()
        if archive.exists() or archive.is_symlink() or output == archive or output in archive.parents:
            raise ValueError('Archive must be a new path outside the output tree')
        if repository == archive.resolve() or repository in archive.resolve().parents:
            raise ValueError('Keep the private archive outside the input checkout')
    commit, tree = tree_at(repository, ref)
    policy_bytes = read_blob(repository, tree, POLICY)
    policy = json.loads(policy_bytes)
    if policy.get('schema') != 1 or set(policy) != {'schema', 'files', 'overlays'}:
        raise ValueError('Unsupported export policy')
    files, overlays = policy['files'], policy['overlays']
    if not files or len(files) != len(set(files)):
        raise ValueError('Export allowlist must be nonempty and unique')
    for name in files:
        safe_path(name)
        if name not in tree:
            raise ValueError('Allowlisted file is absent from input commit: ' + name)
    for name, source in overlays.items():
        safe_path(name); safe_path(source)
        if not name.endswith('.md') or not source.startswith('publication/source-alpha/') or not source.endswith('.md'):
            raise ValueError('Only Markdown publication templates may overlay source files')
        if source not in files:
            raise ValueError('Overlay template must be allowlisted')
    inputs = read_inputs(repository, tree, files)
    validate_literal_rust_includes(inputs)
    validate_first_party_source_inventory(inputs, tree)
    validate_graphics_checks(inputs)
    outputs, changes = {}, {}
    included = set(files) | set(overlays) | {MANIFEST, '.gitleaksignore'}
    for name, data in inputs.items():
        if name.endswith('.md') and name not in overlays and not name.startswith('publication/source-alpha/'):
            revised, removed = clean_markdown(name, data, included, tree)
            if removed:
                changes[name] = {'operation': 'omit_private_reference_paragraphs', 'removed_blocks': removed}
                data = revised
        outputs[name] = data
    for name, source in overlays.items():
        outputs[name] = inputs[source]
        changes[name] = {'operation': 'reviewed_public_template', 'template': source}
    outputs['.gitleaksignore'] = b'# Source snapshot has no historical finding exemptions. Review every new finding.\n'
    workflow = '.github/workflows/community.yml'
    outputs[workflow] = outputs[workflow].replace(b'python3 scripts/check-open-source.py\n',
                                                  b'python3 scripts/check-open-source.py --source-alpha\n')
    all_tests = b"      - run: python3 -m unittest discover -s scripts -p 'test_*.py' -v\n"
    if all_tests not in outputs[workflow]:
        marker = b'      - run: python3 -m unittest discover -s scripts -p test_publication_tools.py -v\n'
        if marker not in outputs[workflow]:
            raise ValueError('Community test step changed; review export transformation')
        export_tests = b'      - run: python3 -m unittest discover -s scripts -p test_source_alpha.py -v\n'
        if export_tests not in outputs[workflow]:
            outputs[workflow] = outputs[workflow].replace(marker, marker + export_tests)
    changes[workflow] = {'operation': 'select_source_profile_and_test_exporter'}
    journal = 'web/tests/fixtures/journal/manifest.json'
    outputs[journal], receipts = rebind_journal(inputs)
    outputs.update(receipts)
    changes[journal] = {'operation': 'bind_fixture_bytes_without_private_execution_reports'}
    created = False
    archive_created = False
    try:
        output.mkdir(mode=0o700)
        created = True
        for name, data in outputs.items():
            write_file(output, name, data, name in tree and tree[name][0] == '100755')
        # Use the checked-in generator so runtime operation descriptions and embedded
        # guides have exactly the same derivation as the normal MCP catalog checks.
        subprocess.run(['node', 'mcp/scripts/catalog.mjs'], cwd=output, check=True, capture_output=True)
        for name in ['mcp/catalog.json', 'mcp/guides.json']:
            outputs[name] = (output / name).read_bytes()
            if outputs[name] != inputs[name]:
                changes[name] = {'operation': 'regenerate_from_exported_api_guides'}
        for name, data in outputs.items():
            if name.endswith('.md') and not name.startswith('publication/source-alpha/'):
                for match in LINK.finditer(data.decode('utf-8')):
                    target = local_target(name, match.group(1))
                    if target and target not in outputs and target != MANIFEST:
                        raise ValueError('Broken export documentation link: ' + name + ' -> ' + target)
        records = {}
        for name, data in sorted(outputs.items()):
            record = {'sha256': digest(data), 'bytes': len(data),
                      'executable': bool((output / name).stat().st_mode & 0o111)}
            if name in inputs:
                record['input_sha256'] = digest(inputs[name])
            if name in changes:
                record.update(changes[name])
            records[name] = record
        manifest = {'schema': 1, 'scope': 'source-only developer alpha review candidate',
                    'publication_cleared': False, 'distribution_compliance_attested': False,
                    'input_commit': commit, 'policy_sha256': digest(policy_bytes),
                    'files': records, 'manifest_excludes_itself': True}
        write_file(output, MANIFEST, encoded(manifest))
        if archive is not None:
            with archive.open('xb') as handle:
                archive_created = True
                os.fchmod(handle.fileno(), 0o600)
                archive_tree(output, handle)
        return manifest
    except BaseException:
        if created:
            shutil.rmtree(str(output))
        if archive_created and archive.exists():
            archive.unlink()
        raise


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--repository', type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument('--ref', default='HEAD')
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--archive', type=Path)
    args = parser.parse_args()
    try:
        result = prepare(args.repository, args.ref, args.output, args.archive)
    except (ValueError, OSError, KeyError, TypeError, subprocess.CalledProcessError) as error:
        raise SystemExit('Source export failed: ' + str(error))
    print('Prepared {} files from {}; publication clearance remains false'.format(len(result['files']) + 1, result['input_commit']))


if __name__ == '__main__':
    main()
