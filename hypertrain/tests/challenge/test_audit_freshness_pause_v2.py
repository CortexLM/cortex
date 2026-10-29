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

import json
import threading
from pathlib import Path

import pytest

from hypertrain.challenge.exit_v2 import MAX_LAG_ROUNDS, RESULT_PAUSE_ROUNDS
from hypertrain.challenge.store import ChallengeStore

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


def deadline_of(network, job) -> int:
    """LeaseGuard's wall instant: the lease's real deadline."""
    genesis = network.manifest.training.beacon.genesis_time
    return genesis + (min(job["lease_expires"], job["absolute_deadline"]) - 1) * 3


def test_a_view_behind_by_the_whole_cap_still_settles_before_the_deadline(
    one_identity, monkeypatch
):
    # (a) The largest tolerated pause: the pusher stalled RESULT_PAUSE_ROUNDS more than the bound.
    network = one_identity
    job = leased_job(network, monkeypatch, lag=MAX_LAG_ROUNDS + RESULT_PAUSE_ROUNDS)
    assert network.store.clock() < deadline_of(network, job)
    answer = post_execute(network, job)
    assert answer.status_code == 200, (answer.status_code, answer.text)
    assert answer.json()["verdict"]["result"] == "MATCH"


def test_a_view_one_round_past_the_cap_is_a_real_outage_and_refused(one_identity, monkeypatch):
    # (c) Lag beyond the cap, deadline still ahead: a stale beacon, refused and paused as before.
    network = one_identity
    job = leased_job(network, monkeypatch, lag=MAX_LAG_ROUNDS + RESULT_PAUSE_ROUNDS + 1)
    now = network.store._now(network.store._db)
    assert network.store.clock() < deadline_of(network, job)
    answer = post_execute(network, job)
    assert (answer.status_code, answer.json()) == (503, {"detail": "beacon infrastructure paused"})
    assert records(network, "executed-verdict") == {}
    # As before the fix: the refusal rolls the result transaction back, its pin included.
    pin = network.store._db.execute(
        "SELECT 1 FROM beacon_pins_v2 WHERE operation_id=?",
        (f"audit-result:{job['job_id']}:{now}",),
    ).fetchone()
    assert pin is None


@pytest.mark.parametrize(("offset", "status"), [(-1, 200), (0, 503)])
def test_a_result_at_the_lease_real_deadline_is_refused(one_identity, monkeypatch, offset, status):
    # (b) Lag within the cap, but the wall clock reached the lease's real deadline: refused.
    network = one_identity
    job = leased_job(network, monkeypatch, lag=0)
    expires = min(job["lease_expires"], job["absolute_deadline"])
    network.push(expires - 10)
    from hypertrain.auditor import worker

    replay = worker.execute_island_audit

    def late(*args, **kwargs):
        answer = replay(*args, **kwargs)
        network.store.clock = lambda: deadline_of(network, job) + offset
        return answer

    monkeypatch.setattr(worker, "execute_island_audit", late)
    answer = post_execute(network, job)
    genesis = network.manifest.training.beacon.genesis_time
    lag = (int(network.store.clock()) - genesis) // 3 + 1 - network.store._now(network.store._db)
    assert MAX_LAG_ROUNDS < lag <= MAX_LAG_ROUNDS + RESULT_PAUSE_ROUNDS
    assert answer.status_code == status, answer.text
    assert (records(network, "executed-verdict") == {}) is (status == 503)


def test_a_lease_and_the_replay_inputs_still_need_a_fresh_view(one_identity, monkeypatch):
    # The tolerance is the result's only: a lease, or the start of a replay, on a lagging view is
    # refused, however early in the lease.
    network = one_identity
    job = leased_job(network, monkeypatch, lag=0)
    now = network.store._now(network.store._db)
    wall_at(network, now + LAG)
    refused = post_execute(network, job)
    assert (refused.status_code, refused.json()) == (
        503,
        {"detail": "beacon infrastructure paused"},
    )
    assert records(network, "executed-verdict") == {}
    from .audit_lease_fixture_v2 import AUDITOR_B

    asked = lease(network, AUDITOR_B, 2)
    assert (asked.status_code, asked.json()) == (503, {"detail": "beacon infrastructure paused"})


def test_a_settled_result_cannot_be_replayed(one_identity, monkeypatch):
    # The same signed receipt and verdict again, once the lagging result settled and the pusher
    # caught up: /execute is refused, /complete answers the settled result and changes nothing.
    network = one_identity
    store = network.store
    job = leased_job(network, monkeypatch, lag=LAG)
    first = post_execute(network, job)
    assert first.status_code == 200, first.text
    path = f"/worker/jobs/{job['job_id']}/complete"
    verdict = first.json()["verdict"]
    done = network.client.post(network.url + path, json=network.signed(A, "ReplayVerdict", verdict))
    assert done.status_code == 200, done.text

    def state():
        lease_row = store._db.execute(
            "SELECT * FROM audit_leases_v2 WHERE id=?", (job["job_id"],)
        ).fetchone()
        return tuple(lease_row), records(network, "replay"), records(network, "executed-verdict")

    settled = state()
    network.push(store._now(store._db) + LAG)
    again = post_execute(network, job)
    replay = network.client.post(
        network.url + path, json=network.signed(A, "ReplayVerdict", verdict)
    )
    assert (again.status_code, replay.status_code) == (403, 200), (again.text, replay.text)
    assert replay.json() == {"result": "MATCH"}
    assert state() == settled


def test_a_lease_left_by_the_old_code_is_not_settled_by_the_new_one(one_identity, monkeypatch):
    # A lease the deployed code refused (REQUESTED marker, STALE pin) before the server swap: the
    # new code does not turn that refusal into a verdict, the lease runs out as it would have.
    network = one_identity
    job = leased_job(network, monkeypatch, lag=LAG)
    now = network.store._now(network.store._db)
    with monkeypatch.context() as old:
        old.setattr(ChallengeStore, "_result_in_lease_v2", lambda *args: False)
        refused = post_execute(network, job)
    assert refused.status_code == 503, refused.text
    marker = f"{job['job_id']}:{job['lease_nonce']}"
    assert records(network, "audit-execution")[marker]["state"] == "REQUESTED"
    network.push(now + LAG)
    again = post_execute(network, job)
    assert again.status_code == 409, again.text
    assert records(network, "executed-verdict") == {}
    assert records(network, "audit-execution")[marker]["state"] == "REQUESTED"


def test_the_replay_runs_without_the_store_lock_and_its_result_pin_is_unpaused(
    one_identity, monkeypatch
):
    # Lens (4): the pusher can land while the replay runs, the result pin is written unpaused.
    network = one_identity
    store = network.store
    from hypertrain.auditor import worker

    held = []
    real = worker.execute_island_audit

    def probe(*args, **kwargs):
        free = threading.Event()

        def take():
            if store._lock.acquire(timeout=5):
                store._lock.release()
                free.set()

        other = threading.Thread(target=take)
        other.start()
        other.join()
        held.append(free.is_set())
        return real(*args, **kwargs)

    monkeypatch.setattr(worker, "execute_island_audit", probe)
    job = leased_job(network, monkeypatch, lag=LAG)
    now = store._now(store._db)
    answer = post_execute(network, job)
    assert answer.status_code == 200, answer.text
    assert held == [True]
    pin = store._db.execute(
        "SELECT paused,reason,signature_hash FROM beacon_pins_v2 WHERE operation_id=?",
        (f"audit-result:{job['job_id']}:{now}",),
    ).fetchone()
    assert (pin[0], pin[1]) == (0, None) and pin[2] is not None


QUICKNET = json.loads((Path(__file__).parents[1] / "beacon/vectors.json").read_text())["rounds"]


@pytest.fixture
def production(tmp_path, monkeypatch):
    """A production-ledger store holding one real drand quicknet round and one running lease."""
    from .test_production_economics_v2 import hosted

    store, run = hosted(tmp_path, monkeypatch, cuda=False)
    vector = QUICKNET[1]
    store._db.execute(
        "INSERT INTO beacon VALUES(?,?,?,1)",
        (vector["round"], vector["signature"], vector["randomness"]),
    )
    store._db.execute(
        "INSERT INTO audit_leases_v2(id,run_id,state,expires,absolute) "
        "VALUES('job',?,'running',?,?)",
        (run, vector["round"] + 100, vector["round"] + 200),
    )
    yield store, store._run_v2(run), vector["round"]
    store.close_v2_notifications()
    store.close()


def production_wall(store, manifest, rnd):
    store.clock = lambda: manifest.training.beacon.genesis_time + (rnd - 1) * 3


def production_pin(store, operation):
    row = store._db.execute(
        "SELECT paused FROM beacon_pins_v2 WHERE operation_id=?", (operation,)
    ).fetchone()
    return row[0]


@pytest.mark.parametrize("kind", ["audit-result", "complete"])
def test_production_takes_a_lagging_result_on_its_bls_verified_pinned_round(production, kind):
    # (d) Lens (3): the production branch tolerates the same pause, the health round being the
    # pinned round itself, re-verified against the drand quicknet key.
    store, manifest, now = production
    production_wall(store, manifest, now + MAX_LAG_ROUNDS + RESULT_PAUSE_ROUNDS)
    assert store._fresh_v2(manifest, f"{kind}:job:{now}") == now
    assert production_pin(store, f"{kind}:job:{now}") == 0


@pytest.mark.parametrize(
    "fault",
    [
        "forged signature",
        "fixture signature",
        "past the cap",
        "at the deadline",
        "lease",
        "replay inputs",
        "no lease",
        "settled lease",
        "another run's lease",
    ],
)
def test_production_refuses_a_lagging_result_without_its_conditions(production, fault):
    # (d) The pinned round with a bad or a non-BLS signature, or any other condition missing.
    from hypertrain.challenge.store import ChallengeError

    store, manifest, now = production
    operation, lag = f"audit-result:job:{now}", LAG
    if fault == "forged signature":
        signature = QUICKNET[1]["signature"]
        forged = signature[:-2] + ("00" if signature[-2:] != "00" else "01")
        store._db.execute("UPDATE beacon SET signature=? WHERE round=?", (forged, now))
    elif fault == "fixture signature":
        store._db.execute("UPDATE beacon SET signature=?,bls=0 WHERE round=?", ("aa" * 48, now))
    elif fault == "past the cap":
        lag = MAX_LAG_ROUNDS + RESULT_PAUSE_ROUNDS + 1
    elif fault == "lease":
        operation = f"lease:{now}"
    elif fault == "replay inputs":
        operation = f"audit-execute:job:{now}"
    elif fault == "no lease":
        store._db.execute("DELETE FROM audit_leases_v2")
    elif fault == "settled lease":
        store._db.execute("UPDATE audit_leases_v2 SET state='completed'")
    elif fault == "another run's lease":
        store._db.execute("UPDATE audit_leases_v2 SET run_id=?", ("f" * 64,))
    production_wall(store, manifest, now + lag)
    if fault == "at the deadline":  # inside the cap: only the lease's own expiry refuses it
        store._db.execute("UPDATE audit_leases_v2 SET expires=?", (now + LAG + 1,))
        production_wall(store, manifest, now + LAG + 1)
    with pytest.raises(ChallengeError) as refused:
        store._fresh_v2(manifest, operation)
    assert (refused.value.status, refused.value.detail) == (
        503,
        "beacon infrastructure paused; pinned seed retained",
    )
    assert production_pin(store, operation) == 1


def test_the_same_signed_complete_settles_once_the_pusher_catches_up(one_identity, monkeypatch):
    # A signed /complete re-posted after a 503 settles: the refusal rolled its nonce back.
    network = one_identity
    store = network.store
    job = leased_job(network, monkeypatch, lag=0)
    answer = post_execute(network, job)
    assert answer.status_code == 200, answer.text
    now = store._now(store._db)
    wall_at(network, now + MAX_LAG_ROUNDS + RESULT_PAUSE_ROUNDS + 1)
    path = f"/worker/jobs/{job['job_id']}/complete"
    signed = network.signed(A, "ReplayVerdict", answer.json()["verdict"])
    paused = network.client.post(network.url + path, json=signed)
    assert paused.status_code == 503, paused.text
    network.push(now + 2)
    done = network.client.post(network.url + path, json=signed)
    assert (done.status_code, done.json()) == (200, {"result": "MATCH"}), done.text
