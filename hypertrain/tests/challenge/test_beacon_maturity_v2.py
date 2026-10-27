"""A stuck escrow origin must not stop the beacon route (F6 / SHD-D3).

Candidate for hypertrain/tests/challenge/test_beacon_maturity_v2.py. Uses the `shadow_acceptance`
fixture of test_service_network_v2 (real shadow reservation + acceptance over HTTP) and drives the
public route POST /v1/admin/beacon.
"""

from __future__ import annotations

import sqlite3

import pytest

from hypertrain.ledger.escrow_v2 import EscrowError, EscrowV2
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.messages_v2 import OriginAllocation

from . import test_service_network_v2 as fixtures
from .test_service_network_v2 import (  # noqa: F401  (fixtures)
    reserved_trial,
    shadow_acceptance,
    shadow_reservation_work,
)


def accept(n, identity, body) -> dict:
    response = n.client.post(
        n.url + "/admin/join/" + identity + "/shadow-reward", json=body, headers=fixtures.admin()
    )
    assert response.status_code == 200, response.text
    return response.json()


def at_round(n, number: int) -> None:
    n.now = number
    n.store.clock = lambda: n.manifest.training.beacon.genesis_time + (number - 1) * 3


def issue_pending(escrow, origin: str) -> None:
    """A plain reward_pending origin that is already due; the shadow origin sorts between."""
    with escrow.tx():
        escrow._issue(
            OriginAllocation(origin_id=origin, owner=fixtures.COLD[1].ss58, units=500, mature_at=1),
            None,
            None,
        )


def running_lease(n, run_id: str, expires: int) -> None:
    n.store._db.execute(
        "INSERT INTO audit_leases_v2(id,run_id,w,hotkey,challenge,created,absolute,state,"
        "attempts,auditor,nonce,expires,tried,reservation,step_budget,result) "
        "VALUES('lease-1',?,0,?,'c',1,?,'running',1,?,'n',?,'[]',NULL,1,NULL)",
        (run_id, fixtures.HOT[1].ss58, 10**6, fixtures.AUDITORS[0].ss58, expires),
    )


def failure(n, run_id: str, origin: str) -> str | None:
    row = n.store._db.execute(
        "SELECT data FROM records_v2 WHERE run_id=? AND kind='maturity-failed' AND id=?",
        (run_id, origin),
    ).fetchone()
    return None if row is None else row[0]


def rotate_hotkey(n) -> None:
    from hypertrain.miner.admission import sign_rotation, sign_rotation_consent

    run_id = n.manifest.run_id()
    consent = sign_rotation_consent(
        fixtures.REFEREE,
        run_id=run_id,
        coldkey=fixtures.COLD[0].ss58,
        hotkey=fixtures.HOT[0].ss58,
        operation_id="df" * 32,
        expires_beacon=1000,
    )
    response = n.client.post(
        n.url + "/admission/" + fixtures.HOT[0].ss58 + "/rotate",
        json=sign_rotation(fixtures.COLD[0], run_id, consent, 1000),
    )
    assert response.status_code == 200, response.text


@pytest.mark.parametrize("fault", ["rotated_hotkey", "object_missing", "after_credit", "coded"])
def test_a_stuck_origin_does_not_stop_the_beacon_route(
    shadow_acceptance,  # noqa: F811
    monkeypatch,
    fault,
):
    # Given: a shadow origin that cannot mature, one due origin on each side of it, a lapsed lease.
    n, identity, body = shadow_acceptance
    run_id = n.manifest.run_id()
    escrow = n.store._services(run_id)[0]
    before_id, after_id = "00" * 32, "ff" * 32
    issue_pending(escrow, before_id)
    accepted = accept(n, identity, body)
    maturity, shadow = accepted["mature_at"], accepted["origin_ids"][0]
    issue_pending(escrow, after_id)
    running_lease(n, run_id, expires=maturity)
    mature_id = sha256_hex(f"ht-mature/2|{run_id}|{shadow}".encode())
    real_event, real_mature = EscrowV2._event, EscrowV2.mature
    if fault == "rotated_hotkey":
        rotate_hotkey(n)  # the original commit is no longer signed by the admission's hotkey
        expected = "UnauthorizedSigner"
    elif fault == "object_missing":
        n.store.objects._path(body["auditor_receipt"]["body"]["job_hash"]).unlink()
        expected = "ObjectNotFound"
    elif fault == "after_credit":

        def event(self, operation_id, kind, request, owner, *, server):
            if server and operation_id == mature_id:
                raise OSError("disk fault after the units moved")
            return real_event(self, operation_id, kind, request, owner, server=server)

        monkeypatch.setattr(EscrowV2, "_event", event)
        expected = "OSError"
    else:

        def mature(self, operation_id, origin_ids, now_beacon):
            if origin_ids == (shadow,):
                raise EscrowError("OPERATION_CONFLICT")
            return real_mature(self, operation_id, origin_ids, now_beacon)

        monkeypatch.setattr(EscrowV2, "mature", mature)
        expected = "EscrowError:OPERATION_CONFLICT"
    before = escrow.balances()
    # When: the beacon reaches the maturity round.
    at_round(n, maturity)
    response = n.client.post(
        "/v1/admin/beacon", json=fixtures.fixture_beacon(maturity), headers=fixtures.admin()
    )
    # Then: the route answers, both healthy origins matured, the lease expired, the stuck origin
    # is whole and journaled.
    assert response.status_code == 200, response.text
    assert response.json() == {"round": maturity, "latest": maturity}
    after = escrow.balances()
    assert (after.available - before.available, before.reward_pending - after.reward_pending) == (
        1000,
        1000,
    )
    still_pending = n.store._db.execute(
        "SELECT units FROM escrow_units WHERE origin=? AND bucket='reward_pending'", (shadow,)
    ).fetchone()[0]
    assert still_pending == escrow.policy.round_reward_units
    lease = n.store._db.execute("SELECT state FROM audit_leases_v2 WHERE id='lease-1'").fetchone()
    assert lease[0] == "queued"
    assert expected in (failure(n, run_id, shadow) or "")
    escrow.verify()
    # And the same failure at the next round leaves the rollback revision untouched.
    revision = n.store._rollback_snapshot_v2(run_id)
    n.push(maturity + 1)
    assert n.store._rollback_snapshot_v2(run_id) == revision


@pytest.mark.parametrize(
    "make",
    [
        lambda: EscrowError("CONSERVATION"),
        lambda: EscrowError("ORIGIN_CONSERVATION"),
        lambda: sqlite3.OperationalError("disk I/O error"),
    ],
    ids=["conservation", "origin_conservation", "sqlite"],
)
def test_a_broken_ledger_still_stops_the_beacon_route(
    shadow_acceptance,  # noqa: F811
    monkeypatch,
    make,
):
    # Given: a due origin whose settlement lookup reports a broken ledger, and a healthy one.
    n, identity, body = shadow_acceptance
    run_id = n.manifest.run_id()
    escrow = n.store._services(run_id)[0]
    maturity = accept(n, identity, body)["mature_at"]
    issue_pending(escrow, "00" * 32)

    def broken(_run_id, _origin):
        raise make()

    monkeypatch.setattr(n.store, "_shadow_settlement_v2", broken)
    before = escrow.balances()
    at_round(n, maturity)
    # When / Then: the push fails and nothing matured, nothing is journaled.
    with pytest.raises((EscrowError, sqlite3.Error)):
        n.store.push_beacon(fixtures.fixture_beacon(maturity))
    assert escrow.balances() == before
    assert (
        n.store._db.execute(
            "SELECT COUNT(*) FROM records_v2 WHERE kind='maturity-failed'"
        ).fetchone()[0]
        == 0
    )


def test_an_origin_waiting_on_a_dispute_is_not_a_failure(
    shadow_acceptance,  # noqa: F811
):
    # Given: an accepted shadow origin whose miner has a pending dispute.
    n, identity, body = shadow_acceptance
    run_id = n.manifest.run_id()
    admission = n.store._services(run_id)[1]
    accepted = accept(n, identity, body)
    admission.suspend(
        fixtures.HOT[0].ss58,
        now=2,
        reason="PENDING_DISPUTE",
        pending_dispute=True,
        dispute_id="ea" * 32,
        evidence_hash="eb" * 32,
    )
    # When: the beacon reaches its maturity round.
    n.push(accepted["mature_at"])
    # Then: it stays pending without a failure record.
    assert failure(n, run_id, accepted["origin_ids"][0]) is None
