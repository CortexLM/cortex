"""finalize_v2 must not keep the store lock while it replays the applied tape.

The replay of the applied tape (replay_tape, a full recomputation of the aggregate) is the only
expensive part of a finalize. It used to run inside the finalize transaction, under the store lock
that also serves `push_beacon` and `latest_beacon`. `finalize_v2` now replays it first, without
the lock, and the transaction reuses that verdict only for exactly the rows it replayed.

Every test runs the real flow once (a verified round 0 of five members, see
aggregate_lock_fixture_v2.py), restores it per test, aggregates normally and then finalizes. The
REAL `replay_tape` is wrapped, never replaced, so the bytes are the ones production computes; the
wrapper records whether the caller owned the store lock. "The parent" is `finalize_v2` with its
pre-step switched off, which is the code before this change.
"""

from __future__ import annotations

import shutil
import sqlite3
import threading
import time
from collections.abc import Callable, Iterator
from contextlib import closing, contextmanager
from dataclasses import replace
from pathlib import Path
from typing import Any

import pytest
from aggregate_lock_fixture_v2 import Restored, Template, build_template, fixtures

from hypertrain.aggregator import tape_v2
from hypertrain.aggregator.core import TapeError, TapeIntegrityError
from hypertrain.challenge.store import FINALITY_VERDICTS, ChallengeStore
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages import Finalize

WAIT = 20  # seconds: the longest anything may wait before the test calls itself broken
SERVED_WITHIN = 3.0  # seconds a beacon call is given to return while a finalize replays
REPLAY_SECONDS = 2.0  # seconds the slow replay takes in the lock-hold test
MAX_HOLD_SECONDS = 1.0
PARKED_FOR = 1.0  # seconds a second finalize is watched for while the first one replays

Hook = Callable[[int, bool], None]  # (number of the call, did the caller own the store lock)


@pytest.fixture(scope="module")
def template(tmp_path_factory: pytest.TempPathFactory) -> Template:
    return build_template(tmp_path_factory)


@pytest.fixture
def restore(template: Template, tmp_path: Path) -> Iterator[Callable[..., Restored]]:
    made: list[Restored] = []

    def make(source: Template | None = None) -> Restored:
        restored = Restored(source or template, tmp_path / f"state{len(made)}")
        made.append(restored)
        return restored

    yield make
    for restored in made:
        restored.close()


@pytest.fixture
def restored(restore: Callable[..., Restored]) -> Restored:
    return restore()


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


class ReplaySpy:
    """Wraps the REAL tape_v2.replay_tape: who called it, did they own the store lock.

    `before` runs ahead of the real replay (it may raise), `after` once the real replay returned
    (it may block or sleep). Both get the number of the call and whether the lock was owned.
    Install it after the aggregate: aggregate_v2 replays its tape through the same function.
    """

    def __init__(
        self,
        monkeypatch: pytest.MonkeyPatch,
        *restored: Restored,
        before: Hook | None = None,
        after: Hook | None = None,
    ) -> None:
        self.calls: list[tuple[bool, str]] = []
        self._guard = threading.Lock()
        locks = [r.store._lock for r in restored]
        real = tape_v2.replay_tape

        def spy(*args: Any, **kwargs: Any) -> Any:
            owned = any(lock._is_owned() for lock in locks)
            with self._guard:
                number = len(self.calls)
                self.calls.append((owned, threading.current_thread().name))
            if before is not None:
                before(number, owned)
            out = real(*args, **kwargs)
            if after is not None:
                after(number, owned)
            return out

        monkeypatch.setattr(tape_v2, "replay_tape", spy)


@contextmanager
def parent_finalize(monkeypatch: pytest.MonkeyPatch) -> Iterator[None]:
    """finalize_v2 as it was before the pre-step: the transaction alone replays the tape."""
    with monkeypatch.context() as patch:
        patch.setattr(
            ChallengeStore, "_prime_finality_replay_v2", lambda self, run_id, w, raw: None
        )
        yield


def aggregated(r: Restored) -> tuple[dict[str, Any], bytes]:
    """Round 0 aggregated the normal way, and the Finalize the coordinator signs for it."""
    applied = r.store.aggregate_v2(r.run, 0)
    return applied, r.finalize_raw(applied)


def finalize(r: Restored, raw: bytes) -> dict[str, Any]:
    answer: dict[str, Any] = r.store.finalize_v2(r.run, 0, raw)
    return answer


def refusal(call: Callable[[], Any]) -> tuple[type[BaseException], int | None, str]:
    """What a refused call raised: type, HTTP status when it has one, message."""
    try:
        call()
    except Exception as error:
        return type(error), getattr(error, "status", None), str(error)
    raise AssertionError("the call was not refused")


def rewrite_record(r: Restored, kind: str, change: Callable[[dict[str, Any]], None]) -> None:
    """Change a stored record of round 0 in a transaction of its own."""
    with r.store._tx():
        record = r.store._record_v2(r.run, kind, "0")
        change(record)
        r.store._put_record_v2(r.run, kind, "0", record)


def stall_gate(r: Restored) -> None:
    """The wall clock three rounds ahead of the latest beacon: every finalize answers 503."""
    genesis = r.manifest.training.beacon.genesis_time
    r.store.clock = lambda: genesis + (r.now + 3 - 1) * 3


def sealed_finalize(
    r: Restored, applied: dict[str, Any], signer: Any = None, **fields: Any
) -> bytes:
    """A Finalize for round 0 as `finalize_raw` makes it, with `fields` changed and `signer`."""
    body: dict[str, Any] = {
        "w": 0,
        "final_theta_hash_w1": applied["theta_hash"],
        "included": sorted(i["roster"]["hotkey"] for i in applied["tape"]["body"]["inputs"]),
        "entitlements_root": "0" * 64,
    }
    exp = fields.pop("exp", 10000)
    body.update(fields)
    sealed = fixtures.seal(signer or fixtures.COORD, "Finalize", r.run, Finalize(**body), exp)
    return canonicalize(sealed)


def paid(r: Restored) -> list[tuple[Any, ...]]:
    rows = r.store._db.execute(
        "SELECT hotkey,issued FROM escrow_origins WHERE reward_round=0 ORDER BY hotkey"
    )
    return [tuple(row) for row in rows]


def until(condition: Callable[[], bool], timeout: float = WAIT) -> bool:
    deadline = time.perf_counter() + timeout
    while not condition():
        if time.perf_counter() > deadline:
            return False
        time.sleep(0.005)
    return True


def test_a_beacon_round_is_accepted_while_a_finalize_replays_the_tape(
    restored: Restored, monkeypatch: pytest.MonkeyPatch
) -> None:
    _, raw = aggregated(restored)
    reached, release = threading.Event(), threading.Event()

    def block_the_first(number: int, owned: bool) -> None:
        if number == 0:
            reached.set()
            release.wait(WAIT)

    ReplaySpy(monkeypatch, restored, after=block_the_first)
    store, number = restored.store, restored.now + 1
    settling = Call("finalize_v2", lambda: finalize(restored, raw))
    calls = [settling]
    try:
        if not reached.wait(WAIT):
            raise RuntimeError(f"the finalize never reached the replay: {settling.error!r}")
        push = Call("push_beacon", lambda: restored.push(number))
        latest = Call("latest_beacon", store.latest_beacon)
        calls += [push, latest]
        deadline = time.perf_counter() + SERVED_WITHIN
        for call in (push, latest):
            call.ended.wait(max(0.0, deadline - time.perf_counter()))
        still_replaying = not settling.ended.is_set()
        blocked = [call.name for call in (push, latest) if not call.ended.is_set()]
    finally:
        release.set()
        for call in calls:
            call.thread.join(WAIT)

    assert not blocked, (
        f"{' and '.join(blocked)} did not return within {SERVED_WITHIN:.0f} s of a finalize_v2 "
        "that was blocked inside its replay; they returned only when the test released it"
    )
    if any(call.thread.is_alive() for call in calls):
        raise RuntimeError("a thread is still stuck after the replay was released")
    if not still_replaying:
        raise RuntimeError(f"the finalize ended before it was released: {settling.error!r}")
    assert push.error is None and push.result["round"] == number
    assert latest.error is None
    assert settling.error is None, f"the finalize failed: {settling.error!r}"
    assert settling.result["status"] == "SETTLED"


def test_the_store_lock_is_held_for_less_than_a_second_by_a_finalize_that_replays_for_longer(
    restored: Restored, monkeypatch: pytest.MonkeyPatch
) -> None:
    _, raw = aggregated(restored)
    ReplaySpy(monkeypatch, restored, after=lambda number, owned: time.sleep(REPLAY_SECONDS))
    started = time.perf_counter()
    answer = finalize(restored, raw)
    elapsed = time.perf_counter() - started

    assert elapsed > REPLAY_SECONDS, "the finalize did not replay for long: nothing is measured"
    assert answer["status"] == "SETTLED"
    holds = [hold for hold in restored.lock.holds if hold.start >= started]
    longest = max(holds, key=lambda hold: hold.seconds)
    assert longest.seconds < MAX_HOLD_SECONDS, (
        f"thread {longest.thread} held the store lock for {longest.seconds:.2f} s in one stretch "
        f"({' <- '.join(longest.where[:3])}); the finalize replayed for {elapsed:.2f} s "
        f"and the lock may be held for at most {MAX_HOLD_SECONDS:.1f} s"
    )


def test_a_successful_finalize_replays_the_tape_once_and_outside_the_store_lock(
    restored: Restored, monkeypatch: pytest.MonkeyPatch
) -> None:
    applied, raw = aggregated(restored)
    spy = ReplaySpy(monkeypatch, restored)

    answer = finalize(restored, raw)

    assert spy.calls == [(False, threading.current_thread().name)]
    assert answer["status"] == "SETTLED" and answer["theta_hash"] == applied["theta_hash"]
    assert answer["reward_receipt"]
    assert restored.store._record_v2(restored.run, "finalize-result", "0") == answer
    escrow = restored.store._services(restored.run)[0]
    assert len(paid(restored)) == 5
    assert sum(issued for _, issued in paid(restored)) == escrow.policy.round_reward_units


@pytest.mark.parametrize(
    ("kind", "change"),
    [
        ("applied", lambda record: record.update(predecessor_tape_hash="11" * 32)),
        (
            "tape-inputs",
            lambda record: record["inputs"][0]["roster"].update(
                eligible_weight=record["inputs"][0]["roster"]["eligible_weight"] + 1
            ),
        ),
    ],
    ids=["applied", "tape-inputs"],
)
def test_a_verdict_is_not_trusted_for_a_round_whose_record_changed_after_the_replay(
    restore: Callable[..., Restored],
    monkeypatch: pytest.MonkeyPatch,
    kind: str,
    change: Callable[[dict[str, Any]], None],
) -> None:
    # Given: the replay is made without the lock, and a record it replayed from changes before
    # the transaction (what a rollback does to `applied`). The parent refuses that state.
    primed, parent = restore(), restore()
    _, raw = aggregated(primed)
    _, parent_raw = aggregated(parent)
    rewrite_record(parent, kind, change)

    def change_after_the_unlocked_replay(number: int, owned: bool) -> None:
        if not owned:
            rewrite_record(primed, kind, change)

    spy = ReplaySpy(monkeypatch, primed, parent, after=change_after_the_unlocked_replay)
    # When
    error = refusal(lambda: finalize(primed, raw))
    spy_calls, spy.calls = list(spy.calls), []
    with parent_finalize(monkeypatch):
        parent_error = refusal(lambda: finalize(parent, parent_raw))
    # Then: the memo made for the old rows is not used. The transaction replays the new ones
    # with the lock and fails as the parent does; nothing was settled.
    assert [owned for owned, _ in spy_calls] == [False, True]
    assert error == parent_error
    assert error[0] is TapeIntegrityError  # the stored tape no longer replays: the master's fault
    assert paid(primed) == []


def test_a_replay_that_fails_once_before_the_transaction_is_made_again_by_the_transaction(
    restored: Restored, monkeypatch: pytest.MonkeyPatch
) -> None:
    _, raw = aggregated(restored)

    def fail_the_first(number: int, owned: bool) -> None:
        if number == 0:
            raise TapeError("injected, once")

    spy = ReplaySpy(monkeypatch, restored, before=fail_the_first)

    answer = finalize(restored, raw)

    assert answer["status"] == "SETTLED"
    assert [owned for owned, _ in spy.calls] == [False, True]


def test_a_replay_that_always_fails_is_the_transactions_error_and_is_not_remembered(
    restore: Callable[..., Restored], monkeypatch: pytest.MonkeyPatch
) -> None:
    primed, parent = restore(), restore()
    _, raw = aggregated(primed)
    _, parent_raw = aggregated(parent)
    failing = [True]

    def fail(number: int, owned: bool) -> None:
        if failing[0]:
            raise TapeError("injected, always")

    spy = ReplaySpy(monkeypatch, primed, parent, before=fail)

    error = refusal(lambda: finalize(primed, raw))
    primed_calls, spy.calls = list(spy.calls), []
    with parent_finalize(monkeypatch):
        parent_error = refusal(lambda: finalize(parent, parent_raw))

    # The pre-step failed and was set aside, so the error is the one the transaction raised, with
    # the lock, where the parent raises it.
    assert error == parent_error
    assert error[0] is TapeError
    assert [owned for owned, _ in primed_calls] == [False, True]
    assert (
        primed.store._db.execute(
            "SELECT COUNT(*) FROM records_v2 WHERE kind='finalize'"
        ).fetchone()[0]
        == 0
    )
    # Nothing of the failure is kept: with the replay healthy again, the same signed Finalize
    # settles (its nonce was not burned either).
    failing[0] = False
    spy.calls.clear()
    assert finalize(primed, raw)["status"] == "SETTLED"
    assert [owned for owned, _ in spy.calls] == [False]


def test_a_finalize_retried_after_the_stale_gate_does_not_replay_again(
    restored: Restored, monkeypatch: pytest.MonkeyPatch
) -> None:
    applied, raw = aggregated(restored)
    spy = ReplaySpy(monkeypatch, restored)
    stall_gate(restored)

    error = refusal(lambda: finalize(restored, raw))

    assert error[1:] == (503, "beacon infrastructure paused")
    assert [owned for owned, _ in spy.calls] == [False]
    # The retry is a newly signed Finalize of the same round: the verdict belongs to the rows.
    restored.set_clock(restored.now)
    assert finalize(restored, restored.finalize_raw(applied))["status"] == "SETTLED"
    assert [owned for owned, _ in spy.calls] == [False]


def test_two_finalizes_of_one_round_share_one_unlocked_replay(
    restored: Restored, monkeypatch: pytest.MonkeyPatch
) -> None:
    _, raw = aggregated(restored)
    reached, release, overtaken = threading.Event(), threading.Event(), threading.Event()

    def block_the_first(number: int, owned: bool) -> None:
        if number == 0:
            reached.set()
            release.wait(WAIT)
        else:
            overtaken.set()

    spy = ReplaySpy(monkeypatch, restored, after=block_the_first)
    first = Call("finalize-first", lambda: finalize(restored, raw))
    calls = [first]
    try:
        if not reached.wait(WAIT):
            raise RuntimeError(f"the finalize never reached the replay: {first.error!r}")
        second = Call("finalize-second", lambda: finalize(restored, raw))
        calls.append(second)
        # Its first stretch under the lock is done when a hold of its thread is recorded; it is
        # then parked behind the replay that is running, or it replays too.
        reading = until(lambda: any(hold.thread == second.name for hold in restored.lock.holds))
        replayed_too = overtaken.wait(PARKED_FOR)
        finished_early = second.ended.is_set()
    finally:
        release.set()
        for call in calls:
            call.thread.join(WAIT)

    assert reading, "the second finalize never reached the pre-step"
    assert not replayed_too, "the second finalize replayed the tape while the first one did"
    assert not finished_early, "the second finalize returned while the first one was replaying"
    if any(call.thread.is_alive() for call in calls):
        raise RuntimeError("a thread is still stuck after the replay was released")
    assert spy.calls == [(False, first.name)]
    assert first.error is None and second.error is None, (first.error, second.error)
    assert first.result["status"] == "SETTLED"
    assert second.result == first.result


def test_a_finalize_repeated_after_it_settled_replays_nothing(
    restored: Restored, monkeypatch: pytest.MonkeyPatch
) -> None:
    _, raw = aggregated(restored)
    spy = ReplaySpy(monkeypatch, restored)
    settled = finalize(restored, raw)
    assert len(spy.calls) == 1
    # What the repeat could lean on is gone, as it is once other rounds pushed the verdict out.
    restored.store._finality_verdicts_v2.clear()

    repeated = finalize(restored, raw)

    assert repeated == settled
    assert len(spy.calls) == 1, "a round that is already finalized was replayed again"


def refused_finalize(case: str, r: Restored, applied: dict[str, Any]) -> bytes:
    """A Finalize the transaction refuses for `case`; the stored round is made to fit it."""
    match case:
        case "other-members":
            return sealed_finalize(r, applied, included=sorted(k.ss58 for k in fixtures.HOT[:4]))
        case "other-round":
            return sealed_finalize(r, applied, w=1)
        case "foreign-signer":
            return sealed_finalize(r, applied, fixtures.HOT[0])
        case "expired":
            return sealed_finalize(r, applied, exp=1)
        case "repair":
            rewrite_record(r, "applied", lambda record: record.update(kind="repair"))
            return r.finalize_raw(applied)
    raise AssertionError(case)


@pytest.mark.parametrize(
    "case", ["other-members", "other-round", "foreign-signer", "expired", "repair"]
)
def test_a_finalize_the_transaction_refuses_is_refused_without_an_unlocked_replay(
    restore: Callable[..., Restored], monkeypatch: pytest.MonkeyPatch, case: str
) -> None:
    primed, parent = restore(), restore()
    raw = refused_finalize(case, primed, aggregated(primed)[0])
    parent_raw = refused_finalize(case, parent, aggregated(parent)[0])
    spy = ReplaySpy(monkeypatch, primed, parent)

    error = refusal(lambda: finalize(primed, raw))
    unlocked_replays = list(spy.calls)
    with parent_finalize(monkeypatch):
        parent_error = refusal(lambda: finalize(parent, parent_raw))

    assert error == parent_error
    assert unlocked_replays == [], "a finalize the transaction refuses was replayed first"


def damage_tape(r: Restored, applied: dict[str, Any], how: str) -> None:
    path = r.store.objects._path(applied["tape_hash"])
    if how == "missing":
        path.unlink()
    else:
        path.write_bytes(b"not the bytes of this object")


@pytest.mark.parametrize("how", ["missing", "corrupt"])
def test_a_tape_that_cannot_be_read_is_refused_after_the_checks_that_come_first(
    restore: Callable[..., Restored], monkeypatch: pytest.MonkeyPatch, how: str
) -> None:
    # Given: the tape object of the round is damaged and the beacon gate is stale. The parent
    # reads the tape only in the replay, after the gate, so it answers the gate's 503.
    primed, parent = restore(), restore()
    raws = []
    for r in (primed, parent):
        applied, raw = aggregated(r)
        damage_tape(r, applied, how)
        stall_gate(r)
        raws.append(raw)

    error = refusal(lambda: finalize(primed, raws[0]))
    with parent_finalize(monkeypatch):
        parent_error = refusal(lambda: finalize(parent, raws[1]))

    assert parent_error[1:] == (503, "beacon infrastructure paused")
    assert error == parent_error


def test_a_caller_that_owns_the_store_lock_does_not_wait_for_an_unlocked_replay(
    restored: Restored, monkeypatch: pytest.MonkeyPatch
) -> None:
    _, raw = aggregated(restored)
    reached, release = threading.Event(), threading.Event()

    def block_the_first(number: int, owned: bool) -> None:
        if number == 0:
            reached.set()
            release.wait(WAIT)

    spy = ReplaySpy(monkeypatch, restored, after=block_the_first)
    store = restored.store

    def finalize_holding_the_lock() -> dict[str, Any]:
        with store._lock:
            return finalize(restored, raw)

    first = Call("finalize-first", lambda: finalize(restored, raw))
    calls = [first]
    try:
        if not reached.wait(WAIT):
            raise RuntimeError(f"the finalize never reached the replay: {first.error!r}")
        owner = Call("finalize-owner", finalize_holding_the_lock)
        calls.append(owner)
        served = owner.ended.wait(SERVED_WITHIN)
    finally:
        release.set()
        for call in calls:
            call.thread.join(WAIT)

    assert served, (
        f"a finalize made while holding the store lock did not return within {SERVED_WITHIN:.0f} s "
        "of another finalize's unlocked replay: it waits for it and the two deadlock"
    )
    if any(call.thread.is_alive() for call in calls):
        raise RuntimeError("a thread is still stuck after the replay was released")
    assert owner.error is None and first.error is None, (owner.error, first.error)
    assert owner.result["status"] == "SETTLED"
    assert first.result == owner.result
    # The owner replayed inside its own transaction, as the code did before the pre-step.
    assert spy.calls == [(False, first.name), (True, owner.name)]


def test_the_remembered_verdicts_are_bounded(
    restored: Restored, monkeypatch: pytest.MonkeyPatch
) -> None:
    _, raw = aggregated(restored)
    spy = ReplaySpy(monkeypatch, restored)
    stall_gate(restored)
    rounds = FINALITY_VERDICTS + 2

    # Each attempt names other rows, so it is another replay; the stale gate ends it unsettled.
    for probe in range(rounds):
        rewrite_record(restored, "applied", lambda record, probe=probe: record.update(probe=probe))
        assert refusal(lambda: finalize(restored, raw))[1] == 503

    assert len(spy.calls) == rounds
    assert len(restored.store._finality_verdicts_v2) == FINALITY_VERDICTS


def snapshot_of(r: Restored, template: Template, directory: Path) -> Template:
    """A template of the state `r` is in now, made as build_template makes the first one."""
    skipped = shutil.ignore_patterns("*.sock", "challenge.db*", "server_keys.db*")
    shutil.copytree(r.store.state_dir, directory, ignore=skipped)
    with closing(sqlite3.connect(directory / "challenge.db")) as target:
        r.store._db.backup(target)
    return replace(template, snapshot=directory)


def database_rows(r: Restored) -> dict[str, list[tuple[Any, ...]]]:
    db = r.store._db
    names = [row[0] for row in db.execute("SELECT name FROM sqlite_master WHERE type='table'")]
    return {
        name: sorted((tuple(row) for row in db.execute(f"SELECT * FROM {name}")), key=repr)
        for name in sorted(names)
    }


def test_the_database_after_a_finalize_is_the_one_a_finalize_without_the_unlocked_replay_leaves(
    template: Template,
    restore: Callable[..., Restored],
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    # Given: one aggregated round, copied, so both finalizes start from the very same bytes
    # (a tape carries a random sr25519 signature, a second aggregate would not be the same).
    original = restore()
    _, raw = aggregated(original)
    aggregated_template = snapshot_of(original, template, tmp_path / "aggregated")
    primed, parent = restore(aggregated_template), restore(aggregated_template)
    spy = ReplaySpy(monkeypatch, primed, parent)
    for r in (primed, parent):
        r.push(r.now + 1)
    # When
    answer = finalize(primed, raw)
    primed_calls, spy.calls = list(spy.calls), []
    with parent_finalize(monkeypatch):
        parent_answer = finalize(parent, raw)
    # Then: the pre-step ran in one and not in the other, and the stores are the same.
    assert [owned for owned, _ in primed_calls] == [False]
    assert [owned for owned, _ in spy.calls] == [True]
    assert answer["status"] == "SETTLED"
    assert answer == parent_answer
    assert len(paid(primed)) == 5 and paid(primed) == paid(parent)
    rows, parent_rows = database_rows(primed), database_rows(parent)
    assert [name for name in rows if rows[name] != parent_rows[name]] == []
