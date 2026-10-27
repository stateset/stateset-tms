#!/usr/bin/env python3
"""Verify that evidence hash exclusions do not suppress credential detection."""
import hashlib
from collections import Counter
import json
from pathlib import Path
import secrets
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
scanner = shutil.which('gitleaks')
if scanner is None:
    raise SystemExit('Install gitleaks 8.30.1 and add it to PATH before this check.')
with tempfile.TemporaryDirectory(prefix='freight-secret-policy-') as directory:
    fixture = Path(directory)
    # These are random synthetic canaries, never service credentials.
    key = secrets.token_hex(32)
    digest = hashlib.sha256(b'owned synthetic source file').hexdigest()
    files = {
        'credentials.ini': f'api_key = "{key}"\n',
        'docs/evidence/credentials.json': json.dumps({'api_key': key}),
        'src/archive.json': json.dumps({'auth.rs': key}),
        'docs/evidence/digests.json': json.dumps({'auth.rs': digest}),
        'docs/evidence/binary-digests.json': json.dumps({'current_api_sha256': digest, 'predecessor_api_sha256': digest, 'api_key': key}),
        'src/binary-digests.json': json.dumps({'current_api_sha256': digest, 'predecessor_api_sha256': digest}),
        'docs/evidence/nonhash-binary-digests.json': json.dumps({'current_api_sha256': 'edi_t_' + key}),
        'third-party/npm-notices.json': json.dumps({'api_key': key, 'credential-management.js': digest}),
    }
    for name, content in files.items():
        path = fixture / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)
    report = fixture / 'result.json'
    result = subprocess.run([
        scanner, 'dir', '--config', str(ROOT / '.gitleaks.toml'),
        '--gitleaks-ignore-path', str(fixture / 'empty-ignore'),
        '--redact=100', '--no-banner', '--report-format=json',
        '--report-path', str(report), str(fixture),
    ], capture_output=True, text=True)
    if result.returncode != 1 or not report.exists():
        raise SystemExit('FAIL: scanner must detect the synthetic credentials; diagnostics omitted to avoid rendering canaries.')
    findings = json.loads(report.read_text())
    counts = Counter(x['File'].replace(str(fixture) + '/', '') for x in findings)
    paths = set(counts)
    required = {'credentials.ini': 1, 'docs/evidence/credentials.json': 1, 'src/archive.json': 1, 'third-party/npm-notices.json': 1,
                'docs/evidence/binary-digests.json': 1, 'src/binary-digests.json': 2, 'docs/evidence/nonhash-binary-digests.json': 1}
    if paths != set(required) or any(counts[path] != count for path, count in required.items()):
        raise SystemExit('FAIL: credential detection or scoped digest exclusion changed; finding paths: ' + ', '.join(sorted(paths)))
print('PASS secret policy: root/evidence/source/catalog credential canaries detected; scoped source digests excluded')
