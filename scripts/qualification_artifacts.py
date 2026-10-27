"""Lossless private qualification evidence, with explicit stored and body hashes."""

import gzip
import hashlib
from pathlib import Path


def sha(data):
    return hashlib.sha256(data).hexdigest()


def write_artifact(path, raw, compression=None):
    if compression not in (None, 'gzip'):
        raise ValueError('Unsupported evidence compression')
    path = Path(str(path) + ('.gz' if compression == 'gzip' else ''))
    data = gzip.compress(raw, compresslevel=1, mtime=0) if compression else raw
    with path.open('xb') as stream:
        stream.write(data)
    result = {'path': str(path), 'sha256': sha(data)}
    if compression:
        result.update(compression=compression, uncompressed_sha256=sha(raw),
                      uncompressed_bytes=len(raw))
    return result


def read_artifact(record):
    data = Path(record['path']).read_bytes()
    if sha(data) != record['sha256']:
        raise ValueError('Stored evidence digest changed')
    compression = record.get('compression')
    if compression is None:
        return data
    if compression != 'gzip':
        raise ValueError('Unsupported evidence compression')
    data = gzip.decompress(data)
    if (type(record.get('uncompressed_bytes')) is not int
            or len(data) != record['uncompressed_bytes']
            or sha(data) != record['uncompressed_sha256']):
        raise ValueError('Uncompressed evidence digest/size changed')
    return data
