#!/usr/bin/env python3
"""Verify only explicitly listed archive payloads against an externally pinned manifest."""
from __future__ import annotations
import argparse
from contextlib import contextmanager, ExitStack
from dataclasses import dataclass, field
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import stat
import sys

MANIFEST_LIMIT = 8 * 1024 * 1024
RECORD_LIMIT = 100_000
DEFAULT_BYTES = 1024 ** 3


def integer(value, name, maximum=2**63-1):
    if type(value) is not int or not 0 <= value <= maximum:
        raise ValueError(f'invalid {name}')
    return value


def digest(value):
    if type(value) is not str or not re.fullmatch('[0-9a-fA-F]{64}', value):
        raise ValueError('invalid sha256')
    return value.lower()


def mode(value):
    if type(value) is str:
        if not re.fullmatch(r'(?:0o[0-7]{1,4}|[0-7]{3,4})', value):
            raise ValueError('mode must be permission bits as an integer or octal string')
        value = int(value, 8)
    return integer(value, 'mode', 0o7777)


@dataclass(frozen=True)
class Record:
    path: str
    size: int
    sha256: str
    mode: int
    uid: int
    gid: int
    mtime_ns: int | None = None

    @classmethod
    def parse(cls, value):
        if type(value) is not dict:
            raise ValueError('record must be an object')
        path = value.get('path')
        if (type(path) is not str or not path or '\\' in path or '\0' in path
                or PurePosixPath(path).is_absolute() or any(p in ('', '.', '..') for p in path.split('/'))):
            raise ValueError('invalid relative payload path')
        return cls(path, integer(value.get('size'), 'size'), digest(value.get('sha256')),
                   mode(value.get('mode')), integer(value.get('uid'), 'uid', 2**32-2),
                   integer(value.get('gid'), 'gid', 2**32-2),
                   integer(value['mtime_ns'], 'mtime_ns') if 'mtime_ns' in value else None)


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError('duplicate JSON key: ' + key)
        result[key] = value
    return result


@contextmanager
def opened(path, flags=os.O_RDONLY, permissions=0o600):
    """Open every absolute component without following symlinks; retain parent identities."""
    absolute = Path(os.path.abspath(path))
    with ExitStack() as stack:
        fd = os.open('/', os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        stack.callback(os.close, fd)
        parents = []
        for part in absolute.parts[1:-1]:
            fd = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd)
            stack.callback(os.close, fd)
            s = os.fstat(fd)
            parents.append((s.st_dev, s.st_ino))
        fd = os.open(absolute.name, flags | os.O_NOFOLLOW | os.O_NONBLOCK, permissions, dir_fd=fd)
        stack.callback(os.close, fd)
        s = os.fstat(fd)
        if not stat.S_ISREG(s.st_mode) or s.st_nlink != 1:
            raise ValueError('not a singly linked regular file: ' + str(absolute))
        yield fd, tuple(parents)


def identity(s):
    return (s.st_dev, s.st_ino, s.st_mode, s.st_uid, s.st_gid, s.st_nlink,
            s.st_size, s.st_mtime_ns, s.st_ctime_ns)


def read_checked(path, limit, expected_size=None, collect=False, *, reader=os.read):
    with opened(path) as (fd, parents):
        before = os.fstat(fd)
        if before.st_size > limit or (expected_size is not None and before.st_size != expected_size):
            raise ValueError('size/budget mismatch: ' + str(path))
        total, sha, chunks = 0, hashlib.sha256(), []
        while True:
            chunk = reader(fd, min(65536, before.st_size - total + 1))
            if not chunk:
                break
            total += len(chunk)
            if total > before.st_size:
                raise ValueError('file grew during read: ' + str(path))
            sha.update(chunk)
            if collect:
                chunks.append(chunk)
        if total != before.st_size or identity(os.fstat(fd)) != identity(before):
            raise ValueError('file changed during read: ' + str(path))
        with opened(path) as (fresh, current_parents):
            if current_parents != parents or identity(os.fstat(fresh)) != identity(before):
                raise ValueError('path identity changed during read: ' + str(path))
        return before, parents, sha.hexdigest(), b''.join(chunks)


@dataclass
class Result:
    requested: int = 0
    verified: int = 0
    bytes_verified: int = 0
    mtime_requested: int = 0
    mtime_verified: int = 0
    errors: list[str] = field(default_factory=list)
    manifest: str | None = None
    records_key: str | None = None
    expected_sha256: str | None = None
    observed_sha256: str | None = None

    def as_dict(self):
        passed = (not self.errors and self.requested > 0 and self.verified == self.requested
                  and self.mtime_verified == self.mtime_requested)
        return {'schema_version': 1, 'manifest': self.manifest, 'records_key': self.records_key,
                'expected_sha256': self.expected_sha256, 'observed_sha256': self.observed_sha256,
                'outcome': 'passed' if passed else 'failed',
                'exit_code': 0 if passed else 1, 'requested': self.requested, 'verified': self.verified,
                'bytes_verified': self.bytes_verified, 'mtime_requested': self.mtime_requested,
                'mtime_verified': self.mtime_verified, 'errors': self.errors,
                'scope': 'listed regular payloads only; per-file read consistency, not an atomic archive snapshot',
                'unlisted_contents_verified': False, 'child_command_or_tracking_verified': False}


def verify(manifest, records_key, expected_sha256, max_bytes=DEFAULT_BYTES):
    result = Result()
    try:
        pin = digest(expected_sha256)
        if records_key not in ('files', 'payloads'):
            raise ValueError('records_key must be files or payloads')
        integer(max_bytes, 'max_bytes')
        manifest = Path(os.path.abspath(manifest))
        result.manifest, result.records_key, result.expected_sha256 = str(manifest), records_key, pin
        before, parents, actual, raw = read_checked(manifest, MANIFEST_LIMIT, collect=True)
        result.observed_sha256 = actual
        if actual != pin:
            raise ValueError('manifest sha256 mismatch')
        data = json.loads(raw, object_pairs_hook=unique_object,
                          parse_constant=lambda value: (_ for _ in ()).throw(ValueError('invalid JSON constant: ' + value)))
        if type(data) is not dict or type(data.get(records_key)) is not list:
            raise ValueError('explicit records key must contain an array')
        values = data[records_key]
        if not 1 <= len(values) <= RECORD_LIMIT:
            raise ValueError('record count outside budget')
        records = [Record.parse(v) for v in values]
        if len({r.path for r in records}) != len(records):
            raise ValueError('duplicate payload path')
        if sum(r.size for r in records) > max_bytes:
            raise ValueError('payload byte budget exceeded')
        result.requested = len(records)
        result.mtime_requested = sum(r.mtime_ns is not None for r in records)
        for record in records:
            try:
                s, _, sha, _ = read_checked(manifest.parent / record.path, record.size, record.size)
                if (sha, stat.S_IMODE(s.st_mode), s.st_uid, s.st_gid) != (record.sha256, record.mode, record.uid, record.gid):
                    raise ValueError('sha256/metadata mismatch')
                if record.mtime_ns is not None and s.st_mtime_ns != record.mtime_ns:
                    raise ValueError('mtime_ns mismatch')
                result.verified += 1
                result.bytes_verified += record.size
                result.mtime_verified += record.mtime_ns is not None
            except (OSError, ValueError) as error:
                result.errors.append(record.path + ': ' + str(error))
        with opened(manifest) as (fd, current_parents):
            if current_parents != parents or identity(os.fstat(fd)) != identity(before):
                raise ValueError('manifest changed during verification')
    except (OSError, ValueError, UnicodeError, RecursionError) as error:
        result.errors.append(str(error))
    return result


class Parser(argparse.ArgumentParser):
    def error(self, message):
        raise ValueError(message)


def main(argv=None):
    result = Result()
    try:
        parser = Parser(description=__doc__)
        parser.add_argument('--manifest', required=True)
        parser.add_argument('--records-key', required=True, choices=['files', 'payloads'])
        parser.add_argument('--expected-sha256', required=True)
        parser.add_argument('--max-bytes', type=int, default=DEFAULT_BYTES)
        parser.add_argument('--output', help='exclusive new JSON result; omit for stdout only')
        args = parser.parse_args(argv)
        result = verify(args.manifest, args.records_key, args.expected_sha256, args.max_bytes)
        if args.output:
            with opened(args.output, os.O_WRONLY | os.O_CREAT | os.O_EXCL) as (fd, _):
                with os.fdopen(os.dup(fd), 'w', encoding='utf-8') as stream:
                    json.dump(result.as_dict(), stream, indent=2)
                    stream.write('\n')
    except (OSError, ValueError) as error:
        result.errors.append(str(error))
    report = result.as_dict()
    print(json.dumps(report, indent=2))
    return report['exit_code']


if __name__ == '__main__':
    sys.exit(main())
