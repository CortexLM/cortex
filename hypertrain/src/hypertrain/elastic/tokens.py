"""Durable high-water mark of the lease tokens a run's coordinator has issued.

A token fences every older holder and is what a signed leave names, so it must never be issued
twice, not even after the coordinator restarts: a token the coordinator forgot would be handed
to a new lease, and a signed leave of the old one would then release it. Membership saves each
token here before it returns the lease, so a restart continues above every token ever delivered.
The log belongs to one run and has one writer at a time. In production it is a row of the
coordinator's database, where ADR 0003 keeps lease state; a log that is lost, rolled back or
shared reopens the hole, so it fails rather than guesses: a missing or unreadable log is an
error, a log that moved under its coordinator refuses to save, and the local file is locked for
as long as a coordinator holds it. A log restored from an older copy cannot be told from inside,
since the restore brings the mark back with the file: the leave is bound to the coordinator's epoch
(membership.py), which is never stored here.
"""

from __future__ import annotations

import fcntl
import os
import re
import uuid
from pathlib import Path
from typing import Protocol

from hypertrain.elastic.store import _fsync_dir

MAX_TOKEN = 2**53 - 1  # a lease token is a Positive on the wire (messages_v2)
_MARK = re.compile(rb"0|[1-9][0-9]{0,15}")


class TokenLogError(RuntimeError):
    pass


class TokenLog(Protocol):
    def load(self) -> int:
        """The highest token ever saved, 0 for a new run. Raises rather than guess, and when
        another coordinator holds the log."""
        ...

    def save(self, token: int) -> None:
        """Record `token`, which must be above the last mark this log saw and at most
        MAX_TOKEN. Durable once it returns. Raises when it cannot be, or when the stored mark
        is not the one this log last saw (a rollback); then nothing is issued."""
        ...


class MemoryTokenLog:
    """In-process log for tests and the simulator: it forgets everything with the process."""

    def __init__(self) -> None:
        self._token = 0

    def load(self) -> int:
        return self._token

    def save(self, token: int) -> None:
        if not self._token < token <= MAX_TOKEN:
            raise TokenLogError(f"token {token} does not follow the mark {self._token}")
        self._token = token


class LocalTokenLog:
    """One small file replaced atomically: it holds the previous mark or the new one, never a
    torn one. load() takes an exclusive lock on a sidecar file and keeps it until close(), so a
    second coordinator on the same log fails to start instead of issuing the same tokens; the
    operating system drops the lock when the last process holding the descriptor ends. Only the
    process that took the lock saves: a forked copy shares the descriptor and so the lock, and a
    sidecar that was removed or replaced no longer excludes anyone, so both refuse. A new run's
    file is made by create(); a missing file is an error, so a lost volume cannot pass for a new
    run, and so is one that does not parse. Local filesystems with hard links only: flock does
    not promise much on NFS, and create() publishes the file with a link."""

    def __init__(self, path: Path) -> None:
        self._lock: int | None = None
        self._pid = 0
        self._mark: int | None = None
        self.path = Path(path).resolve()  # a symlink or a later chdir must not move the log
        self._sidecar = self.path.with_name(f".{self.path.name}.lock")

    @classmethod
    def create(cls, path: Path, mark: int = 0) -> LocalTokenLog:
        """The log of a new run at `mark`, which is 0 unless an operator restores a lost log
        above every token ever delivered. Refuses a path that already holds a log."""
        log = cls(path)
        if not 0 <= mark <= MAX_TOKEN:
            raise TokenLogError(f"mark {mark} is out of range")
        log.path.parent.mkdir(parents=True, exist_ok=True)
        log._acquire()
        try:
            log._write(mark, exclusive=True)
        except BaseException:
            log.close()
            raise
        return log

    @staticmethod
    def read(path: Path) -> int:
        """The stored mark, without the lock: for tools and tests, not for a coordinator."""
        return LocalTokenLog(path)._read()

    def load(self) -> int:
        self._acquire()
        try:
            self._mark = self._read()
        except BaseException:
            self.close()  # a log that cannot start must not keep the next attempt out
            raise
        return self._mark

    def save(self, token: int) -> None:
        if self._mark is None or not self._held():
            raise TokenLogError(f"token log {self.path.name} is not held by this process")
        if self._read() != self._mark:
            raise TokenLogError(f"token log {self.path.name} changed under its coordinator")
        if not self._mark < token <= MAX_TOKEN:
            raise TokenLogError(f"token {token} does not follow the mark {self._mark}")
        self._write(token)

    def close(self) -> None:
        """Release the lock: the coordinator that held this log is gone."""
        if self._lock is not None:
            os.close(self._lock)
            self._lock = None

    def __del__(self) -> None:
        self.close()

    def _acquire(self) -> None:
        if self._lock is not None:
            return
        fd = os.open(self._sidecar, os.O_RDWR | os.O_CREAT, 0o600)
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            os.close(fd)
            raise TokenLogError(f"token log {self.path.name} is held by another one") from None
        except BaseException:
            os.close(fd)  # no lock support is not a held lock: let the cause through
            raise
        self._lock = fd
        self._pid = os.getpid()

    def _held(self) -> bool:
        """True while this process holds the lock and it is still the one at the sidecar's name."""
        if self._lock is None or self._pid != os.getpid():
            return False
        try:
            return os.path.samestat(os.fstat(self._lock), os.stat(self._sidecar))
        except FileNotFoundError:
            return False

    def _read(self) -> int:
        try:
            raw = self.path.read_bytes()
        except FileNotFoundError:
            raise TokenLogError(f"token log {self.path.name} is missing") from None
        except OSError as exc:
            raise TokenLogError(f"token log {self.path.name} cannot be read: {exc}") from exc
        if not _MARK.fullmatch(raw) or int(raw) > MAX_TOKEN:
            raise TokenLogError(f"token log {self.path.name} does not hold a token")
        return int(raw)

    def _write(self, token: int, *, exclusive: bool = False) -> None:
        tmp = self.path.with_name(f".{self.path.name}.{uuid.uuid4().hex}.tmp")
        fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        try:
            with os.fdopen(fd, "wb") as out:
                out.write(str(token).encode())
                out.flush()
                os.fsync(out.fileno())
            if exclusive:  # a link fails if the name is taken: no log is ever overwritten
                try:
                    os.link(tmp, self.path)
                except FileExistsError:
                    raise TokenLogError(f"token log {self.path.name} already exists") from None
                tmp.unlink()
            else:
                os.replace(tmp, self.path)
            self._mark = token  # the file holds it now, whether or not the directory sync succeeds
        except BaseException:
            tmp.unlink(missing_ok=True)
            raise
        _fsync_dir(self.path.parent)
