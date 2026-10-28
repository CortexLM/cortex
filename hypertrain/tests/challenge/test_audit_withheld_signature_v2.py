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

from . import test_service_network_v2 as fixtures
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


def store_verdict(network, job, result="MISMATCH"):
    """What the master leaves behind when it replayed `job` for its lease holder, in the current
    round: the verdict it stored and the EXECUTED marker. The holder never signs it."""
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


def withhold(network, job, result="MISMATCH"):
    """The master's replay ends in the last round of the lease, so that the sign window cannot end
    before the lease does: these tests are about the expiry, test_audit_revocation_v2.py is
    about the window."""
    network.push(job["lease_expires"] - 1)
    store_verdict(network, job, result)


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


def lease_in_the_last_window(network):
    """One miner, gap 100: the first window ends unleased and A takes the second one late (its
    lease runs to the absolute deadline, round 320)."""
    seed_round(network, n=1, gap=100)
    schedule(network)
    network.push(D_AUDIT + 100)
    network.push(D_AUDIT + 250)
    leased = lease(network, A, 1)
    assert leased.status_code == 200, leased.text
    job = leased.json()["body"]
    assert job["lease_expires"] == job["absolute_deadline"] == D_AUDIT + 300
    return job


def withheld_in_the_last_window(network, result="MISMATCH"):
    """A, late in the second window, has the master replay `result` and never signs it."""
    job = lease_in_the_last_window(network)
    withhold(network, job, result)
    network.push(D_AUDIT + 300)
    return job


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
    # the window of the job began when A's lease ended (round 320) and lasts one lease
    assert body["absolute_deadline"] == D_AUDIT + 300 + lease_rounds(network)


@pytest.fixture
def wide_window(monkeypatch):
    """The run is signed over a policy that pins the window of a 256 s replay: 171 rounds, which
    is longer than one lease."""
    monkeypatch.setitem(fixtures.AUDIT_POLICY_EXTRA, "reaudit_rounds", 171)


def test_a_reaudit_window_fails_the_job_as_soon_as_no_auditor_is_left_to_try(wide_window, network):
    store, run = network.store, network.manifest.run_id()
    job = withheld_in_the_last_window(network)
    leased = lease(network, B, 2)
    assert leased.status_code == 200, leased.text
    second = leased.json()["body"]
    network.push(second["lease_expires"])  # B lapses too: A and B have both had the job
    row = lease_row(network, job["job_id"])
    assert (row["state"], row["result"]) == ("exhausted", "REAUDIT_FAILED")
    assert row["absolute"] > second["lease_expires"]  # not left idle to the end of the window
    # D87: the member alone leaves the round as an infrastructure fault, with the evidence of a
    # failed re-audit; no sanction, and the job's count falls once (the drop is the one writer)
    miner = row["hotkey"]
    # B never asked for the execution: the verdict A left stored is not B's to have withheld.
    silent = store._record_v2(
        run, "audit-attempt-failure", f"{second['job_id']}:{second['lease_nonce']}"
    )
    assert (silent["auditor"], silent["cause"], "withheld" in silent) == (
        B.ss58,
        "AUDITOR_SILENT",
        False,
    )
    assert records(network, "excluded") == {
        f"0:{miner}": {
            "reason": "INFRASTRUCTURE",
            "evidence_hash": sha256_hex(f"ht-reaudit-failed/1|{job['job_id']}".encode()),
        }
    }
    assert records(network, "exit-sanction") == {}
    assert (
        records(network, "audit-rearm")[job["job_id"]]["windows"] == 2
    )  # and no ordinary window follows
    assert store._record_v2(run, "finality", "0")["unresolved_audits"] == 0


def test_a_match_withheld_in_the_last_window_is_dropped_and_gets_no_reaudit(network):
    # The control of the test above: the re-audit is for a MISMATCH only, and a withheld MATCH
    # is not worth a replay by another auditor.
    job = withheld_in_the_last_window(network, "MATCH")
    row = lease_row(network, job["job_id"])
    assert (row["state"], row["result"]) == ("exhausted", "AUDIT_INFRASTRUCTURE")
    assert records(network, "audit-rearm")[job["job_id"]]["windows"] == 2
    assert records(network, "excluded") == {
        f"0:{miners(1)[0].ss58}": {
            "reason": "INFRASTRUCTURE",
            "evidence_hash": sha256_hex(job["job_id"].encode()),
        }
    }
    assert records(network, "exit-sanction") == {}


@pytest.mark.parametrize("sorts", ["below", "above"])
def test_a_withhold_on_another_job_does_not_make_its_auditor_a_withholder_of_this_one(
    network, sorts
):
    # B left a MISMATCH unsigned on a job whose id sorts before or after this one's: the failure
    # rows of a job are the ones under its own id, so B is still the auditor this job can go to
    foreign = ("0" if sorts == "below" else "f") * 64
    with network.store._tx():
        network.store._put_record_v2(
            network.manifest.run_id(),
            "audit-attempt-failure",
            f"{foreign}:{foreign}",
            {
                "job_id": foreign,
                "attempt": 1,
                "auditor": B.ss58,
                "cause": "AUDITOR_SILENT",
                "at": 1,
                "state": "queued",
                "withheld": "MISMATCH",
            },
        )
    job = withheld_in_the_last_window(network)
    row = lease_row(network, job["job_id"])
    assert (row["state"], json.loads(row["tried"])) == ("queued", [A.ss58])
    assert records(network, "excluded") == {}
    served = lease(network, B, 2)
    assert served.status_code == 200, served.text
    assert served.json()["body"]["job_id"] == job["job_id"]


def test_a_match_the_first_holder_withheld_does_not_keep_it_from_the_reaudit_of_the_job(network):
    # A withheld a MATCH, B then withheld a MISMATCH: only B left the verdict that matters unsigned,
    # so the re-audit window may go to A once its ordinary cooldown is over
    seed_round(network, n=1, gap=200)
    schedule(network)
    first = lease(network, A, 1).json()["body"]
    withhold(network, first, "MATCH")
    network.push(first["lease_expires"])
    second = lease(network, B, 2).json()["body"]
    withhold(network, second)
    network.push(second["lease_expires"])
    row = lease_row(network, first["job_id"])
    assert (row["state"], json.loads(row["tried"])) == ("queued", [B.ss58])
    assert records(network, "excluded") == {}
    served = lease(network, A, 3)
    assert served.status_code == 200, served.text
    assert (served.json()["body"]["job_id"], served.json()["body"]["attempt"]) == (
        first["job_id"],
        1,
    )


def test_a_job_whose_only_other_auditor_is_its_miner_is_re_armed_when_the_first_lapses(network):
    # B is a configured auditor and the miner of the job: lease_v2 never hands a miner its own job,
    # so once A has had it nobody is left, and waiting for the end of the window would idle.
    seed_round(network, n=1, gap=200, keys=[B])
    schedule(network)
    job = lease(network, A, 1).json()["body"]
    assert job["lease_expires"] < job["absolute_deadline"]
    network.push(job["lease_expires"])
    assert records(network, "audit-rearm") == {
        job["job_id"]: {
            "windows": 2,
            "at": job["lease_expires"],
            "absolute": job["lease_expires"] + 2 * lease_rounds(network),
        }
    }


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
    assert (second["job_id"], second["attempt"]) == (first["job_id"], 1)  # the window's own attempt
    withhold(network, second)
    network.push(second["lease_expires"])  # the re-audit lease is over: A and B both withheld
    row = lease_row(network, first["job_id"])
    assert (row["state"], row["result"]) == ("exhausted", "REAUDIT_FAILED")
    assert records(network, "audit-rearm") == {}  # nobody is left to try in a second window
    assert records(network, "excluded") == {
        f"0:{miners(1)[0].ss58}": {
            "reason": "INFRASTRUCTURE",
            "evidence_hash": sha256_hex(f"ht-reaudit-failed/1|{first['job_id']}".encode()),
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
    assert records(network, "reaudit") == {}  # only a withheld MISMATCH opens a re-audit window
    assert refused_until(lease(network, A, 2)) == first["lease_expires"] + lease_rounds(network)
    network.push(D_AUDIT + 200)  # the cooldown is over and the second window is open
    second = lease(network, A, 3).json()["body"]
    withhold(network, second)
    network.push(second["lease_expires"])
    bar = cooldown.STRIKE_BAR_ABSOLUTE_WINDOWS * 2 * lease_rounds(network)
    assert refused_until(lease(network, A, 4)) == second["lease_expires"] + bar
