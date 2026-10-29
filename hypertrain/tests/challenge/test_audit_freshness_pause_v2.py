"""A beacon view that falls behind while the master replays an audit must not lose the verdict.

S17d (run f3901a09, rounds 0 and 1): the replay took about 172 s; while it ran the store lock was
held by the audit's own transactions and the pusher's posts landed late, so when the result
transaction opened, the master's newest verified round was 3 behind the emitted one (13:22:31Z:
33492259 against 33492262). `_fresh_v2` refused it with 503 "beacon infrastructure paused", the
verdict was never stored, the auditor's next leases were refused (503, then 409 "one live lease
per auditor") and the lease expired as EXECUTOR_INTERRUPTED. One activated identity, one seeded
round, the master's own CPU replay; the clock is moved, the beacon is not pushed.
"""
# ruff: noqa: F811  (the `network` fixture parameter shadows the imported fixture, as in test_multirun.py)

from __future__ import annotations

import pytest

from hypertrain.challenge.exit_v2 import MAX_LAG_ROUNDS

from . import test_service_network_v2 as fixtures
from .audit_lease_fixture_v2 import (
    AUDITOR_A,
    lease,
    miners,
    network,  # noqa: F401  (pytest fixture import)
    schedule,
    seed_round,
)
from .five_identity_network_v2 import activate_identities
from .test_audit_reaudit_v2 import genuine, receipt, records

A = AUDITOR_A
LAG = MAX_LAG_ROUNDS + 1  # the S17d view: one round past the bound


@pytest.fixture
def one_identity(network):
    activate_identities(network, fixtures, range(1))
    return network


def wall_at(network, number):
    """Set the master's wall clock to the emission time of round `number`, pushing nothing."""
    network.store.clock = lambda: network.manifest.training.beacon.genesis_time + (number - 1) * 3


def leased_job(network, monkeypatch, *, lag):
    """Lease the seeded job to A; its replay ends with the wall clock `lag` rounds past the
    master's newest verified beacon, as the S17d pusher left it."""
    from hypertrain.auditor import worker

    store = network.store
    key = miners(1)[0]
    admission = store._db.execute(
        "SELECT admission_id FROM admissions_v2 WHERE hotkey=?", (key.ss58,)
    ).fetchone()[0]
    seed_round(network, n=1, executable=True, gap=100, admission_ids={0: admission})
    genuine(network, key)
    real = worker.execute_island_audit

    def replay(*args, **kwargs):
        answer = real(*args, **kwargs)
        wall_at(network, store._now(store._db) + lag)
        return answer

    monkeypatch.setattr(worker, "execute_island_audit", replay)
    schedule(network)
    leased = lease(network, A, 1)
    assert leased.status_code == 200, leased.text
    return leased.json()["body"]


def post_execute(network, job):
    path = f"/worker/jobs/{job['job_id']}/execute"
    return network.client.post(network.url + path, json=receipt(network, A, job["lease_nonce"]))


@pytest.mark.xfail(
    strict=True, reason="store.py:2438 refuses the result: 503 beacon infrastructure paused"
)
def test_a_beacon_view_behind_at_the_result_still_settles_the_lease(one_identity, monkeypatch):
    network = one_identity
    store, run = network.store, network.manifest.run_id()
    job = leased_job(network, monkeypatch, lag=LAG)
    now = store._now(store._db)
    answer = post_execute(network, job)
    genesis = network.manifest.training.beacon.genesis_time
    emitted = (int(store.clock()) - genesis) // 3 + 1
    # The view the result transaction saw: `now` verified, `emitted` on the wall, bound 2.
    assert (emitted - now, MAX_LAG_ROUNDS) == (LAG, 2)
    # The replay finished well inside the lease: its real deadline is far ahead.
    deadline = genesis + (min(job["lease_expires"], job["absolute_deadline"]) - 1) * 3
    assert store.clock() < deadline
    assert answer.status_code == 200, (answer.status_code, answer.text)
    marker = records(network, "audit-execution")[f"{job['job_id']}:{job['lease_nonce']}"]
    assert marker["state"] == "EXECUTED"
    assert records(network, "audit-failure-latch") == {}
    pin = store._db.execute(
        "SELECT paused,reason FROM beacon_pins_v2 WHERE run_id=? AND operation_id=?",
        (run, f"audit-result:{job['job_id']}:{now}"),
    ).fetchone()
    assert tuple(pin) == (0, None)
    # The auditor signs at once, the pusher still behind: the lease settles as the replay said.
    verdict = answer.json()["verdict"]
    path = f"/worker/jobs/{job['job_id']}/complete"
    done = network.client.post(network.url + path, json=network.signed(A, "ReplayVerdict", verdict))
    assert done.status_code == 200, (done.status_code, done.text)
    assert done.json()["result"] == verdict["result"] == "MATCH"
    assert records(network, "audit-attempt-failure") == {}
