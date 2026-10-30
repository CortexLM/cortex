"""A beacon round must not wait for a long transaction of the store that hosts the run.

Run 0ef600b4 (TEST stack, 30/10, a run hosted by a CHILD store): while the child ran a long admin
transaction (`open` 73 s, `aggregate` 3 min 23 s, `finalize` up to 4.5 min) the root committed
every pushed round at once (`/v1/beacon/latest` kept moving), then `push_beacon` waited for the
child's `_tx()` lock: the pusher's POST timed out after 10 s, its `pending` grew to 11, the child's
view (the `beacon` table of runs/<run>/challenge.db, what `_fresh_v2` reads) fell up to 15 rounds
behind and the callers that took the lock first after the transaction were refused "beacon
infrastructure paused". Here the long transaction is the child's own `_tx()`, held open by a
thread; the clock follows the pushed rounds, as `Network.push` sets it.
"""

from __future__ import annotations

import sqlite3
import threading
from collections.abc import Iterator
from contextlib import contextmanager

import pytest

from hypertrain.challenge.store import ChallengeError, ChallengeStore

from .child_run_fixture_v2 import child_network  # noqa: F401  (pytest fixture import)
from .test_service_network_v2 import fixture_beacon

WAIT = 10  # seconds: far above a push that does not wait, far below a test timeout


def wall_at(network, number):
    """The master's wall clock at the emission time of round `number`, for root and child."""
    at = network.manifest.training.beacon.genesis_time + (number - 1) * 3
    network.root.clock = lambda: at
    network.store.clock = lambda: at


def committed_view(store: ChallengeStore) -> int:
    """MAX(round) committed in the store's own file, read as lagloop.py reads it on s100."""
    db = sqlite3.connect(f"file:{store.state_dir / 'challenge.db'}?mode=ro", uri=True)
    try:
        return int(db.execute("SELECT MAX(round) FROM beacon").fetchone()[0] or 0)
    finally:
        db.close()


@contextmanager
def long_transaction(store: ChallengeStore, *, fail: bool = False) -> Iterator[dict[str, object]]:
    """Hold `store._tx()` open in a thread until the block ends. The transaction writes the meta
    key `long-transaction`; `seen` holds the views it read at its start and at its end, and the
    error its thread met, if any. With `fail` it raises at its end, so it rolls back."""
    started, release = threading.Event(), threading.Event()
    seen: dict[str, object] = {}

    def body() -> None:
        try:
            with store._tx() as db:
                seen["start"] = store._now(db)
                db.execute("INSERT OR REPLACE INTO meta VALUES('long-transaction', '1')")
                started.set()
                release.wait(WAIT)
                seen["end"] = store._now(db)
                if fail:
                    raise RuntimeError("the admin transaction fails")
        except Exception as error:
            seen["error"] = error

    holder = threading.Thread(target=body, name="long-admin-tx")
    holder.start()
    assert started.wait(WAIT), "the long transaction never started"
    try:
        yield seen
    finally:
        release.set()
        holder.join(WAIT)
        assert not holder.is_alive(), "the long transaction never ended"


def pushed_during(store: ChallengeStore, network, number, **kwargs) -> dict[str, object]:
    """Push round `number` while a long transaction of `store` is open; the push must return."""
    answers: list[object] = []
    with long_transaction(store, **kwargs) as seen:
        pusher = push_in_thread(network, number, answers)
        pusher.join(WAIT / 2)
        assert not pusher.is_alive(), "the push waited for the child's transaction"
    assert answers == [{"round": number, "latest": number}]
    return seen


def push_in_thread(network, number, answers: list[object]) -> threading.Thread:
    """`root.push_beacon` of round `number` in a thread; its answer, or error, goes to `answers`."""

    def push() -> None:
        try:
            answers.append(network.root.push_beacon(fixture_beacon(number)))
        except Exception as error:
            answers.append(error)

    pusher = threading.Thread(target=push, name="push")
    pusher.start()
    return pusher


@pytest.mark.parametrize("fail", [False, True], ids=["commits", "rolls-back"])
def test_a_round_pushed_during_a_long_child_transaction_is_answered_and_then_seen(
    child_network,  # noqa: F811
    fail,
):
    network, child = child_network, child_network.store
    before = child._now(child._db)
    number = before + 1
    wall_at(network, number)
    answers: list[object] = []
    with long_transaction(child, fail=fail) as seen:
        pusher = push_in_thread(network, number, answers)
        pusher.join(WAIT / 2)
        # The S21 timeouts: the root holds the round, the push must not wait for the child.
        assert not pusher.is_alive(), "the push waited for the child's transaction"
        assert answers == [{"round": number, "latest": number}]
        assert network.root.latest_beacon()["round"] == number
        # Not inside the transaction: its view and the child's file stay where they were.
        assert committed_view(child) == before
        # The first holder after the long one, waiting for the lock, must see it; even one that
        # takes the bare lock (an aggregate flight), which writes no staged round itself.
        after: list[int] = []

        def follow() -> None:
            with child._lock:
                after.append(child._now(child._db))

        follower = threading.Thread(target=follow, name="follower")
        follower.start()
    follower.join(WAIT)
    assert (seen["start"], seen["end"], "error" in seen) == (before, before, fail)
    assert after == [number]
    assert committed_view(child) == number


def state_of(store: ChallengeStore, run: str) -> tuple[list[tuple], ...]:
    """What a round's arrival can change: the beacon rows, the pins and every run record."""
    return tuple(
        [tuple(row) for row in store._db.execute(sql, args)]
        for sql, args in (
            ("SELECT * FROM beacon ORDER BY round", ()),
            ("SELECT * FROM beacon_pins_v2 WHERE run_id=? ORDER BY operation_id", (run,)),
            ("SELECT kind,id,data FROM records_v2 WHERE run_id=? ORDER BY kind,id", (run,)),
        )
    )


def test_a_push_at_any_point_of_a_long_transaction_leaves_the_same_records(
    child_network,  # noqa: F811
    tmp_path,
):
    """Pins, seeds and records do not depend on when a round arrives inside one transaction: the
    transaction reads one view throughout and the round is written once it ends."""
    network, child = child_network, child_network.store
    run, start = network.manifest.run_id(), child._now(child._db)
    wall_at(network, start + 1)
    snapshot = tmp_path / "start.db"
    child._db.execute("VACUUM INTO ?", (str(snapshot),))
    outcomes = []
    for at in range(3):  # the round lands before the first, the second or the third read
        source = sqlite3.connect(snapshot)  # every case starts from the same child file
        source.backup(child._db)
        source.close()
        reads, answers = [], []
        with long_transaction(child):
            for point in range(3):
                if point == at:
                    pusher = push_in_thread(network, start + 1, answers)
                    pusher.join(WAIT / 2)
                    assert not pusher.is_alive(), "the push waited for the child's transaction"
                # the shared connection is inside the holder's transaction: these are its reads
                reads.append(child._fresh_v2(network.manifest, f"interleave:{point}"))
        network.push(start + 2)  # the next push finds the lock free and runs what is due
        outcomes.append((reads, state_of(child, run)))
    assert outcomes[0][0] == [start] * 3
    assert [row[0] for row in outcomes[0][1][0][-2:]] == [start + 1, start + 2]
    assert outcomes[1] == outcomes[0] and outcomes[2] == outcomes[0]


class FailingBeaconWrite:
    """A child connection whose next write of beacon rows fails, as on a full disk."""

    def __init__(self, db: sqlite3.Connection) -> None:
        self._db = db
        self.armed = True

    def executemany(self, sql: str, rows: object) -> sqlite3.Cursor:
        if self.armed and "INTO beacon" in sql:
            self.armed = False
            raise sqlite3.OperationalError("database or disk is full")
        return self._db.executemany(sql, rows)  # type: ignore[arg-type]

    def __getattr__(self, name: str) -> object:
        return getattr(self._db, name)


def test_a_round_staged_when_the_process_stops_is_in_the_child_after_a_restart(
    child_network,  # noqa: F811
    monkeypatch,
):
    network, child, root = child_network, child_network.store, child_network.root
    number = child._now(child._db) + 1
    wall_at(network, number)
    with long_transaction(child):
        answers: list[object] = []
        pusher = push_in_thread(network, number, answers)
        pusher.join(WAIT / 2)
        assert not pusher.is_alive(), "the push waited for the child's transaction"
        monkeypatch.setattr(child, "_db", FailingBeaconWrite(child._db))
    # The write of the staged round failed: it is only in memory when the process stops.
    assert committed_view(child) == number - 1
    reopened = ChallengeStore(
        root.state_dir, root.params, root.coord, root.owner_hotkey, root.verify_beacon, root.objects
    )
    try:
        again = reopened.for_run(network.manifest.run_id())
        assert again is not reopened
        assert committed_view(child) == number
        assert again._beacon_v2(number) == root._beacon_v2(number)
    finally:
        reopened.close()


def test_a_staged_round_whose_write_fails_is_written_by_the_next_transaction(
    child_network,  # noqa: F811
    monkeypatch,
):
    network, child = child_network, child_network.store
    number = child._now(child._db) + 1
    wall_at(network, number)
    failing = FailingBeaconWrite(child._db)
    with long_transaction(child) as seen:
        answers: list[object] = []
        pusher = push_in_thread(network, number, answers)
        pusher.join(WAIT / 2)
        assert not pusher.is_alive(), "the push waited for the child's transaction"
        monkeypatch.setattr(child, "_db", failing)
    # The holder's own work is committed and it met no error; the staged round's write failed.
    assert "error" not in seen and not failing.armed
    assert child._db.execute("SELECT value FROM meta WHERE key='long-transaction'").fetchone()
    assert committed_view(child) == number - 1
    with child._tx() as db:
        assert child._now(db) == number


def test_a_staged_round_lands_once_and_never_overwrites_a_stored_one(
    child_network,  # noqa: F811
):
    network, child, root = child_network, child_network.store, child_network.root
    number = child._now(child._db) + 1
    wall_at(network, number)
    held = ("ab" * 48, "cd" * 32)  # a row only the child holds for this round
    with child._tx() as db:
        db.execute("INSERT INTO beacon VALUES(?, ?, ?, 0)", (number, *held))
    for _ in range(2):  # a pusher that retries the round lands it once, with no error
        seen = pushed_during(child, network, number)
        assert "error" not in seen
    rows = child._db.execute("SELECT signature, randomness FROM beacon WHERE round=?", (number,))
    assert [tuple(row) for row in rows] == [held]
    assert root._beacon_v2(number).signature == fixture_beacon(number)["signature"] != held[0]
    with pytest.raises(ChallengeError) as refused:  # a free child refuses it, as it always did
        root.push_beacon(fixture_beacon(number))
    assert refused.value.status == 409


def test_a_round_staged_under_an_outer_lock_holder_waits_for_its_last_transaction(
    child_network,  # noqa: F811
):
    network, child = child_network, child_network.store
    before = child._now(child._db)
    wall_at(network, before + 1)
    answers: list[object] = []
    with child._lock:  # e.g. an aggregate flight: one view across its reads and transactions
        pusher = push_in_thread(network, before + 1, answers)
        pusher.join(WAIT / 2)
        assert not pusher.is_alive(), "the push waited for the child's lock"
        with child._tx() as db:
            assert child._now(db) == before
        assert child._now(child._db) == before
    with child._tx() as db:
        assert child._now(db) == before + 1


def test_a_late_round_on_a_free_child_leaves_the_pause_its_view_was_in(
    child_network,  # noqa: F811
):
    """The pause witness of a free child reads the view before the pushed round, as before."""
    network, child = child_network, child_network.store
    run, before = network.manifest.run_id(), child._now(child._db)
    wall_at(network, before + 10)
    network.root.push_beacon(fixture_beacon(before + 1))
    pauses = child._db.execute(
        "SELECT id, data FROM records_v2 WHERE run_id=? AND kind='beacon-pause'", (run,)
    ).fetchall()
    assert [tuple(row) for row in pauses] == [(str(before), f'{{"to":{before + 7}}}')]


@pytest.mark.parametrize("case", ["outage-then-busy-head", "long-transaction"])
def test_a_view_callers_were_held_at_is_witnessed_though_its_rounds_were_staged(
    child_network,  # noqa: F811
    case,
):
    """The pause witness reads the view before the staged rounds were written, as it reads a
    free child's view before the pushed round: the pause waives the blame of whoever was held."""
    network, child = child_network, child_network.store
    run, view = network.manifest.run_id(), child._now(child._db)
    if case == "outage-then-busy-head":  # the pusher was down; its head lands on a busy child
        wall = view + 21
        wall_at(network, wall)
        pushed_during(child, network, wall)
        late = view + 1  # then the oldest pending round, on a free child
    else:  # the child's lock is held across 10 rounds the pusher delivers on time
        answers: list[object] = []
        with long_transaction(child):
            for number in range(view + 1, view + 11):
                wall_at(network, number)
                pusher = push_in_thread(network, number, answers)
                pusher.join(WAIT / 2)
                assert not pusher.is_alive(), "the push waited for the child's transaction"
        wall = late = view + 11
        wall_at(network, wall)
    network.root.push_beacon(fixture_beacon(late))
    pauses = child._db.execute(
        "SELECT id, data FROM records_v2 WHERE run_id=? AND kind='beacon-pause'", (run,)
    ).fetchall()
    assert [tuple(row) for row in pauses] == [(str(view), f'{{"to":{wall - 3}}}')]


def test_a_round_staged_while_a_free_push_runs_is_written_before_it_returns(
    child_network,  # noqa: F811
    monkeypatch,
):
    network, child = child_network, child_network.store
    number = child._now(child._db) + 1
    wall_at(network, number + 1)
    store, answers = child._store_beacon_v2, []

    def store_while_another_round_arrives(br):
        result = store(br)
        if br.round == number:  # the push of `number` holds the lock: `number + 1` is staged
            pusher = push_in_thread(network, number + 1, answers)
            pusher.join(WAIT / 2)
            assert not pusher.is_alive(), "the push waited for the child's lock"
        return result

    monkeypatch.setattr(child, "_store_beacon_v2", store_while_another_round_arrives)
    network.root.push_beacon(fixture_beacon(number))
    assert answers == [{"round": number + 1, "latest": number + 1}]
    assert committed_view(child) == number + 1
