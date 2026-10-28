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
import sys
import threading
import time
import traceback
from pathlib import Path
from typing import Any

import pytest

from hypertrain.auditor.replay import AuditInputError
from hypertrain.challenge import store as store_module
from hypertrain.challenge.admission_store import AdmissionError
from hypertrain.challenge.store import ChallengeError
from hypertrain.data.trial_assignment import trial_samples
from hypertrain.gpu_ops.work_screen import WorkScreenError
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

    worker = threading.Thread(target=reference, daemon=True)
    asker = threading.Thread(target=probe, daemon=True)
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
    if not outcome or not isinstance(outcome[0], IslandFailure):
        raise RuntimeError(f"the reference did not end in the fake executor's failure: {outcome}")
    return served


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
            threading.Thread(target=store.export_trial_v2, args=(run_id, second)),
            threading.Thread(
                target=store.admission_v2,
                args=(run_id, "status", b"", backend.service.HOT[1].ss58),
            ),
        ]
        for request in requests:
            request.start()
        for request in requests:
            request.join(3)
        assert not any(request.is_alive() for request in requests)
        # the other trial's own reference runs to its commitment while the first is blocked
        assert store.trial_reference_v2(run_id, second)["reference_commitment"]
        assert not blocked.done and len(executor.launches) == 2
    finally:
        executor.release.set()
    blocked.result()
    assert committed(hosted, first) and committed(hosted, second)


def test_a_five_second_reference_holds_the_store_lock_for_less_than_a_second(hosted):
    run_id, store, executor = hosted.manifest.run_id(), hosted.store, Executor(delay=5.0)
    store._role_launch = executor
    admission_id = pending_reference(hosted)
    waits: list[float] = []
    stop = threading.Event()

    def contender() -> None:
        while not stop.is_set():
            started = time.perf_counter()
            assert store._lock.acquire(timeout=WAIT)
            store._lock.release()
            waits.append(time.perf_counter() - started)
            time.sleep(0.02)

    watcher = threading.Thread(target=contender, daemon=True)
    watcher.start()
    began = time.perf_counter()
    try:
        value = store.trial_reference_v2(run_id, admission_id)
    finally:
        stop.set()
        watcher.join(WAIT)
    assert time.perf_counter() - began >= 5.0  # the fake executor really took its five seconds
    assert len(waits) > 100  # the lock was asked for all along, not only at the ends
    assert max(waits) < 1.0, f"the lock was unavailable for {max(waits):.2f} s"
    assert value["reference_commitment"] == row_of(hosted, admission_id)["reference_commitment"]


def test_two_calls_for_one_trial_launch_once_and_return_one_commitment(hosted):
    run_id, store, executor = hosted.manifest.run_id(), hosted.store, Executor()
    store._role_launch = executor
    admission_id = pending_reference(hosted)
    first = Call(store, run_id, admission_id)
    try:
        assert executor.reached.wait(WAIT)
        second = Call(store, run_id, admission_id)
        until(lambda: waiting(store) == 1)
        time.sleep(0.5)  # long enough for a second launch, if there were one, to start
        assert not second.done and len(executor.launches) == 1
    finally:
        executor.release.set()
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


def test_a_launch_that_dies_between_claim_and_settle_leaves_nothing_durable_and_a_retry_works(
    hosted, monkeypatch
):
    monkeypatch.setattr(store_module, "REFERENCE_WAIT_SECONDS", 3.0)
    run_id, store = hosted.manifest.run_id(), hosted.store
    executor = Executor(error=SystemExit("the worker was killed"))
    store._role_launch = executor
    admission_id = pending_reference(hosted)
    before = [tuple(r) for r in store._db.execute("SELECT * FROM admission_trial_results")]
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
    assert [tuple(r) for r in store._db.execute("SELECT * FROM admission_trial_results")] == before
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
    # by the screening of its result. Either way the waiter gets the same failure.
    expected = IslandFailure if honour_cancel else WorkScreenError
    with pytest.raises(expected) as stopped:
        first.result()
    with pytest.raises(expected) as alike:
        second.result()
    assert alike.value is stopped.value
    assert not committed(hosted, admission_id)
    assert store._reference_flights_v2 == {} and store._lease_guards_v2 == {}
    assert row_of(hosted, admission_id)["clean_count"] == 0


def test_a_lease_cancelled_while_the_result_waits_for_the_lock_is_not_committed(
    hosted, monkeypatch
):
    run_id, store, executor = hosted.manifest.run_id(), hosted.store, Executor(delay=0)
    store._role_launch = executor
    admission_id = pending_reference(hosted)
    key = f"reference:{admission_id}:{challenge_of(hosted, admission_id).nonce}"
    fresh = store._fresh_v2

    def cancels_at_the_settlement(manifest: Any, operation: str, rnd: int | None = None) -> int:
        now = fresh(manifest, operation, rnd)
        if operation.startswith("reference-settle:"):
            store._lease_guards_v2[key].revoke()  # the lease ends after the result is in
        return now

    monkeypatch.setattr(store, "_fresh_v2", cancels_at_the_settlement)
    with pytest.raises(AuditInputError, match="expired or revoked"):
        store.trial_reference_v2(run_id, admission_id)
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
        assert second.result(15)["reference_commitment"]  # not behind the stale flight
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


def test_staging_survives_a_write_that_died_halfway(hosted, monkeypatch):
    run_id, store = hosted.manifest.run_id(), hosted.store
    admission_id = pending_reference(hosted)
    challenge = challenge_of(hosted, admission_id)
    epoch = row_of(hosted, admission_id)["trial_epoch"]
    samples = trial_samples(
        hosted.manifest, admission_id, challenge.nonce, store._beacon_v2(challenge.seed_beacon)
    )
    real = Path.write_bytes

    def dies_halfway(self: Path, data: bytes) -> int:
        real(self, data[: len(data) // 2])
        raise OSError("disk full")

    monkeypatch.setattr(Path, "write_bytes", dies_halfway)
    with pytest.raises(OSError, match="disk full"):
        store.stage_trial_v2(run_id, challenge, samples, epoch)
    monkeypatch.setattr(Path, "write_bytes", real)
    # the half-written file is not left under the name of an input: the next staging works
    job, directory = store.stage_trial_v2(run_id, challenge, samples, epoch)
    assert (directory / "start_state").read_bytes() and job.sample_ids == list(samples)


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
