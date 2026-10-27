#!/usr/bin/env python3
"""Check first-party licensing, quickstart links and public-PR runner policy."""
import json
import importlib.util
import hashlib
from pathlib import Path
import re
import argparse

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--source-alpha', action='store_true', help='Validate the isolated, hosted-only source export')
args = parser.parse_args()

ROOT = Path(__file__).resolve().parents[1]
LICENSE = 'MIT OR Apache-2.0'
errors = []

def require(condition, message):
    if not condition:
        errors.append(message)

manifests = ['Cargo.toml', 'crates/storage-codec/Cargo.toml',
             'crates/commerce-bridge/Cargo.toml', 'crates/freight-tui/Cargo.toml',
             'integration-contracts/Cargo.toml', 'runtime-contracts/Cargo.toml']
for name in manifests:
    package = (ROOT / name).read_text().split('[package]', 1)[1].split('\n[', 1)[0]
    require(re.search(r'^license\s*=\s*"MIT OR Apache-2.0"\s*$', package, re.MULTILINE) is not None,
            f'{name}: first-party SPDX license missing')
for directory in ['web', 'mcp']:
    package = json.loads((ROOT / directory / 'package.json').read_text())
    lock = json.loads((ROOT / directory / 'package-lock.json').read_text())['packages']['']
    require(package.get('license') == lock.get('license') == LICENSE,
            f'{directory}: package/lock licenses must agree')
    require(package['version'] == lock['version'], f'{directory}: package/lock versions differ')
    require(package.get('private') is True, f'{directory}: retain explicit package publication control')
for name in ['LICENSE', 'LICENSE-MIT', 'LICENSE-APACHE', 'SECURITY.md',
             'CONTRIBUTING.md', 'THIRD_PARTY_NOTICES.md', 'docs/open-source-readiness.md',
             'docs/security/secret-scan.md',
             'docs/security/publication-review.md', 'tests/fixtures/identity/README.md',
             'third-party/npm-notices.json', 'third-party/README.md', 'docs/security/upstream-notices.md']:
    require((ROOT / name).is_file(), f'Missing {name}')
require((ROOT / ('SOURCE_ALPHA.json' if args.source_alpha else 'docs/development-history.md')).is_file(),
        'Missing source-profile documentation or manifest')
for suffix in ['MIT', 'APACHE']:
    require((ROOT / f'LICENSE-{suffix}').read_text().rstrip() ==
            (ROOT / f'vendor/stateset-primitives/LICENSE-{suffix}').read_text().rstrip(),
            f'LICENSE-{suffix}: license text or attribution differs from vendored original')
try:
    vendor = ROOT / 'vendor/sqlx-core'
    provenance = json.loads((vendor / 'UPSTREAM.json').read_text())
    require(provenance['crate'] == 'sqlx-core' and provenance['version'] == '0.8.6',
            'Vendored SQLx release changed; review the security patch and provenance')
    originals, changes = provenance['upstream_files_sha256'], provenance['modified_files']
    require(set(changes) == {'Cargo.toml'}, 'SQLx patch must remain dependency-only')
    require({'src/lib.rs', 'Cargo.toml.orig', 'LICENSE-MIT', 'LICENSE-APACHE'} <= set(originals),
            'SQLx provenance must include original Rust sources, Cargo metadata and licenses')
    for name, digest in originals.items():
        relative = Path(name)
        require(not relative.is_absolute() and '..' not in relative.parts,
                'Unsafe vendored SQLx provenance path')
        if relative.is_absolute() or '..' in relative.parts:
            continue
        path = vendor / relative
        expected = changes[name]['patched_sha256'] if name in changes else digest
        require(path.is_file() and not path.is_symlink() and
                hashlib.sha256(path.read_bytes()).hexdigest() == expected,
                'Vendored SQLx input differs from reviewed provenance: ' + name)
    require(set(originals) | {'UPSTREAM.json'} ==
            {str(p.relative_to(vendor)) for p in vendor.rglob('*') if p.is_file()},
            'Vendored SQLx membership differs from reviewed provenance')
except (OSError, ValueError, KeyError, TypeError) as error:
    errors.append('Vendored SQLx provenance validation failed: ' + str(error))
for name in ['README.md', 'CONTRIBUTING.md', 'SECURITY.md', 'THIRD_PARTY_NOTICES.md',
             'docs/open-source-readiness.md', 'docs/security/secret-scan.md',
             'docs/security/publication-review.md', 'docs/security/upstream-notices.md', 'third-party/README.md']:
    path = ROOT / name
    if not path.exists():
        continue
    for link in re.findall(r'\]\(([^)]+)\)', path.read_text()):
        if re.match(r'[a-z]+:', link) or link.startswith('#'):
            continue
        target = link.split('#', 1)[0]
        require((path.parent / target).exists(), f'{name}: broken relative link {target}')
spec = importlib.util.spec_from_file_location('freight_notice_policy', ROOT / 'scripts/collect-third-party-notices.py')
notice_policy = importlib.util.module_from_spec(spec)
spec.loader.exec_module(notice_policy)
try:
    catalog = notice_policy.load_catalog(ROOT)
    require(bool(catalog), 'Pinned upstream notice catalog is missing or empty')
    locks = json.loads((ROOT / 'web/package-lock.json').read_text())['packages']
    for (name, version), entry in catalog.items():
        locked = locks.get('node_modules/' + name, {})
        require(locked.get('version') == version and locked.get('integrity') == entry['release_integrity']
                and locked.get('license') == entry['license'],
                'Pinned notice release differs from the web lock: ' + name)
except (ValueError, OSError, KeyError, TypeError) as error:
    errors.append('Upstream notice catalog validation failed: ' + str(error))
workflows = []
if args.source_alpha:
    require(not (ROOT / '.github/workflows/check.yml').exists(), 'Source alpha must omit the private full workflow')
    require(set(path.name for path in (ROOT / '.github/workflows').iterdir()) == {'community.yml'},
            'Source alpha may contain only the reviewed hosted workflow')
else:
    workflow = (ROOT / '.github/workflows/check.yml').read_text()
    require('runs-on: ubuntu-24.04' in workflow and 'self-hosted' not in workflow,
            'Full checks must use ephemeral GitHub-hosted runners')
    workflows.append(('full', workflow))
community = (ROOT / '.github/workflows/community.yml').read_text()
require('name: Community source checks' in community, 'Community check needs its stable branch-protection context')
if not args.source_alpha:
    require('name: Freight integration checks' in workflow, 'Integration check needs a distinct status context')
protection = json.loads((ROOT / 'publication/branch-protection.json').read_text())
require(protection['required_status_checks'] == {'strict': True, 'checks': [{'context': 'Community source checks', 'app_id': 15368}]},
        'Source branch-protection payload must require the named GitHub Actions check')
if not args.source_alpha:
    release_protection = json.loads((ROOT / 'publication/release-branch-protection.json').read_text())
    expected_release = {**protection, 'required_status_checks': {
        'strict': True, 'checks': [
            {'context': 'Community source checks', 'app_id': 15368},
            {'context': 'Freight integration checks', 'app_id': 15368}]}}
    require(release_protection == expected_release,
            'Full release protection must require both GitHub Actions checks and preserve all review guards')
require(protection.get('enforce_admins') is True and protection.get('allow_force_pushes') is False
        and protection.get('allow_deletions') is False and protection.get('required_conversation_resolution') is True,
        'Source branch-protection payload must preserve required review and history')
require(json.loads((ROOT / 'publication/workflow-permissions.json').read_text()) ==
        {'default_workflow_permissions': 'read', 'can_approve_pull_request_reviews': False},
        'Repository default-token payload must be read-only without automated approvals')
require('runs-on: ubuntu-24.04' in community and 'self-hosted' not in community,
        'Community checks must use ephemeral GitHub-hosted runners')
action_pins = json.loads((ROOT / '.github/action-pins.json').read_text())
require(action_pins.get('schema') == 1, 'Unsupported CI action pin schema')
pinned_actions = action_pins.get('actions', {})
for name, text in workflows + [('community', community)]:
    uses = re.findall(r'^\s+- uses: (\S+)', text, re.MULTILINE)
    require(set(pinned_actions) == {use.split('@')[0] for use in uses},
            f'{name}: action inventory must match reviewed pins')
    for use in uses:
        action, _, revision = use.partition('@')
        require(re.fullmatch(r'[0-9a-f]{40}', revision) is not None
                and pinned_actions.get(action, {}).get('commit') == revision,
                f'{name}: action {action} must use its reviewed immutable commit')
    require('toolchain: 1.90.0' in text, f'{name}: pinned toolchain action must explicitly select Rust 1.90.0')
    require('contents: read' in text and 'persist-credentials: false' in text,
            f'{name}: require read-only permissions and nonpersisted checkout credentials')
    require('cargo-deny --version 0.20.2 --locked' in text and 'check licenses' in text,
            f'{name}: require the pinned dependency license policy')
    require("unittest discover -s scripts -p 'test_*.py' -v" in text
            and 'collect-third-party-notices.py' in text,
            f'{name}: require the complete Python suite and notice collection')
    require('pull_request_target' not in text and '${{ secrets.' not in text,
            f'{name}: do not expose privileged triggers or repository secrets')
if errors:
    raise SystemExit('\n'.join('FAIL: ' + error for error in errors))
print('PASS open-source metadata: 6 Cargo manifests, 2 npm package/lock pairs, licenses, documentation links, pinned notice catalog and runner policy')
