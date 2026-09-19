"""Private durable run checkpoints and content-addressed compaction archives."""

from __future__ import annotations

import fcntl
import hashlib
import json
import os
import sqlite3
import stat
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path
from typing import Any

from cortex.state import StateSecurityError, secure_sqlite_path, secure_state_directory

from .errors import ToolRejected


class RunJournal:
    def __init__(self, directory: str | Path) -> None:
        self.directory = secure_state_directory(directory)
        path = secure_sqlite_path(self.directory / "runs.sqlite3")
        self.connection = sqlite3.connect(path, timeout=10)
        self.connection.execute("PRAGMA journal_mode=WAL")
        self.connection.executescript(
            """
            CREATE TABLE IF NOT EXISTS runs (
                run_id TEXT PRIMARY KEY,
                task_digest TEXT NOT NULL,
                limits_digest TEXT NOT NULL,
                deadline REAL NOT NULL,
                state TEXT NOT NULL,
                state_digest TEXT NOT NULL,
                status TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS archives (
                run_id TEXT NOT NULL,
                digest TEXT NOT NULL,
                body TEXT NOT NULL,
                PRIMARY KEY (run_id, digest)
            );
            """
        )

    def close(self) -> None:
        self.connection.close()

    @contextmanager
    def claim(self, run_id: str) -> Iterator[None]:
        filename = hashlib.sha256(run_id.encode()).hexdigest() + ".lock"
        path = self.directory / filename
        flags = os.O_RDWR | os.O_CLOEXEC | os.O_NONBLOCK | os.O_NOFOLLOW
        created = False
        try:
            try:
                descriptor = os.open(path, flags | os.O_CREAT | os.O_EXCL, 0o600)
                created = True
            except FileExistsError:
                descriptor = os.open(path, flags)
        except OSError as error:
            raise StateSecurityError(f"{path}: unsafe RLM job lock") from error
        try:
            if created:
                os.fchmod(descriptor, 0o600)
            metadata = os.fstat(descriptor)
            if (
                not stat.S_ISREG(metadata.st_mode)
                or metadata.st_uid != os.geteuid()
                or stat.S_IMODE(metadata.st_mode) != 0o600
                or metadata.st_nlink != 1
            ):
                raise StateSecurityError(f"{path}: job lock must be a private owned regular file")
            try:
                fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                raise ToolRejected("RLM job is already running") from None
            yield
        finally:
            os.close(descriptor)

    def load(self, run_id: str) -> dict[str, Any] | None:
        row = self.connection.execute(
            "SELECT task_digest, limits_digest, deadline, state, state_digest, status "
            "FROM runs WHERE run_id = ?",
            (run_id,),
        ).fetchone()
        if row is None:
            return None
        task_digest, limits_digest, deadline, body, digest, status = row
        if hashlib.sha256(body.encode()).hexdigest() != digest:
            raise ToolRejected("RLM checkpoint integrity check failed")
        return {
            "task_digest": task_digest,
            "limits_digest": limits_digest,
            "deadline": deadline,
            "state": json.loads(body),
            "status": status,
        }

    def save(
        self,
        run_id: str,
        *,
        task_digest: str,
        limits_digest: str,
        deadline: float,
        state: dict[str, Any],
        status: str = "running",
    ) -> None:
        body = json.dumps(state, sort_keys=True, separators=(",", ":"), allow_nan=False)
        digest = hashlib.sha256(body.encode()).hexdigest()
        with self.connection:
            self.connection.execute(
                "INSERT INTO runs VALUES (?, ?, ?, ?, ?, ?, ?) "
                "ON CONFLICT(run_id) DO UPDATE SET state = excluded.state, "
                "state_digest = excluded.state_digest, status = excluded.status",
                (run_id, task_digest, limits_digest, deadline, body, digest, status),
            )

    def archive(self, run_id: str, body: str) -> str:
        digest = hashlib.sha256(body.encode()).hexdigest()
        with self.connection:
            self.connection.execute(
                "INSERT OR IGNORE INTO archives VALUES (?, ?, ?)", (run_id, digest, body)
            )
        return digest

    def read_archive(self, run_id: str, digest: str) -> str:
        row = self.connection.execute(
            "SELECT body FROM archives WHERE run_id = ? AND digest = ?", (run_id, digest)
        ).fetchone()
        if row is None or hashlib.sha256(row[0].encode()).hexdigest() != digest:
            raise ToolRejected("RLM archive missing or corrupt")
        return str(row[0])
