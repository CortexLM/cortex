"""Audit lease liveness: dating, head of line, single flight, cooldown, fail-fast, second window."""
# ruff: noqa: F811  (the `network` fixture parameter shadows the imported fixture, as in test_multirun.py)

from __future__ import annotations

import pytest

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
