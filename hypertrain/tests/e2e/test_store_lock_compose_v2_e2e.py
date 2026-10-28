"""An aggregate and a remote reference work at once on one store; the beacon pusher is served.

Two long calls used to hold the store's single RLock for the whole of their work: `aggregate_v2`
(the tape) and `trial_reference_v2` (the remote executor). Each is now a claim under the lock, the
work without it and a settlement under it again, and test_aggregate_lock_v2_e2e.py and
tests/challenge/test_reference_lock_v2.py state that for each alone. Nothing there says the two
compose: that one store, one lock, serves the pusher (`push_beacon`, `latest_beacon`) while BOTH are
mid-work, and that each still settles correctly once the other's rounds have moved the beacon.

The scenario is built once (a verified round 0 of five members and a sixth identity that joined
after it and holds an accepted challenge, see `build_scenario`) and restored per test. The tape is
held inside the REAL `tape_v2.compute_body`; the reference is held inside the executor, which then
finishes the REAL reference on the CPU. Every wait has a timeout and everything held is released in
the fixture's teardown: a broken implementation fails a test, it never hangs the suite.
"""

from __future__ import annotations

import shutil
import sqlite3
import threading
import time
from collections.abc import Callable, Iterator
from contextlib import closing
from dataclasses import dataclass
from functools import partial
from pathlib import Path
from typing import Any

import pytest
import test_rollback_service_v2 as rollback
from aggregate_lock_fixture_v2 import Restored, Template

from challenge import five_identity_network_v2 as five
from hypertrain.aggregator import tape_v2
from hypertrain.miner.island_launch import launch_island
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.messages_v2 import EscrowLock, JoinChallenge

fixtures = rollback.fixtures

MEMBERS = 5  # ACTIVE identities in round 0; one more identity joins after it
WAIT = 60.0  # seconds anything may take after the release before the test calls itself broken
REACH = 10.0  # seconds each long phase is given to get to the point where it is held
HELD = 2.5  # seconds both phases are held, at once, while the pusher runs against them
PACE = 0.1  # seconds between two iterations of the pusher
ROUNDS = 60  # beacon rounds the pusher delivers; far below the trial's 100-round window
MAX_SECONDS = 1.0  # the longest the pusher may wait for a call, the longest the lock may be held
MIN_PUSHES = 10  # rounds the pusher must have delivered while both phases were held


@dataclass(frozen=True)
class Scenario:
    template: Template
    admission_id: str  # the sixth identity: an accepted challenge, its reference not yet run


def build_scenario(tmp_path_factory: pytest.TempPathFactory) -> Scenario:
    """Body of a module-scoped fixture: round 0 of five members past its audits, not aggregated,
    plus a sixth identity that joined after it, is funded and holds an accepted challenge.

    The five-identity network is built for six keys (six coldkeys in the genesis) but activates
    five: the sixth joins through the real routes, as an applicant does, once round 0 is done.
    """
    activate = five.activate_identities
    with pytest.MonkeyPatch.context() as patch:
        patch.setattr(
            five,
            "activate_identities",
            lambda network, fx, indexes, **kw: activate(network, fx, range(MEMBERS), **kw),
        )
        generator = five.five_identity_network(tmp_path_factory, fixtures, size=MEMBERS + 1)
        network = next(generator)
    try:
        rollback.execute_round(network, 0, aggregate=False, hotkeys=fixtures.HOT[:MEMBERS])
        admission_id = apply_and_open_trial(network, MEMBERS)
        return Scenario(snapshot_template(network, tmp_path_factory), admission_id)
    finally:
        generator.close()


def snapshot_template(network: Any, tmp_path_factory: pytest.TempPathFactory) -> Template:
    """The state of `network`'s store, copied as aggregate_lock_fixture_v2.build_template does."""
    store = network.store
    snapshot = tmp_path_factory.mktemp("template") / "state"
    # server_keys.db belongs to the HTTP app, which a restored bare store does not have.
    shutil.copytree(
        store.state_dir,
        snapshot,
        ignore=shutil.ignore_patterns("*.sock", "challenge.db*", "server_keys.db*"),
    )
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


def apply_and_open_trial(network: Any, index: int) -> str:
    """Join `fixtures.HOT[index]`, fund it, and ask for its trial; return the admission id."""
    response = network.join(index)
    assert response.status_code == 200, response.text
    admission_id = str(response.json()["admission_id"])
    lock = EscrowLock(
        operation_id=sha256_hex(b"compose-applicant-lock"),
        owner=fixtures.COLD[index].ss58,
        units=1000,
        origin_ids=[network.origin_ids[index]],
        admission_id=admission_id,
        dispute_id=None,
        kind="LOCK_ADMISSION",
    )
    response = network.client.post(
        network.url + "/escrow/lock",
        json=network.signed(fixtures.COLD[index], "EscrowLock", lock),
    )
    assert response.status_code == 200, response.text
    network.push(network.now + 1)
    response = network.challenge(admission_id)
    assert response.status_code == 200, response.text
    return admission_id


@pytest.fixture(scope="module")
def scenario(tmp_path_factory: pytest.TempPathFactory) -> Scenario:
    return build_scenario(tmp_path_factory)


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


class HeldTape:
    """The real `tape_v2.compute_body`, whose first call waits for `release` once it has computed.

    make_tape and replay_tape both look compute_body up as a module global, so the aggregate is
    held inside its computation, with every byte computed by production code.
    """

    def __init__(self, monkeypatch: pytest.MonkeyPatch) -> None:
        self.reached, self.release = threading.Event(), threading.Event()
        self.reached_at = 0.0
        real = tape_v2.compute_body
        calls: list[str] = []

        def held(*args: Any, **kwargs: Any) -> Any:
            out = real(*args, **kwargs)
            calls.append(threading.current_thread().name)
            if len(calls) == 1:
                self.reached_at = time.perf_counter()
                self.reached.set()
                self.release.wait(WAIT)
            return out

        monkeypatch.setattr(tape_v2, "compute_body", held)


class HeldExecutor:
    """The role launch of a remote executor that is still working: the first launch says it was
    reached and waits for `release`, then runs the real reference on the CPU."""

    def __init__(self) -> None:
        self.reached, self.release = threading.Event(), threading.Event()
        self.reached_at = 0.0

    def __call__(self, kind: str, job: Any, directory: Path, context: Any) -> Any:
        def launch(
            job: Any, directory: Path, *, backend: str, cancel: Any = None, trace: bool = False
        ) -> Any:
            self.reached_at = time.perf_counter()
            self.reached.set()
            self.release.wait(WAIT)
            return launch_island(job, directory, backend="cpu", cancel=cancel)

        return launch


@dataclass
class Timed:
    """One call of the pusher: when it began and, once it returned, for how long."""

    name: str
    started: float
    seconds: float | None = None


class Pusher:
    """The beacon pusher: one round after another and the latest round asked for, each timed.

    `Restored.push` moves the clock to the round and hands it to `push_beacon`, as the pusher's
    HTTP request does. A call still waiting counts for as long as it has waited so far.
    """

    def __init__(self, restored: Restored) -> None:
        self.restored = restored
        self.first = restored.now
        self.calls: list[Timed] = []
        self.accepted: list[tuple[int, float]] = []  # round, when the store answered
        self.error: BaseException | None = None
        self.stopped = threading.Event()
        self.thread = threading.Thread(target=self._run, name="pusher", daemon=True)

    def _timed(self, name: str, call: Callable[[], Any]) -> Any:
        entry = Timed(name, time.perf_counter())
        self.calls.append(entry)
        out = call()
        entry.seconds = time.perf_counter() - entry.started
        return out

    def _run(self) -> None:
        try:
            while not self.stopped.is_set():
                if len(self.accepted) < ROUNDS:
                    number = self.first + len(self.accepted) + 1
                    answer = self._timed("push_beacon", partial(self.restored.push, number))
                    assert answer["round"] == number, f"round {number} was answered {answer}"
                    self.accepted.append((number, time.perf_counter()))
                self._timed("latest_beacon", self.restored.store.latest_beacon)
                self.stopped.wait(PACE)
        except BaseException as error:  # handed to the test, which reports it
            self.error = error

    def slowest(self) -> tuple[str, float]:
        """The call that waited longest, finished or not."""
        now = time.perf_counter()
        return max(
            ((c.name, now - c.started if c.seconds is None else c.seconds) for c in self.calls),
            key=lambda item: item[1],
        )

    def pushed_between(self, begin: float, end: float) -> int:
        return sum(begin <= answered <= end for _, answered in self.accepted)


class Compose:
    """One restored store and the actors of the test: the aggregate, the reference, the pusher."""

    def __init__(
        self, restored: Restored, admission_id: str, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        self.restored, self.admission_id = restored, admission_id
        self.tape, self.executor = HeldTape(monkeypatch), HeldExecutor()
        store = restored.store
        store._role_launch = self.executor
        store._services(restored.run)[1].backend = "cuda"  # the reference goes to the role launch
        self.pusher = Pusher(restored)
        self.calls: list[Call] = []

    def aggregate(self) -> Call:
        store, run = self.restored.store, self.restored.run
        self.calls.append(Call("aggregate_v2", lambda: store.aggregate_v2(run, 0)))
        return self.calls[-1]

    def reference(self) -> Call:
        store, run, admission = self.restored.store, self.restored.run, self.admission_id
        self.calls.append(
            Call("trial_reference_v2", lambda: store.trial_reference_v2(run, admission))
        )
        return self.calls[-1]

    def release(self) -> None:
        """Let both held phases go."""
        self.tape.release.set()
        self.executor.release.set()

    def finish(self) -> list[str]:
        """Release everything, stop the pusher, join every thread; name those still running."""
        self.release()
        self.pusher.stopped.set()
        threads = [call.thread for call in self.calls] + [self.pusher.thread]
        for thread in threads:
            if thread.ident is not None:
                thread.join(WAIT)
        return [thread.name for thread in threads if thread.is_alive()]


@pytest.fixture
def compose(
    scenario: Scenario, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> Iterator[Compose]:
    restored = Restored(scenario.template, tmp_path / "state")
    scene = Compose(restored, scenario.admission_id, monkeypatch)
    try:
        yield scene
    finally:
        stuck = scene.finish()
        restored.close()
    assert not stuck, f"threads still running after everything was released: {stuck}"


def row(restored: Restored, admission_id: str) -> Any:
    with restored.store._lock:
        return restored.store._db.execute(
            "SELECT * FROM admissions_v2 WHERE admission_id=?", (admission_id,)
        ).fetchone()


@pytest.mark.parametrize("first", ["aggregate", "reference"])
def test_the_pusher_is_served_while_an_aggregate_and_a_remote_reference_both_work(
    compose: Compose, first: str
) -> None:
    # Given: the applicant's trial outlives every round the pusher will deliver, and the wall-clock
    # cutoff of the reference's lease (genesis plus three seconds a round) is not close
    restored, store = compose.restored, compose.restored.store
    challenge = JoinChallenge.model_validate_json(row(restored, compose.admission_id)["challenge"])
    assert challenge.deadline_beacon - restored.now > ROUNDS + 10, "the trial would close under it"
    cutoff = restored.manifest.training.beacon.genesis_time + (challenge.deadline_beacon - 1) * 3
    assert cutoff - time.time() > 60, "the lease of the reference is about to end by the wall clock"
    begun = time.perf_counter()
    compose.pusher.thread.start()
    # When: both long phases are started, one after the other, and each is held at its work
    starts = {"aggregate": compose.aggregate, "reference": compose.reference}
    hooks = {"aggregate": compose.tape.reached, "reference": compose.executor.reached}
    order = [first, "reference" if first == "aggregate" else "aggregate"]
    started: list[Call] = []
    reached: dict[str, bool] = {}
    for kind in order:
        started.append(starts[kind]())
        reached[kind] = hooks[kind].wait(REACH)
        if not reached[kind]:
            break
    both = all(reached.get(kind) for kind in order)
    since = (
        max(compose.tape.reached_at, compose.executor.reached_at) if both else time.perf_counter()
    )
    if both:
        time.sleep(HELD)
    still_running = [call.name for call in started if not call.ended.is_set()]
    pushed_during = compose.pusher.pushed_between(since, time.perf_counter())
    compose.release()
    for call in started:
        call.ended.wait(WAIT)
    compose.pusher.stopped.set()
    compose.pusher.thread.join(WAIT)
    slowest = compose.pusher.slowest()  # every call, the settlements' too
    # Then: the pusher was never kept waiting, so the lock was not held through either long phase
    assert compose.pusher.error is None, f"the pusher failed: {compose.pusher.error!r}"
    assert slowest[1] < MAX_SECONDS, (
        f"the beacon pusher waited {slowest[1]:.2f} s for one {slowest[0]} "
        f"(limit {MAX_SECONDS:.1f} s) while an aggregate and a reference worked; "
        f"phases that reached their work: {reached}"
    )
    # and the two phases really overlapped, for as long as the test says
    assert both and still_running == [c.name for c in started], (
        f"the two phases were not in flight at once: reached {reached}, running {still_running}"
    )
    assert pushed_during >= MIN_PUSHES, f"only {pushed_during} rounds were delivered meanwhile"
    holds = [hold for hold in restored.lock.holds if hold.start >= begun]
    longest = max(holds, key=lambda hold: hold.seconds)
    assert longest.seconds < MAX_SECONDS, (
        f"thread {longest.thread} held the store lock for {longest.seconds:.2f} s in one stretch "
        f"({' <- '.join(longest.where[:3])}); limit {MAX_SECONDS:.1f} s"
    )
    # and the beacon moved: every round was accepted and the latest one is the last
    assert [number for number, _ in compose.pusher.accepted] == list(
        range(compose.pusher.first + 1, compose.pusher.first + 1 + len(compose.pusher.accepted))
    )
    assert store.latest_beacon()["round"] == compose.pusher.accepted[-1][0] > compose.pusher.first
    # and once released, both settle correctly into the state the beacon reached meanwhile
    aggregate = next(call for call in started if call.name == "aggregate_v2")
    reference = next(call for call in started if call.name == "trial_reference_v2")
    assert aggregate.ended.is_set() and reference.ended.is_set(), "a call never returned"
    assert aggregate.error is None, f"the aggregate failed: {aggregate.error!r}"
    assert reference.error is None, f"the reference failed: {reference.error!r}"
    assert {"tape", "theta_hash", "out_state"} <= aggregate.result.keys()
    assert (
        aggregate.result["tape_hash"] == store._record_v2(restored.run, "applied", "0")["tape_hash"]
    )
    stored = row(restored, compose.admission_id)
    commitment = reference.result["reference_commitment"]
    assert commitment and commitment == stored["reference_commitment"]
    assert stored["reference_json"] is not None
