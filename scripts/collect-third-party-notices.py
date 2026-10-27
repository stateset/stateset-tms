#!/usr/bin/env python3
"""Collect installed dependency notices; report distribution reviews still needed.

This conservative source inventory is not an attestation of binary compliance.
It includes the resolved Cargo workspace graph and installed locked npm packages.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
from urllib.parse import urlsplit

ROOT = Path(__file__).resolve().parents[1]
NOTICE = re.compile(r'^(licen[cs]e|copying|copyright|notice|unlicense|authors)(?:$|[._-])', re.I)
NOTICE_DIR = re.compile(r'^(licen[cs]es?|notices?|legal)$', re.I)
SKIP = {'node_modules', '.git', 'target', '__pycache__', '.cache'}


def sha(data):
    return hashlib.sha256(data).hexdigest()


def notice_files(directory):
    """Retain nested notices and LICENSES/*, refusing symlinked notice material."""
    found = []
    for current, directories, files in os.walk(directory, followlinks=False):
        base = Path(current)
        for name in list(directories):
            path = base / name
            if path.is_symlink() and (NOTICE_DIR.match(name) or NOTICE.match(name)):
                raise ValueError('symlinked notice directory is not accepted')
        directories[:] = sorted(n for n in directories if n not in SKIP and not (base / n).is_symlink())
        relative = base.relative_to(directory)
        in_notices = any(NOTICE_DIR.match(part) for part in relative.parts)
        for name in sorted(files):
            if in_notices or NOTICE.match(name):
                path = base / name
                if path.is_symlink() or not path.is_file():
                    raise ValueError('notice material must be regular files')
                found.append(path)
    return sorted(found)


NATIVE = re.compile(r'(?:\.(?:node|wasm|dll|dylib|a|exe|pyd)$|\.so(?:\.|$))', re.I)
MATERIAL_ROLES = {'license', 'reference_license', 'attribution', 'provenance',
                  'build_recipe', 'build_recipe_license', 'native_inventory', 'native_notices'}


def hash_file(path):
    digest = hashlib.sha256()
    with path.open('rb') as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b''):
            digest.update(chunk)
    return digest.hexdigest()


def safe_file(root, relative, prefix=None):
    if not isinstance(relative, str) or not relative or '\\' in relative or '\x00' in relative:
        raise ValueError('invalid material path')
    path = Path(relative)
    if path.is_absolute() or '..' in path.parts or (prefix and path.parts[0] != prefix):
        raise ValueError('unsafe material path')
    for index in range(1, len(path.parts) + 1):
        if root.joinpath(*path.parts[:index]).is_symlink():
            raise ValueError('symlinked material is not accepted')
    result = root / path
    if not result.is_file():
        raise ValueError('material must be an existing regular file')
    return result


def load_catalog(root):
    path = root / 'third-party/npm-notices.json'
    if path.is_symlink():
        raise ValueError('symlinked upstream catalog is not accepted')
    if not path.exists():
        return {}
    data = json.loads(safe_file(root, 'third-party/npm-notices.json').read_text())
    if data.get('schema_version') != 1 or not isinstance(data.get('entries'), list):
        raise ValueError('unsupported upstream notice catalog')
    result = {}
    for entry in data['entries']:
        identity = (entry.get('name'), entry.get('version'))
        if not all(isinstance(v, str) and v for v in identity) or identity in result:
            raise ValueError('duplicate or invalid upstream package identity')
        if not isinstance(entry.get('release_integrity'), str) or not entry['release_integrity'].startswith('sha512-'):
            raise ValueError('upstream package requires a pinned release integrity')
        if not isinstance(entry.get('license'), str) or not isinstance(entry.get('package_files_sha256'), dict) or 'package.json' not in entry['package_files_sha256']:
            raise ValueError('upstream package requires license and package-file bindings')
        registry = urlsplit(entry.get('registry_url', ''))
        if registry.scheme != 'https' or not registry.hostname or registry.username or registry.password or registry.query or registry.fragment:
            raise ValueError('upstream registry requires a credential-free HTTPS source')
        for relative, digest in entry['package_files_sha256'].items():
            if not isinstance(relative, str) or not relative or Path(relative).is_absolute() or '..' in Path(relative).parts or '\\' in relative or '\x00' in relative:
                raise ValueError('unsafe package binding path')
            if not isinstance(digest, str) or not re.fullmatch('[a-f0-9]{64}', digest):
                raise ValueError('invalid package-file digest')
        if not isinstance(entry.get('materials'), list):
            raise ValueError('upstream material list is missing')
        seen = set()
        for material in entry['materials']:
            relative = material.get('path')
            source = safe_file(root, relative, 'third-party')
            if relative in seen or material.get('role') not in MATERIAL_ROLES:
                raise ValueError('duplicate material or unsupported material role')
            seen.add(relative)
            url = material.get('source_url', '')
            parsed = urlsplit(url)
            if parsed.scheme != 'https' or not parsed.hostname or parsed.username or parsed.password or parsed.query or parsed.fragment:
                raise ValueError('upstream material requires a credential-free HTTPS source')
            if hash_file(source) != material.get('sha256'):
                raise ValueError('upstream material hash changed')
        result[identity] = entry
    return result


def apply_upstream(root, directory, output, record, integrity, catalog):
    entry = catalog.get((record['name'], record['version']))
    record['upstream_materials'] = []
    if entry is None:
        return {}
    if integrity != entry['release_integrity'] or record.get('license') != entry['license']:
        raise ValueError('upstream notice release binding differs from lock')
    verified = {}
    for relative, expected in sorted(entry['package_files_sha256'].items()):
        source = safe_file(directory, relative)
        digest = hash_file(source)
        if digest != expected:
            raise ValueError('installed package differs from upstream notice binding')
        verified[relative] = digest
    for material in entry['materials']:
        source = safe_file(root, material['path'], 'third-party')
        data = source.read_bytes()
        if sha(data) != material['sha256']:
            raise ValueError('upstream material changed during collection')
        relative = Path('upstream') / Path(material['path']).relative_to('third-party')
        destination = output / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(data)
        copied = {key: material[key] for key in ['role', 'source_url', 'description'] if key in material}
        copied.update({'path': relative.as_posix(), 'sha256': sha(data), 'bytes': len(data)})
        record['upstream_materials'].append(copied)
    if 'native_component_versions' in entry:
        versions = json.loads(safe_file(directory, 'versions.json').read_text())
        if versions != entry['native_component_versions']:
            raise ValueError('native component versions differ from upstream notice binding')
        record['native_component_versions'] = versions
    record['upstream_release_binding'] = {'integrity': integrity, 'package_files_verified': len(verified),
                                         'registry_url': entry['registry_url']}
    if entry.get('copyright_statement'):
        record['copyright_statement'] = entry['copyright_statement']
    return verified


def native_artifacts(directory, verified=None):
    artifacts = []
    verified = verified or {}
    magic = (b'\x7fELF', b'\x00asm', b'MZ', b'!<arch>\n',
             b'\xfe\xed\xfa\xce', b'\xce\xfa\xed\xfe', b'\xfe\xed\xfa\xcf', b'\xcf\xfa\xed\xfe',
             b'\xca\xfe\xba\xbe', b'\xbe\xba\xfe\xca')
    for current, directories, files in os.walk(directory, followlinks=False):
        base = Path(current)
        for name in directories:
            if name not in SKIP and (base / name).is_symlink():
                raise ValueError('native scan refuses symlinked package directories')
        directories[:] = sorted(n for n in directories if n not in SKIP)
        for name in sorted(files):
            source = base / name
            if source.is_symlink() or not source.is_file():
                raise ValueError('native scan requires regular package files')
            with source.open('rb') as candidate:
                header = candidate.read(8)
            by_name = bool(NATIVE.search(name))
            by_magic = any(header.startswith(prefix) for prefix in magic)
            if by_name or by_magic:
                relative = source.relative_to(directory).as_posix()
                artifacts.append({'path': relative, 'bytes': source.stat().st_size,
                                  'detected_by': 'filename_and_magic' if by_name and by_magic else 'filename' if by_name else 'magic',
                                  'sha256': verified.get(relative) or hash_file(source)})
    return artifacts


def copy_material(directory, files, output):
    result = []
    for source in files:
        relative = source.relative_to(directory)
        destination = output / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        data = source.read_bytes()
        destination.write_bytes(data)
        result.append({'path': relative.as_posix(), 'sha256': sha(data), 'bytes': len(data)})
    return result


def collect_package(directory, output, record, inherited=None, upstream=None):
    files = [] if inherited is not None else notice_files(directory)
    if inherited is not None:
        files = [inherited / n for n in ['LICENSE', 'LICENSE-MIT', 'LICENSE-APACHE']]
        notices = copy_material(inherited, files, output)
        record['notice_origin'] = 'workspace root licenses'
    else:
        notices = copy_material(directory, files, output)
        record['notice_origin'] = 'package files'
    record['notices'] = notices
    # Native packages often explain their additional terms in README/versions.
    supplements = []
    for name in ['README.md', 'README', 'versions.json']:
        path = directory / name
        if path.is_symlink():
            raise ValueError('supplementary material must not be a symlink')
        if path.is_file() and path not in files:
            supplements.append(path)
    record['supplements'] = copy_material(directory, supplements, output)
    verified = {}
    if upstream is not None:
        root, integrity, catalog = upstream
        verified = apply_upstream(root, directory, output, record, integrity, catalog)
    if record.get('ecosystem') == 'npm':
        record['native_artifacts'] = native_artifacts(directory, verified)
    reasons = []
    if not record.get('license'):
        reasons.append('missing_license_expression')
    if not notices and not any(m['role'] == 'license' for m in record.get('upstream_materials', [])):
        reasons.append('missing_license_text')
    expression = record.get('license') or ''
    if any(marker in expression for marker in ['GPL', 'MPL', 'CC-BY']):
        reasons.append('license_requires_distribution_review')
    if record.get('ecosystem') == 'npm' and expression not in {
            'MIT', 'ISC', 'Apache-2.0', 'BSD-2-Clause', 'BSD-3-Clause', '0BSD', 'Unlicense', 'Zlib'}:
        reasons.append('npm_license_expression_requires_review')
    if record.get('native_artifacts'):
        reasons.append('native_artifacts_require_distribution_review')
    if record['name'].startswith('@img/sharp-libvips-'):
        reasons.append('bundled_native_libraries_require_separate_review')
    record['review_reasons'] = reasons
    return record


def cargo_records(root, metadata, output):
    nodes = {n['id']: n for n in metadata['resolve']['nodes']}
    pending = list(metadata['workspace_members'])
    reachable = set()
    while pending:
        name = pending.pop()
        if name not in reachable:
            reachable.add(name)
            pending.extend(nodes[name]['dependencies'])
    records = []
    members = set(metadata['workspace_members'])
    for package in sorted(metadata['packages'], key=lambda p: (p['name'], p['version'])):
        if package['id'] not in reachable:
            continue
        directory = Path(package['manifest_path']).parent
        source_identity = package.get('source') or 'local'
        slug = package['name'] + '-' + package['version'] + '-' + sha(source_identity.encode())[:10]
        if not re.fullmatch(r'[A-Za-z0-9_.+-]+', slug):
            raise ValueError('unsupported Cargo package identity')
        record = {k: package.get(k) for k in ['name', 'version', 'license', 'repository']}
        record.update({'ecosystem': 'cargo', 'material_directory': 'cargo/' + slug,
                       'source_identity_sha256': sha(source_identity.encode())})
        inherited = root if package['id'] in members else None
        records.append(collect_package(directory, output / record['material_directory'], record, inherited))
    return records


def npm_records(root, name, output, catalog=None):
    package_root = root / name
    lock_path = package_root / 'package-lock.json'
    lock = json.loads(lock_path.read_text())
    records, absent = [], []
    for location, locked in sorted(lock['packages'].items()):
        if not location:
            continue
        parts = Path(location).parts
        if Path(location).is_absolute() or '..' in parts or not location.startswith('node_modules/'):
            raise ValueError('unsafe npm lockfile location')
        directory = package_root / location
        if any((package_root.joinpath(*parts[:i])).is_symlink() for i in range(1, len(parts) + 1)):
            raise ValueError('symlinked npm package is not accepted')
        installed_path = directory / 'package.json'
        if not installed_path.exists():
            if not locked.get('optional'):
                raise ValueError('required locked npm package is not installed: ' + name + '/' + location)
            absent.append({'location': name + '/' + location, 'version': locked.get('version'),
                           'license': locked.get('license'), 'reason': 'optional package not installed'})
            continue
        if installed_path.is_symlink():
            raise ValueError('installed npm metadata must not be a symlink')
        installed = json.loads(installed_path.read_text())
        expected_name = locked.get('name') or location.rsplit('node_modules/', 1)[1]
        if installed.get('name') != expected_name:
            raise ValueError('installed npm name differs from lock location: ' + name + '/' + location)
        if installed.get('version') != locked.get('version'):
            raise ValueError('installed npm version differs from lock: ' + name + '/' + location)
        if installed.get('license') != locked.get('license'):
            raise ValueError('installed npm license differs from lock: ' + name + '/' + location)
        # Keep nested locations distinct, including repeated versions of a package.
        record = {k: installed.get(k) for k in ['name', 'version', 'license', 'repository']}
        if not isinstance(record['name'], str):
            raise ValueError('installed npm package has no name')
        record.update({'ecosystem': 'npm', 'locked_location': name + '/' + location,
                       'material_directory': 'npm/' + name + '/' + location})
        records.append(collect_package(directory, output / record['material_directory'], record,
                                       upstream=(root, locked.get('integrity'), catalog or {})))
    return records, absent


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True, help='new directory; never overwrite an existing bundle')
    parser.add_argument('--target', help='Rust target triple (default: rustc host)')
    parser.add_argument('--fail-on-review', action='store_true', help='exit 2 if any package needs distribution review')
    args = parser.parse_args()
    target = args.target
    if target is None:
        details = subprocess.check_output(['rustc', '-vV'], text=True)
        target = next(line.split(': ', 1)[1] for line in details.splitlines() if line.startswith('host: '))
    metadata = json.loads(subprocess.check_output([
        'cargo', 'metadata', '--locked', '--offline', '--all-features',
        '--filter-platform', target, '--format-version', '1',
    ], cwd=ROOT))
    catalog = load_catalog(ROOT)
    output = args.output.absolute()
    output.mkdir(mode=0o700, parents=False, exist_ok=False)
    try:
        records = cargo_records(ROOT, metadata, output)
        absent = []
        for directory in ['web', 'mcp']:
            packages, optional = npm_records(ROOT, directory, output, catalog)
            records.extend(packages)
            absent.extend(optional)
        locks = {n: sha((ROOT / n).read_bytes()) for n in [
            'Cargo.lock', 'web/package-lock.json', 'mcp/package-lock.json']}
        review = [{'ecosystem': r['ecosystem'], 'name': r['name'], 'version': r['version'],
                   'material_directory': r['material_directory'], 'reasons': r['review_reasons']}
                  for r in records if r['review_reasons']]
        manifest = {'schema_version': 1, 'scope': 'conservative resolved Cargo workspace and installed locked npm source packages; not an exact binary SBOM',
                    'rust_target': target, 'lockfiles': locks, 'packages': records,
                    'optional_npm_not_installed': absent, 'distribution_review_required': review,
                    'upstream_catalog_sha256': hash_file(ROOT / 'third-party/npm-notices.json') if catalog else None,
                    'distribution_compliance_attested': False}
        (output / 'manifest.json').write_text(json.dumps(manifest, sort_keys=True, indent=2) + '\n')
        (output / 'README.txt').write_text(
            'Dependency notice collection, not distribution clearance.\n'
            'See manifest.json for package terms, copied material hashes, optional omissions and review reasons.\n'
            'Review native component terms, corresponding-source/relinking requirements, system packages and the exact delivered artifact separately.\n')
        print('Collected notices for {} Cargo and {} npm packages; {} optional npm packages not installed; {} package reviews remain.'.format(
            sum(r['ecosystem'] == 'cargo' for r in records), sum(r['ecosystem'] == 'npm' for r in records), len(absent), len(review)))
        return 2 if args.fail_on_review and review else 0
    except BaseException:
        shutil.rmtree(output)
        raise


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        # Never render package file contents or credentials.
        sys.exit('Notice collection failed: ' + str(error))
