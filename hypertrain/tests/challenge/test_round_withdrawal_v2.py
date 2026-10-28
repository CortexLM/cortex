"""A round finality closed without a reward is recorded once; later rounds still mint."""
# ruff: noqa: F811  (the `network` fixture parameter shadows the imported fixture)

from __future__ import annotations

import json
from dataclasses import asdict, replace

import pytest

from hypertrain.challenge.finality_v2 import RoundStatus, exclude, require_open, retire
from hypertrain.ledger.escrow_v2 import (
    EscrowError,
    EscrowV2,
    FinalityEvidence,
    RewardWork,
    authority_message,
    digest,
)
from hypertrain.protocol.envelope_v2 import EnvelopeV2, seal
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages import Finalize, ReplayEnv, ReplayVerdict
from hypertrain.protocol.messages_v2 import (
    AuditChallengeV2,
    CommitV2,
    EscrowTransfer,
    OriginAllocation,
    RewardFinalize,
)

from . import test_service_network_v2 as N
from .test_service_network_v2 import network  # noqa: F401  (pytest fixture import)

H = "11" * 32
THETA = "bb" * 32


def signed_round(n, escrow: EscrowV2, w: int) -> tuple[Finalize, RewardFinalize, FinalityEvidence]:
    """Coordinator-signed Finalize and RewardFinalize for round w over one audited MATCH commit."""
    manifest, run = n.manifest, n.manifest.run_id()
    hot, cold = N.HOT[0], N.COLD[0]
    samples = tuple(range(manifest.training.batch_samples()))
    final = Finalize(w=w, final_theta_hash_w1=THETA, included=[hot.ss58], entitlements_root=H)
    commit = CommitV2(
        w=w,
        hotkey=hot.ss58,
        leaf_scheme="ht-leaf-v1",
        n_leaves=manifest.training.inner.H // manifest.training.inner.J + 1,
        leaves_root="aa" * 32,
        metrics_root=H,
        final_theta_hash=THETA,
        ef_in_hash=H,
        ef_out_hash=H,
        delta_hash="cc" * 32,
        delta_bytes=1,
        tokens=len(samples) * manifest.training.model.seq_len,
    )
    challenge = AuditChallengeV2(
        w=w,
        target=hot.ss58,
        beacon_round=1,
        beacon_sig_sha256=H,
        mode="full",
        segments=[],
        reasons=["probation"],
        serve_deadline=200,
        anchor_hash=H,
        audit_mode="anchored-full",
    )
    reference = manifest.training.reference_spec
    verdict = ReplayVerdict(
        challenge_hash=digest(challenge),
        first_bad_leaf=None,
        result="MATCH",
        recomputed_leaves_root=commit.leaves_root,
        replay_env=ReplayEnv(
            image_digest=reference.image_digest,
            driver=reference.driver_allowlist[0],
            gpu_uuid_sha256=H,
            sm_count=1,
        ),
    )

    def sealed(key, kind, body) -> EnvelopeV2:
        return EnvelopeV2.model_validate(seal(key, kind, run, body, 200))

    evidence = FinalityEvidence(
        sealed(N.COORD, "Finalize", final),
        (
            RewardWork(
                sealed(hot, "CommitV2", commit),
                sealed(N.AUDITORS[0], "ReplayVerdict", verdict),
                sealed(N.COORD, "AuditChallengeV2", challenge),
                cold.ss58,
                samples,
            ),
        ),
        "dd" * 32,
        False,
        10,
        20,
    )
    budget = escrow.policy.round_reward_units
    origin = OriginAllocation(
        origin_id=escrow.reward_origin(w, hot.ss58, digest(final)),
        owner=cold.ss58,
        units=budget,
        mature_at=20,
    )
    unsigned = RewardFinalize(
        run_id=run,
        w=w,
        finalize_hash=digest(final),
        tape_hash=evidence.tape_hash,
        verdict_root=sha256_hex(canonicalize([digest(verdict)])),
        allocation_hash=sha256_hex(canonicalize([origin.body()])),
        budget_units=budget,
        origin_ids=[origin.origin_id],
        mature_at=20,
        authority_sig="0" * 128,
    )
    reward = RewardFinalize.model_validate(
        {**unsigned.body(), "authority_sig": N.COORD.sign(authority_message(unsigned)).hex()}
    )
    return final, reward, evidence


def put_finality(store, status: RoundStatus) -> None:
    with store._tx():
        store._put_record_v2(status.run_id, "finality", str(status.w), asdict(status))


def unresolved(run: str, w: int, *, applied: bool = True) -> RoundStatus:
    """Round w with no outstanding audit or dispute, neither settled nor excluded."""
    return RoundStatus(run, w, applied, "PENDING", 0, 0, False, THETA, None)


@pytest.fixture
def ledger(network, monkeypatch) -> EscrowV2:
    """The run's own escrow over the store database; external replay and assignment are fixed."""
    escrow = network.store._services(network.manifest.run_id())[0]
    samples = tuple(range(network.manifest.training.batch_samples()))
    monkeypatch.setattr(escrow, "replay_finality", lambda _: THETA)
    monkeypatch.setattr(escrow, "accepted_at", lambda _: 1)  # the evidence is synthetic
    monkeypatch.setattr(escrow, "owner_of", lambda _: N.COLD[0].ss58)
    monkeypatch.setattr(escrow, "assignment_of", lambda _w, _hotkey: samples)
    return escrow


def minted_since(escrow: EscrowV2, issued_before: int) -> int:
    assert escrow.balances().conserved()
    return escrow.balances().issued - issued_before


def closed_unpaid(run: str, w: int) -> RoundStatus:
    """Round w excluded and applied: closed without a reward, as finality_v2 leaves it."""
    retired = replace(exclude(unresolved(run, w, applied=False), H), applied=True)
    assert retired.closed
    return retired


def rolled_back(run: str, w: int) -> RoundStatus:
    """The record rollback_v2 writes for both rounds it repairs."""
    return replace(
        unresolved(run, w), disposition="EXCLUDED", rollback_complete=True, resolution_hash=H
    )


def withdrawals(ledger: EscrowV2) -> list[dict]:
    rows = ledger.db.execute("SELECT request FROM escrow_events WHERE kind='ROUND_WITHDRAWN'")
    return [json.loads(r["request"]) for r in rows]


# Rounds 0..n-1 as finality holds them, round n is offered to the ledger. A round that cannot be
# paid or repaired any more is recorded and passed over; every other state keeps blocking.
def retired(run: str, w: int) -> RoundStatus:
    """A round that could not reach a tape, closed with the model unchanged (ADR 0011 Part 8)."""
    return retire(replace(unresolved(run, w), applied=False), "00" * 32, H)


CLOSURES = {
    "whole-round-exclusion": ((closed_unpaid, closed_unpaid), [0, 1]),
    "accepted-rollback": ((rolled_back, rolled_back), [0, 1]),
    "retired": ((retired, retired), [0, 1]),
    "pending": ((unresolved,), []),
    "settled": (
        (lambda run, w: replace(unresolved(run, w), disposition="SETTLED", resolution_hash=H),),
        [],
    ),
    "awaiting-rollback": ((lambda run, w: exclude(unresolved(run, w), H),), []),
    "open-audit": ((lambda run, w: replace(closed_unpaid(run, w), unresolved_audits=1),), []),
    "open-dispute": ((lambda run, w: replace(closed_unpaid(run, w), unresolved_disputes=1),), []),
    "no-record": ((lambda run, w: None,), []),
}


@pytest.mark.parametrize(("rounds", "recorded"), CLOSURES.values(), ids=CLOSURES.keys())
def test_next_round_mints_only_when_the_previous_rounds_closed_without_reward(
    network, ledger: EscrowV2, rounds, recorded
) -> None:
    # Given: rounds 0..n-1 in the state under test and round n applied.
    store, run, offered = network.store, network.manifest.run_id(), len(rounds)
    _, reward, evidence = signed_round(network, ledger, offered)
    for w, status in enumerate(rounds):
        if (record := status(run, w)) is not None:
            put_finality(store, record)
    if recorded:
        require_open(run, offered, store._history_v2(run))
    put_finality(store, unresolved(run, offered))
    issued = ledger.balances().issued
    # When: the reward of round n is finalized with signed evidence.
    if recorded:
        ledger.reward_finalize(reward, evidence)
        # Then: the honest work of round n is paid and each closed round is recorded once.
        assert minted_since(ledger, issued) == ledger.policy.round_reward_units
        assert [(w["w"], w["closure_hash"]) for w in withdrawals(ledger)] == [
            (w, H) for w in recorded
        ]
        ledger.verify()
    else:
        # Then: nothing is recorded, nothing is minted, the order still blocks.
        with pytest.raises(EscrowError, match="FINALIZE_ORDER"):
            ledger.reward_finalize(reward, evidence)
        assert (withdrawals(ledger), minted_since(ledger, issued)) == ([], 0)


@pytest.mark.parametrize("paid_inputs", [0, 1], ids=["nobody-paid", "one-input-paid"])
def test_a_round_finalized_with_no_paid_input_is_passed_over_and_a_paid_one_is_not(
    network, ledger: EscrowV2, paid_inputs: int
) -> None:
    # Given: round 0 settled by a Finalize that lists `paid_inputs` hotkeys (none when every
    # input of its tape was banned), and round 1 applied.
    store, run = network.store, network.manifest.run_id()
    final0, _, _ = signed_round(network, ledger, 0)
    _, reward1, evidence1 = signed_round(network, ledger, 1)
    final0 = final0.model_copy(update={"included": final0.included[:paid_inputs]})
    settled = replace(unresolved(run, 0), disposition="SETTLED", resolution_hash=H)
    put_finality(store, settled)
    with store._tx():
        envelope = EnvelopeV2.model_validate(seal(N.COORD, "Finalize", run, final0, 200))
        store._put_record_v2(run, "finalize", "0", envelope.model_dump(mode="json"))
    put_finality(store, unresolved(run, 1))
    issued = ledger.balances().issued
    # When: round 1 is finalized.
    if paid_inputs:
        # Then: a settled round that did pay somebody is never passed over.
        with pytest.raises(EscrowError, match="FINALIZE_ORDER"):
            ledger.reward_finalize(reward1, evidence1)
        assert (withdrawals(ledger), minted_since(ledger, issued)) == ([], 0)
    else:
        # Then: the round nobody was paid in is recorded once and round 1 mints.
        ledger.reward_finalize(reward1, evidence1)
        assert [(w["w"], w["closure_hash"]) for w in withdrawals(ledger)] == [(0, H)]
        assert minted_since(ledger, issued) == ledger.policy.round_reward_units
        ledger.verify()


def test_rounds_closed_without_reward_are_recorded_in_gaps_only_and_order_still_holds(
    network, ledger: EscrowV2
) -> None:
    # Given: rounds 0 and 1 are closed unpaid (an accepted rollback closes two rounds), round 2 is
    # pending and round 3 is applied.
    store, run = network.store, network.manifest.run_id()
    _, reward2, evidence2 = signed_round(network, ledger, 2)
    _, reward3, evidence3 = signed_round(network, ledger, 3)
    put_finality(store, closed_unpaid(run, 0))
    put_finality(store, closed_unpaid(run, 1))
    put_finality(store, unresolved(run, 2))
    put_finality(store, unresolved(run, 3))
    # When/Then: round 3 cannot jump over the pending round 2, and nothing is recorded by the
    # refusal (the transaction rolled back).
    with pytest.raises(EscrowError, match="FINALIZE_ORDER"):
        ledger.reward_finalize(reward3, evidence3)
    assert withdrawals(ledger) == []
    # When: round 2 is paid, then round 3.
    ledger.reward_finalize(reward2, evidence2)
    ledger.reward_finalize(reward3, evidence3)
    # Then: rounds 0 and 1 were recorded once each, before the first mint.
    assert [w["w"] for w in withdrawals(ledger)] == [0, 1]
    ledger.verify()


def test_a_closed_round_is_recorded_once(network, ledger: EscrowV2) -> None:
    store, run = network.store, network.manifest.run_id()
    _, reward1, evidence1 = signed_round(network, ledger, 1)
    _, reward2, evidence2 = signed_round(network, ledger, 2)
    put_finality(store, closed_unpaid(run, 0))
    put_finality(store, unresolved(run, 1))
    first = ledger.reward_finalize(reward1, evidence1)
    events = ledger.db.execute("SELECT COUNT(*) FROM escrow_events").fetchone()[0]
    # A replay of round 1 and the mint of round 2 record nothing more.
    assert ledger.reward_finalize(reward1, evidence1) == first
    put_finality(store, unresolved(run, 2))
    ledger.reward_finalize(reward2, evidence2)
    assert [w["w"] for w in withdrawals(ledger)] == [0]
    assert ledger.db.execute("SELECT COUNT(*) FROM escrow_events").fetchone()[0] == events + 1
    ledger.verify()


@pytest.mark.parametrize("close", [closed_unpaid, rolled_back], ids=["exclusion", "rollback"])
def test_the_evidence_of_a_closed_round_is_never_paid(network, ledger: EscrowV2, close) -> None:
    store, run = network.store, network.manifest.run_id()
    _, reward0, evidence0 = signed_round(network, ledger, 0)
    _, reward1, evidence1 = signed_round(network, ledger, 1)
    put_finality(store, close(run, 0))
    put_finality(store, unresolved(run, 1))
    issued = ledger.balances().issued
    # Nothing is recorded yet, so round 0 is the first open round: only its closure refuses it.
    with pytest.raises(EscrowError, match="ROUND_CLOSED_UNPAID"):
        ledger.reward_finalize(reward0, evidence0)
    assert (withdrawals(ledger), minted_since(ledger, issued)) == ([], 0)
    # Once round 1 has recorded it, the same evidence is still refused for what it is.
    ledger.reward_finalize(reward1, evidence1)
    with pytest.raises(EscrowError, match="ROUND_CLOSED_UNPAID"):
        ledger.reward_finalize(reward0, evidence0)
    assert minted_since(ledger, issued) == ledger.policy.round_reward_units


def test_the_event_chain_commits_to_the_withdrawal(network, ledger: EscrowV2) -> None:
    store, run = network.store, network.manifest.run_id()
    _, reward1, evidence1 = signed_round(network, ledger, 1)
    put_finality(store, closed_unpaid(run, 0))
    put_finality(store, unresolved(run, 1))
    ledger.reward_finalize(reward1, evidence1)
    ledger.verify()
    reopened = EscrowV2(
        ledger.db,
        ledger.manifest,
        ledger.policy,
        auditors=ledger.auditors,
        replay_finality=ledger.replay_finality,
        settlement=ledger.settlement,
        owner_of=ledger.owner_of,
        assignment_of=ledger.assignment_of,
    )
    assert reopened.balances() == ledger.balances()
    ledger.db.execute("DELETE FROM escrow_events WHERE kind='ROUND_WITHDRAWN'")
    with pytest.raises(EscrowError, match="EVENT_CHAIN"):
        ledger.verify()


def test_a_client_event_cannot_occupy_the_withdrawal_id(network, ledger: EscrowV2) -> None:
    store, run = network.store, network.manifest.run_id()
    owner = N.COLD[0].ss58
    held = ledger.db.execute(
        "SELECT origin FROM escrow_units WHERE owner=? AND bucket='available' AND units>0 LIMIT 1",
        (owner,),
    ).fetchone()
    assert held is not None, "the fixture funds COLD[0]"
    naive = sha256_hex(f"ht-round-withdraw-v2|{run}|0".encode())
    ledger.transfer(
        EscrowTransfer(
            operation_id=naive,
            owner=owner,
            units=1,
            origin_ids=[held["origin"]],
            admission_id=None,
            dispute_id=None,
            recipient=N.COLD[1].ss58,
        ),
        signer=owner,
    )
    _, reward1, evidence1 = signed_round(network, ledger, 1)
    put_finality(store, closed_unpaid(run, 0))
    put_finality(store, unresolved(run, 1))
    ledger.reward_finalize(reward1, evidence1)
    assert [w["w"] for w in withdrawals(ledger)] == [0]
