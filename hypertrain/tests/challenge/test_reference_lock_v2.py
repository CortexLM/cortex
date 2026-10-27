"""A remote reference must not hold the challenge's store lock (docs/adr/0012 section 2, Part B).

`trial_reference_v2` takes `self._lock` and keeps it through the whole launch, so while a remote
executor works nobody else is served, the beacon pusher included: its requests time out and the
freshness gate closes. The tests below state the required behaviour with a fake slow executor.
They are strict xfails: they fail today, and whoever releases the lock must remove the marker (a
strict xfail that starts to pass fails the suite).
"""

from __future__ import annotations

import importlib.util
import sys
import threading
from pathlib import Path
from typing import Any

import pytest

from hypertrain.miner.island_launch import IslandFailure
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages_v2 import EscrowLock

spec = importlib.util.spec_from_file_location(
    "reference_lock_backend_fixture", Path(__file__).parent / "test_backend_authority_v2.py"
)
assert spec is not None and spec.loader is not None
backend = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = backend
spec.loader.exec_module(backend)
network = backend.network

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


def pending_reference(network: Any) -> str:
    """An admission with an accepted challenge, whose reference is the next thing to run."""
    assert network.join().status_code == 200
    _, admission, _ = network.store._services(network.manifest.run_id())
    row = network.store._db.execute("SELECT * FROM admissions_v2").fetchone()
    lock = EscrowLock(
        operation_id="fc" * 32,
        owner=backend.service.COLD[0].ss58,
        units=1000,
        origin_ids=[network.origin_ids[0]],
        admission_id=row["admission_id"],
        dispute_id=None,
        kind="LOCK_ADMISSION",
    )
    signed = network.signed(backend.service.COLD[0], "EscrowLock", lock)
    admission.lock(canonicalize(signed), now=network.now)
    network.push(network.now + 1)
    admission.challenge(row["admission_id"], now=network.now)
    admission.backend = "cuda"  # the reference goes to the role launch, as for a CUDA run
    return str(row["admission_id"])


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


XFAIL = pytest.mark.xfail(
    strict=True,
    raises=AssertionError,  # only "the probe was still waiting"; a broken setup raises RuntimeError
    reason="Part B of ADR 0012: trial_reference_v2 holds the store lock through the remote launch",
)


@XFAIL
def test_the_latest_beacon_is_served_while_a_remote_reference_runs(network):
    assert served_while_the_reference_runs(network, network.store.latest_beacon)


@XFAIL
def test_a_beacon_round_is_accepted_while_a_remote_reference_runs(network):
    from tests.challenge.test_service_network_v2 import fixture_beacon

    payload = fixture_beacon(network.now + 2)
    assert served_while_the_reference_runs(network, lambda: network.store.push_beacon(payload))
