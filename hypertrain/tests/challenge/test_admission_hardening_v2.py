"""Store-level regressions of the admission hardening lot (ADR 0010), through the HTTP routes."""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

import pytest

from env_pin_fixtures import honest_env
from hypertrain.protocol.envelope_v2 import signing_message
from hypertrain.protocol.messages_v2 import (
    DisputeV2,
    EscrowLock,
    EscrowRelease,
    ResolutionV2,
)

_spec = importlib.util.spec_from_file_location(
    "admission_hardening_service_fixture", Path(__file__).parent / "test_service_network_v2.py"
)
assert _spec is not None and _spec.loader is not None
service = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = service
_spec.loader.exec_module(service)
network = service.network


def open_trials(n) -> int:
    return n.store._db.execute(
        "SELECT COUNT(*) FROM admission_trial_results WHERE outcome='OPEN'"
    ).fetchone()[0]


def test_beacon_push_frees_the_places_of_unproven_challenges(network):
    # Given: two identities open a trial each (window 2..102) and never prove it
    ids = [network.join(i).json()["admission_id"] for i in (0, 1)]
    network.push(2)
    for identity in ids:
        response = network.challenge(identity)
        assert response.status_code == 200, response.text
    assert open_trials(network) == 2
    # When: the beacon passes the deadline and nobody asks for anything
    network.push(103)
    # Then: the places are free without any request, and nobody was suspended
    assert open_trials(network) == 0
    states = network.store._db.execute("SELECT state FROM admissions_v2").fetchall()
    assert [row[0] for row in states] == ["APPLIED", "APPLIED"]
    # and a newcomer gets its trial instead of TRIAL_CAPACITY
    newcomer = network.join(2).json()["admission_id"]
    network.push(104)
    response = network.challenge(newcomer)
    assert response.status_code == 200, response.text


def test_unfunded_join_past_its_window_does_not_block_its_coldkey(network):
    # Given: HOT[0] applied with COLD[0] and never funded or asked for its challenge
    first = network.join(0)
    assert first.status_code == 200, first.text
    network.push(103)  # its window (to round 102) is over
    # When: the same coldkey applies again with another registered hotkey
    request = service.sign_join(
        service.HOT[1],
        service.COLD[0],
        run_id=network.manifest.run_id(),
        request_id="ab" * 32,
        expires_beacon=10000,
        policy_hash=network.manifest.network.admission_policy_hash,
        hardware_hint=service.HardwareHint(device_name="cpu", device_count=1, driver="advisory"),
        env=honest_env(network.manifest.training.reference_spec.env.pin()),
    )
    second = network.client.post(
        network.url + "/join", content=service.canonicalize(request.body())
    )
    # Then: the dead application was retired by the dispatcher's join
    assert second.status_code == 200, second.text
    record = network.client.get(network.url + f"/admission/{service.HOT[0].ss58}").json()["record"]
    assert (record["state"], record["reason"]) == ("SUSPENDED", "APPLIED_EXPIRED")


def lock_bond(network, units: int = 1000, operation: str = "bc" * 32):
    identity = network.join().json()["admission_id"]
    lock = EscrowLock(
        operation_id=operation,
        owner=service.COLD[0].ss58,
        units=units,
        origin_ids=[network.origin_ids[0]],
        admission_id=identity,
        dispute_id=None,
        kind="LOCK_ADMISSION",
    )
    response = network.client.post(
        network.url + "/escrow/lock", json=network.signed(service.COLD[0], "EscrowLock", lock)
    )
    assert response.status_code == 200, response.text
    return identity, lock


def release_request(network, identity, lock, entry, units: int = 1000, **changes):
    body = {
        "operation_id": "cd" * 32,
        "owner": service.COLD[0].ss58,
        "units": units,
        "origin_ids": entry["origin_ids"],
        "admission_id": identity,
        "dispute_id": None,
        "lock_id": lock.operation_id,
        "finality_hash": entry["finality_hash"],
        "closed_dispute_root": entry["closed_dispute_root"],
        **changes,
    }
    return network.signed(service.COLD[0], "EscrowRelease", EscrowRelease(**body))


def balances(network):
    return network.store._services(network.manifest.run_id())[0].balances()


def test_owner_takes_the_bond_back_through_the_routes(network):
    # Given: a funded application. The holder's signed request tells the owner what to sign; the
    # public status shows the amounts only.
    identity, lock = lock_bond(network)
    status = network.client.get(network.url + f"/admission/{service.HOT[0].ss58}").json()
    assert [e["units"] for e in status["bond"]["locks"]] == [1000]
    terms = network.bond(identity)
    assert terms.status_code == 200, terms.text
    (entry,) = terms.json()["locks"]
    assert (entry["lock_id"], entry["units"], entry["releasable"]) == (
        lock.operation_id,
        1000,
        True,
    )
    # When: hashes of another state are refused, the derived ones accepted
    stale = release_request(network, identity, lock, entry, finality_hash="ab" * 32)
    refused = network.client.post(network.url + "/escrow/release", json=stale)
    assert refused.status_code == 409 and "DISPUTED_OR_UNVESTED" in refused.text
    assert balances(network).admission_locked == 1000
    accepted = network.client.post(
        network.url + "/escrow/release", json=release_request(network, identity, lock, entry)
    )
    # Then
    assert accepted.status_code == 200, accepted.text
    assert balances(network).admission_locked == 0 and balances(network).conserved()
    status = network.client.get(network.url + f"/admission/{service.HOT[0].ss58}").json()
    assert status["bond"] == {"locks": []}
    assert network.bond(identity).json() == {"locks": []}


def convict_through_the_routes(network, identity, loser):
    """The miner contests its verdict over HTTP; the referee rules against `loser`."""
    from hypertrain.auditor.island_bisect import RefereeEvidence
    from hypertrain.challenge.disputes_v2 import Contest
    from hypertrain.protocol.hashing import sha256_hex

    run_id = network.manifest.run_id()
    inner = network.manifest.training.inner
    contest = Contest(
        verdict_hash="11" * 32,
        challenge_hash="22" * 32,
        w=0,
        miner=service.HOT[0].ss58,
        auditor=service.AUDITORS[0].ss58,
        referee=service.REFEREE.ss58,
        step_span=inner.H // inner.J,
        layer_span=2,
        op_span=2,
        admission_id=identity,
        coldkey=service.COLD[0].ss58,
        evidence_hash="55" * 32,
    )
    with network.store._tx():
        network.store._put_record_v2(run_id, "contest", contest.verdict_hash, contest.body())
        network.store._put_record_v2(
            run_id,
            "finality",
            "0",
            {
                "run_id": run_id,
                "w": 0,
                "applied": False,
                "disposition": "PENDING",
                "unresolved_disputes": 0,
                "unresolved_audits": 0,
                "rollback_complete": False,
                "state_hash": "00" * 32,
                "resolution_hash": None,
            },
        )
    dispute_id = sha256_hex(service.canonicalize([run_id, contest.verdict_hash, contest.miner]))
    contest_lock = EscrowLock(
        operation_id="44" * 32,
        owner=service.COLD[0].ss58,
        units=100,
        origin_ids=[network.origin_ids[0]],
        admission_id=None,
        dispute_id=dispute_id,
        kind="LOCK_CONTEST",
    )
    for envelope, route in (
        (network.signed(service.COLD[0], "EscrowLock", contest_lock), "/escrow/lock"),
        (
            network.signed(
                service.HOT[0],
                "DisputeV2",
                DisputeV2(
                    hotkey=service.HOT[0].ss58,
                    verdict_hash=contest.verdict_hash,
                    action="contest",
                    bond_lock=contest_lock.operation_id,
                ),
            ),
            "/dispute",
        ),
    ):
        response = network.client.post(network.url + route, json=envelope)
        assert response.status_code == 200, response.text
    turn = response.json()
    disputes = network.store._services(run_id)[2]
    evidence = RefereeEvidence(
        dispute_id=dispute_id,
        transcript_hash=turn["transcript_hash"],
        reason="FRAUD",
        loser=loser,
        predecessor_hash="11" * 32,
        inputs_hash="22" * 32,
        output_hash="33" * 32,
        op_spec="verified.actual.op",
    )
    disputes.register_evidence(evidence)
    resolution = ResolutionV2(
        dispute_id=dispute_id,
        transcript_hash=turn["transcript_hash"],
        reason="FRAUD",
        loser=loser,
        evidence_hash=evidence.digest(),
    )
    return network.signed(service.REFEREE, "ResolutionV2", resolution)


def test_a_lock_named_after_a_dispute_is_not_released_by_that_dispute(network):
    # Given: the owner names its bond after the dispute id that its own contest will have, and
    # the referee rules for the miner: the dispute settles in its favour
    from hypertrain.protocol.hashing import sha256_hex

    run_id = network.manifest.run_id()
    dispute_id = sha256_hex(service.canonicalize([run_id, "11" * 32, service.HOT[0].ss58]))
    identity, lock = lock_bond(network, operation=dispute_id)
    verdict = convict_through_the_routes(network, identity, service.AUDITORS[0].ss58)
    resolved = network.client.post(network.url + "/resolution", json=verdict)
    assert resolved.status_code == 200, resolved.text
    settlement = network.store._services(run_id)[2].settlement(dispute_id)
    assert settlement.outcome == "MATCH" and not settlement.unresolved
    # When: the owner signs a release with the hashes of that dispute's settlement
    entry = {
        "origin_ids": [network.origin_ids[0]],
        "finality_hash": settlement.finality_hash,
        "closed_dispute_root": settlement.closed_dispute_root,
    }
    response = network.client.post(
        network.url + "/escrow/release", json=release_request(network, identity, lock, entry)
    )
    # Then: the bond is settled by the admission's own state, not by the dispute it is named after
    assert response.status_code == 409 and "DISPUTED_OR_UNVESTED" in response.text
    assert balances(network).admission_locked == 1000


def test_a_conviction_that_cannot_be_settled_rolls_the_resolution_back(network, monkeypatch):
    # Given: a funded miner whose referee verdict goes against it, and a settlement hook that fails
    from hypertrain.challenge.admission import Admission
    from hypertrain.challenge.admission_store import AdmissionError

    identity, _ = lock_bond(network)
    verdict = convict_through_the_routes(network, identity, service.HOT[0].ss58)

    def fails(self, dispute_id, *, now):
        raise AdmissionError("UNPROVEN_TRAINING_FRAUD")

    db = network.store._db
    with monkeypatch.context() as patch:
        patch.setattr(Admission, "settle_fraud", fails)
        # When
        refused = network.client.post(network.url + "/resolution", json=verdict)
    # Then: nothing of the resolution is kept, so the verdict can be posted again
    assert refused.status_code == 409, refused.text
    assert (
        db.execute("SELECT json_extract(turn,'$.resolution') FROM disputes_v2").fetchone()[0]
        is None
    )
    assert db.execute("SELECT COUNT(*) FROM escrow_events WHERE kind='SLASH'").fetchone()[0] == 0
    assert (balances(network).admission_locked, balances(network).burned) == (1000, 0)
    retried = network.client.post(network.url + "/resolution", json=verdict)
    assert retried.status_code == 200, retried.text
    assert (balances(network).admission_locked, balances(network).burned) == (0, 1100)


@pytest.mark.parametrize("miner_loses", [True, False])
def test_referee_fraud_against_the_miner_seizes_the_bond_through_the_routes(network, miner_loses):
    # Given: a funded miner that contested its verdict, a referee verdict, posted twice
    identity, _ = lock_bond(network)
    loser = service.HOT[0].ss58 if miner_loses else service.AUDITORS[0].ss58
    verdict = convict_through_the_routes(network, identity, loser)
    # When
    for _ in range(2):
        response = network.client.post(network.url + "/resolution", json=verdict)
        assert response.status_code == 200, response.text
    # Then
    b = balances(network)
    state = network.store._db.execute(
        "SELECT state,reason FROM admissions_v2 WHERE admission_id=?", (identity,)
    ).fetchone()
    assert b.conserved() and b.dispute_locked == 0
    slashes = network.store._db.execute(
        "SELECT COUNT(*) FROM escrow_events WHERE kind='SLASH'"
    ).fetchone()[0]
    if miner_loses:
        # the contest bond (100) and the admission bond (1000) burn, once; the miner is banned
        assert (b.admission_locked, b.burned, slashes) == (0, 1100, 2)
        assert (state["state"], state["reason"]) == ("BANNED", "FINALIZED_TRAINING_FRAUD")
        # and the owner cannot take the bond back
        status = network.client.get(network.url + f"/admission/{service.HOT[0].ss58}").json()
        assert status["bond"] == {"locks": []}
    else:
        # the auditor lost: the miner's bond is intact and it stays suspended pending recovery
        assert (b.admission_locked, b.burned, slashes) == (1000, 0, 0)
        assert state["state"] == "SUSPENDED"


def test_coldkey_alone_cannot_burn_a_registered_unjoined_hotkey_over_http(network):
    # Given: HOT[0] joined; HOT[1] is registered on the subnet but has not joined
    assert network.join(0).status_code == 200
    run_id = network.manifest.run_id()
    body = {
        "hotkey": service.HOT[0].ss58,
        "new_hotkey": service.HOT[1].ss58,
        "operation_id": "ab" * 32,
    }
    envelope = {
        "v": "ht/2",
        "type": "RotateRequest",
        "run_id": run_id,
        "body": body,
        "signer": service.COLD[0].ss58,
        "exp_drand": 10000,
        "sig": service.COLD[0].sign(signing_message("RotateRequest", body, 10000, run_id)).hex(),
    }
    # When: the coldkey signs alone, as the Lens did
    refused = network.client.post(
        network.url + f"/admission/{service.HOT[0].ss58}/rotate",
        content=service.canonicalize(envelope),
    )
    # Then: refused, and the victim joins with its own two keys
    assert refused.status_code == 409, refused.text
    assert network.join(1).status_code == 200
