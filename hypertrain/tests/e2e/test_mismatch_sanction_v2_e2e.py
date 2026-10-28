"""A confirmed audit mismatch sanctions the miner after one contest window, and only then.

The forged commit is the real one of `test_dispute_exit_v2_e2e`: honest leaves under another
delta hash, audited by the real worker. The sanction is booked at the first beacon push after the
verdict and applied one window later, through the same hook as a no-show; in the explicit
record-only mode it is only recorded. The sanction is on by default (D81, ADR 0011 Part 8,
decision 18).
"""

from __future__ import annotations

import dataclasses
import json

import pytest
import test_rollback_service_v2 as rollback
from test_dispute_exit_v2_e2e import contest_and_pause

from challenge.five_identity_network_v2 import five_identity_network
from hypertrain.auditor import worker
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages_v2 import DisputeV2, EscrowLock

fixtures = rollback.fixtures


@pytest.fixture
def n(tmp_path_factory):
    """Five ACTIVE identities, rebuilt per test: a round consumes the network."""
    yield from five_identity_network(tmp_path_factory, fixtures)


def row_of(n, hotkey):
    found = n.store._db.execute(
        "SELECT data FROM records_v2 WHERE kind='exit-sanction' AND id=?", (f"0:{hotkey}",)
    ).fetchone()
    return None if found is None else json.loads(found[0])


def standing(n, hotkey):
    return tuple(
        n.store._db.execute(
            "SELECT state,reason,strikes FROM admissions_v2 WHERE hotkey=?", (hotkey,)
        ).fetchone()
    )


def forged_round(n):
    """Round 0 audited with the fifth identity's delta hash forged, aggregated, not finalized, and
    the beacon push that books the sanction. Returns the booked row."""
    forger = fixtures.HOT[4]
    rollback.execute_round(n, 0, forge={forger.ss58: {"delta_hash": "aa" * 32}})
    assert row_of(n, forger.ss58) is None
    n.push(n.now + 1)
    return row_of(n, forger.ss58)


def due(n, row):
    return row["first_seen"] + n.manifest.training.verify.T.dispute_per_level


def test_a_mismatch_in_the_record_only_mode_is_booked_waits_one_window_and_touches_nobody(n):
    # Given: the explicit record-only mode, and a forged delta hash over honest leaves, audited for
    # real
    forger = fixtures.HOT[4]
    n.store.enforce_audit_mismatch = False
    escrow = n.store._services(n.manifest.run_id())[0]
    burned = escrow.balances().burned
    row = forged_round(n)

    # Then: the verdict is graded as the one shape that needs no located leaf, and waits
    assert (row["state"], row["blame"], row["grade"]) == (
        "PENDING",
        "MINER",
        "COMMIT_CONTRADICTS_LEAVES",
    )
    n.push(due(n, row) - 1)
    assert row_of(n, forger.ss58)["state"] == "PENDING"

    # When: the window is over
    n.push(due(n, row))

    # Then: the sanction is recorded as the one that would apply; nothing is touched
    assert row_of(n, forger.ss58)["state"] == "WOULD_APPLY"
    assert standing(n, forger.ss58) == ("ACTIVE", "JOIN_ACCEPTED", 0)
    assert escrow.balances().burned == burned


def test_a_confirmed_mismatch_suspends_strikes_and_burns_the_bond_by_default(n):
    # Given: the same forged round, with the store as the app builds it (D81: enforcing)
    forger, cold = fixtures.HOT[4], fixtures.COLD[4]
    escrow = n.store._services(n.manifest.run_id())[0]
    burned = escrow.balances().burned
    row = forged_round(n)
    n.push(due(n, row) - 1)
    assert standing(n, forger.ss58) == ("ACTIVE", "JOIN_ACCEPTED", 0)

    # When: the window is over
    n.push(due(n, row))

    # Then: the hook ran: suspended with the reason, one strike, the bond burned
    assert row_of(n, forger.ss58)["state"] == "APPLIED"
    assert standing(n, forger.ss58) == ("SUSPENDED", "AUDIT_MISMATCH", 1)
    assert escrow.balances().burned - burned == 1000 and escrow.balances().conserved()

    # And a verdict whose sanction is applied cannot be contested any more
    contest = json.loads(
        n.store._db.execute(
            "SELECT data FROM records_v2 WHERE kind='contest' AND json_extract(data,'$.miner')=?",
            (forger.ss58,),
        ).fetchone()[0]
    )
    origin = n.store._db.execute(
        "SELECT origin FROM escrow_units WHERE owner=? AND bucket='available' AND units>=100",
        (cold.ss58,),
    ).fetchone()[0]
    dispute_id = sha256_hex(
        canonicalize([n.manifest.run_id(), contest["verdict_hash"], forger.ss58])
    )
    lock = EscrowLock(
        operation_id="df" * 32,
        owner=cold.ss58,
        units=100,
        origin_ids=[origin],
        admission_id=None,
        dispute_id=dispute_id,
        kind="LOCK_CONTEST",
    )
    funded = n.client.post(n.url + "/escrow/lock", json=n.signed(cold, "EscrowLock", lock))
    assert funded.status_code == 200, funded.text
    body = DisputeV2(
        hotkey=forger.ss58,
        verdict_hash=contest["verdict_hash"],
        action="contest",
        bond_lock=lock.operation_id,
    )
    late = n.client.post(n.url + "/dispute", json=n.signed(forger, "DisputeV2", body))
    assert (late.status_code, "SANCTION_APPLIED" in late.text) == (409, True)


def test_a_contest_that_ends_without_a_finding_clears_nobody(n):
    # Given: the forger contests its verdict inside the window and both parties answer honestly,
    # the transcript pausing where no referee can score it
    forger, cold = fixtures.HOT[4], fixtures.COLD[4]
    escrow = n.store._services(n.manifest.run_id())[0]
    burned = escrow.balances().burned
    row = forged_round(n)
    dispute_id = contest_and_pause(n, forger, cold)
    assert row_of(n, forger.ss58)["state"] == "PENDING"

    # When: the beacon closes the dispute (INFRASTRUCTURE, no loser), and the window ends
    n.push(due(n, row))

    # Then: the closure exonerates nobody, so the verdict stands and the sanction applies; the
    # contest bond came back, the admission bond burned
    closed = n.store._services(n.manifest.run_id())[2].get(dispute_id)
    assert closed.resolution is not None
    assert (closed.resolution.reason, closed.resolution.loser) == ("INFRASTRUCTURE", None)
    assert row_of(n, forger.ss58)["state"] == "APPLIED"
    assert standing(n, forger.ss58) == ("SUSPENDED", "AUDIT_MISMATCH", 1)
    assert escrow.balances(cold.ss58).dispute_locked == 0
    assert escrow.balances().burned - burned == 1000


def test_a_mismatch_imputed_to_the_audit_host_sanctions_nobody(n, monkeypatch):
    # Given: the forged round of the first test, but the auditor's host recomputes a leaves root
    # that is not the committed one and names no leaf, which no artifact of the miner explains
    # (sanction_v2.grade: an anomaly of the audit, never the miner's fault). Only the result of
    # the replay is faked, at the worker's boundary; the lease, the verdict, the exclusion, the
    # booking and the sweeper are the real ones, and the store enforces as the app builds it.
    forger = fixtures.HOT[4]
    real = worker.execute_island_audit

    def host_fault(*args, **kwargs):
        outcome, artifacts = real(*args, **kwargs)
        if outcome.result == "MISMATCH":
            outcome = dataclasses.replace(outcome, recomputed_leaves_root="cc" * 32)
        return outcome, artifacts

    monkeypatch.setattr(worker, "execute_island_audit", host_fault)
    escrow = n.store._services(n.manifest.run_id())[0]
    burned = escrow.balances().burned
    row = forged_round(n)

    # Then: the verdict is a MISMATCH that locates nothing, booked as the infrastructure's and
    # waived at once
    assert (row["state"], row["blame"], row["grade"]) == ("WAIVED", "INFRASTRUCTURE", "NONE")
    verdict = json.loads(
        n.store._db.execute(
            "SELECT data FROM records_v2 WHERE kind='verdict' AND id=?", (f"0:{forger.ss58}",)
        ).fetchone()[0]
    )["body"]
    assert (verdict["result"], verdict["first_bad_leaf"]) == ("MISMATCH", None)

    # When: the contest window is over and more rounds pass
    n.push(due(n, row))
    n.push(due(n, row) + 3)

    # Then: nobody was sanctioned: no strike, no state change, no bond burned
    assert row_of(n, forger.ss58)["state"] == "WAIVED"
    assert standing(n, forger.ss58) == ("ACTIVE", "JOIN_ACCEPTED", 0)
    assert escrow.balances().burned == burned
