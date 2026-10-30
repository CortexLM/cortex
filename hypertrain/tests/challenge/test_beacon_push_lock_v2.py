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

from hypertrain.challenge.store import ChallengeStore

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
def long_transaction(store: ChallengeStore) -> Iterator[dict[str, object]]:
    """Hold `store._tx()` open in a thread until the block ends; `seen` holds the views the
    transaction read at its start and at its end."""
    started, release = threading.Event(), threading.Event()
    seen: dict[str, object] = {}

    def body() -> None:
        with store._tx() as db:
            seen["start"] = store._now(db)
            started.set()
            release.wait(WAIT)
            seen["end"] = store._now(db)

    holder = threading.Thread(target=body, name="long-admin-tx")
    holder.start()
    assert started.wait(WAIT), "the long transaction never started"
    try:
        yield seen
    finally:
        release.set()
        holder.join(WAIT)
        assert not holder.is_alive(), "the long transaction never ended"


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


def test_a_round_pushed_during_a_long_child_transaction_is_answered_and_then_seen(
    child_network,  # noqa: F811
):
    network, child = child_network, child_network.store
    before = child._now(child._db)
    number = before + 1
    wall_at(network, number)
    answers: list[object] = []
    with long_transaction(child) as seen:
        pusher = push_in_thread(network, number, answers)
        pusher.join(WAIT / 2)
        # The S21 timeouts: the root holds the round, the push must not wait for the child.
        assert not pusher.is_alive(), "the push waited for the child's transaction"
        assert answers == [{"round": number, "latest": number}]
        assert network.root.latest_beacon()["round"] == number
        # Not inside the transaction: its view and the child's file stay where they were.
        assert committed_view(child) == before
        # The first transaction after the long one, already waiting for the lock, must see it.
        after: list[int] = []

        def follow() -> None:
            with child._tx() as db:
                after.append(child._now(db))

        follower = threading.Thread(target=follow, name="follower")
        follower.start()
    follower.join(WAIT)
    assert seen == {"start": before, "end": before}
    assert after == [number]
    assert committed_view(child) == number
