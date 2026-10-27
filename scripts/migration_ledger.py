"""Verify installed schema entries against the complete selected source."""

import hashlib
from pathlib import Path
import re


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def verify_migration_ledger(snapshot, installed):
    expected = []
    for path in (Path(snapshot) / 'migrations').glob('*.sql'):
        match = re.fullmatch(r'([0-9]+)_[a-z0-9_]+\.sql', path.name)
        require(match and path.is_file() and not path.is_symlink(), 'Invalid frozen migration input')
        expected.append({'version': int(match[1]),
                         'checksum': hashlib.sha256(path.read_bytes()).hexdigest()})
    expected.sort(key=lambda item: item['version'])
    require(expected and [item['version'] for item in expected] == list(range(1, len(expected) + 1)),
            'Frozen migration ledger must be nonempty, unique and contiguous')
    require([{'version': item['version'], 'checksum': item['checksum']}
             for item in installed] == expected, 'Actual schema job did not verify the exact migration ledger')
    return expected
