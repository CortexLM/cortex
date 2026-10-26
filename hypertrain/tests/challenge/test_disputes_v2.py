"""Real funded admission, durable transitions, signed delivery and bounded silence."""

from __future__ import annotations

import importlib.util
import sys
import threading
from dataclasses import replace
from pathlib import Path

import pytest

from hypertrain.auditor.island_bisect import RefereeEvidence
from hypertrain.challenge.disputes_v2 import (
    Availability,
    Contest,
    DisputeError,
    DisputesV2,
    EventAck,
    signed_message,
)
from hypertrain.protocol.envelope_v2 import seal
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages_v2 import (
    BisectV2,
    DisputePolicyV2,
    DisputeV2,
    EscrowLock,
    ResolutionV2,
    RunManifestV2,
)


def fixtures():
    if "l5_escrow_fixture" in sys.modules:
        return sys.modules["l5_escrow_fixture"]
    spec = importlib.util.spec_from_file_location(
        "l5_escrow_fixture",
        Path(__file__).parents[1] / "ledger/test_escrow_v2.py",
    )
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def service(path: Path, *, funded: bool = True, coordinator_allowlisted: bool = False):
    f = fixtures()
    ref = Keypair(b"\x71" * 32)
    next_ref = Keypair(b"\x72" * 32)
    policy = DisputePolicyV2(
        referees=[ref.ss58, next_ref.ss58, *([f.COORD.ss58] if coordinator_allowlisted else [])],
        max_open=32,
        max_transcript_entries=256,
        max_entry_bytes=65536,
        fanout=2,
        max_referee_reassignments=1,
        max_referee_cost_units=100,
    )
    setup = f.setup()
    body = setup.manifest.body()
    body["training"]["inner"].update(H=2, J=1)
    body["network"]["dispute_policy_hash"] = policy.digest()
    setup = replace(setup, manifest=RunManifestV2.model_validate(body))
    escrow = f.ledger(setup, path / "ledger.db")
    origin = f.fund(escrow)[0] if funded else "33" * 32
    contest = Contest(
        verdict_hash="11" * 32,
        challenge_hash="22" * 32,
        w=0,
        miner=f.HOT.ss58,
        auditor=f.AUDITOR.ss58,
        referee=ref.ss58,
        step_span=2,
        layer_span=2,
        op_span=2,
    )
    d = DisputesV2(
        escrow,
        policy,
        f.COORD,
        contest_of=lambda _: contest,
        owner_of=lambda h: f.COLD.ss58 if h == f.HOT.ss58 else f.OTHER.ss58,
        span=lambda c, level, ctx: 2,
    )
    escrow.settlement = d.settlement
    identifier = sha256_hex(canonicalize([escrow.run_id, contest.verdict_hash, contest.miner]))
    lock = EscrowLock(
        operation_id="44" * 32,
        owner=f.COLD.ss58,
        units=100,
        origin_ids=[origin],
        admission_id=None,
        dispute_id=identifier,
        kind="LOCK_CONTEST",
    )
    if funded:
        escrow.lock(lock, signer=f.COLD.ss58)
    request = DisputeV2(
        hotkey=f.HOT.ss58,
        verdict_hash=contest.verdict_hash,
        action="contest",
        bond_lock=lock.operation_id,
    )
    raw = canonicalize(seal(f.HOT, "DisputeV2", escrow.run_id, request, 100000))
    return d, f, ref, next_ref, raw


def reply(d: DisputesV2, key: Keypair, hashes: list[str], now: int = 2):
    turn = d.get(next(iter(d.db.execute("SELECT id FROM disputes_v2")))[0])
    model = BisectV2(
        dispute_id=turn.dispute_id,
        seq=turn.seq,
        level=turn.level,
        ctx=turn.ctx,
        interval=turn.interval,
        N=2,
        hashes=hashes,
        party=key.ss58,
        previous_transcript_hash=turn.transcript_hash,
    )
    raw = canonicalize(seal(key, "BisectV2", d.run_id, model, 100000))
    return d.bisect(raw, now=now), raw


def ack_turn(d: DisputesV2, key: Keypair, now: int):
    event = d.events(key.ss58, 0)[-1]
    ack = EventAck(
        run_id=d.run_id,
        cursor=event.cursor,
        event_hash=event.digest(),
        party=key.ss58,
        received_beacon=now,
        sig="",
    )
    ack = ack.model_copy(update={"sig": key.sign(signed_message(ack)).hex()})
    d.acknowledge(ack, now=now)
    return event


def availability(d: DisputesV2, turn, keys: tuple[Keypair, Keypair], now: int):
    proofs = []
    for key in keys:
        proof = Availability(
            run_id=d.run_id,
            dispute_id=turn.dispute_id,
            seq=turn.seq,
            transcript_hash=turn.transcript_hash,
            observed_beacon=now,
            service_healthy=True,
            beacon_healthy=True,
            reference_healthy=True,
            signer=key.ss58,
            sig="",
        )
        proofs.append(proof.model_copy(update={"sig": key.sign(signed_message(proof)).hex()}))
    return tuple(proofs)


def test_progression_when_pairs_restart_and_duplicates(tmp_path: Path) -> None:
    # Given
    d, f, _, _, raw = service(tmp_path)
    first = d.open(raw, now=1)
    deadline = first.absolute_deadline
    # When
    turn, sent = reply(d, f.HOT, ["11" * 32, "22" * 32, "33" * 32])
    duplicate = d.bisect(sent, now=3)
    assert duplicate == turn  # duplicate never renews turn deadline
    turn, _ = reply(d, f.AUDITOR, ["11" * 32, "aa" * 32, "bb" * 32])
    assert turn.level == "layer" and turn.ctx == [1]
    turn, _ = reply(d, f.HOT, ["11" * 32, "22" * 32, "33" * 32])
    turn, _ = reply(d, f.AUDITOR, ["11" * 32, "aa" * 32, "bb" * 32])
    assert turn.level == "op" and turn.ctx == [1, 0]
    turn, _ = reply(d, f.HOT, ["11" * 32, "22" * 32, "33" * 32])
    turn, _ = reply(d, f.AUDITOR, ["11" * 32, "aa" * 32, "bb" * 32])
    restarted = DisputesV2(
        d.escrow, d.policy, f.COORD, contest_of=d.contest_of, owner_of=d.owner_of, span=d.span
    )
    # Then
    assert restarted.get(first.dispute_id) == turn
    assert restarted.bisect(sent, now=4) == duplicate
    assert turn.paused and turn.seq == 6 and turn.absolute_deadline == deadline
    assert restarted.open(raw, now=4) == turn
    assert d.escrow.balances().dispute_locked == 100


def test_reassign_rejects_coordinator_in_pinned_policy(tmp_path: Path) -> None:
    # Given: policy pinned before ledger construction, actual matured origins/contest lock.
    d, f, ref, _, raw = service(tmp_path, coordinator_allowlisted=True)
    opened = d.open(raw, now=1)
    reply(d, f.HOT, ["11" * 32] * 3)
    before, _ = reply(d, f.AUDITOR, ["11" * 32] * 3)
    balances = d.escrow.balances()
    assert d.policy.digest() == d.escrow.manifest.network.dispute_policy_hash
    assert f.COORD.ss58 in d.policy.referees and balances.dispute_locked == 100
    # When / Then
    with pytest.raises(DisputeError, match="REFEREE_REASSIGNMENT_BOUND"):
        d.reassign(opened.dispute_id, f.COORD.ss58, now=3)
    assert d.get(opened.dispute_id) == before
    assert before.contest.referee == ref.ss58
    assert d.escrow.balances() == balances


def test_settlement_preserves_only_accepted_recovery_linkage(tmp_path: Path) -> None:
    # Given: trusted contest callback supplies stable accepted-record linkage before opening.
    d, f, ref, _, raw = service(tmp_path)
    accepted = d.contest_of("11" * 32).model_copy(
        update={
            "admission_id": "55" * 32,
            "coldkey": f.COLD.ss58,
            "evidence_hash": "66" * 32,
        }
    )
    d.contest_of = lambda _: accepted
    turn = d.open(raw, now=1)
    evidence = RefereeEvidence(
        dispute_id=turn.dispute_id,
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
        dispute_id=turn.dispute_id,
        transcript_hash=turn.transcript_hash,
        reason="MATCH",
        loser=None,
        evidence_hash=evidence.digest(),
    )
    d.resolution(canonicalize(seal(ref, "ResolutionV2", d.run_id, resolution, 100000)), now=3)
    # When / Then: incident evidence is not replaced by resolution evidence or a caller flag.
    status = d.settlement(turn.dispute_id)
    assert status == d.settlement(turn.lock_id)
    assert (
        status.run_id,
        status.admission_id,
        status.coldkey,
        status.dispute_id,
        status.evidence_hash,
    ) == (
        d.run_id,
        accepted.admission_id,
        f.COLD.ss58,
        turn.dispute_id,
        accepted.evidence_hash,
    )
    assert not status.unresolved and status.release_beacon == 3
    with pytest.raises(DisputeError):
        d.settlement("77" * 32)


@pytest.mark.parametrize("level,ctx", [("step", []), ("layer", [1]), ("op", [1, 0])])
@pytest.mark.parametrize("auditor", [False, True])
def test_duplicate_point_contradiction_rejects_without_transition(
    tmp_path: Path,
    level: str,
    ctx: list[int],
    auditor: bool,
) -> None:
    # Given: valid funded live turn at [1,2], three signed coordinates [1,2,2].
    d, f, _, _, raw = service(tmp_path)
    opened = d.open(raw, now=1)
    turn = opened.model_copy(update={"level": level, "ctx": ctx, "interval": (1, 2)})
    with d.escrow.tx():
        d._save(turn)
    if auditor:
        turn, _ = reply(d, f.HOT, ["11" * 32, "22" * 32, "22" * 32])
    before = d.get(opened.dispute_id)
    entries = d.db.execute("SELECT COUNT(*) FROM dispute_entries_v2").fetchone()[0]
    events = d.db.execute("SELECT COUNT(*) FROM dispute_events_v2").fetchone()[0]
    balances = d.escrow.balances()
    # When / Then: wire-valid but semantically contradictory claim cannot advance.
    key = f.AUDITOR if auditor else f.HOT
    with pytest.raises(DisputeError, match="CONTRADICTORY_REPEATED_POINT"):
        reply(d, key, ["11" * 32, "22" * 32, "aa" * 32])
    assert d.get(opened.dispute_id) == before
    assert d.db.execute("SELECT COUNT(*) FROM dispute_entries_v2").fetchone()[0] == entries
    assert d.db.execute("SELECT COUNT(*) FROM dispute_events_v2").fetchone()[0] == events
    assert d.escrow.balances() == balances
    after, _ = reply(d, key, ["11" * 32, "bb" * 32, "bb" * 32])
    assert after.interval[0] < after.interval[1]


def test_referee_reassignment_when_infrastructure_once_only(tmp_path: Path) -> None:
    # Given
    d, f, _, next_ref, raw = service(tmp_path)
    turn = d.open(raw, now=1)
    reply(d, f.HOT, ["11" * 32] * 3)
    paused, _ = reply(d, f.AUDITOR, ["11" * 32] * 3)
    # When
    reassigned = d.reassign(turn.dispute_id, next_ref.ss58, now=3)
    # Then
    assert reassigned.absolute_deadline == paused.absolute_deadline
    assert reassigned.contest.referee == next_ref.ss58 and reassigned.reassignments == 1
    with pytest.raises(DisputeError):
        d.reassign(turn.dispute_id, d.policy.referees[0], now=4)
    assert d.pause(turn.dispute_id).absolute_deadline == paused.absolute_deadline


@pytest.mark.parametrize("miner_loses", [True, False])
def test_losing_lock_when_fraud_evidence_accepted(tmp_path: Path, miner_loses: bool) -> None:
    # Given: internal replay evidence, not a public claimant assertion.
    d, f, ref, _, raw = service(tmp_path)
    accepted = d.contest_of("11" * 32).model_copy(update={"auditor_coldkey": f.OTHER.ss58})
    d.contest_of = lambda _: accepted
    origin = d.db.execute(
        "SELECT origin FROM escrow_units WHERE owner=? AND units>0", (f.OTHER.ss58,)
    ).fetchone()[0]
    other_lock = EscrowLock(
        operation_id="88" * 32,
        owner=f.OTHER.ss58,
        units=200,
        origin_ids=[origin],
        admission_id="99" * 32,
        dispute_id=None,
        kind="LOCK_ADMISSION",
    )
    d.escrow.lock(other_lock, signer=f.OTHER.ss58)
    turn = d.open(raw, now=1)
    loser = f.HOT.ss58 if miner_loses else f.AUDITOR.ss58
    evidence = RefereeEvidence(
        dispute_id=turn.dispute_id,
        transcript_hash=turn.transcript_hash,
        reason="FRAUD",
        loser=loser,
        predecessor_hash="11" * 32,
        inputs_hash="22" * 32,
        output_hash="33" * 32,
        op_spec="verified.actual.op",
    )
    d.register_evidence(evidence)
    resolution = ResolutionV2(
        dispute_id=turn.dispute_id,
        transcript_hash=turn.transcript_hash,
        reason="FRAUD",
        loser=loser,
        evidence_hash=evidence.digest(),
    )
    d.resolution(canonicalize(seal(ref, "ResolutionV2", d.run_id, resolution, 100000)), now=3)
    # When
    d.settle_lock(turn.dispute_id, now=3, referee_cost=10)
    d.settle_lock(turn.dispute_id, now=3, referee_cost=10)
    # Then
    status = d.settlement(turn.dispute_id)
    assert status.loser == loser and status.referee == ref.ss58
    assert status.loser_coldkey == (f.COLD.ss58 if miner_loses else f.OTHER.ss58)
    assert (status.lock_id, status.lock_owner, status.dispute_id) == (
        turn.lock_id,
        f.COLD.ss58,
        turn.dispute_id,
    )
    assert status.resolution_hash == resolution.digest()
    assert status.outcome == ("FRAUD" if miner_loses else "MATCH")
    assert d.escrow.balances().burned == (90 if miner_loses else 0)
    assert d.escrow.balances().dispute_locked == 0
    assert d.escrow.balances(f.COLD.ss58).available == (19900 if miner_loses else 20000)
    assert d.escrow.balances(f.OTHER.ss58).available == (9810 if miner_loses else 9800)
    assert d.escrow.balances(f.OTHER.ss58).admission_locked == 200
    assert d.escrow.balances().conserved()


def test_settlement_rejects_unresolved_or_pre_horizon_lock(tmp_path: Path) -> None:
    # Given: actual funded contest with no accepted resolution.
    d, _, _, _, raw = service(tmp_path)
    turn = d.open(raw, now=1)
    before = d.escrow.balances()
    with pytest.raises(DisputeError, match="UNRESOLVED_CONTEST"):
        d.settle_lock(turn.dispute_id, now=2)
    d.exclude(turn.dispute_id, now=turn.absolute_deadline)
    from hypertrain.ledger.escrow_v2 import EscrowError

    # When / Then: matching closed authority still cannot bypass its release horizon.
    with pytest.raises(EscrowError, match="DISPUTED_OR_UNVESTED"):
        d.settle_lock(turn.dispute_id, now=turn.absolute_deadline - 1)
    assert d.escrow.balances() == before


def test_state_serve_when_request_persisted_and_replayed(tmp_path: Path) -> None:
    from hypertrain.protocol.messages import StateServe

    # Given
    d, f, _, _, raw = service(tmp_path)
    turn = d.open(raw, now=1)
    serve = StateServe(
        hotkey=f.HOT.ss58,
        challenge_hash=turn.contest.challenge_hash,
        t=1,
        uri="https://objects.invalid/checkpoint",
        tensor_root="33" * 32,
        merkle_proof_leaf_in_leaves_root=[],
    )
    signed = canonicalize(seal(f.HOT, "StateServe", d.run_id, serve, 100000))
    with pytest.raises(DisputeError):
        d.state_serve(signed, now=2)
    d.state_request(turn.dispute_id, 1, now=2)
    # When
    assert d.state_serve(signed, now=2) == serve
    assert d.state_serve(signed, now=3) == serve
    # Then
    assert d.db.execute("SELECT COUNT(*) FROM dispute_states_v2").fetchone()[0] == 1


@pytest.mark.parametrize(
    "field,value",
    [
        ("seq", 1),
        ("previous_transcript_hash", "aa" * 32),
        ("party", "auditor"),
        ("ctx", [1]),
        ("interval", (0, 1)),
    ],
)
def test_rejects_when_turn_binding_changes(tmp_path: Path, field: str, value) -> None:
    # Given
    d, f, _, _, raw = service(tmp_path)
    turn = d.open(raw, now=1)
    model = dict(
        dispute_id=turn.dispute_id,
        seq=0,
        level="step",
        ctx=[],
        interval=(0, 2),
        N=2,
        hashes=["11" * 32] * 3,
        party=f.HOT.ss58,
        previous_transcript_hash=turn.transcript_hash,
    )
    model[field] = f.AUDITOR.ss58 if value == "auditor" else value
    from pydantic import ValidationError

    # When / Then
    from hypertrain.protocol.envelope import UnauthorizedSigner

    with pytest.raises((DisputeError, ValidationError, UnauthorizedSigner)):
        m = BisectV2.model_validate(model)
        d.bisect(canonicalize(seal(f.HOT, "BisectV2", d.run_id, m, 100000)), now=2)
    assert d.get(turn.dispute_id) == turn


def test_unfunded_flood_when_hundred_contests(tmp_path: Path) -> None:
    # Given
    d, _, _, _, raw = service(tmp_path, funded=False)
    # When / Then
    for _ in range(100):
        with pytest.raises(DisputeError, match="UNFUNDED"):
            d.open(raw, now=1)
    assert d.db.execute("SELECT COUNT(*) FROM disputes_v2").fetchone()[0] == 0
    assert d.db.execute("SELECT COUNT(*) FROM dispute_events_v2").fetchone()[0] == 0


def test_queue_capacity_when_thirty_two_funded_contests(tmp_path: Path) -> None:
    # Given
    d, f, ref, _, _ = service(tmp_path)
    origin = d.db.execute(
        "SELECT origin FROM escrow_units WHERE owner=? AND units>0", (f.COLD.ss58,)
    ).fetchone()[0]
    for i in range(33):
        key = Keypair(bytes([150 + i]) * 32)
        contest = Contest(
            verdict_hash=f"{i + 1:064x}",
            challenge_hash="22" * 32,
            w=0,
            miner=key.ss58,
            auditor=f.AUDITOR.ss58,
            referee=ref.ss58,
            step_span=2,
            layer_span=2,
            op_span=2,
        )
        d.contest_of = lambda _, contest=contest: contest
        d.owner_of = lambda _: f.COLD.ss58
        identifier = sha256_hex(canonicalize([d.run_id, contest.verdict_hash, key.ss58]))
        lock = EscrowLock(
            operation_id=f"{i + 200:064x}",
            owner=f.COLD.ss58,
            units=100,
            origin_ids=[origin],
            admission_id=None,
            dispute_id=identifier,
            kind="LOCK_CONTEST",
        )
        d.escrow.lock(lock, signer=f.COLD.ss58)
        raw = canonicalize(
            seal(
                key,
                "DisputeV2",
                d.run_id,
                DisputeV2(
                    hotkey=key.ss58,
                    verdict_hash=contest.verdict_hash,
                    action="contest",
                    bond_lock=lock.operation_id,
                ),
                100000,
            )
        )
        # When / Then
        if i < 32:
            d.open(raw, now=1)
        else:
            with pytest.raises(DisputeError, match="QUEUE_CAPACITY"):
                d.open(raw, now=1)
    assert d.db.execute("SELECT COUNT(*) FROM disputes_v2").fetchone()[0] == 32


@pytest.mark.parametrize("miner_silent", [False, True])
def test_timeout_when_authenticated_delivery_healthy(tmp_path: Path, miner_silent: bool) -> None:
    # Given
    d, f, ref, _, raw = service(tmp_path)
    turn = d.open(raw, now=1)
    if not miner_silent:
        turn, _ = reply(d, f.HOT, ["11" * 32] * 3)
    key = f.HOT if miner_silent else f.AUDITOR
    now = turn.turn_deadline + 1
    proofs = availability(d, turn, (f.COORD, ref), now)
    with pytest.raises(DisputeError, match="NO_AUTHENTICATED"):
        d.timeout(turn.dispute_id, proofs, now=now)
    ack_turn(d, key, 2)
    # When
    settled = d.timeout(turn.dispute_id, proofs, now=now)
    d.settle_lock(turn.dispute_id, now=now)
    # Then
    assert settled.resolution.loser == key.ss58
    assert settled.resolution.reason == ("PARTY_TIMEOUT" if miner_silent else "AUDITOR_FAULT")
    assert d.escrow.balances().burned == 0 and d.escrow.balances().dispute_locked == 0


def test_timeout_rejects_when_unhealthy_or_duplicate_observer(tmp_path: Path) -> None:
    # Given
    d, f, ref, _, raw = service(tmp_path)
    turn = d.open(raw, now=1)
    ack_turn(d, f.HOT, 2)
    now = turn.turn_deadline + 1
    good = availability(d, turn, (f.COORD, ref), now)
    # When / Then
    with pytest.raises(DisputeError, match="UNCONFIRMED"):
        d.timeout(turn.dispute_id, (good[0], good[0]), now=now)
    with pytest.raises(DisputeError, match="UNCONFIRMED"):
        d.timeout(
            turn.dispute_id,
            (good[0], good[1].model_copy(update={"service_healthy": False})),
            now=now,
        )
    assert d.get(turn.dispute_id).resolution is None


def test_resolution_when_only_independent_verified_referee(tmp_path: Path) -> None:
    # Given
    d, f, ref, _, raw = service(tmp_path)
    turn = d.open(raw, now=1)
    evidence = RefereeEvidence(
        dispute_id=turn.dispute_id,
        transcript_hash=turn.transcript_hash,
        reason="MATCH",
        loser=None,
        predecessor_hash="11" * 32,
        inputs_hash="22" * 32,
        output_hash="33" * 32,
        op_spec="full-match",
    )
    resolution = ResolutionV2(
        dispute_id=turn.dispute_id,
        transcript_hash=turn.transcript_hash,
        reason="MATCH",
        loser=None,
        evidence_hash=evidence.digest(),
    )
    from hypertrain.protocol.envelope import UnauthorizedSigner

    with pytest.raises(UnauthorizedSigner):
        d.resolution(
            canonicalize(seal(f.AUDITOR, "ResolutionV2", d.run_id, resolution, 100000)), now=2
        )
    with pytest.raises(DisputeError, match="UNVERIFIED"):
        d.resolution(canonicalize(seal(ref, "ResolutionV2", d.run_id, resolution, 100000)), now=2)
    d.register_evidence(evidence)
    # When
    settled = d.resolution(
        canonicalize(seal(ref, "ResolutionV2", d.run_id, resolution, 100000)), now=2
    )
    d.settle_lock(turn.dispute_id, now=2)
    # Then
    assert settled.resolution.reason == "MATCH" and d.escrow.balances().dispute_locked == 0


def test_absolute_bound_when_infrastructure_exhausted(tmp_path: Path) -> None:
    # Given
    d, _, _, _, raw = service(tmp_path)
    turn = d.open(raw, now=1)
    # When
    settled = d.exclude(turn.dispute_id, now=turn.absolute_deadline)
    d.settle_lock(turn.dispute_id, now=turn.absolute_deadline)
    # Then
    assert settled.resolution.reason == "INFRASTRUCTURE" and settled.resolution.loser is None
    assert d.escrow.balances().burned == 0


def test_events_when_subscribed_before_transition(tmp_path: Path) -> None:
    # Given: condition acquired before trigger; wait_for rechecks durable table under the same lock.
    d, _, _, _, raw = service(tmp_path)
    ready = threading.Event()
    result = []

    def consumer():
        with d.condition:
            ready.set()
            result.extend(d.events(fixtures().HOT.ss58, 0, timeout=2))

    worker = threading.Thread(target=consumer)
    worker.start()
    assert ready.wait(2)
    # When
    d.open(raw, now=1)
    worker.join(3)
    # Then
    assert not worker.is_alive() and len(result) == 1
    with pytest.raises(DisputeError):
        d.events(result[0].party, -1, timeout=0)
    with pytest.raises(DisputeError):
        d.events(result[0].party, 0, timeout=31)
