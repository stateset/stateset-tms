#!/usr/bin/env python3
"""Create a private, value-free review inventory of fetched Git history.

Candidates are review pointers, not proven leaks or publication clearance.
Unreachable objects, external assets and binary visual content are not inspected.
"""
import argparse
from collections import Counter
import ipaddress
import json
import os
from pathlib import Path
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
PATTERNS = {
    'email_address': re.compile(r'(?<![A-Za-z0-9.!#$%&\x27*+/=?^_`{|}~-])[A-Za-z0-9.!#$%&\x27*+/=?^_`{|}~-]{1,64}@[A-Za-z0-9](?:[A-Za-z0-9.-]{0,251}[A-Za-z0-9])?\.[A-Za-z]{2,63}'),
    'personal_home_path': re.compile(r'/(?:home|Users)/[^/\s"<>]+/'),
    'internal_hostname': re.compile(r'\b[A-Za-z0-9][A-Za-z0-9.-]{0,251}\.(?:internal|corp|lan)\b', re.I),
    'credential_url': re.compile(r'\b[A-Za-z][A-Za-z0-9+.-]*://[^\s/:@"<>]+:[^\s/@"<>]+@'),
    'private_key_material': re.compile(r'-----BEGIN (?:RSA |EC |OPENSSH |ENCRYPTED )?PRIVATE KEY-----'),
    'ipv4_address': re.compile(r'\b(?:[0-9]{1,3}\.){3}[0-9]{1,3}\b'),
}
DOCUMENTATION_NETWORKS = [ipaddress.ip_network(n) for n in ['192.0.2.0/24', '198.51.100.0/24', '203.0.113.0/24']]


def classify(data):
    if b'\x00' in data[:8192]:
        return {'binary_requires_manual_review': []}
    text = data.decode('utf-8', errors='replace')
    result = {}
    for category, pattern in PATTERNS.items():
        lines = set()
        for match in pattern.finditer(text):
            if category == 'email_address':
                domain = match.group().rsplit('@', 1)[1].lower()
                if domain in ['example.com', 'example.net', 'example.org'] or domain.endswith(('.test', '.example', '.invalid', '.localhost')):
                    continue
                if match.group().lower() == 'security@stateset.io':
                    continue
            if category == 'ipv4_address':
                try:
                    address = ipaddress.ip_address(match.group())
                except ValueError:
                    continue
                if address.is_loopback or address.is_unspecified or any(address in n for n in DOCUMENTATION_NETWORKS):
                    continue
            lines.add(text.count('\n', 0, match.start()) + 1)
            if len(lines) == 3:
                break
        if lines:
            result[category] = sorted(lines)
    return result


def git(repository, *arguments):
    return subprocess.check_output(['git', '-C', str(repository), *arguments])


def snapshot_refs(repository):
    entries = git(repository, 'for-each-ref', '--format=%(refname) %(objectname)').decode().splitlines()
    refs = {line.split(' ', 1)[0]: line.split(' ', 1)[1] for line in entries}
    refs['HEAD'] = git(repository, 'rev-parse', 'HEAD').decode().strip()
    return refs


def tree(repository, commit):
    result = {}
    for entry in git(repository, 'ls-tree', '-r', '-z', '--full-tree', commit).split(b'\x00'):
        if not entry:
            continue
        details, path = entry.split(b'\t', 1)
        mode, kind, oid = details.decode().split(' ')
        if kind == 'blob':
            result.setdefault(oid, []).append(os.fsdecode(path))
    return result


def audit(repository, max_blob_bytes=16 * 1024 * 1024):
    refs = snapshot_refs(repository)
    commits = git(repository, 'rev-list', '--all', 'HEAD').decode().splitlines()
    objects = {}
    for commit in commits:
        for oid, paths in tree(repository, commit).items():
            objects.setdefault(oid, set()).update(paths)
    current = tree(repository, 'HEAD')
    authors = git(repository, 'log', '--all', 'HEAD', '--format=%aN <%aE>').splitlines()
    # Names/emails are not included in the report; owner clearance remains manual.
    author_count = len(set(authors))
    batch = subprocess.Popen(['git', '-C', str(repository), 'cat-file', '--batch'],
                             stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    candidates, total_bytes = [], 0
    try:
        for oid, paths in sorted(objects.items()):
            batch.stdin.write((oid + '\n').encode())
            batch.stdin.flush()
            header = batch.stdout.readline().decode().split()
            if len(header) != 3 or header[0] != oid or header[1] != 'blob':
                raise ValueError('Git returned an unexpected object')
            size = int(header[2]); total_bytes += size
            if size > max_blob_bytes:
                remaining = size
                while remaining:
                    chunk = batch.stdout.read(min(remaining, 1024 * 1024))
                    if not chunk:
                        raise ValueError('Git object stream ended early')
                    remaining -= len(chunk)
                reasons = {'oversize_uninspected': []}
            else:
                data = batch.stdout.read(size)
                if len(data) != size:
                    raise ValueError('Git object stream ended early')
                reasons = classify(data)
            if batch.stdout.read(1) != b'\n':
                raise ValueError('Git object framing changed')
            if any(Path(p).suffix.lower() in ['.der', '.p12', '.pfx', '.key', '.pem'] for p in paths):
                reasons['key_or_certificate_file'] = []
            if any(Path(p).name.startswith('.env') and Path(p).name not in ['.env.example', '.env.sample'] for p in paths):
                reasons['environment_file'] = []
            if any(Path(p).suffix.lower() in ['.png', '.jpg', '.jpeg', '.gif', '.webp', '.svg', '.pdf'] for p in paths):
                reasons['document_or_image_requires_manual_review'] = []
            if any(p.lower().endswith(('.dump', '.sql.gz', '.tar.gz', '.zip', '.backup')) for p in paths):
                reasons['archive_or_backup_file'] = []
            if reasons:
                candidates.append({'blob': oid, 'bytes': size, 'paths': sorted(paths),
                                   'present_at_head': oid in current, 'review_categories': reasons})
        batch.stdin.close()
        if batch.wait(timeout=10) != 0:
            raise ValueError('Git object inspection failed')
    finally:
        if batch.poll() is None:
            batch.kill(); batch.wait()
        batch.stdout.close(); batch.stderr.close()
        if not batch.stdin.closed:
            batch.stdin.close()
    if snapshot_refs(repository) != refs:
        raise ValueError('Git refs changed during review; repeat on a stable snapshot')
    counts = Counter(category for item in candidates for category in item['review_categories'])
    return {'schema_version': 1, 'scope': 'unique blobs reachable through fetched refs and HEAD; up to three line pointers per category; no matched values',
            'refs': refs, 'commits': len(commits), 'unique_blobs': len(objects),
            'blob_bytes': total_bytes, 'author_identities_requiring_rights_review': author_count,
            'review_candidate_blobs': len(candidates), 'category_blob_counts': dict(sorted(counts.items())),
            'candidates': candidates, 'publication_cleared': False,
            'limits': ['Pattern candidates require human review; false positives and missed sensitive data are possible',
                       'Binary/oversize bodies need manual review; archive contents are not expanded',
                       'Deleted/unreachable/server-only refs, submodule contents, LFS bodies, issues and uploaded artifacts are outside scope',
                       'Copyright ownership and confidentiality cannot be established by this inventory']}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--repository', type=Path, default=ROOT)
    parser.add_argument('--output', type=Path, required=True, help='new private JSON file, mode 0600')
    args = parser.parse_args()
    result = audit(args.repository)
    # Exclusive creation: do not overwrite a previous audit or follow its symlink.
    descriptor = os.open(args.output, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, 'w') as destination:
        json.dump(result, destination, sort_keys=True, indent=2)
        destination.write('\n')
    print('Inventory: {} commits, {} unique blobs, {} candidate blobs. Human confidentiality/rights review remains required.'.format(
        result['commits'], result['unique_blobs'], result['review_candidate_blobs']))


if __name__ == '__main__':
    try:
        main()
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        sys.exit('History inventory failed: ' + str(error))
