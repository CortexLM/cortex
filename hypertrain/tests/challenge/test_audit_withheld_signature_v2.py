"""A withheld auditor signature: the master replayed a MISMATCH and the holder never signed it.

The miner is not blamed (no `excluded`, no sanction row); the job goes to another auditor, never to
the one that withheld; the third withheld MISMATCH of one auditor bars it. The master's replay is
simulated by what it leaves behind (the stored verdict and the EXECUTED marker), as in
`test_audit_liveness_v2.py`; `test_audit_reaudit_v2.py` runs the whole path on real replays.
"""
# ruff: noqa: F811  (the `network` fixture parameter shadows the imported fixture, as in test_multirun.py)

from __future__ import annotations

import json
import re

import pytest

from hypertrain.challenge import audit_cooldown_v2 as cooldown
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.messages import ReplayEnv, ReplayVerdict

from .audit_lease_fixture_v2 import (
    AUDITOR_A,
    AUDITOR_B,
    D_AUDIT,
    lease,
    lease_rounds,
    miners,
    network,  # noqa: F401  (pytest fixture import)
    schedule,
    seed_round,
)

A, B = AUDITOR_A, AUDITOR_B


def withhold(network, job, result="MISMATCH"):
    """What the master leaves behind when it replayed `job` for its lease holder: the verdict it
    stored and the EXECUTED marker. The holder never signs it."""
    store, run = network.store, network.manifest.run_id()
    verdict = ReplayVerdict(
        challenge_hash="11" * 32,
        result=result,
        first_bad_leaf=0 if result == "MISMATCH" else None,
        recomputed_leaves_root="ab" * 32,
        replay_env=ReplayEnv(
            image_digest="sha256:" + "22" * 32, driver="d", gpu_uuid_sha256="0" * 64, sm_count=1
        ),
    )
    with store._tx():
        store._put_record_v2(
            run, "executed-verdict", job["job_id"], verdict.model_dump(mode="json")
        )
        store._put_record_v2(
            run,
            "audit-execution",
            f"{job['job_id']}:{job['lease_nonce']}",
            {"state": "EXECUTED", "at": network.now},
        )


def prior_withholds(network, auditor, count):
    """`count` withheld MISMATCHes `auditor` already left on jobs of other rounds."""
    with network.store._tx():
        for i in range(count):
            job_id = sha256_hex(f"{auditor.ss58}|{i}".encode())
            network.store._put_record_v2(
                network.manifest.run_id(),
                "audit-attempt-failure",
                f"{job_id}:{job_id}",
                {
                    "job_id": job_id,
                    "attempt": 1,
                    "auditor": auditor.ss58,
                    "cause": "AUDITOR_SILENT",
                    "at": 1,
                    "state": "queued",
                    "withheld": "MISMATCH",
                },
            )


def records(network, kind):
    return {
        r[0]: json.loads(r[1])
        for r in network.store._db.execute(
            "SELECT id,data FROM records_v2 WHERE run_id=? AND kind=?",
            (network.manifest.run_id(), kind),
        )
    }


def lease_row(network, job_id):
    return network.store._db.execute(
        "SELECT * FROM audit_leases_v2 WHERE id=?", (job_id,)
    ).fetchone()


def refused_until(response):
    assert response.status_code == 403 and "cooldown until round" in response.json()["detail"], (
        response.text
    )
    return int(re.search(r"until round (\d+)", response.json()["detail"]).group(1))


def withheld_in_the_last_window(network):
    """One miner, gap 100: the first window ends unleased, A takes the second one late (its lease
    runs to the absolute deadline, round 320), has the master replay a MISMATCH and never signs."""
    seed_round(network, n=1, gap=100)
    schedule(network)
    network.push(D_AUDIT + 100)
    network.push(D_AUDIT + 250)
    leased = lease(network, A, 1)
    assert leased.status_code == 200, leased.text
    job = leased.json()["body"]
    assert job["lease_expires"] == job["absolute_deadline"] == D_AUDIT + 300
    withhold(network, job)
    network.push(D_AUDIT + 300)
    return job


def test_the_auditor_that_withheld_a_mismatch_is_not_handed_the_job_again_after_the_rearm(network):
    store, run = network.store, network.manifest.run_id()
    seed_round(network, n=1, gap=100)
    schedule(network)
    network.push(D_AUDIT + 1)
    job = lease(network, A, 1).json()["body"]
    assert job["lease_expires"] == job["absolute_deadline"] == D_AUDIT + 100
    withhold(network, job)
    network.push(D_AUDIT + 100)  # the only window ends with A holding the lease: a second opens
    failure = store._record_v2(
        run, "audit-attempt-failure", f"{job['job_id']}:{job['lease_nonce']}"
    )
    assert (failure["cause"], failure.get("withheld")) == ("AUDITOR_SILENT", "MISMATCH")
    assert records(network, "excluded") == {}
    ends = refused_until(lease(network, A, 2))
    network.push(ends)  # A may lease again, but not this job
    assert lease(network, A, 3).status_code == 204
    second = lease(network, B, 4)
    assert second.status_code == 200, second.text
    body = second.json()["body"]
    assert (body["job_id"], body["attempt"], body["auditor_id"]) == (job["job_id"], 1, B.ss58)


def test_a_mismatch_withheld_in_the_last_window_still_gets_a_lease_for_the_other_auditor(network):
    job = withheld_in_the_last_window(network)
    row = lease_row(network, job["job_id"])
    assert (row["state"], row["attempts"], json.loads(row["tried"])) == ("queued", 0, [A.ss58])
    assert records(network, "excluded") == {}
    assert records(network, "exit-sanction") == {}
    assert refused_until(lease(network, A, 2)) == job["lease_expires"] + lease_rounds(network)
    second = lease(network, B, 3)
    assert second.status_code == 200, second.text
    body = second.json()["body"]
    assert (body["job_id"], body["attempt"]) == (job["job_id"], 1)
    assert body["absolute_deadline"] == D_AUDIT + 300 + 2 * lease_rounds(
        network
    )  # the third window


def test_a_third_window_ends_the_job_as_soon_as_no_auditor_is_left_to_try(network):
    store, run = network.store, network.manifest.run_id()
    job = withheld_in_the_last_window(network)
    leased = lease(network, B, 2)
    assert leased.status_code == 200, leased.text
    second = leased.json()["body"]
    network.push(second["lease_expires"])  # B lapses too: A and B have both had the job
    row = lease_row(network, job["job_id"])
    assert (row["state"], row["result"]) == ("exhausted", "AUDIT_INFRASTRUCTURE")
    assert row["absolute"] > second["lease_expires"]  # not left idle to the end of the window
    assert records(network, "excluded") == {
        f"0:{miners(1)[0].ss58}": {
            "reason": "INFRASTRUCTURE",
            "evidence_hash": sha256_hex(job["job_id"].encode()),
        }
    }
    assert records(network, "exit-sanction") == {}
    assert records(network, "audit-rearm")[job["job_id"]]["windows"] == 3  # and no fourth
    assert store._record_v2(run, "finality", "0")["unresolved_audits"] == 0


def test_when_both_auditors_withhold_the_miner_is_dropped_once_and_never_sanctioned(network):
    store, run = network.store, network.manifest.run_id()
    seed_round(network, n=1, gap=200)
    schedule(network)
    with store._tx():  # two more jobs this test does not model: a double drop would be visible
        finality = store._record_v2(run, "finality", "0")
        finality["unresolved_audits"] = 3
        store._put_record_v2(run, "finality", "0", finality)
    first = lease(network, A, 1).json()["body"]
    withhold(network, first)
    network.push(first["lease_expires"])
    second = lease(network, B, 2).json()["body"]
    assert (second["job_id"], second["attempt"]) == (first["job_id"], 2)
    withhold(network, second)
    network.push(second["lease_expires"])  # attempt 2 is over: A and B both withheld
    row = lease_row(network, first["job_id"])
    assert (row["state"], row["result"]) == ("exhausted", "AUDIT_INFRASTRUCTURE")
    assert records(network, "audit-rearm") == {}  # nobody is left to try in a second window
    assert records(network, "excluded") == {
        f"0:{miners(1)[0].ss58}": {
            "reason": "INFRASTRUCTURE",
            "evidence_hash": sha256_hex(first["job_id"].encode()),
        }
    }
    assert store._record_v2(run, "finality", "0")["unresolved_audits"] == 2
    network.push(second["lease_expires"] + 1)
    assert records(network, "exit-sanction") == {}


@pytest.mark.parametrize("bars", [False, True])
def test_the_nth_withheld_mismatch_bars_the_auditor_and_the_one_before_does_not(network, bars):
    threshold = cooldown.WITHHELD_STRIKES
    seed_round(network, n=2, gap=200)
    schedule(network)
    prior_withholds(network, A, threshold - 1 if bars else threshold - 2)
    prior_withholds(network, B, threshold)  # what another auditor withheld is not A's count
    job = lease(network, A, 1).json()["body"]
    withhold(network, job)
    network.push(job["lease_expires"])
    absolute = 2 * lease_rounds(network)
    length = cooldown.STRIKE_BAR_ABSOLUTE_WINDOWS * absolute if bars else lease_rounds(network)
    assert refused_until(lease(network, A, 2)) == job["lease_expires"] + length
    assert lease(network, B, 3).status_code == 200  # B withheld nothing today: not refused


@pytest.mark.parametrize("silence", ["without-execution", "withheld-match"])
def test_only_a_withheld_mismatch_counts_toward_the_bar(network, silence):
    threshold = cooldown.WITHHELD_STRIKES
    seed_round(network, n=2, gap=200)
    schedule(network)
    prior_withholds(network, A, threshold)  # A is past the threshold already
    first = lease(network, A, 1).json()["body"]
    if silence == "withheld-match":
        withhold(network, first, "MATCH")
    network.push(first["lease_expires"])
    assert refused_until(lease(network, A, 2)) == first["lease_expires"] + lease_rounds(network)
    network.push(D_AUDIT + 200)  # the cooldown is over and the second window is open
    second = lease(network, A, 3).json()["body"]
    withhold(network, second)
    network.push(second["lease_expires"])
    bar = cooldown.STRIKE_BAR_ABSOLUTE_WINDOWS * 2 * lease_rounds(network)
    assert refused_until(lease(network, A, 4)) == second["lease_expires"] + bar
