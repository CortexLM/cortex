"""Audit lease liveness: dating, head of line, single flight, cooldown, fail-fast, second window."""
# ruff: noqa: F811  (the `network` fixture parameter shadows the imported fixture, as in test_multirun.py)

from __future__ import annotations

import re

import pytest

from hypertrain.challenge.store import ChallengeError
from hypertrain.miner.island_launch import IslandFailure
from hypertrain.protocol.hashing import sha256_hex

from .audit_lease_fixture_v2 import (
    AUDITOR_A,
    AUDITOR_B,
    D_AUDIT,
    execute_request,
    lease,
    lease_rounds,
    miners,
    network,  # noqa: F401  (pytest fixture import)
    schedule,
    seed_round,
)

A, B = AUDITOR_A, AUDITOR_B


def lease_row(store, job_id):
    return store._db.execute("SELECT * FROM audit_leases_v2 WHERE id=?", (job_id,)).fetchone()


def test_a_re_lease_after_the_first_window_validates(network):
    seed_round(network)
    schedule(network)
    late = D_AUDIT + 1  # an auditor that polls one beacon round after the audit round
    network.push(late)
    response = lease(network, A, 1)
    assert response.status_code == 200, response.text
    body = response.json()["body"]
    assert (body["created_beacon"], body["lease_expires"]) == (late, late + lease_rounds(network))


def job_id(network, key):
    return sha256_hex(f"ht-audit-job/2|{network.manifest.run_id()}|0|{key.ss58}".encode())


def head_of_line(network, n=4):
    """Index of the miner whose job leases first (ORDER BY created,id)."""
    ids = [job_id(network, key) for key in miners(n)]
    return ids.index(min(ids))


@pytest.mark.parametrize("poison", ["commit_exp", "nan_leaf"])
def test_a_miner_sealed_input_that_cannot_be_leased_does_not_block_the_queue(network, poison):
    store, run = network.store, network.manifest.run_id()
    first = head_of_line(network)
    seed_round(network, **{poison: {first: D_AUDIT - 1 if poison == "commit_exp" else 0}})
    schedule(network)
    poisoned = miners(4)[first]
    for number, auditor in enumerate((A, B), 1):
        answer = lease(network, auditor, number)
        assert answer.status_code == 200, answer.text
        assert answer.json()["body"]["job_id"] != job_id(network, poisoned)
    row = lease_row(store, job_id(network, poisoned))
    assert (row["state"], row["result"]) == ("exhausted", "MINER_INPUT_REFUSED")
    assert store._record_v2(run, "excluded", f"0:{poisoned.ss58}")["reason"] == "UNVERIFIED"
    finality = store._record_v2(run, "finality", "0")
    assert (finality["disposition"], finality["unresolved_audits"]) == ("PENDING", 3)


def test_a_coordinator_side_refusal_is_not_attributed_to_the_miner(network, monkeypatch):
    """Mutant guard (green before and after): an except that is broader than the two miner causes
    would drop a miner for a fault of the coordinator."""
    from hypertrain.protocol.messages_v2 import AuditJobV2

    def refuse(self, now_round):
        raise ValueError("coordinator-side refusal")

    seed_round(network)
    schedule(network)
    monkeypatch.setattr(AuditJobV2, "validate_embedded", refuse)
    assert lease(network, A, 1).status_code == 409
    queued = network.store._db.execute(
        "SELECT COUNT(*) FROM audit_leases_v2 WHERE state='queued'"
    ).fetchone()[0]
    excluded = network.store._db.execute(
        "SELECT COUNT(*) FROM records_v2 WHERE kind='excluded'"
    ).fetchone()[0]
    assert (queued, excluded) == (4, 0)


def test_a_second_execution_of_one_lease_is_refused_and_keeps_the_running_guard(
    network, monkeypatch
):
    from hypertrain.auditor import worker

    store, run = network.store, network.manifest.run_id()
    seed_round(network, executable=True)
    schedule(network)
    job = lease(network, A, 1).json()["body"]
    seen = {}

    def launch(job_, directory, **kwargs):
        guard = store._lease_guards_v2[job["job_id"]]
        with pytest.raises(ChallengeError) as again:  # the same lease asks again mid-replay
            store.execute_audit_v2(run, job["job_id"], execute_request(network, A, job))
        seen["status"] = again.value.status
        seen["kept"] = store._lease_guards_v2[job["job_id"]] is guard
        raise IslandFailure("stop")

    monkeypatch.setattr(worker, "launch_island", launch)
    with pytest.raises(IslandFailure):
        store.execute_audit_v2(run, job["job_id"], execute_request(network, A, job))
    assert seen == {"status": 409, "kept": True}
    assert job["job_id"] not in store._lease_guards_v2


def test_a_preparation_failure_leaves_no_guard_behind(network, monkeypatch):
    store, run = network.store, network.manifest.run_id()
    seed_round(network, executable=True)
    schedule(network)
    job = lease(network, A, 1).json()["body"]

    def missing_anchor(*args, **kwargs):
        raise ChallengeError(409, "missing accepted anchor")

    monkeypatch.setattr(store, "_restore_anchor_v2", missing_anchor)
    with pytest.raises(ChallengeError, match="missing accepted anchor"):
        store.execute_audit_v2(run, job["job_id"], execute_request(network, A, job))
    assert job["job_id"] not in store._lease_guards_v2


def test_an_unwinding_attempt_does_not_release_the_guard_of_the_next_one(network, monkeypatch):
    from hypertrain.auditor import worker

    store, run = network.store, network.manifest.run_id()
    seed_round(network, executable=True)
    schedule(network)
    job = lease(network, A, 1).json()["body"]
    sentinel = object()

    def launch(job_, directory, **kwargs):
        store._lease_guards_v2[job["job_id"]] = sentinel  # attempt 2 registered while we unwind
        raise IslandFailure("stop")

    monkeypatch.setattr(worker, "launch_island", launch)
    with pytest.raises(IslandFailure):
        store.execute_audit_v2(run, job["job_id"], execute_request(network, A, job))
    assert store._lease_guards_v2[job["job_id"]] is sentinel


def cooldown_until(response):
    assert response.status_code == 403 and "cooldown until round" in response.json()["detail"], (
        response.text
    )
    return int(re.search(r"until round (\d+)", response.json()["detail"]).group(1))


def test_a_silent_auditor_is_refused_only_until_the_cooldown_ends(network):
    seed_round(network)
    schedule(network)
    job = lease(network, A, 1).json()["body"]
    network.push(job["lease_expires"])  # A never answers; B is idle and free
    until = cooldown_until(lease(network, A, 2))
    assert until == job["lease_expires"] + lease_rounds(network)  # one lease
    network.push(until - 1)
    cooldown_until(lease(network, A, 3))
    network.push(until)
    assert lease(network, A, 4).status_code in (200, 204)  # served, or nothing left to serve


def test_a_refused_poll_does_not_push_the_cooldown_further(network):
    """A refusing call rolls its own expiry back, so the next poll derives the cooldown again:
    it must come out the same, or an auditor that keeps polling is never released."""
    seed_round(network)
    schedule(network)
    job = lease(network, A, 1).json()["body"]
    network.store._services_v2.clear()  # a restarted master: no beacon push reaps the lease
    network.push(job["lease_expires"] + 30)  # the expiry is only noticed 30 rounds late
    until = cooldown_until(lease(network, A, 2))
    assert until == job["lease_expires"] + lease_rounds(network)
    network.push(until - 1)
    assert cooldown_until(lease(network, A, 3)) == until
    network.push(until)
    assert lease(network, A, 4).status_code in (200, 204)


def test_two_silent_auditors_never_leave_the_run_without_an_auditor(network):
    seed_round(network)
    schedule(network)
    first = lease(network, A, 1).json()["body"]
    second = lease(network, B, 2).json()["body"]
    network.push(max(first["lease_expires"], second["lease_expires"]))  # both expire in one pass
    answers = [lease(network, key, n) for n, key in enumerate((A, B), 3)]
    assert sorted(r.status_code for r in answers) == [200, 403]  # one refused, one still serves


def test_a_legacy_permanent_disable_record_no_longer_blocks_the_lease(network):
    store, run = network.store, network.manifest.run_id()
    seed_round(network)
    schedule(network)
    with store._tx():
        store._put_record_v2(
            run, "auditor-disabled", A.ss58, {"job_id": "00" * 32, "reason": "LEASE_EXPIRED"}
        )
    assert lease(network, A, 1).status_code == 200


def test_a_cooling_observer_cannot_attest_and_a_released_one_can_try(network):
    store, run = network.store, network.manifest.run_id()
    seed_round(network)
    schedule(network)
    job = lease(network, A, 1).json()["body"]
    store._services_v2.clear()  # a restarted master: no beacon push reaps the lease for us
    network.push(job["lease_expires"])
    grant = "9" * 64
    with store._tx():
        store._put_record_v2(run, "grant", grant, {"body": {"hotkey": miners(1)[0].ss58, "w": 0}})
    with pytest.raises(ChallengeError) as refused:
        store.relay_observer_v2(run, grant, A.ss58)
    assert refused.value.status == 403
    network.push(job["lease_expires"] + lease_rounds(network))
    with pytest.raises(ChallengeError) as released:
        store.relay_observer_v2(run, grant, A.ss58)
    assert released.value.status == 409 and "reference" in released.value.detail


def failure_cause(store, run, job):
    return store._record_v2(run, "audit-attempt-failure", f"{job['job_id']}:{job['lease_nonce']}")[
        "cause"
    ]


def test_an_expiry_while_the_executor_ran_is_not_the_auditors_silence(network, monkeypatch):
    from hypertrain.auditor import worker

    store, run = network.store, network.manifest.run_id()
    seed_round(network, executable=True)
    schedule(network)
    job = lease(network, A, 1).json()["body"]

    def slow_host(job_, directory, **kwargs):
        network.push(job["lease_expires"])  # the beacon reaches the expiry while the replay runs
        raise IslandFailure("cancelled")

    monkeypatch.setattr(worker, "launch_island", slow_host)
    with pytest.raises(IslandFailure):
        store.execute_audit_v2(run, job["job_id"], execute_request(network, A, job))
    assert lease(network, A, 2).status_code == 200  # triggers the expiry; A is not refused
    row = lease_row(store, job["job_id"])
    assert (row["state"], row["attempts"]) == ("queued", 1)
    assert failure_cause(store, run, job) == "EXECUTOR_INTERRUPTED"


def test_a_preparation_refusal_by_the_master_is_not_the_auditors_silence(network, monkeypatch):
    store, run = network.store, network.manifest.run_id()
    seed_round(network, executable=True)
    schedule(network)
    job = lease(network, A, 1).json()["body"]

    def missing_anchor(*args, **kwargs):
        raise ChallengeError(409, "missing accepted anchor")

    monkeypatch.setattr(store, "_restore_anchor_v2", missing_anchor)
    with pytest.raises(ChallengeError, match="missing accepted anchor"):
        store.execute_audit_v2(run, job["job_id"], execute_request(network, A, job))
    network.push(job["lease_expires"])
    assert lease(network, A, 2).status_code == 200  # the committed REQUESTED marker saves A
    assert failure_cause(store, run, job) == "EXECUTOR_INTERRUPTED"


def test_an_expiry_after_the_verdict_was_stored_is_the_auditors_silence(network):
    store, run = network.store, network.manifest.run_id()
    seed_round(network)
    schedule(network)
    job = lease(network, A, 1).json()["body"]
    with store._tx():
        store._put_record_v2(
            run,
            "audit-execution",
            f"{job['job_id']}:{job['lease_nonce']}",
            {"state": "EXECUTED", "at": 21},
        )
    network.push(job["lease_expires"])
    cooldown_until(lease(network, A, 2))
    assert failure_cause(store, run, job) == "AUDITOR_SILENT"
