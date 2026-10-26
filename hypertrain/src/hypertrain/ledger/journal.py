"""Append-only, fsynced, hash-chained JSONL journal (pattern of the r14 orchestrator Journal).

Every line is the canonical JSON of one record carrying `seq`, `prev_hash` and `hash`, where
`hash = sha256(canonical(record without hash))`. Readers verify every byte: any line that is not
exactly the canonical encoding of a correctly chained record raises ChainBreak. The only tolerated
damage is a torn final write (bytes after the last newline that are a strict prefix of a record):
under the lock it is quarantined to a 0600 sidecar, truncated, and a `journal_repaired` record is
appended. Tail bytes that are a whole record plus junk are corruption, not a torn write.
"""

from __future__ import annotations

import fcntl
import hashlib
import json
import os
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path
from typing import Any

GENESIS_HASH = "0" * 64


class ChainBreak(Exception):
    """The journal bytes are not a valid hash chain."""


def canonical(value: Any) -> bytes:
    return json.dumps(
        value, sort_keys=True, separators=(",", ":"), ensure_ascii=True, allow_nan=False
    ).encode()


def _sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def durable_write(path: Path, data: bytes) -> None:
    """Write via tmp + fsync + rename + directory fsync, mode 0600."""
    tmp = path.with_name(f".{path.name}.tmp-{os.getpid()}")
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    try:
        os.write(fd, data)
        os.fsync(fd)
    finally:
        os.close(fd)
    os.replace(tmp, path)
    dfd = os.open(path.parent, os.O_RDONLY)
    try:
        os.fsync(dfd)
    finally:
        os.close(dfd)


@contextmanager
def _flock(path: Path) -> Iterator[None]:
    with open(path, "a") as handle:
        fcntl.flock(handle, fcntl.LOCK_EX)
        yield


def parse_chain(data: bytes) -> list[dict[str, Any]]:
    """Verify newline-terminated journal bytes; raise ChainBreak on any deviation."""
    records: list[dict[str, Any]] = []
    prev = GENESIS_HASH
    if data and not data.endswith(b"\n"):
        raise ChainBreak("journal does not end with a newline")
    for index, line in enumerate(data.split(b"\n")[:-1] if data else []):
        try:
            record = json.loads(line)
        except ValueError as exc:
            raise ChainBreak(f"line {index}: not JSON") from exc
        if not isinstance(record, dict) or not {"seq", "prev_hash", "hash", "kind"} <= set(record):
            raise ChainBreak(f"line {index}: not a journal record")
        if canonical(record) != line:
            raise ChainBreak(f"line {index}: not canonical")
        body = {k: v for k, v in record.items() if k != "hash"}
        if record["seq"] != index or record["prev_hash"] != prev:
            raise ChainBreak(f"line {index}: broken link")
        if record["hash"] != _sha(canonical(body)):
            raise ChainBreak(f"line {index}: hash mismatch")
        prev = record["hash"]
        records.append(record)
    return records


class Journal:
    def __init__(self, directory: Path) -> None:
        self.dir = Path(directory)
        self.dir.mkdir(parents=True, exist_ok=True)
        self.path = self.dir / "journal.jsonl"
        self.lock = self.dir / "journal.lock"
        with _flock(self.lock):
            self._records = self._load_and_repair()
            self._size = self.path.stat().st_size if self.path.exists() else 0

    @property
    def records(self) -> list[dict[str, Any]]:
        """Verified records; callers must treat the list as read-only."""
        return self._records

    def _load_and_repair(self) -> list[dict[str, Any]]:
        data = self.path.read_bytes() if self.path.exists() else b""
        cut = data.rfind(b"\n") + 1
        tail = data[cut:]
        records = parse_chain(data[:cut])
        if tail:
            try:
                whole = json.loads(tail[:-1]) if len(tail) > 1 else None
            except ValueError:
                whole = None
            if isinstance(whole, dict):
                raise ChainBreak("final newline replaced: corruption, not a torn write")
            try:
                complete = json.loads(tail)
            except ValueError:
                complete = None
            if isinstance(complete, dict):
                # whole record, only its newline was lost: it must still chain correctly
                records = parse_chain(data + b"\n")
                self._append_raw(b"\n")
            else:
                sidecar = self.dir / f"journal.torn-{cut}-{_sha(tail)[:16]}.bin"
                durable_write(sidecar, tail)
                fd = os.open(self.path, os.O_WRONLY)
                try:
                    os.ftruncate(fd, cut)
                    os.fsync(fd)
                finally:
                    os.close(fd)
                self._records = records
                records.append(
                    self._make_and_write(
                        "journal_repaired",
                        {
                            "action": "torn_tail_quarantined",
                            "offset": cut,
                            "tail_bytes": len(tail),
                            "tail_sha256": _sha(tail),
                            "sidecar": sidecar.name,
                        },
                    )
                )
        return records

    def _append_raw(self, data: bytes) -> None:
        fd = os.open(self.path, os.O_WRONLY | os.O_APPEND | os.O_CREAT, 0o600)
        try:
            os.write(fd, data)
            os.fsync(fd)
        finally:
            os.close(fd)

    def _make_and_write(self, kind: str, value: dict[str, Any]) -> dict[str, Any]:
        if {"seq", "prev_hash", "hash", "kind"} & set(value):
            raise ValueError("reserved journal field")
        prev = self._records[-1]["hash"] if self._records else GENESIS_HASH
        record: dict[str, Any] = {"kind": kind, "seq": len(self._records), "prev_hash": prev}
        record.update(value)
        record["hash"] = _sha(canonical(record))
        self._append_raw(canonical(record) + b"\n")
        return record

    def append(self, kind: str, value: dict[str, Any]) -> dict[str, Any]:
        with _flock(self.lock):
            on_disk = self.path.stat().st_size if self.path.exists() else 0
            if on_disk != self._size:
                raise ChainBreak("journal changed under this writer (single writer required)")
            record = self._make_and_write(kind, value)
            self._size += len(canonical(record)) + 1
            self._records.append(record)
            return record
