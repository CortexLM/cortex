"""Append-only fsynced JSONL journal (pattern: wan-orchestrator-r14 orchestrate.py:70-145).

A torn final line (crash mid-write) is quarantined to a 0600 sidecar under the journal lock before
any further append; damage before the final line is fatal (JournalCorrupt).
"""

from __future__ import annotations

import contextlib
import fcntl
import hashlib
import json
import os
import time
from collections.abc import Iterator
from pathlib import Path
from typing import Any

Record = dict[str, Any]


class JournalCorrupt(Exception):
    pass


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def fsha(path: str | Path) -> str:
    return sha256(Path(path).read_bytes())


def durable_write(path: str | Path, data: bytes, mode: int = 0o600) -> None:
    """Create-exclusive, fsync file + parent dir (never overwrites)."""
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, mode)
    with os.fdopen(fd, "wb") as stream:
        stream.write(data)
        stream.flush()
        os.fsync(stream.fileno())
    dfd = os.open(Path(path).parent, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(dfd)
    finally:
        os.close(dfd)


@contextlib.contextmanager
def flock(path: str | Path, blocking: bool = True) -> Iterator[None]:
    with open(path, "a") as handle:
        fcntl.flock(handle, fcntl.LOCK_EX | (0 if blocking else fcntl.LOCK_NB))
        yield


class Journal:
    def __init__(self, run_dir: str | Path) -> None:
        self.dir = Path(run_dir)
        self.path = self.dir / "journal.jsonl"
        self.lock = self.dir / "journal.lock"
        if self.path.exists():
            with flock(self.lock):
                self._repair()

    def _repair(self) -> None:
        if not self.path.exists():
            return
        data = self.path.read_bytes()
        if not data or data.endswith(b"\n"):
            return
        cut = data.rfind(b"\n") + 1
        tail = data[cut:]
        try:
            complete = isinstance(json.loads(tail), dict)
        except ValueError:
            complete = False
        sidecar = None
        fd = os.open(self.path, os.O_WRONLY | os.O_APPEND)
        try:
            if complete:
                os.write(fd, b"\n")
                action = "newline_restored"
            else:
                sidecar = f"journal.torn-{time.time_ns()}-{os.getpid()}.bin"
                durable_write(self.dir / sidecar, tail)
                os.ftruncate(fd, cut)
                action = "torn_tail_quarantined"
            os.fsync(fd)
        finally:
            os.close(fd)
        self._write(
            self._line(
                {
                    "kind": "journal_repaired",
                    "unix": time.time(),
                    "pid": os.getpid(),
                    "action": action,
                    "offset": cut,
                    "tail_sha256": sha256(tail),
                    "sidecar": sidecar,
                }
            )
        )

    @staticmethod
    def _line(record: Record) -> str:
        return json.dumps(record, sort_keys=True, allow_nan=False) + "\n"

    def _write(self, line: str) -> None:
        fd = os.open(self.path, os.O_WRONLY | os.O_APPEND | os.O_CREAT, 0o600)
        with os.fdopen(fd, "a") as stream:
            stream.write(line)
            stream.flush()
            os.fsync(stream.fileno())

    def append(self, kind: str, **value: Any) -> Record:
        record: Record = {"kind": kind, "unix": time.time(), "pid": os.getpid(), **value}
        line = self._line(record)
        with flock(self.lock):
            self._repair()
            self._write(line)
        return record

    def records(self) -> list[Record]:
        if not self.path.exists():
            return []
        lines = self.path.read_text().split("\n")
        out: list[Record] = []
        for index, line in enumerate(lines):
            if not line:
                continue
            try:
                out.append(json.loads(line))
            except ValueError:
                if index < len(lines) - 1 and any(lines[index + 1 :]):
                    raise JournalCorrupt(f"line {index + 1}") from None
        return out

    def all(self, kind: str, **match: Any) -> list[Record]:
        return [
            r
            for r in self.records()
            if r["kind"] == kind and all(r.get(k) == v for k, v in match.items())
        ]

    def last(self, kind: str, **match: Any) -> Record | None:
        found = self.all(kind, **match)
        return found[-1] if found else None
