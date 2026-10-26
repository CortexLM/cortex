"""Checkpoint storage contract of the elastic core, a durable local store and an in-memory one.

The core writes only content-addressed, write-once objects: the key is the sha256 hex of the
bytes. The head of each checkpoint chain lives in the coordinator, never in a mutable store key,
so a store needs no compare-and-swap and never rewrites a key (R2 accepts one write per second
per key). The R2 implementation of this protocol arrives with hypertrain.dataplane (ADR 0003).
"""

from __future__ import annotations

import hashlib
import os
import uuid
from pathlib import Path
from typing import Protocol

from hypertrain.data.store import CorruptObjectError, ObjectNotFound, _check_key, _verified


class CheckpointStore(Protocol):
    """put returns the sha256 hex of `data` and never exposes a partial or unsynced object
    under that key; get verifies the content hash. exists is true once a put of that key has
    completed and does not read the bytes, so only get proves them: the store's root belongs to
    the store alone."""

    def put(self, data: bytes) -> str: ...
    def get(self, sha: str) -> bytes: ...
    def exists(self, sha: str) -> bool: ...


def _fsync_dir(path: Path) -> None:
    _fsync_file(path, os.O_DIRECTORY)


def _fsync_file(path: Path, flags: int = 0) -> None:
    fd = os.open(path, os.O_RDONLY | flags)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


class LocalCheckpointStore:
    """Local-filesystem store whose put survives a crash at any instant.

    The bytes are fsynced under a unique temporary name, renamed onto the key, and the directory
    is fsynced, so the key names nothing or the complete object. hypertrain.data.store's
    LocalFSStore renames without fsync and never rewrites an existing key; the v2 paths keep it.
    A put over an object that fails its hash rewrites it.
    """

    def __init__(self, root: Path) -> None:
        self.root = Path(root)
        self.root.mkdir(parents=True, exist_ok=True)

    def _path(self, sha: str) -> Path:
        return self.root / _check_key(sha)[:2] / sha

    def put(self, data: bytes) -> str:
        sha = hashlib.sha256(data).hexdigest()
        path = self._path(sha)
        if self._intact(path, sha):
            # an earlier writer may have died between its rename and its directory fsync
            _fsync_file(path)
            _fsync_dir(path.parent)
            return sha
        if not path.parent.is_dir():
            path.parent.mkdir(parents=True, exist_ok=True)
            _fsync_dir(self.root)
        tmp = path.parent / f".{sha}.{uuid.uuid4().hex}.tmp"
        fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        try:
            try:
                view = memoryview(data)
                while view:
                    view = view[os.write(fd, view) :]
                os.fsync(fd)
            finally:
                os.close(fd)
            os.replace(tmp, path)
        except BaseException:
            tmp.unlink(missing_ok=True)
            raise
        _fsync_dir(path.parent)
        return sha

    def get(self, sha: str) -> bytes:
        path = self._path(sha)
        if not path.is_file():
            raise ObjectNotFound(sha)
        return _verified(sha, path.read_bytes())

    def exists(self, sha: str) -> bool:
        return self._path(sha).is_file()

    @staticmethod
    def _intact(path: Path, sha: str) -> bool:
        try:
            _verified(sha, path.read_bytes())
        except (FileNotFoundError, CorruptObjectError):
            return False
        return True


class MemoryCheckpointStore:
    """In-process store for tests and the simulator: an object is visible once put() returns."""

    def __init__(self) -> None:
        self._objects: dict[str, bytes] = {}

    def put(self, data: bytes) -> str:
        sha = hashlib.sha256(data).hexdigest()
        self._objects.setdefault(sha, bytes(data))
        return sha

    def get(self, sha: str) -> bytes:
        try:
            data = self._objects[_check_key(sha)]
        except KeyError:
            raise ObjectNotFound(sha) from None
        return _verified(sha, data)

    def exists(self, sha: str) -> bool:
        return _check_key(sha) in self._objects
