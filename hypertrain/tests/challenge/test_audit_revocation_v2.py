"""A lease whose stored verdict is not signed in time is revoked, so the re-audit has room.

The master replays for the lease holder and stores the verdict; the holder only has to sign it.
`SIGN_WINDOW_ROUNDS` after the verdict was stored (not after the claim: a holder whose replay is
still running is never revoked) the lease ends as a silence, the job goes to the other auditor in
the same window, and the miner is never blamed (ADR 0011 Part 9). Light tests write the facts the
master leaves behind; the last test signs a real verdict on either side of the window.
"""
# ruff: noqa: F811  (the `network` fixture parameter shadows the imported fixture, as in test_multirun.py)

from __future__ import annotations

import json

import pytest

from hypertrain.challenge import audit_cooldown_v2 as cooldown
from hypertrain.protocol.hashing import sha256_hex

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
from .test_audit_reaudit_v2 import execute, one_identity  # noqa: F401
from .test_audit_withheld_signature_v2 import (
    lease_row,
    prior_withholds,
    records,
    refused_until,
    store_verdict,
)

A, B = AUDITOR_A, AUDITOR_B


def test_an_unsigned_verdict_is_revoked_when_the_sign_window_ends_and_goes_to_the_other_auditor(
    network,
):
    seed_round(network, n=1, gap=100, commit_exp=D_AUDIT + 100)  # an honest miner seals d_final
    schedule(network)
    network.push(D_AUDIT + 1)
    job = lease(network, A, 1).json()["body"]
    assert job["lease_expires"] == job["absolute_deadline"] == D_AUDIT + 100
    network.push(D_AUDIT + 10)
    store_verdict(network, job)  # the master's replay ended: the verdict is stored in round 30
    stored = network.now
    network.push(stored + cooldown.SIGN_WINDOW_ROUNDS - 1)
    assert lease_row(network, job["job_id"])["state"] == "running"  # one round left to sign
    network.push(stored + cooldown.SIGN_WINDOW_ROUNDS)
    revoked = stored + cooldown.SIGN_WINDOW_ROUNDS
    row = lease_row(network, job["job_id"])
    assert (row["state"], row["attempts"], json.loads(row["tried"])) == ("queued", 1, [A.ss58])
    key = f"{job['job_id']}:{job['lease_nonce']}"
    failure = network.store._record_v2(network.manifest.run_id(), "audit-attempt-failure", key)
    assert (failure["cause"], failure["withheld"], failure["at"]) == (
        "AUDITOR_SILENT",
        "MISMATCH",
        revoked,
    )
    assert (records(network, "excluded"), records(network, "exit-sanction")) == ({}, {})
    cooling = records(network, "auditor-cooldown")[A.ss58]
    assert (cooling["since"], cooling["until"]) == (revoked, revoked + lease_rounds(network))
    leased = lease(network, B, 2)
    assert leased.status_code == 200, leased.text
    body = leased.json()["body"]
    assert (body["job_id"], body["attempt"], body["auditor_id"]) == (job["job_id"], 2, B.ss58)
    assert body["lease_expires"] == body["absolute_deadline"] == D_AUDIT + 100


def test_the_sign_window_counts_from_the_stored_verdict_and_never_revokes_a_replay_in_progress(
    network,
):
    store = network.store
    seed_round(network, n=2, gap=100, commit_exp=D_AUDIT + 100)
    schedule(network)
    network.push(D_AUDIT + 1)
    replayed = lease(network, A, 1).json()["body"]
    replaying = lease(network, B, 2).json()["body"]
    network.push(D_AUDIT + 70)  # 70 rounds after both claims
    store_verdict(network, replayed)
    with store._tx():  # B asked for its execution and the master is still replaying
        store._put_record_v2(
            network.manifest.run_id(),
            "audit-execution",
            f"{replaying['job_id']}:{replaying['lease_nonce']}",
            {"state": "REQUESTED", "at": D_AUDIT + 2},
        )
    stored = network.now
    network.push(stored + cooldown.SIGN_WINDOW_ROUNDS - 1)
    assert {lease_row(network, j["job_id"])["state"] for j in (replayed, replaying)} == {"running"}
    network.push(stored + cooldown.SIGN_WINDOW_ROUNDS)
    assert lease_row(network, replayed["job_id"])["state"] == "queued"
    assert lease_row(network, replaying["job_id"])["state"] == "running"  # nobody revokes a replay
    assert not records(network, "audit-attempt-failure").keys() & {
        f"{replaying['job_id']}:{replaying['lease_nonce']}"
    }
    network.push(D_AUDIT + 100)  # the commit ends with the round: no window is left for anyone
    assert records(network, "exit-sanction") == {}
    for job in (replayed, replaying):
        miner = lease_row(network, job["job_id"])["hotkey"]
        assert records(network, "excluded")[f"0:{miner}"] == {
            "reason": "INFRASTRUCTURE",
            "evidence_hash": sha256_hex(job["job_id"].encode()),
        }


@pytest.mark.parametrize("result", ["MATCH", "MISMATCH"])
def test_a_revocation_cools_the_auditor_down_from_the_revocation_round(network, result):
    threshold = cooldown.WITHHELD_STRIKES
    seed_round(network, n=2, gap=200)
    schedule(network)
    prior_withholds(network, A, threshold if result == "MATCH" else threshold - 1)
    job = lease(network, A, 1).json()["body"]
    network.push(D_AUDIT + 10)
    store_verdict(network, job, result)
    network.push(network.now + cooldown.SIGN_WINDOW_ROUNDS)
    revoked = network.now
    bar = cooldown.STRIKE_BAR_ABSOLUTE_WINDOWS * 2 * lease_rounds(network)
    length = lease_rounds(network) if result == "MATCH" else bar
    assert refused_until(lease(network, A, 2)) == revoked + length


def test_a_poll_that_notices_the_revocation_late_does_not_push_the_cooldown_further(network):
    seed_round(network, n=1, gap=200)
    schedule(network)
    job = lease(network, A, 1).json()["body"]
    network.push(D_AUDIT + 10)
    store_verdict(network, job)
    revoked = network.now + cooldown.SIGN_WINDOW_ROUNDS
    network.store._services_v2.clear()  # a restarted master: no beacon push reaps the lease
    network.push(revoked + 25)  # the revocation is only noticed 25 rounds late
    until = refused_until(lease(network, A, 2))
    assert until == revoked + lease_rounds(network)
    network.push(until - 1)
    assert refused_until(lease(network, A, 3)) == until
    network.push(until)
    assert lease(network, A, 4).status_code == 204  # served: nothing is left for A to take


def test_the_holder_of_a_revoked_lease_is_not_handed_the_job_again_when_its_cooldown_ends(network):
    seed_round(network, n=1, gap=200)
    schedule(network)
    job = lease(network, A, 1).json()["body"]
    network.push(D_AUDIT + 10)
    store_verdict(network, job)
    network.push(network.now + cooldown.SIGN_WINDOW_ROUNDS)
    until = refused_until(lease(network, A, 2))
    network.push(until)  # B never polled: A is free again, and the job is still not A's
    assert lease(network, A, 3).status_code == 204
    assert lease_row(network, job["job_id"])["state"] == "queued"


@pytest.mark.parametrize("signs", ["inside-the-window", "after-the-window"])
def test_a_signature_inside_the_sign_window_is_untouched_and_a_later_one_is_refused(
    one_identity, signs
):
    network = one_identity
    store, run = network.store, network.manifest.run_id()
    key = miners(1)[0]
    admission = store._db.execute(
        "SELECT admission_id FROM admissions_v2 WHERE hotkey=?", (key.ss58,)
    ).fetchone()[0]
    seed_round(network, n=1, executable=True, gap=200, admission_ids={0: admission})
    schedule(network)
    job = lease(network, A, 1).json()["body"]
    verdict = execute(network, A, job)  # the master replays the seeded fake leaves: a MISMATCH
    stored = store._record_v2(run, "audit-execution", f"{job['job_id']}:{job['lease_nonce']}")["at"]
    inside = signs == "inside-the-window"
    network.push(stored + cooldown.SIGN_WINDOW_ROUNDS - (1 if inside else 0))
    path = f"/worker/jobs/{job['job_id']}/complete"
    answer = network.client.post(
        network.url + path, json=network.signed(A, "ReplayVerdict", verdict)
    )
    if inside:
        assert answer.status_code == 200, answer.text
        assert answer.json()["result"] == "MISMATCH"
        assert lease_row(network, job["job_id"])["state"] == "completed"
        assert records(network, "audit-attempt-failure") == {}
        assert records(network, "auditor-cooldown") == {}
    else:
        assert answer.status_code == 409, answer.text
        assert records(network, "replay") == {}
        row = lease_row(network, job["job_id"])
        assert (row["state"], row["attempts"], json.loads(row["tried"])) == ("queued", 1, [A.ss58])
        again = lease(network, B, 2)
        assert again.status_code == 200, again.text
        assert again.json()["body"]["attempt"] == 2
