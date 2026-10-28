"""One verified round 0, built once and restored cheaply, on a store whose lock is timed.

`build_template` runs the repository's real flow once (five ACTIVE identities, real CPU training,
relay upload, real audits: `five_identity_network` and `rollback.execute_round(aggregate=False)`)
and snapshots the state directory. `Restored` copies the snapshot to a private directory and opens
a bare `ChallengeStore` on it, so a test drives `aggregate_v2`, `finalize_v2`, `push_beacon` and
`latest_beacon` directly, a few hundredths of a second after the fixture is requested.

`TimedRLock` replaces the store lock and records every stretch a thread holds it, so a test can
state how long a call kept the store from serving anything else.
"""

from __future__ import annotations

import shutil
import sqlite3
import sys
import threading
import time
from contextlib import closing
from dataclasses import dataclass
from pathlib import Path
from types import FrameType
from typing import Any

import pytest
import test_rollback_service_v2 as rollback

from challenge.five_identity_network_v2 import five_identity_network
from hypertrain.challenge.store import ChallengeStore
from hypertrain.data.store import LocalFSStore
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages import Finalize

fixtures = rollback.fixtures


@dataclass(frozen=True)
class Hold:
    """One outermost stretch of a thread holding the lock: when, for how long, taken where."""

    thread: str
    start: float
    seconds: float
    where: tuple[str, ...]


def _caller() -> tuple[str, ...]:
    """The five innermost frames that asked for the lock, `file:line:function` each."""
    frames: list[str] = []
    frame: FrameType | None = sys._getframe(2)  # 0 is here, 1 the lock method, 2 its caller
    while frame is not None and len(frames) < 5:
        if "contextlib" not in frame.f_code.co_filename:
            name = frame.f_code.co_filename.rsplit("/", 1)[-1]
            frames.append(f"{name}:{frame.f_lineno}:{frame.f_code.co_name}")
        frame = frame.f_back
    return tuple(frames)


class TimedRLock:
    """Drop-in for the store's RLock: `with`, acquire, release and `_is_owned`.

    A thread that re-enters the lock (a nested `_tx`, an escrow savepoint) extends its current
    hold; only the outermost acquire and the matching last release delimit a recorded Hold.
    """

    def __init__(self) -> None:
        self._inner = threading.RLock()
        self._mine = threading.local()
        self._guard = threading.Lock()
        self.holds: list[Hold] = []

    def acquire(self, blocking: bool = True, timeout: float = -1) -> bool:
        got = self._inner.acquire(blocking, timeout)
        if got:
            depth = getattr(self._mine, "depth", 0)
            if depth == 0:
                self._mine.start = time.perf_counter()
                self._mine.where = _caller()
            self._mine.depth = depth + 1
        return got

    def release(self) -> None:
        end = time.perf_counter()
        self._inner.release()  # raises when the thread does not own it, before any bookkeeping
        self._mine.depth -= 1
        if self._mine.depth == 0:
            hold = Hold(
                threading.current_thread().name,
                self._mine.start,
                end - self._mine.start,
                self._mine.where,
            )
            with self._guard:
                self.holds.append(hold)

    __enter__ = acquire

    def __exit__(self, *exc: object) -> None:
        self.release()

    def _is_owned(self) -> bool:
        return bool(self._inner._is_owned())  # type: ignore[attr-defined]


@dataclass(frozen=True)
class Template:
    snapshot: Path
    manifest: Any
    params: Any
    coord: Any
    owner_hotkey: str
    verify_beacon: Any


def build_template(tmp_path_factory: pytest.TempPathFactory) -> Template:
    """Body of a module-scoped fixture: round 0 of five members, past its audits, not aggregated."""
    generator = five_identity_network(tmp_path_factory, fixtures)
    network = next(generator)
    try:
        rollback.execute_round(network, 0, aggregate=False)
        store = network.store
        snapshot = tmp_path_factory.mktemp("template") / "state"
        # server_keys.db belongs to the HTTP app, which a restored bare store does not have.
        skipped = shutil.ignore_patterns("*.sock", "challenge.db*", "server_keys.db*")
        shutil.copytree(store.state_dir, snapshot, ignore=skipped)
        # The database is copied through SQLite's backup, not as files: it is open in WAL mode.
        with closing(sqlite3.connect(snapshot / "challenge.db")) as target:
            store._db.backup(target)
        return Template(
            snapshot,
            network.manifest,
            store.params,
            store.coord,
            store.owner_hotkey,
            store.verify_beacon,
        )
    finally:
        generator.close()


class Restored:
    """A bare root ChallengeStore on a private copy of the template, with a TimedRLock."""

    def __init__(self, template: Template, directory: Path, *, warm_services: bool = True) -> None:
        shutil.copytree(template.snapshot, directory)
        self.manifest = template.manifest
        self.run: str = template.manifest.run_id()
        self.store = ChallengeStore(
            directory,
            template.params,
            template.coord,
            template.owner_hotkey,
            template.verify_beacon,
            LocalFSStore(directory / "objects"),
        )
        # The escrow and the intake capture `store._lock` when the services are built, so a lock
        # swapped in after that is never seen by them: swap before the first `_services`.
        self.lock = TimedRLock()
        self.store._lock = self.lock
        self.now: int = self.store._now(self.store._db)
        self.set_clock(self.now)
        # A cold store builds its services lazily and its first call opens a transaction of its
        # own, which fails inside aggregate_v2's: build them outside any transaction, as the
        # requests of a running process always have. A test of the cold store passes False.
        if warm_services:
            self.store._services(self.run)

    def set_clock(self, number: int) -> None:
        """The wall clock as if drand round `number` had just been emitted (see Network.push)."""
        genesis = self.manifest.training.beacon.genesis_time
        self.store.clock = lambda: genesis + (number - 1) * 3

    def push(self, number: int) -> dict[str, Any]:
        """Push beacon round `number` straight into the store, as Network.push does over HTTP."""
        self.set_clock(number)
        self.now = number
        return self.store.push_beacon(fixtures.fixture_beacon(number))

    def finalize_raw(self, applied: dict[str, Any]) -> bytes:
        """The signed Finalize for round 0: it must list exactly the inputs of the applied tape."""
        included = sorted(i["roster"]["hotkey"] for i in applied["tape"]["body"]["inputs"])
        final = Finalize(
            w=0,
            final_theta_hash_w1=applied["theta_hash"],
            included=included,
            entitlements_root="0" * 64,
        )
        return canonicalize(fixtures.seal(fixtures.COORD, "Finalize", self.run, final, 10000))

    def close(self) -> None:
        self.store.close()
