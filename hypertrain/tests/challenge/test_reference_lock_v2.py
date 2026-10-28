"""A remote reference must not hold the challenge's store lock (docs/adr/0012 section 2, Part B).

`trial_reference_v2` claims under the lock, runs the launch without it and settles under it again.
The tests state that behaviour with a fake executor the test controls (no GPU, no network): the lock
is free while the executor works, a repeated request waits for the one launch, and a result is
committed only into the state its claim read. The executor runs the real reference on the CPU once
the test lets it go, so a released reference commits like a real one.
"""

from __future__ import annotations

import importlib.util
import json
import os
import shutil
import sys
import threading
import time
import traceback
from pathlib import Path
from typing import Any

import pytest

from hypertrain.challenge import store as store_module
from hypertrain.challenge.admission_store import AdmissionError
from hypertrain.challenge.store import ChallengeError
from hypertrain.data.trial_assignment import trial_samples
from hypertrain.gpu_ops.work_screen import WorkScreenError, screen_work
from hypertrain.miner.island_launch import IslandFailure, launch_island
from hypertrain.protocol.messages_v2 import JoinChallenge

spec = importlib.util.spec_from_file_location(
    "reference_lock_backend_fixture", Path(__file__).parent / "test_backend_authority_v2.py"
)
assert spec is not None and spec.loader is not None
backend = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = backend
spec.loader.exec_module(backend)
network = backend.network

from .child_run_fixture_v2 import child_network, pending_reference  # noqa: E402, F401


@pytest.fixture(params=["root", "child"])
def hosted(request: pytest.FixtureRequest) -> Any:
    """A network whose run is hosted by the root store, or by a child store with a lock of its
    own (the layout of the s100 TEST run). `hosted.store` is the hosting store in both."""
    return request.getfixturevalue("network" if request.param == "root" else "child_network")


def entry(hosted: Any) -> Any:
    """The store the beacon pusher talks to: the root, which hands each round to its children."""
    return getattr(hosted, "root", hosted.store)


WAIT = 30  # seconds the test lets anything take before it calls it stuck


class SlowExecutor:
    """The role launch of a remote executor that is still working: it says when the reference
    reached it, then waits until the test releases it, and fails the reference afterwards."""

    def __init__(self) -> None:
        self.reached, self.release = threading.Event(), threading.Event()

    def __call__(self, kind: str, job: Any, directory: Path, context: Any) -> Any:
        def launch(
            job: Any, directory: Path, *, backend: str, cancel: Any = None, trace: bool = False
        ):
            self.reached.set()
            self.release.wait(WAIT)
            raise IslandFailure("the fake executor stops once the probes have run")

        return launch


class Executor:
    """A remote executor under the test's control that finishes the reference for real.

    The first launch says it was reached and waits for `release` (at most `delay` seconds), then
    raises `error` if one was given; every launch ends in the real reference on the CPU. With
    `honour_cancel` False the executor ignores the cancel event it was handed, as a remote one that
    does not watch it would. It records every launch and the cancel event it received."""

    def __init__(
        self,
        *,
        delay: float = WAIT,
        error: BaseException | None = None,
        honour_cancel: bool = True,
    ) -> None:
        self.reached, self.release = threading.Event(), threading.Event()
        self.delay, self.error, self.honour_cancel = delay, error, honour_cancel
        self.launches: list[int] = []  # trial epoch of each launch
        self.cancels: list[threading.Event | None] = []
        self._mutex = threading.Lock()

    def __call__(self, kind: str, job: Any, directory: Path, context: Any) -> Any:
        def launch(
            job: Any, directory: Path, *, backend: str, cancel: Any = None, trace: bool = False
        ):
            with self._mutex:
                first = not self.launches
                self.launches.append(job.w)
                self.cancels.append(cancel)
            if first:
                self.reached.set()
                self.release.wait(self.delay)
                if self.error is not None:
                    raise self.error
            return launch_island(
                job, directory, backend="cpu", cancel=cancel if self.honour_cancel else None
            )

        return launch


class Call:
    """A `trial_reference_v2` on its own thread, as a request thread of the service would run it."""

    def __init__(self, store: Any, run_id: str, admission_id: str, **options: Any) -> None:
        self.outcome: list[Any] = []
        self.thread = threading.Thread(
            target=self._run, args=(store, run_id, admission_id, options), daemon=True
        )
        self.thread.start()

    def _run(self, store: Any, run_id: str, admission_id: str, options: dict[str, Any]) -> None:
        try:
            self.outcome.append(store.trial_reference_v2(run_id, admission_id, **options))
        except BaseException as error:  # noqa: BLE001 - the tests read the failure back
            self.outcome.append(error)

    @property
    def done(self) -> bool:
        return not self.thread.is_alive()

    def result(self, timeout: float = WAIT) -> Any:
        """The value the call returned; the failure it ended with is raised."""
        self.thread.join(timeout)
        if self.thread.is_alive():
            raise RuntimeError("the call is stuck")
        if isinstance(self.outcome[0], BaseException):
            raise self.outcome[0]
        return self.outcome[0]


def waiting(store: Any) -> int:
    """How many repeated requests wait for a running reference, by the places they hold."""
    return store_module.REFERENCE_WAITERS - store._reference_waiters._value


def until(predicate: Any, timeout: float = 10.0) -> None:
    deadline = time.monotonic() + timeout
    while not predicate():
        if time.monotonic() > deadline:
            raise RuntimeError("the condition never held")
        time.sleep(0.01)


def row_of(network: Any, admission_id: str) -> Any:
    return network.store._db.execute(
        "SELECT * FROM admissions_v2 WHERE admission_id=?", (admission_id,)
    ).fetchone()


def challenge_of(network: Any, admission_id: str) -> JoinChallenge:
    return JoinChallenge.model_validate_json(row_of(network, admission_id)["challenge"])


def committed(network: Any, admission_id: str) -> bool:
    row = row_of(network, admission_id)
    return row["reference_commitment"] is not None or row["reference_json"] is not None


def served_while_the_reference_runs(network: Any, probe: Any) -> bool:
    """Start the reference on a slow executor, run `probe` on another thread, report if it ended."""
    store, executor = network.store, SlowExecutor()
    store._role_launch = executor
    admission_id = pending_reference(network)
    outcome: list[BaseException] = []

    def reference() -> None:
        try:
            store.trial_reference_v2(network.manifest.run_id(), admission_id)
        except BaseException as error:  # noqa: BLE001 - the fake executor's failure is expected
            outcome.append(error)

    failed: list[BaseException] = []

    def ask() -> None:
        try:
            probe()
        except BaseException as error:  # noqa: BLE001 - a probe that fails was not served
            failed.append(error)

    worker = threading.Thread(target=reference, daemon=True)
    asker = threading.Thread(target=ask, daemon=True)
    worker.start()
    try:
        if not executor.reached.wait(WAIT):
            raise RuntimeError("the reference never reached the executor: the setup is broken")
        asker.start()
        asker.join(timeout=3)
        served = not asker.is_alive()
    finally:
        executor.release.set()
        worker.join(WAIT)
        asker.join(WAIT)
    if worker.is_alive() or asker.is_alive():
        raise RuntimeError("a thread is stuck after the executor was released")
    if failed:
        raise failed[0]
    if not outcome or not isinstance(outcome[0], IslandFailure):
        raise RuntimeError(f"the reference did not end in the fake executor's failure: {outcome}")
    return served


class Attempt(threading.Thread):
    """A request on its own thread that remembers how it ended."""

    def __init__(self, function: Any, *args: Any) -> None:
        super().__init__(daemon=True)
        self.function, self.args_, self.error = function, args, None

    def run(self) -> None:
        try:
            self.function(*self.args_)
        except BaseException as error:  # noqa: BLE001 - the test reads it back
            self.error = error


def test_the_latest_beacon_is_served_while_a_remote_reference_runs(hosted):
    assert served_while_the_reference_runs(hosted, hosted.store.latest_beacon)


def test_a_beacon_round_is_accepted_while_a_remote_reference_runs(hosted):
    from tests.challenge.test_service_network_v2 import fixture_beacon

    payload = fixture_beacon(hosted.now + 2)
    assert served_while_the_reference_runs(hosted, lambda: entry(hosted).push_beacon(payload))
    # the store that hosts the run has the round too, not only the root
    assert hosted.store._db.execute(
        "SELECT 1 FROM beacon WHERE round=?", (payload["round"],)
    ).fetchone()


def test_another_trials_requests_are_served_while_a_remote_reference_runs(hosted):
    run_id, store, executor = hosted.manifest.run_id(), hosted.store, Executor()
    store._role_launch = executor
    first = pending_reference(hosted, 0)
    second = pending_reference(hosted, 1)
    blocked = Call(store, run_id, first)
    try:
        assert executor.reached.wait(WAIT)
        requests = [
            Attempt(store.export_trial_v2, run_id, second),
            Attempt(store.admission_v2, run_id, "status", b"", backend.service.HOT[1].ss58),
        ]
        for request in requests:
            request.start()
        for request in requests:
            request.join(3)
        assert not any(request.is_alive() for request in requests)
        assert [request.error for request in requests] == [None, None]  # served, not failed
        # the other trial's own reference runs to its commitment while the first is blocked
        assert store.trial_reference_v2(run_id, second)["reference_commitment"]
        assert not blocked.done and len(executor.launches) == 2
    finally:
        executor.release.set()
    blocked.result()
    assert committed(hosted, first) and committed(hosted, second)


class LockWatch:
    """Asks for the store lock every 20 ms on a thread of its own and times each ask."""

    def __init__(self, store: Any) -> None:
        self.waits: list[float] = []
        self._store, self._stop = store, threading.Event()
        self._thread = threading.Thread(target=self._run, daemon=True)

    def _run(self) -> None:
        while not self._stop.is_set():
            started = time.perf_counter()
            assert self._store._lock.acquire(timeout=WAIT)
            self._store._lock.release()
            self.waits.append(time.perf_counter() - started)
            time.sleep(0.02)

    def __enter__(self) -> LockWatch:
        self._thread.start()
        return self

    def __exit__(self, *error: object) -> None:
        self._stop.set()
        self._thread.join(WAIT)


def test_a_five_second_reference_holds_the_store_lock_for_less_than_a_second(hosted):
    run_id, store, executor = hosted.manifest.run_id(), hosted.store, Executor(delay=5.0)
    store._role_launch = executor
    admission_id = pending_reference(hosted)
    began = time.perf_counter()
    with LockWatch(store) as watch:
        value = store.trial_reference_v2(run_id, admission_id)
    assert time.perf_counter() - began >= 5.0  # the fake executor really took its five seconds
    assert len(watch.waits) > 100  # the lock was asked for all along, not only at the ends
    assert max(watch.waits) < 1.0, f"the lock was unavailable for {max(watch.waits):.2f} s"
    assert value["reference_commitment"] == row_of(hosted, admission_id)["reference_commitment"]


# Parameters of the models the CPU staging of a trial has to carry: 8 layers d=512, the 101 M
# profile of D68 (docs/adr/0015), 12 layers d=768.
MODELS = {"66M": 66_331_136, "101M": 100_676_352, "162M": 162_417_408}
SECONDS_PER_PARAMETER = 2e-8  # staging on the CPU: about one second per 50 M parameters


class SizedStaging:
    """What staging costs for a model of `parameters`, on the fixture's tiny model.

    The real `init_params` and `pack_state` still run (the job, its hashes and the reference stay
    the fixture's own); each then takes the time a model of that size takes: half the total in the
    parameter initialisation, a quarter in each of the two state serialisations. `peak` is the most
    stagings seen running at once."""

    def __init__(self, monkeypatch: pytest.MonkeyPatch, parameters: int) -> None:
        from hypertrain.auditor import replay
        from hypertrain.trainer import model

        self.seconds = parameters * SECONDS_PER_PARAMETER
        self.peak, self._active, self._mutex = 0, 0, threading.Lock()
        init, pack = model.init_params, replay.pack_state

        def slow_init(cfg: Any) -> Any:
            with self._mutex:
                self._active += 1
                self.peak = max(self.peak, self._active)
            try:
                out = init(cfg)
                time.sleep(self.seconds / 2)
                return out
            finally:
                with self._mutex:
                    self._active -= 1

        def slow_pack(theta: Any, st: Any = None) -> bytes:
            out = pack(theta, st)
            if theta:  # the empty v0 costs nothing
                time.sleep(self.seconds / 4)
            return out

        monkeypatch.setattr(model, "init_params", slow_init)
        monkeypatch.setattr(replay, "pack_state", slow_pack)


@pytest.mark.parametrize("parameters", MODELS.values(), ids=MODELS)
def test_a_reference_of_a_large_model_holds_the_store_lock_for_less_than_a_second(
    hosted, monkeypatch, parameters
):
    run_id, store, executor = hosted.manifest.run_id(), hosted.store, Executor(delay=0)
    store._role_launch = executor
    admission_id = pending_reference(hosted)
    staging = SizedStaging(monkeypatch, parameters)
    began = time.perf_counter()
    with LockWatch(store) as watch:
        value = store.trial_reference_v2(run_id, admission_id)
    assert time.perf_counter() - began >= staging.seconds  # the model's work ran inside the call
    assert len(watch.waits) > staging.seconds * 5  # the lock was asked for all along
    assert max(watch.waits) < 1.0, f"the lock was unavailable for {max(watch.waits):.2f} s"
    assert value["reference_commitment"] == row_of(hosted, admission_id)["reference_commitment"]


def test_two_references_of_a_large_model_stage_one_at_a_time_and_leave_the_lock_free(
    hosted, monkeypatch
):
    run_id, store, executor = hosted.manifest.run_id(), hosted.store, Executor(delay=0)
    store._role_launch = executor
    first, second = pending_reference(hosted, 0), pending_reference(hosted, 1)
    staging = SizedStaging(monkeypatch, MODELS["66M"])
    with LockWatch(store) as watch:
        calls = [Call(store, run_id, first), Call(store, run_id, second)]
        values = [call.result() for call in calls]
    assert staging.peak == 1  # as the store lock made it: one model in memory at a time
    assert max(watch.waits) < 1.0, f"the lock was unavailable for {max(watch.waits):.2f} s"
    assert [v["reference_commitment"] for v in values] == [
        row_of(hosted, first)["reference_commitment"],
        row_of(hosted, second)["reference_commitment"],
    ]


def test_exporting_a_trial_of_a_large_model_holds_the_store_lock_for_less_than_a_second(
    hosted, monkeypatch
):
    run_id, store = hosted.manifest.run_id(), hosted.store
    admission_id = pending_reference(hosted)
    staging = SizedStaging(monkeypatch, MODELS["101M"])
    put = store.objects.put

    def slow_put(data: bytes) -> str:  # five objects, the two states are the large ones
        time.sleep(staging.seconds / 5)
        return put(data)

    monkeypatch.setattr(store.objects, "put", slow_put)
    began = time.perf_counter()
    with LockWatch(store) as watch:
        exported = store.export_trial_v2(run_id, admission_id)
    assert time.perf_counter() - began >= 2 * staging.seconds  # staging, then the five puts
    assert max(watch.waits) < 1.0, f"the lock was unavailable for {max(watch.waits):.2f} s"
    assert set(exported["objects"]) == {"start_state", "ef_in", "v0", "samples", "sample_proofs"}


class GuardedDb:
    """The store's connection, which notes every statement a thread runs without the store lock."""

    def __init__(self, real: Any, lock: Any) -> None:
        self.real, self.unlocked, self._lock = real, [], lock

    def execute(self, sql: str, *args: Any) -> Any:
        if not self._lock._is_owned():
            self.unlocked.append(" ".join(sql.split())[:80])
        return self.real.execute(sql, *args)

    def __getattr__(self, name: str) -> Any:
        return getattr(self.real, name)


@pytest.mark.parametrize("call", ["trial_reference_v2", "export_trial_v2"])
def test_a_trial_call_reads_the_store_connection_only_under_the_store_lock(hosted, call):
    # The connection is shared: a read from the unlocked staging, launch or puts could see another
    # thread's uncommitted rows, so what they need is read before the lock is let go.
    run_id, store = hosted.manifest.run_id(), hosted.store
    store._role_launch = Executor(delay=0)
    admission_id = pending_reference(hosted)
    guarded = GuardedDb(store._db, store._lock)
    store._db = guarded
    try:
        getattr(store, call)(run_id, admission_id)
    finally:
        store._db = guarded.real
    assert guarded.unlocked == []


def test_a_staging_in_the_root_holds_back_one_in_a_child_store(child_network):  # noqa: F811
    hosted = child_network
    run_id, store = hosted.manifest.run_id(), hosted.store
    admission_id = pending_reference(hosted)
    challenge = challenge_of(hosted, admission_id)
    samples = trial_samples(
        hosted.manifest, admission_id, challenge.nonce, store._beacon_v2(challenge.seed_beacon)
    )
    staging = store.plan_trial_v2(
        run_id, challenge, samples, row_of(hosted, admission_id)["trial_epoch"]
    )
    with hosted.root._staging:  # a model of the occupant run is in memory
        waiting_for_it = Attempt(staging)
        waiting_for_it.start()
        waiting_for_it.join(0.5)
        assert waiting_for_it.is_alive()
    waiting_for_it.join(WAIT)
    assert not waiting_for_it.is_alive() and waiting_for_it.error is None


def test_two_calls_for_one_trial_launch_once_and_return_one_commitment(hosted):
    run_id, store, executor = hosted.manifest.run_id(), hosted.store, Executor()
    store._role_launch = executor
    admission_id = pending_reference(hosted)
    plan, staged = store.plan_trial_v2, []

    def counting(*args: Any) -> Any:
        staging = plan(*args)
        return lambda: (staged.append(1), staging())[1]

    store.plan_trial_v2 = counting  # type: ignore[method-assign]
    first = Call(store, run_id, admission_id)
    try:
        assert executor.reached.wait(WAIT)
        second = Call(store, run_id, admission_id)
        until(lambda: waiting(store) == 1)
        time.sleep(0.5)  # long enough for a second launch, if there were one, to start
        assert not second.done and len(executor.launches) == 1
    finally:
        executor.release.set()
    assert len(staged) == 1  # the waiter did not stage the job again
    assert first.result() == second.result()
    assert (
        first.result()["reference_commitment"]
        == row_of(hosted, admission_id)["reference_commitment"]
    )
    assert len(executor.launches) == 1


def test_a_repeated_request_is_refused_past_the_waiter_budget(hosted):
    run_id, store, executor = hosted.manifest.run_id(), hosted.store, Executor()
    store._role_launch = executor
    admission_id = pending_reference(hosted)
    leader = Call(store, run_id, admission_id)
    waiters: list[Call] = []
    try:
        assert executor.reached.wait(WAIT)
        waiters = [Call(store, run_id, admission_id) for _ in range(store_module.REFERENCE_WAITERS)]
        until(lambda: waiting(store) == store_module.REFERENCE_WAITERS)
        assert not any(w.done for w in waiters)
        with pytest.raises(ChallengeError) as refused:
            Call(store, run_id, admission_id).result(3)  # no worker is held for it
        assert refused.value.status == 503
    finally:
        executor.release.set()
    results = [leader.result(), *(w.result() for w in waiters)]
    assert len({r["reference_commitment"] for r in results}) == 1
    assert len(executor.launches) == 1
    for _ in range(store_module.REFERENCE_WAITERS):  # every place was given back
        assert store._reference_waiters.acquire(blocking=False)


def test_a_waiter_gives_up_after_its_wait_and_leaves_the_launch_alone(hosted, monkeypatch):
    run_id, store, executor = hosted.manifest.run_id(), hosted.store, Executor()
    store._role_launch = executor
    admission_id = pending_reference(hosted)
    leader = Call(store, run_id, admission_id)
    try:
        assert executor.reached.wait(WAIT)
        monkeypatch.setattr(store_module, "REFERENCE_WAIT_SECONDS", 0.5)
        with pytest.raises(ChallengeError) as impatient:
            Call(store, run_id, admission_id).result(5)
        assert impatient.value.status == 503 and "still running" in impatient.value.detail
        assert not leader.done
        for _ in range(store_module.REFERENCE_WAITERS):  # its place is free again
            assert store._reference_waiters.acquire(blocking=False)
        for _ in range(store_module.REFERENCE_WAITERS):
            store._reference_waiters.release()
    finally:
        executor.release.set()
    assert leader.result()["reference_commitment"]


def test_a_failed_launch_fails_its_waiters_alike_frees_the_flight_and_a_retry_launches_again(
    hosted, monkeypatch
):
    monkeypatch.setattr(store_module, "REFERENCE_WAIT_SECONDS", 3.0)  # a stuck flight fails fast
    run_id, store = hosted.manifest.run_id(), hosted.store
    executor = Executor(error=IslandFailure("the executor lost its host"))
    store._role_launch = executor
    admission_id = pending_reference(hosted)
    first = Call(store, run_id, admission_id)
    try:
        assert executor.reached.wait(WAIT)
        second = Call(store, run_id, admission_id)
        until(lambda: waiting(store) == 1)
        assert not second.done
    finally:
        executor.release.set()
    with pytest.raises(IslandFailure) as lost:
        first.result()
    with pytest.raises(IslandFailure) as alike:
        second.result()  # the waiter gets the leader's failure, not a new 409
    assert alike.value is lost.value
    assert store._reference_flights_v2 == {} and store._lease_guards_v2 == {}
    assert not committed(hosted, admission_id)
    retried = store.trial_reference_v2(run_id, admission_id)
    assert retried["reference_commitment"] == row_of(hosted, admission_id)["reference_commitment"]
    assert len(executor.launches) == 2


def test_a_launch_that_dies_between_claim_and_settle_leaves_no_commitment_and_a_retry_works(hosted):
    run_id, store = hosted.manifest.run_id(), hosted.store
    executor = Executor(error=SystemExit("the worker was killed"))
    store._role_launch = executor
    admission_id = pending_reference(hosted)
    tables = (
        "admission_trial_results",
        "admission_trials",
        "admission_reservations",
        "records_v2",
        "escrow_events",
    )

    def snapshot() -> list[list[tuple[Any, ...]]]:
        return [[tuple(r) for r in store._db.execute(f"SELECT * FROM {t}")] for t in tables]

    before = snapshot()
    first = Call(store, run_id, admission_id)
    try:
        assert executor.reached.wait(WAIT)
        # in flight: the claim wrote no reference row
        assert not committed(hosted, admission_id)
        nonce = challenge_of(hosted, admission_id).nonce
        staged = store.state_dir / "trials-v2" / nonce
        assert (staged / "start_state").is_file()
    finally:
        executor.release.set()
    with pytest.raises(SystemExit):
        first.result()
    assert store._reference_flights_v2 == {} and store._lease_guards_v2 == {}
    assert not committed(hosted, admission_id)
    assert (
        snapshot() == before
    )  # the claim and the death wrote nothing to the admission or the ledger
    retried = store.trial_reference_v2(run_id, admission_id)  # the staged inputs are reused
    assert retried["reference_commitment"] == row_of(hosted, admission_id)["reference_commitment"]


@pytest.mark.parametrize("honour_cancel", [True, False], ids=["executor-stops", "executor-ignores"])
def test_the_beacon_passing_the_deadline_cancels_the_launch_and_no_commitment_is_made(
    hosted, honour_cancel
):
    run_id, store = hosted.manifest.run_id(), hosted.store
    executor = Executor(honour_cancel=honour_cancel)
    store._role_launch = executor
    admission_id = pending_reference(hosted)
    challenge = challenge_of(hosted, admission_id)
    key = f"reference:{admission_id}:{challenge.nonce}"
    first = Call(store, run_id, admission_id)
    try:
        assert executor.reached.wait(WAIT)
        assert set(store._lease_guards_v2) == {key}  # registered like an audit's guard
        cancel = executor.cancels[0]
        assert cancel is not None and not cancel.is_set()
        second = Call(store, run_id, admission_id)  # waits on the flight
        until(lambda: waiting(store) == 1)
        hosted.push(challenge.deadline_beacon + 1)  # the pusher is served; the reaper runs
        assert cancel.is_set()
        outcome = store._db.execute(
            "SELECT outcome FROM admission_trial_results WHERE epoch=?",
            (row_of(hosted, admission_id)["trial_epoch"],),
        ).fetchone()[0]
        assert outcome == "EXPIRED"
    finally:
        executor.release.set()
    # An executor that watches the event stops with its own failure; one that does not is caught
    # by the screening of its result. Either way the answer is the window's, a 409, not a server
    # error, and the waiter gets the same one.
    cause = IslandFailure if honour_cancel else WorkScreenError
    with pytest.raises(ChallengeError) as stopped:
        first.result()
    with pytest.raises(ChallengeError) as alike:
        second.result()
    assert alike.value is stopped.value
    assert (stopped.value.status, stopped.value.detail) == (409, "REFERENCE_TRIAL_CLOSED")
    assert isinstance(stopped.value.__cause__, cause)
    assert not committed(hosted, admission_id)
    assert store._reference_flights_v2 == {} and store._lease_guards_v2 == {}
    assert row_of(hosted, admission_id)["clean_count"] == 0


def test_a_lease_cancelled_while_the_result_waits_for_the_lock_is_not_committed(
    hosted, monkeypatch
):
    run_id, store, executor = hosted.manifest.run_id(), hosted.store, Executor()
    store._role_launch = executor
    admission_id = pending_reference(hosted)
    key = f"reference:{admission_id}:{challenge_of(hosted, admission_id).nonce}"
    _, admission, _ = store._services(run_id)
    ran, run = threading.Event(), admission.run_reference

    def run_then_say_so(*args: Any, **options: Any) -> Any:
        result = run(*args, **options)
        ran.set()
        return result

    monkeypatch.setattr(admission, "run_reference", run_then_say_so)
    call = Call(store, run_id, admission_id)
    assert executor.reached.wait(WAIT)
    with store._lock:  # the store is busy when the result arrives
        executor.release.set()
        assert ran.wait(WAIT)
        time.sleep(0.5)  # the call is parked on the lock now, past its first check
        store._lease_guards_v2[key].revoke()  # the lease ends while it waits
    with pytest.raises(ChallengeError) as over:
        call.result()
    assert (over.value.status, over.value.detail) == (409, "REFERENCE_TRIAL_CLOSED")
    assert not committed(hosted, admission_id)
    assert store._reference_flights_v2 == {} and store._lease_guards_v2 == {}


def test_a_result_for_a_stale_claim_is_dropped_and_the_new_challenge_gets_its_own_launch(hosted):
    run_id, store, executor = hosted.manifest.run_id(), hosted.store, Executor()
    store._role_launch = executor
    admission_id = pending_reference(hosted)
    _, admission, _ = store._services(run_id)
    hotkey = backend.service.HOT[0].ss58
    stale = challenge_of(hosted, admission_id)
    first = Call(store, run_id, admission_id)
    try:
        assert executor.reached.wait(WAIT)
        # while the first launch runs, the identity is suspended, resumed and asked again
        admission.suspend(hotkey, reason="infrastructure", now=hosted.now)
        hosted.push(hosted.now + 1)
        admission.resume(hotkey, now=hosted.now)
        hosted.push(hosted.now + 1)
        admission.challenge(admission_id, now=hosted.now)
        fresh = challenge_of(hosted, admission_id)
        assert fresh.nonce != stale.nonce
        second = Call(store, run_id, admission_id)
        assert second.result()["reference_commitment"]  # not behind the stale flight
        assert not first.done and len(executor.launches) == 2
    finally:
        executor.release.set()
    with pytest.raises(AdmissionError, match="REFERENCE_STATE_CHANGED"):
        first.result()
    row = row_of(hosted, admission_id)
    assert row["challenge"] == fresh.model_dump_json()
    assert json.loads(row["reference_json"])["challenge_hash"] == fresh.digest()
    assert store._reference_flights_v2 == {} and store._lease_guards_v2 == {}


def test_the_commitment_is_made_at_the_round_of_the_settlement_not_of_the_claim(hosted):
    run_id, store, executor = hosted.manifest.run_id(), hosted.store, Executor()
    store._role_launch = executor
    admission_id = pending_reference(hosted)
    claimed = hosted.now
    first = Call(store, run_id, admission_id)
    try:
        assert executor.reached.wait(WAIT)
        hosted.push(claimed + 3)  # the pusher goes on while the executor works
    finally:
        executor.release.set()
    first.result()
    assert row_of(hosted, admission_id)["reference_beacon"] == claimed + 3


def test_a_beacon_that_went_stale_during_the_launch_commits_nothing_and_the_retry_does(hosted):
    # The pusher stops while the executor works: the drand clock goes on, the store gets no round.
    # The settlement passes the freshness gate again, so the commitment is not made on a round
    # that is stale (nor tested against the deadline with it); the retry finds the publication.
    run_id, store, executor = hosted.manifest.run_id(), hosted.store, Executor()
    store._role_launch = executor
    admission_id = pending_reference(hosted)
    behind = hosted.now + 7
    call = Call(store, run_id, admission_id)
    try:
        assert executor.reached.wait(WAIT)
        genesis = hosted.manifest.training.beacon.genesis_time
        store.clock = lambda: genesis + (behind - 1) * 3
    finally:
        executor.release.set()
    with pytest.raises(ChallengeError) as stale:
        call.result()
    assert (stale.value.status, stale.value.detail) == (503, "beacon infrastructure paused")
    assert not committed(hosted, admission_id)
    assert store._reference_flights_v2 == {} and store._lease_guards_v2 == {}
    hosted.push(behind)  # the pusher is back
    retried = store.trial_reference_v2(run_id, admission_id)
    assert retried["reference_commitment"] == row_of(hosted, admission_id)["reference_commitment"]
    assert row_of(hosted, admission_id)["reference_beacon"] == behind
    assert len(executor.launches) == 2  # the second finds the publication of the first


def test_a_request_for_custody_beside_one_without_has_its_own_flight_and_retains_it(hosted):
    run_id, store, executor = hosted.manifest.run_id(), hosted.store, Executor()
    store._role_launch = executor
    admission_id = pending_reference(hosted)
    _, admission, _ = store._services(run_id)
    admission.backend = "cpu"  # custody is retained for the CPU backend of the test ledger only
    plain = Call(store, run_id, admission_id)
    try:
        assert executor.reached.wait(WAIT)
        custody = Call(store, run_id, admission_id, retain_shadow_custody=True)
        # it is not a waiter of the first call (which is held): the flag is part of the flight
        assert custody.result()["reference_commitment"]
        assert waiting(store) == 0 and not plain.done
    finally:
        executor.release.set()
    assert plain.result() == custody.result()
    assert len(executor.launches) == 2  # one trial, two launches: the docstring says so
    kept = store._db.execute("SELECT COUNT(*) FROM records_v2 WHERE kind='shadow-execution'")
    assert kept.fetchone()[0] == 1


def test_claim_and_settle_hold_the_store_lock_and_the_run_does_not(hosted, monkeypatch):
    run_id, store, executor = hosted.manifest.run_id(), hosted.store, Executor(delay=0)
    store._role_launch = executor
    admission_id = pending_reference(hosted)
    _, admission, _ = store._services(run_id)
    owned: dict[str, bool] = {}

    def spy(name: str) -> None:
        real = getattr(admission, name)

        def watched(*args: Any, **options: Any) -> Any:
            owned[name] = store._lock._is_owned()
            return real(*args, **options)

        monkeypatch.setattr(admission, name, watched)

    for name in ("claim_reference", "run_reference", "settle_reference"):
        spy(name)
    store.trial_reference_v2(run_id, admission_id)
    assert owned == {"claim_reference": True, "run_reference": False, "settle_reference": True}


def test_the_wall_clock_cutoff_cancels_a_hung_launch_without_any_beacon(hosted, monkeypatch):
    # A remote executor that hangs while the pusher is down: only the guard's timer can stop it.
    from hypertrain.auditor import worker

    init = worker.LeaseGuard.__init__

    def cutoff_in_a_second(self: Any, expires: int, absolute: int, deadline_unix: float) -> None:
        init(self, expires, absolute, time.time() + 1.0)

    monkeypatch.setattr(worker.LeaseGuard, "__init__", cutoff_in_a_second)
    run_id, store, executor = hosted.manifest.run_id(), hosted.store, Executor()
    store._role_launch = executor
    admission_id = pending_reference(hosted)
    call = Call(store, run_id, admission_id)
    try:
        assert executor.reached.wait(WAIT)
        cancel = executor.cancels[0]
        assert cancel is not None and cancel.wait(10)  # no round was pushed
    finally:
        executor.release.set()
    with pytest.raises(ChallengeError) as over:
        call.result()
    assert (over.value.status, over.value.detail) == (409, "REFERENCE_TRIAL_CLOSED")
    assert not committed(hosted, admission_id)


def test_a_reference_asked_in_the_deadline_round_is_refused_without_a_launch(hosted):
    run_id, store, executor = hosted.manifest.run_id(), hosted.store, Executor(delay=0)
    store._role_launch = executor
    admission_id = pending_reference(hosted)
    hosted.push(challenge_of(hosted, admission_id).deadline_beacon)  # the trial is still OPEN
    with pytest.raises(ChallengeError) as late:
        store.trial_reference_v2(run_id, admission_id)
    assert (late.value.status, late.value.detail) == (409, "REFERENCE_TRIAL_CLOSED")
    assert executor.launches == [] and not committed(hosted, admission_id)


def test_a_challenge_without_its_receipt_is_refused_before_anything_runs(hosted):
    run_id, store, executor = hosted.manifest.run_id(), hosted.store, Executor(delay=0)
    store._role_launch = executor
    admission_id = pending_reference(hosted)
    nonce = challenge_of(hosted, admission_id).nonce
    store._db.execute(
        "DELETE FROM admission_reservations WHERE reservation=?",
        (f"challenge|{admission_id}|{nonce}",),
    )
    with pytest.raises(ChallengeError) as missing:
        store.trial_reference_v2(run_id, admission_id)
    assert (missing.value.status, missing.value.detail) == (409, "MISSING_CHALLENGE_RECEIPT")
    assert executor.launches == [] and store._reference_flights_v2 == {}


def test_a_job_whose_deadline_is_not_the_challenges_never_reaches_the_executor(hosted, monkeypatch):
    run_id, store, executor = hosted.manifest.run_id(), hosted.store, Executor(delay=0)
    store._role_launch = executor
    admission_id = pending_reference(hosted)
    plan = store.plan_trial_v2

    def staged_with_another_deadline(*args: Any) -> Any:
        staging = plan(*args)

        def staged() -> Any:
            job, directory = staging()
            return job.model_copy(update={"deadline": job.deadline + 1}), directory

        return staged

    monkeypatch.setattr(store, "plan_trial_v2", staged_with_another_deadline)
    with pytest.raises(ChallengeError) as changed:
        store.trial_reference_v2(run_id, admission_id)
    assert (changed.value.status, changed.value.detail) == (
        409,
        "reference launch subject changed",
    )
    assert executor.launches == [] and not committed(hosted, admission_id)


def test_the_waiter_budget_is_one_for_the_root_and_its_child_stores(child_network):  # noqa: F811
    hosted = child_network
    run_id, store, executor = hosted.manifest.run_id(), hosted.store, Executor()
    store._role_launch = executor
    admission_id = pending_reference(hosted)
    leader, taken = Call(store, run_id, admission_id), 0
    try:
        assert executor.reached.wait(WAIT)
        for _ in range(store_module.REFERENCE_WAITERS):  # places held by waiters of other runs
            assert hosted.root._reference_waiters.acquire(blocking=False)
            taken += 1
        with pytest.raises(ChallengeError) as refused:
            Call(store, run_id, admission_id).result(3)
        assert refused.value.status == 503
    finally:
        for _ in range(taken):
            hosted.root._reference_waiters.release()
        executor.release.set()
    assert leader.result()["reference_commitment"]


def test_a_caller_that_holds_the_store_lock_is_refused(hosted):
    run_id, store, executor = hosted.manifest.run_id(), hosted.store, Executor()
    store._role_launch = executor
    admission_id = pending_reference(hosted)
    with store._lock, pytest.raises(RuntimeError, match="store lock"):
        store.trial_reference_v2(run_id, admission_id)
    assert not executor.launches and store._reference_flights_v2 == {}


def test_a_beacon_push_reads_the_registered_guards_under_the_store_lock(hosted):
    store = hosted.store

    class Registry(dict):
        def values(self):  # type: ignore[no-untyped-def]
            assert store._lock._is_owned(), "the guards were read without the store lock"
            return super().values()

    store._lease_guards_v2 = Registry()
    hosted.push(hosted.now + 1)


def test_exporting_a_trial_never_rewrites_the_inputs_a_running_launch_reads(hosted):
    run_id, store, executor = hosted.manifest.run_id(), hosted.store, Executor()
    store._role_launch = executor
    admission_id = pending_reference(hosted)
    nonce = challenge_of(hosted, admission_id).nonce
    first = Call(store, run_id, admission_id)
    try:
        assert executor.reached.wait(WAIT)
        names = ("start_state", "ef_in", "v0", "samples", "sample_proofs")
        staged = store.state_dir / "trials-v2" / nonce
        seen = {n: (os.stat(staged / n).st_ino, os.stat(staged / n).st_mtime_ns) for n in names}
        time.sleep(0.05)
        store.export_trial_v2(run_id, admission_id)
        assert {
            n: (os.stat(staged / n).st_ino, os.stat(staged / n).st_mtime_ns) for n in names
        } == seen
    finally:
        executor.release.set()
    first.result()


def miner_submission(hosted: Any, admission_id: str, workdir: Path) -> dict[str, Any]:
    """What an honest miner posts to /join/proof: the real CPU reference of a private copy of the
    staged inputs, signed by the admission's hotkey."""
    store, row = hosted.store, row_of(hosted, admission_id)
    challenge = challenge_of(hosted, admission_id)
    beacon = store._beacon_v2(challenge.seed_beacon)
    samples = trial_samples(hosted.manifest, admission_id, challenge.nonce, beacon)
    job, staged = store.stage_trial_v2(
        hosted.manifest.run_id(), challenge, samples, row["trial_epoch"]
    )
    shutil.copytree(staged, workdir)
    screen, proof = screen_work(job, workdir, challenge, now_beacon=hosted.now, backend="cpu")
    key = backend.service.HOT[0]
    return {
        "proof": hosted.signed(key, "WorkProof", proof),
        "screen": hosted.signed(key, "WorkScreenV2", screen),
    }


class Post:
    """A request of the miner on its own thread, as a client with an open connection would hold."""

    def __init__(self, hosted: Any, path: str, body: Any) -> None:
        self.outcome: list[Any] = []
        self.thread = threading.Thread(target=self._run, args=(hosted, path, body), daemon=True)
        self.thread.start()

    def _run(self, hosted: Any, path: str, body: Any) -> None:
        try:
            self.outcome.append(hosted.client.post(hosted.url + path, json=body))
        except BaseException as error:  # noqa: BLE001 - the test client re-raises server errors
            self.outcome.append(error)

    def result(self, timeout: float = WAIT) -> Any:
        self.thread.join(timeout)
        assert not self.thread.is_alive(), "the request is stuck"
        return self.outcome[0]


def test_the_proof_route_runs_the_reference_while_the_beacon_goes_on_and_accepts_the_proof(
    hosted, tmp_path
):
    # Given: an honest miner's signed proof and a remote executor still at work
    store, executor = hosted.store, Executor()
    store._role_launch = executor
    admission_id = pending_reference(hosted)
    signed = miner_submission(hosted, admission_id, tmp_path / "miner")
    claimed = hosted.now
    # When: the proof is posted; the service must compute the trusted reference first
    post = Post(hosted, "/join/proof", signed)
    try:
        assert executor.reached.wait(WAIT)
        # Then: the pusher's own route answers at once while the executor works ...
        began = time.perf_counter()
        hosted.push(claimed + 3)
        assert time.perf_counter() - began < 1.0
        assert not committed(hosted, admission_id)
    finally:
        executor.release.set()
    # ... and the proof is accepted against the reference committed at the later round
    response = post.result()
    assert response.status_code == 200, response.text
    assert response.json()["clean_count"] == 1
    row = row_of(hosted, admission_id)
    assert row["reference_beacon"] == claimed + 3 and len(executor.launches) == 1


def test_the_deadline_round_cancels_a_launch_that_is_still_running(hosted, tmp_path):
    # The window of a reference is the trial's: the lease guard cancels the launch when the
    # verified beacon reaches the deadline round, as the wall-clock timer did before (the round is
    # emitted at that very second). A proof may still arrive in the deadline round; a reference
    # that is not done by then is not committed, whoever was slow.
    store, executor = hosted.store, Executor()
    store._role_launch = executor
    admission_id = pending_reference(hosted)
    signed = miner_submission(hosted, admission_id, tmp_path / "miner")
    post = Post(hosted, "/join/proof", signed)
    try:
        assert executor.reached.wait(WAIT)
        cancel = executor.cancels[0]
        hosted.push(challenge_of(hosted, admission_id).deadline_beacon - 1)
        assert cancel is not None and not cancel.is_set()
        hosted.push(challenge_of(hosted, admission_id).deadline_beacon)
        assert cancel.is_set()
    finally:
        executor.release.set()
    late = post.result()
    assert (late.status_code, late.json()) == (409, {"detail": "REFERENCE_TRIAL_CLOSED"})
    assert not committed(hosted, admission_id)


def test_a_slow_reference_that_ends_one_round_before_the_deadline_is_committed(hosted, tmp_path):
    store, executor = hosted.store, Executor()
    store._role_launch = executor
    admission_id = pending_reference(hosted)
    signed = miner_submission(hosted, admission_id, tmp_path / "miner")
    last = challenge_of(hosted, admission_id).deadline_beacon - 1
    post = Post(hosted, "/join/proof", signed)
    try:
        assert executor.reached.wait(WAIT)
        hosted.push(last)  # 98 of the 100 rounds went while the executor worked
    finally:
        executor.release.set()
    response = post.result()
    assert response.status_code == 200, response.text
    assert response.json()["clean_count"] == 1
    assert row_of(hosted, admission_id)["reference_beacon"] == last


def test_a_reference_that_ends_after_the_window_commits_nothing_and_sanctions_nobody(
    hosted, tmp_path
):
    # The window closes while the executor works: the result is dropped, the trial is closed as
    # bookkeeping (EXPIRED), and the miner's standing is not touched (no strike, no suspension).
    store, executor = hosted.store, Executor(honour_cancel=False)
    store._role_launch = executor
    admission_id = pending_reference(hosted)
    signed = miner_submission(hosted, admission_id, tmp_path / "miner")
    before = row_of(hosted, admission_id)
    post = Post(hosted, "/join/proof", signed)
    try:
        assert executor.reached.wait(WAIT)
        hosted.push(challenge_of(hosted, admission_id).deadline_beacon + 1)
    finally:
        executor.release.set()
    late = post.result()
    assert (late.status_code, late.json()) == (409, {"detail": "REFERENCE_TRIAL_CLOSED"})
    after = row_of(hosted, admission_id)
    assert not committed(hosted, admission_id)
    assert (after["state"], after["strikes"], after["pending_dispute"], after["clean_count"]) == (
        before["state"],
        before["strikes"],
        before["pending_dispute"],
        before["clean_count"],
    )
    outcome = store._db.execute(
        "SELECT outcome FROM admission_trial_results WHERE epoch=?", (after["trial_epoch"],)
    ).fetchone()[0]
    assert outcome == "EXPIRED"


def test_a_probe_that_does_not_need_the_lock_is_reported_served(network, monkeypatch):
    monkeypatch.setitem(globals(), "WAIT", 5)  # an executor that is never released ends early
    assert served_while_the_reference_runs(network, lambda: time.sleep(0.3)) is True
    assert network.store._role_launch.release.is_set()


def test_a_reference_that_never_reaches_the_executor_is_a_setup_error(network, monkeypatch):
    class StopsBeforeTheLaunch(SlowExecutor):
        def __call__(self, kind: str, job: Any, directory: Path, context: Any) -> Any:
            def launch(job: Any, directory: Path, **options: Any) -> Any:
                raise IslandFailure("the fake executor stops before it says it was reached")

            return launch

    monkeypatch.setitem(globals(), "WAIT", 2)
    monkeypatch.setitem(globals(), "SlowExecutor", StopsBeforeTheLaunch)
    with pytest.raises(RuntimeError) as error:
        served_while_the_reference_runs(network, lambda: None)
    assert "RuntimeError: the reference never reached the executor" in "".join(
        traceback.format_exception(error.value)
    )


def test_the_reference_reaches_the_executor_as_a_cuda_run(network, monkeypatch):
    seen: list[str] = []

    class RecordsTheBackend(SlowExecutor):
        def __call__(self, kind: str, job: Any, directory: Path, context: Any) -> Any:
            def launch(job: Any, directory: Path, *, backend: str, **options: Any) -> Any:
                seen.append(backend)
                self.reached.set()
                raise IslandFailure("the fake executor stops at once")

            return launch

    monkeypatch.setitem(globals(), "SlowExecutor", RecordsTheBackend)
    served_while_the_reference_runs(network, lambda: None)
    assert seen == ["cuda"]
