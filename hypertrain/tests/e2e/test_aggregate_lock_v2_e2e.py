"""aggregate_v2 computes its tape without the store lock, and answers the same as it always did.

The tape (make_tape, then replay_tape) is the only expensive part of an aggregate. The store lock
also serves `push_beacon` and `latest_beacon`, so holding it for the whole computation stops the
beacon pusher for as long as the computation runs. An aggregate is now a claim under the lock, the
computation without it, and a settlement under it that writes only if the round is still what the
claim read. These tests run the real flow once (a verified round 0 of five members, see
aggregate_lock_fixture_v2.py), restore it per test, and slow or hold the REAL `copy_flags` or
`compute_body` that both make_tape and replay_tape reach, so the bytes of the tape are the ones the
production code computes. Every wait has a timeout and every held computation is released in the
fixture's teardown: a broken implementation fails a test, it never hangs the suite.
"""

from __future__ import annotations

import gc
import json
import shutil
import threading
import time
import weakref
from collections.abc import Callable, Iterator
from pathlib import Path
from typing import Any

import pytest
from aggregate_lock_fixture_v2 import Call, Restored, Template, fixtures

from hypertrain.aggregator import tape_v2, weighted_v2
from hypertrain.aggregator.core import OuterState, TapeError
from hypertrain.challenge import store as store_module
from hypertrain.challenge.store import ChallengeError, ChallengeStore
from hypertrain.data.store import LocalFSStore
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages_v2 import AggregationPolicyV2
from hypertrain.trainer import model
from hypertrain.trainer.config import TrainConfig

WAIT = 20  # seconds: the longest anything may wait before the test calls itself broken
SERVED_WITHIN = 3.0  # seconds a beacon call is given to return while an aggregate computes
COMPUTE_SECONDS = 2.0  # seconds the slow compute takes in the lock-hold test
MAX_HOLD_SECONDS = 1.0


@pytest.fixture
def restored(template: Template, tmp_path: Path) -> Iterator[Restored]:
    r = Restored(template, tmp_path / "state")
    yield r
    r.close()


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


# --- the rest of the contract: single flight, claim and settlement, bit-identical answers ---


@pytest.fixture
def restore(template: Template, tmp_path: Path) -> Iterator[Callable[..., Restored]]:
    """More restores of the template beside `restored`, one directory each, closed at the end."""
    made: list[Restored] = []

    def make(name: str, *, warm_services: bool = True) -> Restored:
        made.append(Restored(template, tmp_path / name, warm_services=warm_services))
        return made[-1]

    yield make
    for each in made:
        each.close()


class Computation:
    """The real `compute_body`, counted by thread; its first call may be held, or end in an error.

    make_tape and replay_tape both look compute_body up as a module global, so this one wrapper
    sees the two runs a tape takes (make, then replay) while the real function computes every byte.
    A held call is released by `go`, and gives up by itself after WAIT seconds.
    """

    def __init__(
        self,
        monkeypatch: pytest.MonkeyPatch,
        *,
        hold: bool = False,
        fail: BaseException | None = None,
    ) -> None:
        self.threads: list[str] = []
        self.reached = threading.Event()
        self.go = threading.Event()
        real = tape_v2.compute_body

        def pause() -> None:
            if hold:
                self.reached.set()
                self.go.wait(WAIT)

        def counted(*args: Any, **kwargs: Any) -> Any:
            self.threads.append(threading.current_thread().name)
            first = len(self.threads) == 1
            if first and fail is not None:
                pause()
                raise fail
            out = real(*args, **kwargs)
            if first:
                pause()
            return out

        monkeypatch.setattr(tape_v2, "compute_body", counted)

    def wait_reached(self, call: Call) -> None:
        deadline = time.monotonic() + WAIT
        while not self.reached.wait(0.05):
            if call.ended.is_set() or time.monotonic() > deadline:
                raise RuntimeError(f"{call.name} never reached the computation: {call.error!r}")


class Scene:
    """The calls and the held computations of one test, all released and joined by `finish`."""

    def __init__(self, store: ChallengeStore, run: str, monkeypatch: pytest.MonkeyPatch) -> None:
        self.store, self.run, self.monkeypatch = store, run, monkeypatch
        self.calls: list[Call] = []
        self.computations: list[Computation] = []

    def aggregate(self, name: str) -> Call:
        self.calls.append(Call(name, lambda: self.store.aggregate_v2(self.run, 0)))
        return self.calls[-1]

    def compute(self, *, hold: bool = False, fail: BaseException | None = None) -> Computation:
        self.computations.append(Computation(self.monkeypatch, hold=hold, fail=fail))
        return self.computations[-1]

    def finish(self) -> list[str]:
        """Release everything held, join every thread, name those still running."""
        for computation in self.computations:
            computation.go.set()
        for call in self.calls:
            call.thread.join(WAIT)
        return [call.name for call in self.calls if call.thread.is_alive()]


@pytest.fixture
def scene(restored: Restored, monkeypatch: pytest.MonkeyPatch) -> Iterator[Scene]:
    scene = Scene(restored.store, restored.run, monkeypatch)
    yield scene
    stuck = scene.finish()
    assert not stuck, f"threads still running after the computation was released: {stuck}"


def until(condition: Callable[[], bool], what: str) -> None:
    deadline = time.monotonic() + WAIT
    while not condition():
        if time.monotonic() > deadline:
            raise RuntimeError(f"{what}: did not happen within {WAIT} s")
        time.sleep(0.01)


def finished(*calls: Call) -> None:
    for call in calls:
        if not call.ended.wait(WAIT):
            raise RuntimeError(f"{call.name} did not end within {WAIT} s")


def waiting(store: ChallengeStore) -> int:
    """How many repeated calls are parked on a running aggregate."""
    return store_module.AGGREGATE_WAITERS - store._aggregate_waiters._value


def table(store: ChallengeStore, sql: str, *args: Any) -> list[tuple[Any, ...]]:
    with store._lock:
        return [tuple(row) for row in store._db.execute(sql, args)]


def update(store: ChallengeStore, sql: str, *args: Any) -> None:
    with store._tx():
        store._db.execute(sql, args)


def records(store: ChallengeStore) -> dict[tuple[str, str], str]:
    return {
        (kind, ident): data
        for kind, ident, data in table(store, "SELECT kind,id,data FROM records_v2")
    }


def excluded(store: ChallengeStore) -> dict[str, Any]:
    rows = table(store, "SELECT id,data FROM records_v2 WHERE kind='excluded'")
    return {ident: json.loads(data) for ident, data in rows}


def pins(store: ChallengeStore) -> set[str]:
    return {op for (op,) in table(store, "SELECT operation_id FROM beacon_pins_v2")}


def roster(r: Restored) -> list[str]:
    return [entry["hotkey"] for entry in r.store._record_v2(r.run, "round", "0")["body"]["roster"]]


def lapse(r: Restored, member: str) -> None:
    """The screen of `member` ended one round before the latest one."""
    update(r.store, "UPDATE admissions_v2 SET screen_until=? WHERE hotkey=?", r.now - 1, member)


def lapsed_record(r: Restored, member: str) -> dict[str, str]:
    evidence = f"ht-ineligible/1|{r.run}|0|{member}|SCREEN_LAPSED"
    return {
        "reason": "INELIGIBLE",
        "stage": "aggregate",
        "cause": "SCREEN_LAPSED",
        "evidence_hash": sha256_hex(evidence.encode()),
    }


def outcome(r: Restored, applied: dict[str, Any]) -> dict[str, Any]:
    """What an aggregate answers and leaves behind, minus the tape_hash a random signature moves."""
    body = applied["tape"]["body"]
    stored = r.store._record_v2(r.run, "applied", "0")
    assert applied["tape_hash"] == stored["tape_hash"], "the answer is not the tape that was stored"
    return {
        "body": sha256_hex(canonicalize(body, allow_float=False)),
        "input_root": body["input_root"],
        "out_state": applied["out_state"],
        "theta_hash": applied["theta_hash"],
        "prev_state": applied["prev_state"],
        "applied": {key: value for key, value in stored.items() if key != "tape_hash"},
        "tape-inputs": r.store._record_v2(r.run, "tape-inputs", "0"),
        "finality": r.store._record_v2(r.run, "finality", "0"),
        "excluded": excluded(r.store),
    }


def recomputed(r: Restored, lapsed: str | None = None) -> dict[str, Any]:
    """The same outcome made by hand on a round that was never aggregated.

    The inputs come from `verified_inputs_v2` and the tape from `make_tape`, exactly as the
    aggregate did before the lock was released: genesis state, no predecessor. A member whose screen
    ended is first left out with the record test_member_state_v2_e2e pins for it.
    """
    store, manifest = r.store, r.manifest
    expected_excluded: dict[str, Any] = {}
    if lapsed is not None:
        expected_excluded[f"0:{lapsed}"] = lapsed_record(r, lapsed)
        with store._tx():
            store._put_record_v2(r.run, "excluded", f"0:{lapsed}", expected_excluded[f"0:{lapsed}"])
    finality = store._record_v2(r.run, "finality", "0")
    inputs = store.verified_inputs_v2(r.run, 0)
    policy = AggregationPolicyV2.model_validate_json(
        store.objects.get(manifest.network.aggregation_policy_hash)
    )
    theta = model.init_params(TrainConfig.from_manifest_v2(manifest).model)
    prev_state = store.objects.put(
        OuterState.init({k: v.detach().cpu().numpy() for k, v in theta.items()}).to_bytes()
    )
    tape = tape_v2.make_tape(
        store.objects,
        manifest,
        policy,
        store.objects.get(manifest.network.economics_policy_hash),
        store._coord(),
        w=0,
        prev_state=prev_state,
        predecessor_tape_hash="0" * 64,
        inputs=inputs,
        reference_reward_units=store._services(r.run)[0].policy.R_collectible_units,
    )
    theta_hash = tape.body.out_hashes["theta_hash"]
    return {
        "body": sha256_hex(canonicalize(tape.body.body(), allow_float=False)),
        "input_root": tape.body.input_root,
        "out_state": tape.body.out_state,
        "theta_hash": theta_hash,
        "prev_state": prev_state,
        "applied": {
            "out_state": tape.body.out_state,
            "theta_hash": theta_hash,
            "prev_state": prev_state,
            "predecessor_tape_hash": "0" * 64,
        },
        "tape-inputs": {"inputs": [store._serialize_input_v2(x) for x in inputs]},
        "finality": {**finality, "applied": True, "state_hash": theta_hash},
        "excluded": expected_excluded,
    }


def test_the_answer_and_the_records_are_the_ones_an_undisturbed_single_computation_gives(
    restored: Restored, restore: Callable[..., Restored], scene: Scene
) -> None:
    # Given: three copies of a round whose fifth member lapsed; one is aggregated plainly, one
    # while beacon rounds arrive during its computation, and one is worked out by hand.
    plain, by_hand = restore("plain"), restore("by-hand")
    member = roster(restored)[-1]
    for each in (plain, restored, by_hand):
        lapse(each, member)
    expected = recomputed(by_hand, member)
    undisturbed = outcome(plain, plain.store.aggregate_v2(plain.run, 0))
    # When
    computation = scene.compute(hold=True)
    held = scene.aggregate("held")
    computation.wait_reached(held)
    first = restored.now
    for number in range(first + 1, first + 4):
        restored.push(number)
    computation.go.set()
    finished(held)
    # Then
    assert held.error is None, f"the aggregate failed: {held.error!r}"
    assert undisturbed == expected
    assert outcome(restored, held.result) == expected
    assert len(expected["tape-inputs"]["inputs"]) == 4


def test_a_repeated_call_waits_for_the_running_aggregate_and_a_third_one_is_refused(
    restored: Restored, scene: Scene
) -> None:
    store = restored.store
    computation = scene.compute(hold=True)
    leader = scene.aggregate("leader")
    computation.wait_reached(leader)
    followers = [scene.aggregate(f"follower-{i}") for i in range(store_module.AGGREGATE_WAITERS)]
    until(lambda: waiting(store) == len(followers), "the repeated calls are parked")

    refused = scene.aggregate("refused")
    refused_at_once = refused.ended.wait(SERVED_WITHIN)
    computation.go.set()
    finished(leader, *followers)

    assert refused_at_once, "a call past the waiters' budget waited for the running aggregate"
    assert isinstance(refused.error, ChallengeError)
    assert (refused.error.status, "already running" in refused.error.detail) == (503, True)
    assert leader.error is None, f"the aggregate failed: {leader.error!r}"
    assert [follower.error for follower in followers] == [None] * len(followers)
    assert all(follower.result == leader.result for follower in followers)
    # make_tape and replay_tape each compute once; a repeated call must not compute again.
    assert computation.threads == ["leader", "leader"]
    assert store._aggregate_flights_v2 == {}
    assert waiting(store) == 0


def test_a_waiter_that_waits_too_long_is_told_to_retry_while_the_computation_goes_on(
    restored: Restored, scene: Scene, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(store_module, "AGGREGATE_WAIT_SECONDS", 0.3)
    store = restored.store
    computation = scene.compute(hold=True)
    leader = scene.aggregate("leader")
    computation.wait_reached(leader)

    follower = scene.aggregate("follower")
    gave_up = follower.ended.wait(SERVED_WITHIN)
    leader_still_computing = not leader.ended.is_set()
    computation.go.set()
    finished(leader)

    assert gave_up, "a waiter did not give up after AGGREGATE_WAIT_SECONDS"
    assert isinstance(follower.error, ChallengeError)
    assert (follower.error.status, "still running" in follower.error.detail) == (503, True)
    assert leader_still_computing
    assert leader.error is None, f"the aggregate failed: {leader.error!r}"
    assert store._aggregate_flights_v2 == {}
    assert waiting(store) == 0


def test_a_computation_that_fails_fails_its_waiter_and_leaves_no_record_behind(
    restored: Restored, restore: Callable[..., Restored], scene: Scene
) -> None:
    # Given: the answer an aggregate that is not disturbed gives, and what the store holds now
    plain = restore("plain")
    undisturbed = outcome(plain, plain.store.aggregate_v2(plain.run, 0))
    store = restored.store
    before, pinned_before = records(store), pins(store)
    injected = TapeError("injected")
    computation = scene.compute(hold=True, fail=injected)
    # When: the leader's computation fails while a repeated call waits for it
    leader = scene.aggregate("leader")
    computation.wait_reached(leader)
    follower = scene.aggregate("follower")
    until(lambda: waiting(store) == 1, "the repeated call is parked")
    computation.go.set()
    finished(leader, follower)
    # Then: both raise it, only the gate's pin was committed, nothing is left running
    assert leader.error is injected
    assert follower.error is injected
    assert records(store) == before
    assert pins(store) - pinned_before == {f"aggregate:0:{restored.now}"}
    assert store._aggregate_flights_v2 == {}
    assert waiting(store) == 0
    # and the next call is not wedged by it: it computes and answers as an undisturbed one does
    retry = scene.aggregate("retry")
    finished(retry)
    assert retry.error is None, f"the retry failed: {retry.error!r}"
    assert outcome(restored, retry.result) == undisturbed


def test_a_failed_computation_is_not_kept_alive_by_the_calls_that_saw_it_fail(
    restored: Restored, monkeypatch: pytest.MonkeyPatch
) -> None:
    # Given: a computation that fails with an object only its own frame refers to, and a waiter.
    # A traceback keeps every frame it passed through, and with them the failed computation's
    # arrays (gigabytes at the 100.7 M profile): they must go with the last call that saw the
    # failure, not wait for a collection of reference cycles. The collector is off to show it.
    store = restored.store
    reached, go = threading.Event(), threading.Event()
    ballast: list[weakref.ref[object]] = []

    def fails(*args: Any, **kwargs: Any) -> Any:
        mine = type("Ballast", (), {})()
        ballast.append(weakref.ref(mine))
        reached.set()
        go.wait(WAIT)
        raise TapeError("injected")

    monkeypatch.setattr(tape_v2, "compute_body", fails)
    collecting = gc.isenabled()
    gc.collect()
    gc.disable()
    try:
        # When: the leader fails while a repeated call waits, and both are let go of
        leader = Call("leader", lambda: store.aggregate_v2(restored.run, 0))
        if not reached.wait(WAIT):
            raise RuntimeError(f"the aggregate never reached the computation: {leader.error!r}")
        follower = Call("follower", lambda: store.aggregate_v2(restored.run, 0))
        until(lambda: waiting(store) == 1, "the repeated call is parked")
        go.set()
        finished(leader, follower)
        failures = [leader.error, follower.error]
        leader.error = follower.error = None
        # Then: both saw it fail, and nothing refers to the frame it failed in
        assert all(isinstance(error, TapeError) for error in failures)
        del failures
        assert len(ballast) == 1
        assert ballast[0]() is None, "the failed computation is still alive in a reference cycle"
    finally:
        go.set()
        if collecting:
            gc.enable()


def exclude_member(r: Restored, member: str) -> None:
    with r.store._tx():
        record = {"reason": "NO_SHOW", "stage": "delta", "evidence_hash": "ab" * 32}
        r.store._put_record_v2(r.run, "excluded", f"0:{member}", record)


def count_clean_finalization(r: Restored, member: str) -> None:
    update(r.store, "UPDATE admissions_v2 SET clean_count=clean_count+1 WHERE hotkey=?", member)


@pytest.mark.parametrize(
    ("change", "left_out", "counted"),
    [
        pytest.param(exclude_member, True, False, id="a member is excluded"),
        pytest.param(lapse, True, False, id="a member's screen ended"),
        pytest.param(count_clean_finalization, False, True, id="a member gains a clean count"),
    ],
)
def test_a_round_that_changes_while_its_tape_computes_is_refused_and_writes_nothing(
    restored: Restored,
    scene: Scene,
    change: Callable[[Restored, str], None],
    left_out: bool,
    counted: bool,
) -> None:
    store, members = restored.store, roster(restored)
    member = members[-1]
    clean = table(store, "SELECT clean_count FROM admissions_v2 WHERE hotkey=?", member)[0][0]
    computation = scene.compute(hold=True)
    leader = scene.aggregate("leader")
    computation.wait_reached(leader)
    # When: what the plan reads changes between the claim and the settlement
    change(restored, member)
    after_change = records(store)
    computation.go.set()
    finished(leader)
    # Then: the settlement refuses and neither the tape's records nor the exclusions it derived
    # again at that point are left behind
    assert isinstance(leader.error, ChallengeError), f"expected a refusal: {leader.error!r}"
    assert (leader.error.status, "AGGREGATE_STALE" in leader.error.detail) == (409, True)
    assert records(store) == after_change
    assert store._aggregate_flights_v2 == {}
    # and a retry aggregates the round as it is now
    retry = scene.aggregate("retry")
    finished(retry)
    assert retry.error is None, f"the retry failed: {retry.error!r}"
    stored = store._record_v2(restored.run, "tape-inputs", "0")["inputs"]
    inputs = {item["roster"]["hotkey"]: item["clean_finalizations"] for item in stored}
    assert sorted(inputs) == sorted(set(members) - ({member} if left_out else set()))
    if counted:
        assert inputs[member] == clean + 1


def test_a_round_retired_while_its_tape_computes_gets_no_tape(
    restored: Restored, scene: Scene
) -> None:
    # Given: an aggregate whose tape is computing, and a round that is retired meanwhile (the
    # unlocked computation lets a retirement land that the lock held out). A retired round carries
    # a null tape and carried anchors that are final: it takes no work (_require_unretired_v2).
    store = restored.store
    computation = scene.compute(hold=True)
    leader = scene.aggregate("leader")
    computation.wait_reached(leader)
    with store._tx():
        store._put_record_v2(restored.run, "retired", "0", {"v": "ht-round-retired-v1", "w": 0})
    after_retirement = records(store)
    computation.go.set()
    finished(leader)
    # Then: the settlement refuses, as the claim of a call made now would, and writes nothing
    assert isinstance(leader.error, ChallengeError), f"expected a refusal: {leader.error!r}"
    assert (leader.error.status, "ROUND_RETIRED" in leader.error.detail) == (409, True)
    assert records(store) == after_retirement
    assert store._aggregate_flights_v2 == {}


def test_nothing_a_claim_derives_is_written_before_the_tape_is(
    restored: Restored, scene: Scene
) -> None:
    store, member = restored.store, roster(restored)[-1]
    lapse(restored, member)
    before, pinned_before = records(store), pins(store)
    computation = scene.compute(hold=True)
    leader = scene.aggregate("leader")
    computation.wait_reached(leader)
    # While the tape computes: the claim left out the member whose screen ended in its plan, but
    # the record is the settlement's to write; only the freshness gate's pin was committed.
    during, pinned = records(store), pins(store)
    computation.go.set()
    finished(leader)

    assert during == before
    assert pinned - pinned_before == {f"aggregate:0:{restored.now}"}
    assert leader.error is None, f"the aggregate failed: {leader.error!r}"
    assert excluded(store) == {f"0:{member}": lapsed_record(restored, member)}


def test_a_member_whose_screen_ends_while_the_tape_computes_stays_in_the_round_it_was_claimed_at(
    restored: Restored, restore: Callable[..., Restored], scene: Scene
) -> None:
    # Given: the screen of one member lasts one round past the claim's
    undisturbed, late = restore("undisturbed"), restore("late")
    member = roster(restored)[-1]
    for each in (restored, undisturbed, late):
        update(
            each.store,
            "UPDATE admissions_v2 SET screen_until=? WHERE hotkey=?",
            each.now + 1,
            member,
        )
    expected = outcome(undisturbed, undisturbed.store.aggregate_v2(undisturbed.run, 0))
    # When: beacon rounds go well past that screen while the tape computes
    computation = scene.compute(hold=True)
    leader = scene.aggregate("leader")
    computation.wait_reached(leader)
    restored.push(restored.now + 10)
    computation.go.set()
    finished(leader)
    # Then: every decision was made at the claim's round, so the member is still an input
    assert leader.error is None, f"the aggregate failed: {leader.error!r}"
    got = outcome(restored, leader.result)
    assert len(got["tape-inputs"]["inputs"]) == 5 and got["excluded"] == {}
    assert got == expected
    # whereas a call made after those rounds left the member out
    late.push(late.now + 10)
    outcome_late = outcome(late, late.store.aggregate_v2(late.run, 0))
    assert len(outcome_late["tape-inputs"]["inputs"]) == 4
    assert outcome_late["excluded"] == {f"0:{member}": lapsed_record(late, member)}


def test_the_computation_reads_no_database_row_and_holds_no_lock(
    restored: Restored, monkeypatch: pytest.MonkeyPatch
) -> None:
    store = restored.store
    inside = threading.local()
    statements: list[tuple[bool, str]] = []
    window: list[float] = []  # when the computation began and ended
    owned: dict[str, list[bool]] = {"init_params": [], "compute_body": []}

    def watching(name: str, real: Callable[..., Any]) -> Any:
        def watched(*args: Any, **kwargs: Any) -> Any:
            owned[name].append(restored.lock._is_owned())
            return real(*args, **kwargs)

        return watched

    real_compute = store._aggregate_compute_v2

    def computing(*args: Any, **kwargs: Any) -> Any:
        inside.on = True
        window.append(time.perf_counter())
        try:
            return real_compute(*args, **kwargs)
        finally:
            window.append(time.perf_counter())
            inside.on = False

    monkeypatch.setattr(store, "_aggregate_compute_v2", computing)
    monkeypatch.setattr(model, "init_params", watching("init_params", model.init_params))
    monkeypatch.setattr(tape_v2, "compute_body", watching("compute_body", tape_v2.compute_body))
    store._db.set_trace_callback(lambda sql: statements.append((getattr(inside, "on", False), sql)))
    try:
        store.aggregate_v2(restored.run, 0)
    finally:
        store._db.set_trace_callback(None)

    assert len(window) == 2, "the computation did not run: nothing is measured"
    assert any(not on for on, _ in statements), "the trace saw no statement: it measures nothing"
    assert [sql for on, sql in statements if on] == []
    # genesis state once, then the two runs of the tape
    assert owned == {"init_params": [False], "compute_body": [False, False]}
    begin, end = window
    overlapping = [h for h in restored.lock.holds if h.start < end and h.start + h.seconds > begin]
    assert overlapping == [], "the store lock was held while the tape computed"


def test_aggregate_is_refused_to_a_thread_that_holds_the_store_lock(
    restored: Restored, scene: Scene
) -> None:
    store = restored.store
    computation = scene.compute()
    with store._lock, pytest.raises(RuntimeError):
        store.aggregate_v2(restored.run, 0)

    assert computation.threads == []
    assert store._aggregate_flights_v2 == {}
    assert table(store, "SELECT 1 FROM records_v2 WHERE kind='applied'") == []


def test_a_store_whose_services_were_never_built_can_aggregate(
    restore: Callable[..., Restored],
) -> None:
    cold = restore("cold", warm_services=False)
    assert cold.store._services_v2 == {}, "the services were built: this tests nothing"

    applied = cold.store.aggregate_v2(cold.run, 0)

    assert applied["tape_hash"] == cold.store._record_v2(cold.run, "applied", "0")["tape_hash"]


def test_a_repeated_call_after_success_returns_the_stored_answer_without_computing(
    restored: Restored, scene: Scene
) -> None:
    store = restored.store
    computation = scene.compute()
    first = store.aggregate_v2(restored.run, 0)
    assert len(computation.threads) == 2, "the first call did not compute: nothing is measured"

    again = store.aggregate_v2(restored.run, 0)

    assert again == first
    assert len(computation.threads) == 2


@pytest.mark.parametrize(
    ("run", "w"),
    [
        pytest.param(None, 1, id="a round that was never opened"),
        pytest.param("00" * 32, 0, id="a run that does not exist"),
    ],
)
def test_a_round_or_run_that_does_not_exist_is_refused_with_the_status_it_always_had(
    restored: Restored, scene: Scene, run: str | None, w: int
) -> None:
    store, computation = restored.store, scene.compute()
    before = records(store)

    with pytest.raises(ChallengeError) as refused:
        store.aggregate_v2(run or restored.run, w)

    assert refused.value.status == 409  # as the parent: `_record_v2` has no 404 for a missing round
    assert computation.threads == []
    assert store._aggregate_flights_v2 == {}
    assert records(store) == before


def test_an_aggregate_behind_the_beacon_is_refused_before_it_computes(
    restored: Restored, scene: Scene
) -> None:
    store, computation = restored.store, scene.compute()
    restored.set_clock(restored.now + 3)  # three rounds were emitted that never arrived

    with pytest.raises(ChallengeError) as refused:
        store.aggregate_v2(restored.run, 0)

    assert (refused.value.status, refused.value.detail) == (503, "beacon infrastructure paused")
    assert computation.threads == []
    assert store._aggregate_flights_v2 == {}
    assert table(store, "SELECT 1 FROM records_v2 WHERE kind='applied'") == []


def test_a_beacon_that_falls_behind_while_the_tape_computes_does_not_discard_it(
    restored: Restored, scene: Scene
) -> None:
    store = restored.store
    computation = scene.compute(hold=True)
    leader = scene.aggregate("leader")
    computation.wait_reached(leader)
    restored.set_clock(restored.now + 5)
    with pytest.raises(ChallengeError) as paused, store._tx():
        store._fresh_v2(restored.manifest, "probe")
    assert paused.value.status == 503, "the gate is still open: the test asks nothing"

    computation.go.set()
    finished(leader)

    assert leader.error is None, f"the tape was thrown away: {leader.error!r}"
    assert leader.result["tape_hash"] == store._record_v2(restored.run, "applied", "0")["tape_hash"]


def test_a_round_aggregated_meanwhile_by_another_process_is_answered_and_not_overwritten(
    restored: Restored, scene: Scene
) -> None:
    # Given: a second store on the same state, as a second process would be
    store = restored.store
    other = ChallengeStore(
        store.state_dir,
        store.params,
        store.coord,
        store.owner_hotkey,
        store.verify_beacon,
        LocalFSStore(store.state_dir / "objects"),
    )
    try:
        other.clock = lambda: store.clock()
        other._services(restored.run)
        computation = scene.compute(hold=True)
        leader = scene.aggregate("leader")
        computation.wait_reached(leader)
        # When: the other one aggregates the round while the leader's tape is still computing
        theirs = other.aggregate_v2(restored.run, 0)
        written = records(store)
        computation.go.set()
        finished(leader)
    finally:
        other.close()
    # Then: the leader answers with the tape that was stored and writes nothing
    assert leader.error is None, f"the aggregate failed: {leader.error!r}"
    assert leader.result == theirs
    assert records(store) == written
    assert store._record_v2(restored.run, "applied", "0")["tape_hash"] == theirs["tape_hash"]


def test_the_root_accepts_a_beacon_round_while_a_hosted_run_computes_its_aggregate(
    template: Template, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # The root's push_beacon is what measures the child's lock: it ends by pushing the round into
    # every child store, under that store's lock. The root's latest_beacon reads the root's own
    # database and cannot see the child's lock; it only shows that the root answers.
    # Given: the production layout, a root store whose run lives in a child store of its own
    run = template.manifest.run_id()
    shutil.copytree(template.snapshot, tmp_path / "root" / "runs" / run)
    root = ChallengeStore(
        tmp_path / "root",
        template.params,
        template.coord,
        template.owner_hotkey,
        template.verify_beacon,
        LocalFSStore(tmp_path / "root" / "objects"),
    )
    child = root.for_run(run)
    assert child is not root, "the run is not hosted in a child store"
    number = child._now(child._db) + 1
    genesis = template.manifest.training.beacon.genesis_time
    root.clock = lambda: genesis + (number - 1) * 3
    child._services(run)
    scene = Scene(child, run, monkeypatch)
    try:
        computation = scene.compute(hold=True)
        aggregate = scene.aggregate("aggregate")
        computation.wait_reached(aggregate)
        # When: the root is asked for a beacon round and for the latest one
        push = Call("push_beacon", lambda: root.push_beacon(fixtures.fixture_beacon(number)))
        latest = Call("latest_beacon", root.latest_beacon)
        scene.calls += [push, latest]
        deadline = time.perf_counter() + SERVED_WITHIN
        for call in (push, latest):
            call.ended.wait(max(0.0, deadline - time.perf_counter()))
        blocked = [call.name for call in (push, latest) if not call.ended.is_set()]
        child_has_round = child._now(child._db) == number
        computation.go.set()
        finished(aggregate)
    finally:
        stuck = scene.finish()
        root.close()
    # Then
    assert not stuck
    assert not blocked, f"{blocked} waited for the hosted run's aggregate to end"
    assert push.error is None and child_has_round
    assert latest.error is None and latest.result["round"] in (0, number)
    assert aggregate.error is None, f"the aggregate failed: {aggregate.error!r}"
