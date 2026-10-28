"""aggregate_v2 must not keep the store lock while it computes the tape.

The tape (make_tape, then replay_tape) is the only expensive part of an aggregate. The store lock
also serves `push_beacon` and `latest_beacon`, so holding it for the whole computation stops the
beacon pusher for as long as the computation runs. Both tests run the real flow once (a verified
round 0 of five members, see aggregate_lock_fixture_v2.py), restore it per test, and slow the
REAL `copy_flags` that both make_tape and replay_tape reach, so the bytes of the tape are the ones
the production code computes.
"""

from __future__ import annotations

import threading
import time
from collections.abc import Callable, Iterator
from pathlib import Path
from typing import Any

import pytest
from aggregate_lock_fixture_v2 import Restored, Template, build_template

from hypertrain.aggregator import weighted_v2

WAIT = 20  # seconds: the longest anything may wait before the test calls itself broken
SERVED_WITHIN = 3.0  # seconds a beacon call is given to return while an aggregate computes
COMPUTE_SECONDS = 2.0  # seconds the slow compute takes in the lock-hold test
MAX_HOLD_SECONDS = 1.0

# Only "the lock was held" may count as the expected failure: a broken setup raises RuntimeError.
HOLDS_THE_LOCK = pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="aggregate_v2 holds the store lock for the whole computation (parent behaviour); "
    "removed by the fix",
)


@pytest.fixture(scope="module")
def template(tmp_path_factory: pytest.TempPathFactory) -> Template:
    return build_template(tmp_path_factory)


@pytest.fixture
def restored(template: Template, tmp_path: Path) -> Iterator[Restored]:
    r = Restored(template, tmp_path / "state")
    yield r
    r.close()


class Call:
    """`call()` on a thread of its own: when it ended, how, and what it returned."""

    def __init__(self, name: str, call: Callable[[], Any]) -> None:
        self.name = name
        self.ended = threading.Event()
        self.result: Any = None
        self.error: BaseException | None = None
        self._call = call
        self.thread = threading.Thread(target=self._run, name=name, daemon=True)
        self.thread.start()

    def _run(self) -> None:
        try:
            self.result = self._call()
        except BaseException as error:  # handed to the test, which reports it
            self.error = error
        finally:
            self.ended.set()


def slow_copy_flags(monkeypatch: pytest.MonkeyPatch, pause: Callable[[], None]) -> None:
    """Call the real copy_flags, then `pause()` once, inside the computation of the tape.

    weighted_v2 binds copy_flags by name, so this is the attribute the v2 path reads; patching
    hypertrain.aggregator.core.copy_flags would not reach it.
    """
    real = weighted_v2.copy_flags
    paused: list[bool] = []

    def slow(*args: Any, **kwargs: Any) -> Any:
        out = real(*args, **kwargs)
        if not paused:
            paused.append(True)
            pause()
        return out

    monkeypatch.setattr(weighted_v2, "copy_flags", slow)


@HOLDS_THE_LOCK
def test_a_beacon_round_is_accepted_while_an_aggregate_computes(
    restored: Restored, monkeypatch: pytest.MonkeyPatch
) -> None:
    reached, release = threading.Event(), threading.Event()

    def block_until_released() -> None:
        reached.set()
        release.wait(WAIT)

    slow_copy_flags(monkeypatch, block_until_released)
    store, number = restored.store, restored.now + 1
    aggregate = Call("aggregate_v2", lambda: store.aggregate_v2(restored.run, 0))
    calls = [aggregate]
    try:
        if not reached.wait(WAIT):
            raise RuntimeError(f"the aggregate never reached copy_flags: {aggregate.error!r}")
        push = Call("push_beacon", lambda: restored.push(number))
        latest = Call("latest_beacon", store.latest_beacon)
        calls += [push, latest]
        deadline = time.perf_counter() + SERVED_WITHIN
        for call in (push, latest):
            call.ended.wait(max(0.0, deadline - time.perf_counter()))
        still_computing = not aggregate.ended.is_set()
        blocked = [call for call in (push, latest) if not call.ended.is_set()]
    finally:
        release.set()
        for call in calls:
            call.thread.join(WAIT)
    if any(call.thread.is_alive() for call in calls):
        raise RuntimeError("a thread is still stuck after the compute was released")
    if not still_computing:
        raise RuntimeError(f"the aggregate ended before it was released: {aggregate.error!r}")

    names = [call.name for call in blocked]
    assert not names, (
        f"{' and '.join(names)} did not return within {SERVED_WITHIN:.0f} s of an aggregate_v2 "
        "that was blocked inside its computation; they returned only when the test released it"
    )
    assert push.error is None and push.result["round"] == number
    assert latest.error is None
    assert aggregate.error is None, f"the aggregate failed: {aggregate.error!r}"
    assert {"tape", "theta_hash", "out_state"} <= aggregate.result.keys()


@HOLDS_THE_LOCK
def test_the_store_lock_is_held_for_less_than_a_second_by_an_aggregate_that_computes_for_longer(
    restored: Restored, monkeypatch: pytest.MonkeyPatch
) -> None:
    slow_copy_flags(monkeypatch, lambda: time.sleep(COMPUTE_SECONDS))
    started = time.perf_counter()
    applied = restored.store.aggregate_v2(restored.run, 0)
    elapsed = time.perf_counter() - started

    assert elapsed > COMPUTE_SECONDS, "the aggregate did not compute for long: nothing is measured"
    assert {"tape", "theta_hash", "out_state"} <= applied.keys()
    holds = [hold for hold in restored.lock.holds if hold.start >= started]
    longest = max(holds, key=lambda hold: hold.seconds)
    assert longest.seconds < MAX_HOLD_SECONDS, (
        f"thread {longest.thread} held the store lock for {longest.seconds:.2f} s in one stretch "
        f"({' <- '.join(longest.where[:3])}); the aggregate computed for {elapsed:.2f} s "
        f"and the lock may be held for at most {MAX_HOLD_SECONDS:.1f} s"
    )
