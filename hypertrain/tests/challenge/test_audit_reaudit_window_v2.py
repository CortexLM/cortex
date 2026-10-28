"""The re-audit window of a withheld MISMATCH opens at the detection; its failure retires the round.

The master replays, the holder of the lease never signs (ADR 0011 Part 9, D84). The job then gets a
window of its own for the other auditors, which starts at the round the lease ended at (derived from
what the master stored, never from the round that noticed it), lasts the policy's `reaudit_rounds`
and never outlasts the miner's commit. A second auditor that completes decides the miner (MATCH
clears it, MISMATCH reaches D47); one that does not, or nobody, is an infrastructure fault: the
round is retired and no member is sanctioned. The tests write the facts the master leaves behind
(stored verdict, EXECUTED marker) and drive the fake clock with `network.push`; the verdicts of the
second auditor on real replays are in `test_audit_reaudit_v2.py`.
"""
# ruff: noqa: F811  (the `network` fixture parameter shadows the imported fixture, as in test_multirun.py)

from __future__ import annotations

import json

import pytest

from hypertrain.challenge import audit_cooldown_v2 as cooldown
from hypertrain.protocol.hashing import sha256_hex

from . import audit_lease_fixture_v2 as fixture
from . import test_service_network_v2 as fixtures
from .audit_lease_fixture_v2 import (
    AUDITOR_A,
    AUDITOR_B,
    D_AUDIT,
    lease,
    miners,
    network,  # noqa: F401  (pytest fixture import)
    schedule,
    seed_round,
)
from .test_audit_reaudit_v2 import (
    one_identity,  # noqa: F401  (pytest fixture import)
    standing,
)
from .test_audit_withheld_signature_v2 import lease_row, records, store_verdict
from .test_exit_reaper_v2 import FakeHook

A, B = AUDITOR_A, AUDITOR_B
T_REAUDIT = 171  # 2 x the 256 s replay of the 101 M run, in rounds of 3 s (ADR 0011 Part 9)
D_FINAL = D_AUDIT + 200  # an honest miner seals the d_final of a gap of 200


@pytest.fixture(autouse=True)
def reaudit_policy(monkeypatch):
    """The run is signed over an audit policy that pins the window of the 101 M run's replay."""
    monkeypatch.setitem(fixtures.AUDIT_POLICY_EXTRA, "reaudit_rounds", T_REAUDIT)


@pytest.fixture
def hook(network):
    network.store.enforce_audit_mismatch = True  # the sanction path is open: nothing may use it
    network.store.sanction_hook = FakeHook()
    return network.store.sanction_hook


def withheld_by_a(network, *, commit_exp, gap=200, stored_after=10, admission_ids=None):
    """A leases one round after the audit round, the master's replay ends `stored_after` rounds
    later and A never signs: the sign window ends the lease. Returns the job and the detection."""
    seed_round(
        network,
        n=1,
        executable=True,
        gap=gap,
        commit_exp=commit_exp,
        admission_ids=admission_ids,
    )
    schedule(network)
    network.push(D_AUDIT + 1)
    job = lease(network, A, 1).json()["body"]
    network.push(D_AUDIT + 1 + stored_after)
    store_verdict(network, job)
    detected = min(job["lease_expires"], network.now + cooldown.SIGN_WINDOW_ROUNDS)
    network.push(detected)
    return job, detected


def window_of(network):
    return records(network, "reaudit")[f"0:{miners(1)[0].ss58}"]


def retired(network):
    return records(network, "retired")


def nobody_is_blamed(network, hook):
    assert (records(network, "excluded"), records(network, "exit-sanction")) == ({}, {})
    assert (records(network, "exit-failed"), hook.calls) == ({}, [])


@pytest.mark.parametrize(
    ("commit_exp", "until"),
    [(10_000, D_AUDIT + 11 + 20 + T_REAUDIT), (D_FINAL, D_FINAL)],
    ids=["window-of-its-own", "clipped-by-the-commit"],
)
def test_the_window_starts_at_the_detection_not_at_the_lease_and_ends_with_the_commit(
    network, commit_exp, until
):
    # the verdict is stored in round 31 and the sign window ends the lease in round 51
    job, detected = withheld_by_a(network, commit_exp=commit_exp)
    assert detected == D_AUDIT + 1 + 10 + cooldown.SIGN_WINDOW_ROUNDS
    # Then: the window is T rounds after the detection (the lease began 30 rounds before it), cut
    # by the commit's expiry where that comes first, and it opens although D + T is past it
    assert window_of(network) == {"job_id": job["job_id"], "detected": detected, "until": until}
    row = lease_row(network, job["job_id"])
    assert (row["state"], row["attempts"], json.loads(row["tried"]), row["absolute"]) == (
        "queued",
        0,
        [A.ss58],
        until,
    )
    assert (records(network, "excluded"), records(network, "exit-sanction")) == ({}, {})
    second = lease(network, B, 2).json()["body"]
    assert (second["auditor_id"], second["attempt"], second["absolute_deadline"]) == (
        B.ss58,
        1,
        until,
    )
    unresolved = network.store._record_v2(network.manifest.run_id(), "finality", "0")
    assert unresolved["unresolved_audits"] == 1


def test_the_second_auditor_withholding_too_retires_the_round_and_blames_nobody(one_identity, hook):
    network = one_identity
    store, run = network.store, network.manifest.run_id()
    key = miners(1)[0]
    admission = store._db.execute(
        "SELECT admission_id FROM admissions_v2 WHERE hotkey=?", (key.ss58,)
    ).fetchone()[0]
    burned = store._services(run)[0].balances().burned
    job, detected = withheld_by_a(network, commit_exp=D_FINAL, admission_ids={0: admission})
    second = lease(network, B, 2).json()["body"]
    network.push(detected + 3)
    store_verdict(network, second)  # B's replay ended; B never signs either
    network.push(network.now + cooldown.SIGN_WINDOW_ROUNDS)
    row = lease_row(network, job["job_id"])
    assert (row["state"], row["result"]) == ("exhausted", "REAUDIT_FAILED")
    done = retired(network)["0"]
    assert (done["cause"], done["survivors"]) == ("REAUDIT_FAILED", [key.ss58])
    status = store._record_v2(run, "finality", "0")
    assert (status["disposition"], status["unresolved_audits"]) == ("RETIRED", 0)
    nobody_is_blamed(network, hook)
    assert records(network, "audit-rearm") == {}  # no ordinary window follows the re-audit
    network.push(network.now + network.manifest.training.verify.T.dispute_per_level + 1)
    assert standing(network, key.ss58) == ("ACTIVE", "JOIN_ACCEPTED", 0)
    assert store._services(run)[0].balances().burned == burned


def test_a_window_nobody_uses_ends_in_a_retirement_at_its_bound_and_not_before(network, hook):
    job, _ = withheld_by_a(network, commit_exp=10_000)
    until = window_of(network)["until"]
    network.push(until - 1)
    assert lease_row(network, job["job_id"])["state"] == "queued" and retired(network) == {}
    network.push(until)  # the bound: the job fails and the same push retires the round
    assert lease_row(network, job["job_id"])["result"] == "REAUDIT_FAILED"
    assert (list(retired(network)), retired(network)["0"]["cause"]) == (["0"], "REAUDIT_FAILED")
    nobody_is_blamed(network, hook)


def test_a_commit_that_ends_before_the_window_can_open_retires_the_round_at_the_detection(
    network, hook
):
    # an honest commit seals d_final = round 120; the verdict is stored in round 99, so the lease
    # ends in 119 and one round is all the window has
    job, detected = withheld_by_a(network, commit_exp=D_AUDIT + 100, gap=100, stored_after=78)
    assert (detected, window_of(network)["until"]) == (D_AUDIT + 99, D_AUDIT + 100)
    assert lease_row(network, job["job_id"])["result"] == "REAUDIT_FAILED"
    assert retired(network)["0"]["cause"] == "REAUDIT_FAILED"
    nobody_is_blamed(network, hook)


def test_a_late_observation_leaves_the_window_where_the_detection_put_it(network):
    seed_round(network, n=1, executable=True, gap=200, commit_exp=10_000)
    schedule(network)
    network.push(D_AUDIT + 1)
    job = lease(network, A, 1).json()["body"]
    network.push(D_AUDIT + 11)
    store_verdict(network, job)
    detected = network.now + cooldown.SIGN_WINDOW_ROUNDS
    network.store._services_v2.clear()  # a restarted master: no beacon push reaps the lease
    network.push(detected + 25)  # the revocation is only noticed 25 rounds late
    assert lease(network, A, 2).status_code == 403  # A cools down: the call rolls itself back
    assert records(network, "reaudit") == {}
    leased = lease(network, B, 3)  # another observer, later still
    assert leased.status_code == 200, leased.text
    assert window_of(network) == {
        "job_id": job["job_id"],
        "detected": detected,
        "until": detected + T_REAUDIT,
    }
    assert leased.json()["body"]["absolute_deadline"] == detected + T_REAUDIT


def test_a_detection_noticed_after_its_window_is_over_fails_the_job_without_leasing_it(
    network, hook
):
    seed_round(network, n=1, executable=True, gap=200, commit_exp=10_000)
    schedule(network)
    network.push(D_AUDIT + 1)
    job = lease(network, A, 1).json()["body"]
    network.push(D_AUDIT + 11)
    store_verdict(network, job)
    detected = network.now + cooldown.SIGN_WINDOW_ROUNDS
    network.store._services_v2.clear()  # a restarted master: the first poll is the observer
    network.push(detected + T_REAUDIT + 5)  # nobody noticed for longer than the window
    answer = lease(network, B, 2)  # a lease with no rounds left would not be a valid job
    assert answer.status_code == 204, answer.text
    assert lease_row(network, job["job_id"])["result"] == "REAUDIT_FAILED"
    assert window_of(network)["detected"] == detected
    nobody_is_blamed(network, hook)


def test_a_miner_that_is_the_only_other_auditor_cannot_be_re_audited_and_the_round_is_retired(
    network, hook, monkeypatch
):
    # B is configured as an auditor and is also the miner: it cannot audit itself, so once A has
    # withheld there is nobody to open a window for
    monkeypatch.setattr(fixture, "miners", lambda n: [B])
    seed_round(network, n=1, executable=True, gap=200, commit_exp=D_FINAL)
    schedule(network)
    network.push(D_AUDIT + 1)
    job = lease(network, A, 1).json()["body"]
    network.push(D_AUDIT + 11)
    store_verdict(network, job)
    network.push(network.now + cooldown.SIGN_WINDOW_ROUNDS)
    assert lease_row(network, job["job_id"])["result"] == "REAUDIT_FAILED"
    assert retired(network)["0"]["cause"] == "REAUDIT_FAILED"
    nobody_is_blamed(network, hook)


def test_a_service_admitted_run_opens_no_window_and_keeps_the_infrastructure_drop(network, hook):
    # The run cannot retire a round (a service-admitted run keeps its roster complete), so a window
    # that could fail would wedge it: the exception to D84 is stated in ADR 0011 Part 9.
    seed_round(network, n=1, executable=True, gap=200, commit_exp=D_FINAL)
    schedule(network)
    network.push(D_AUDIT + 1)
    job = lease(network, A, 1).json()["body"]
    network.push(D_AUDIT + 11)
    store_verdict(network, job)
    with network.store._tx():
        network.store._put_record_v2(network.manifest.run_id(), "service-admission", "run", {})
    network.push(network.now + cooldown.SIGN_WINDOW_ROUNDS)
    row = lease_row(network, job["job_id"])  # revoked as before, and back in the same window
    assert (row["state"], row["attempts"], row["absolute"]) == ("queued", 1, D_FINAL)
    assert records(network, "reaudit") == {}
    network.push(D_FINAL)
    row = lease_row(network, job["job_id"])
    assert (row["state"], row["result"]) == ("exhausted", "AUDIT_INFRASTRUCTURE")
    assert records(network, "excluded") == {
        f"0:{miners(1)[0].ss58}": {
            "reason": "INFRASTRUCTURE",
            "evidence_hash": sha256_hex(job["job_id"].encode()),
        }
    }
    assert (records(network, "exit-sanction"), retired(network), hook.calls) == ({}, {}, [])
