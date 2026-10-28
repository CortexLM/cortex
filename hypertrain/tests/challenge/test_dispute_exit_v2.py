"""Disputes that nobody can decide: which open turns the server may close, and what it records."""

from __future__ import annotations

from pathlib import Path
from typing import Any

import pytest

from hypertrain.auditor.island_bisect import RefereeEvidence
from hypertrain.challenge.admission import Admission
from hypertrain.challenge.admission_store import AdmissionError
from hypertrain.challenge.disputes_v2 import DisputesV2, Turn
from hypertrain.ledger.escrow_v2 import EscrowError
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages_v2 import BisectV2, DisputeV2, EscrowLock, ResolutionV2

from . import test_service_network_v2 as fixtures
from .test_disputes_v2 import ack_turn, service

network = fixtures.network

# Wire-visible evidence tags: a rename is a change of bytes, so they are literals here.
HORIZON = "bounded-infrastructure-exclusion"
FROZEN = "unadjudicable-paused-transcript"
ABANDONED = "abandoned-turn"
FINAL_OP = {"paused": True, "level": "op", "ctx": [1, 0], "interval": (1, 2)}


def instants(turn: Turn, T: int) -> dict[str, int]:
    return {
        "early": 2,
        "grace-end": turn.turn_deadline + T,
        "grace-over": turn.turn_deadline + T + 1,
        "horizon": turn.absolute_deadline,
        "past-horizon": turn.absolute_deadline + 1,
    }


def clamped(turn: Turn) -> dict[str, int]:
    """A late move: bisect() clamps turn_deadline to the absolute horizon."""
    return {"turn_deadline": turn.absolute_deadline}


def evidence_tag(db, closed: Turn) -> str:
    assert closed.resolution is not None
    body = db.execute(
        "SELECT body FROM dispute_evidence_v2 WHERE hash=?", (closed.resolution.evidence_hash,)
    ).fetchone()[0]
    return RefereeEvidence.model_validate_json(body).op_spec


@pytest.mark.parametrize(
    "update,at,expected",
    [
        pytest.param({}, "grace-end", None, id="in-grace"),
        pytest.param({}, "grace-over", ABANDONED, id="after-grace"),
        pytest.param({}, "early", None, id="before-turn-deadline"),
        pytest.param({}, "past-horizon", HORIZON, id="past-horizon-wins-over-grace"),
        pytest.param(clamped, "horizon", None, id="clamped-turn-at-horizon"),
        pytest.param(clamped, "past-horizon", HORIZON, id="clamped-turn-past-horizon"),
        pytest.param({"paused": True}, "early", FROZEN, id="step-pause"),
        pytest.param({"paused": True, "interval": (0, 1)}, "early", FROZEN, id="step-pause-narrow"),
        pytest.param(
            {"paused": True, "level": "layer", "ctx": [1]}, "early", FROZEN, id="layer-pause"
        ),
        pytest.param({**FINAL_OP, "interval": (0, 2)}, "early", FROZEN, id="op-pause-wide"),
        pytest.param(FINAL_OP, "early", None, id="final-op-pause"),
        pytest.param(FINAL_OP, "horizon", None, id="final-op-pause-at-horizon"),
        pytest.param(FINAL_OP, "past-horizon", HORIZON, id="final-op-pause-past-horizon"),
        pytest.param(
            {**FINAL_OP, "paused": False}, "grace-end", None, id="final-op-unpaused-in-grace"
        ),
    ],
)
def test_blocked_cause_table(tmp_path: Path, update, at: str, expected: str | None) -> None:
    # Given: a funded contest opened at round 1; the table varies only its shape and the round.
    d, _, _, _, raw = service(tmp_path)
    turn = d.open(raw, now=1)
    T = d.escrow.manifest.training.verify.T.dispute_per_level
    candidate = turn.model_copy(update=update(turn) if callable(update) else update)
    # When / Then
    assert d.blocked_cause(candidate, instants(turn, T)[at]) == expected


def test_resolved_turn_is_never_blocked(tmp_path: Path) -> None:
    # Given: a contest the horizon has already closed.
    d, _, _, _, raw = service(tmp_path)
    turn = d.open(raw, now=1)
    closed = d.exclude(turn.dispute_id, now=turn.absolute_deadline + 1)
    assert closed.resolution is not None
    # When / Then: no later round makes it closable again.
    assert d.blocked_cause(closed, closed.absolute_deadline + 5) is None


def test_acknowledged_silence_is_abandoned_all_the_same(tmp_path: Path) -> None:
    # Given: the expected party acknowledged the turn, then went silent.
    d, f, _, _, raw = service(tmp_path)
    turn = d.open(raw, now=1)
    ack_turn(d, f.HOT, 2)
    T = d.escrow.manifest.training.verify.T.dispute_per_level
    # When / Then: an acknowledgement does not extend the grace window.
    assert d.blocked_cause(turn, turn.turn_deadline + T) is None
    assert d.blocked_cause(turn, turn.turn_deadline + T + 1) == ABANDONED


@pytest.mark.parametrize(
    "extra,tag",
    [
        pytest.param({}, HORIZON, id="default"),
        pytest.param({"cause": FROZEN}, FROZEN, id="frozen"),
        pytest.param({"cause": ABANDONED}, ABANDONED, id="abandoned"),
    ],
)
def test_exclusion_evidence_names_its_cause(tmp_path: Path, extra: dict, tag: str) -> None:
    # Given: a paused contest, the one shape exclude() accepts before the horizon.
    d, _, _, _, raw = service(tmp_path)
    turn = d.open(raw, now=1)
    d.pause(turn.dispute_id)
    # When
    closed = d.exclude(turn.dispute_id, now=2, **extra)
    # Then: the omitted cause keeps the bytes exclude() has always written.
    assert evidence_tag(d.db, closed) == tag


# The store half drives the real routes of the network fixture. The contest record that the audit
# would have written is seeded; the funding and the dispute itself go through /escrow/lock and
# /dispute.
W = 0
VERDICT = sha256_hex(b"dsp01-verdict")
HOT, COLD, AUDITORS, REFEREE = fixtures.HOT, fixtures.COLD, fixtures.AUDITORS, fixtures.REFEREE
AGREE = ["11" * 32] * 3


def disputes(net) -> DisputesV2:
    return net.store._services(net.manifest.run_id())[2]


def post_dispute(net) -> tuple[str, Any]:
    """Admitted miner 0, accepted contest record, funded LOCK_CONTEST, signed /dispute: the
    dispute id and the response, whatever it is."""
    assert net.join(0).status_code == 200
    run_id, store, inner = net.manifest.run_id(), net.store, net.manifest.training.inner
    admission_id = store._db.execute(
        "SELECT admission_id FROM admissions_v2 WHERE hotkey=?", (HOT[0].ss58,)
    ).fetchone()[0]
    contest = {
        "verdict_hash": VERDICT,
        "challenge_hash": sha256_hex(b"dsp01-challenge"),
        "w": W,
        "miner": HOT[0].ss58,
        "auditor": AUDITORS[0].ss58,
        "referee": REFEREE.ss58,
        "step_span": inner.H // inner.J,
        "layer_span": 2,
        "op_span": 2,
        "admission_id": admission_id,
        "coldkey": COLD[0].ss58,
        "evidence_hash": sha256_hex(b"dsp01-evidence"),
        "auditor_coldkey": None,
    }
    with store._lock:
        store._put_record_v2(run_id, "contest", VERDICT, contest)
        store._put_record_v2(run_id, "finality", str(W), {"w": W})
        store._put_record_v2(run_id, "excluded", f"{W}:{HOT[0].ss58}", {"reason": "UNVERIFIED"})
    dispute_id = sha256_hex(canonicalize([run_id, VERDICT, HOT[0].ss58]))
    origin = store._db.execute(
        "SELECT origin FROM escrow_units WHERE owner=? AND bucket='available' AND units>=100",
        (COLD[0].ss58,),
    ).fetchone()[0]
    lock = EscrowLock(
        operation_id="ce" * 32,
        owner=COLD[0].ss58,
        units=100,
        origin_ids=[origin],
        admission_id=None,
        dispute_id=dispute_id,
        kind="LOCK_CONTEST",
    )
    response = net.client.post(
        net.url + "/escrow/lock", json=net.signed(COLD[0], "EscrowLock", lock)
    )
    assert response.status_code == 200, response.text
    body = DisputeV2(
        hotkey=HOT[0].ss58, verdict_hash=VERDICT, action="contest", bond_lock=lock.operation_id
    )
    response = net.client.post(net.url + "/dispute", json=net.signed(HOT[0], "DisputeV2", body))
    return dispute_id, response


def open_dispute(net) -> str:
    """post_dispute, which the store must accept."""
    dispute_id, response = post_dispute(net)
    assert response.status_code == 200, response.text
    return dispute_id


def answer(net, key, dispute_id: str, hashes: list[str]) -> None:
    turn = disputes(net).get(dispute_id)
    body = BisectV2(
        dispute_id=dispute_id,
        seq=turn.seq,
        level=turn.level,
        ctx=turn.ctx,
        interval=turn.interval,
        N=2,
        hashes=hashes,
        party=key.ss58,
        previous_transcript_hash=turn.transcript_hash,
    )
    response = net.client.post(net.url + "/bisect", json=net.signed(key, "BisectV2", body))
    assert response.status_code == 200, response.text


def unresolved(net) -> int:
    return net.store._record_v2(net.manifest.run_id(), "finality", str(W))["unresolved_disputes"]


def test_dead_end_contest_closes_at_the_next_beacon(network) -> None:
    # Given: the Lens S2 shape. Both parties tell the same truth at step level, the turn pauses
    # off the final op, referee_v2 answers 409 and nothing else can end the contest.
    run_id, store = network.manifest.run_id(), network.store
    escrow = store._services(run_id)[0]
    before = escrow.balances()
    dispute_id = open_dispute(network)
    answer(network, HOT[0], dispute_id, AGREE)
    answer(network, AUDITORS[0], dispute_id, AGREE)
    frozen = disputes(network).get(dispute_id)
    assert (frozen.paused, frozen.level, frozen.resolution) == (True, "step", None)
    assert unresolved(network) == 1 and escrow.balances().dispute_locked == 100
    excluded = store._record_v2(run_id, "excluded", f"{W}:{HOT[0].ss58}")
    # When: the next verified beacon round arrives.
    network.push(network.now + 1)
    # Then: closed without a loser, bond back in full, the audit's own exclusion untouched.
    closed = disputes(network).get(dispute_id)
    assert closed.resolution is not None
    assert (closed.resolution.reason, closed.resolution.loser) == ("INFRASTRUCTURE", None)
    assert closed.resolved_beacon == network.now
    assert evidence_tag(store._db, closed) == FROZEN
    assert escrow.balances() == before and escrow.balances().burned == 0
    assert unresolved(network) == 0
    assert store._record_v2(run_id, "excluded", f"{W}:{HOT[0].ss58}") == excluded


def test_silent_contest_closes_after_the_grace_window(network) -> None:
    # Given: a funded contest whose miner never answers.
    escrow = network.store._services(network.manifest.run_id())[0]
    before = escrow.balances()
    dispute_id = open_dispute(network)
    opened = disputes(network).get(dispute_id)
    T = escrow.manifest.training.verify.T.dispute_per_level
    # When / Then: the operator's window, turn_deadline up to turn_deadline + T, leaves it open.
    network.push(opened.turn_deadline + T)
    assert disputes(network).get(dispute_id).resolution is None
    # When / Then: the next round closes it.
    network.push(opened.turn_deadline + T + 1)
    closed = disputes(network).get(dispute_id)
    assert closed.resolution is not None
    assert (closed.resolution.reason, closed.resolution.loser) == ("INFRASTRUCTURE", None)
    assert evidence_tag(network.store._db, closed) == ABANDONED
    assert escrow.balances() == before and escrow.balances().burned == 0
    assert unresolved(network) == 0


@pytest.mark.parametrize(
    "past,closed_by_push,status,final_reason",
    [
        pytest.param(0, False, 200, "MATCH", id="horizon"),
        pytest.param(1, True, 409, "INFRASTRUCTURE", id="after-horizon"),
    ],
)
def test_horizon_leaves_the_referee_its_last_round(
    network, past: int, closed_by_push: bool, status: int, final_reason: str
) -> None:
    # Given: a paused final op, the shape the referee can score, and its signed MATCH resolution.
    # The cheap fixture writes no span records, so the shape is set on the stored turn.
    dispute_id = open_dispute(network)
    d = disputes(network)
    turn = d.get(dispute_id)
    with network.store._tx():
        d._save(turn.model_copy(update=FINAL_OP))
    turn = d.get(dispute_id)
    evidence = RefereeEvidence(
        dispute_id=dispute_id,
        transcript_hash=turn.transcript_hash,
        reason="MATCH",
        loser=None,
        predecessor_hash="11" * 32,
        inputs_hash="22" * 32,
        output_hash="33" * 32,
        op_spec="accepted-anchor-replay",
    )
    d.register_evidence(evidence)
    resolution = ResolutionV2(
        dispute_id=dispute_id,
        transcript_hash=turn.transcript_hash,
        reason="MATCH",
        loser=None,
        evidence_hash=evidence.digest(),
    )
    # When: the beacon reaches the last round resolution() accepts, or the one after it.
    network.push(turn.absolute_deadline + past)
    assert (d.get(dispute_id).resolution is not None) == closed_by_push
    response = network.client.post(
        network.url + "/resolution",
        json=network.signed(REFEREE, "ResolutionV2", resolution, expiry=10**6),
    )
    # Then: at the horizon the referee still decides; one round later the server already did.
    assert response.status_code == status, response.text
    final = d.get(dispute_id)
    assert final.resolution is not None and final.resolution.reason == final_reason
    if closed_by_push:
        assert evidence_tag(network.store._db, final) == HORIZON


@pytest.mark.parametrize(
    "make,journaled",
    [
        pytest.param(lambda: EscrowError("OPERATION_CONFLICT"), "EscrowError:OPERATION_CONFLICT"),
        pytest.param(lambda: KeyError("boom"), "KeyError"),
    ],
    ids=["coded", "uncoded"],
)
def test_failing_close_is_journaled_once_and_keeps_the_revision(
    network, monkeypatch, make, journaled: str
) -> None:
    # Given: a contest past its horizon whose bond release fails.
    run_id, store = network.manifest.run_id(), network.store
    escrow = store._services(run_id)[0]
    dispute_id = open_dispute(network)
    horizon = disputes(network).get(dispute_id).absolute_deadline

    def failing_release(self, *args, **kwargs):
        raise make()

    monkeypatch.setattr(DisputesV2, "settle_lock", failing_release)
    # When: the beacon route runs the closure.
    network.push(horizon + 1)
    # Then: the route answered 200 (push asserts it), the savepoint undid the exclusion, and the
    # failure is journaled.
    assert disputes(network).get(dispute_id).resolution is None
    assert escrow.balances().dispute_locked == 100
    assert store._record_v2(run_id, "dispute-exit-failed", dispute_id) == {
        "stage": "close",
        "error": journaled,
    }
    # The same failure at the next beacon leaves the rollback revision untouched.
    revision = store._rollback_snapshot_v2(run_id)
    network.push(network.now + 1)
    assert store._rollback_snapshot_v2(run_id) == revision
    # And the closure is retried once the cause is gone.
    monkeypatch.undo()
    network.push(network.now + 1)
    closed = disputes(network).get(dispute_id)
    assert closed.resolution is not None and closed.resolution.reason == "INFRASTRUCTURE"
    assert escrow.balances().dispute_locked == 0


def status(net) -> tuple[str, int, int, str]:
    """State, pending dispute flag, clean trials and last transition reason of miner 0."""
    row = net.store._db.execute(
        "SELECT admission_id,state,pending_dispute,clean_count FROM admissions_v2 WHERE hotkey=?",
        (HOT[0].ss58,),
    ).fetchone()
    reason = net.store._db.execute(
        "SELECT reason FROM admission_transitions WHERE admission_id=? ORDER BY seq DESC LIMIT 1",
        (row["admission_id"],),
    ).fetchone()[0]
    return row["state"], row["pending_dispute"], row["clean_count"], reason


def test_closed_contest_lifts_the_suspension(network) -> None:
    # Given: opening the contest suspended the contested miner until a settlement resumes it.
    admission = network.store._services(network.manifest.run_id())[1]
    dispute_id = open_dispute(network)
    assert status(network) == ("SUSPENDED", 1, 0, "PENDING_DISPUTE")
    strikes = admission.store.record(HOT[0].ss58).strikes
    opened = disputes(network).get(dispute_id)
    T = admission.escrow.manifest.training.verify.T.dispute_per_level
    # When: the server closes the silent contest.
    network.push(opened.turn_deadline + T + 1)
    # Then: an infrastructure fault leaves the miner on a fresh probation, strikes untouched.
    assert status(network) == ("PROBATION", 0, 0, "RESUME_FRESH_PROBATION")
    assert admission.store.record(HOT[0].ss58).strikes == strikes
    assert (
        network.store._db.execute(
            "SELECT COUNT(*) FROM admission_pending WHERE resolved=0"
        ).fetchone()[0]
        == 0
    )


def test_failing_lift_keeps_the_closure_and_is_journaled(network, monkeypatch) -> None:
    # Given: a contest whose miner cannot be resumed.
    def failing_lift(self, *args, **kwargs):
        raise AdmissionError("PENDING_SETTLEMENT")

    monkeypatch.setattr(Admission, "resolve_pending", failing_lift)
    run_id = network.manifest.run_id()
    dispute_id = open_dispute(network)
    opened = disputes(network).get(dispute_id)
    T = network.store._services(run_id)[0].manifest.training.verify.T.dispute_per_level
    # When: the server closes the silent contest.
    network.push(opened.turn_deadline + T + 1)
    # Then: the closure stands, the suspension too, and the failure is journaled apart.
    closed = disputes(network).get(dispute_id)
    assert closed.resolution is not None and closed.resolution.reason == "INFRASTRUCTURE"
    assert status(network)[:2] == ("SUSPENDED", 1)
    assert network.store._record_v2(run_id, "dispute-exit-failed", dispute_id) == {
        "stage": "admission",
        "error": "AdmissionError:PENDING_SETTLEMENT",
    }
