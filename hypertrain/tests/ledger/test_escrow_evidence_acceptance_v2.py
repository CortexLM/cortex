"""Stored reward evidence is judged at the beacon where it was accepted, not at the finalize beacon.

ADR 0014 part 3 (D48): an envelope valid at its acceptance stays valid for the finalisation
of its round. `actual_reward` seals everything with an expiry of 200 against a finalize beacon
of 10, which hides the case; here each leg gets an expiry below the finalize beacon, as the
honest clients seal them.
"""

from __future__ import annotations

import importlib.util
import sys
from dataclasses import replace
from pathlib import Path

import pytest

from hypertrain.ledger.escrow_v2 import EscrowError, EscrowV2, FinalityEvidence
from hypertrain.protocol.envelope import ExpiredError
from hypertrain.protocol.envelope_v2 import EnvelopeV2, SignatureError, seal
from hypertrain.protocol.messages import ReplayVerdict
from hypertrain.protocol.messages_v2 import AuditChallengeV2, CommitV2

spec = importlib.util.spec_from_file_location(
    "evidence_acceptance_fixture", Path(__file__).parent / "test_escrow_v2.py"
)
assert spec is not None and spec.loader is not None
E = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = E
spec.loader.exec_module(E)
actual_reward = E.actual_reward

FINALIZED_BEACON = 10  # the beacon at which `actual_reward` finalizes live round 0
ACCEPTED = E.ACCEPTED_BEACON  # the fixture ledger's default acceptance beacon
LEGS = {
    "commit": ("CommitV2", CommitV2, E.HOT),
    "verdict": ("ReplayVerdict", ReplayVerdict, E.AUDITOR),
    "challenge": ("AuditChallengeV2", AuditChallengeV2, E.COORD),
}


def with_expiry(s, evidence: FinalityEvidence, leg: str, exp: int) -> FinalityEvidence:
    """Same signer and body as the fixture's `leg` envelope, sealed again with expiry `exp`."""
    item = evidence.works[0]
    type_, model, key = LEGS[leg]
    body = model.model_validate(getattr(item, leg).body)
    sealed = EnvelopeV2.model_validate(seal(key, type_, s.manifest.run_id(), body, exp))
    return replace(evidence, works=(replace(item, **{leg: sealed}),))


def finalize(tmp_path: Path, actual_reward, leg: str, exp: int, **options) -> EscrowV2:
    s, evidence, reward = actual_reward
    e = E.ledger(s, tmp_path / "ledger.db", replay=E.verified_shadow, **options)
    e.reward_finalize(reward, with_expiry(s, evidence, leg, exp))
    return e


def assert_round_zero_paid_once(e: EscrowV2) -> None:
    assert [row["w"] for row in e.db.execute("SELECT w FROM escrow_finalized")] == [0]
    assert e.balances().reward_pending == 1_000_000 and e.balances().conserved()


@pytest.mark.parametrize("leg", sorted(LEGS))
def test_reward_settles_when_the_envelope_is_still_live_at_the_finalize_beacon(
    tmp_path: Path, actual_reward, leg: str
) -> None:
    # Control of the two-sided boundary: green with and without the acceptance beacon.
    assert_round_zero_paid_once(finalize(tmp_path, actual_reward, leg, FINALIZED_BEACON))


@pytest.mark.parametrize("leg", sorted(LEGS))
def test_reward_settles_when_the_envelope_expired_after_its_acceptance_and_before_the_finalize(
    tmp_path: Path, actual_reward, leg: str
) -> None:
    # Given: the envelope was accepted at beacon 5 and expired at 9; the round is finalized at 10.
    # When / Then: the round is paid once; the signer's expiry did not decide the reward.
    assert_round_zero_paid_once(finalize(tmp_path, actual_reward, leg, FINALIZED_BEACON - 1))


@pytest.mark.parametrize("leg", sorted(LEGS))
def test_reward_refuses_an_envelope_that_had_expired_when_it_was_accepted(
    tmp_path: Path, actual_reward, leg: str
) -> None:
    # The other side of the boundary. It also passes on the parent; it is here because it alone
    # kills a ledger that stopped checking the expiry at acceptance.
    with pytest.raises(ExpiredError):
        finalize(tmp_path, actual_reward, leg, ACCEPTED - 1)


def test_each_stored_envelope_is_judged_at_its_own_acceptance_beacon(
    tmp_path: Path, actual_reward
) -> None:
    # Given: commit accepted at 3, verdict at 5, challenge at 7; each expired the beacon after.
    s, evidence, reward = actual_reward
    accepted_at = {"CommitV2": 3, "ReplayVerdict": 5, "AuditChallengeV2": 7}
    for leg, (type_, _, _) in LEGS.items():
        evidence = with_expiry(s, evidence, leg, accepted_at[type_])
    e = E.ledger(
        s,
        tmp_path / "ledger.db",
        replay=E.verified_shadow,
        accepted_at=lambda envelope: accepted_at[envelope.type],
    )
    # When / Then: settles; one beacon shared by the three legs would have refused the commit.
    e.reward_finalize(reward, evidence)
    assert_round_zero_paid_once(e)


@pytest.mark.parametrize("beacon", [0, FINALIZED_BEACON + 1])
def test_reward_refuses_an_acceptance_beacon_outside_the_life_of_the_round(
    tmp_path: Path, actual_reward, beacon: int
) -> None:
    s, evidence, reward = actual_reward
    e = E.ledger(s, tmp_path / "ledger.db", replay=E.verified_shadow, accepted_at=lambda _: beacon)
    with pytest.raises(EscrowError, match="ACCEPTANCE_BEACON"):
        e.reward_finalize(reward, evidence)
    assert e.balances().issued == 0


def test_reward_without_an_acceptance_authority_fails_closed(tmp_path: Path, actual_reward) -> None:
    s, evidence, reward = actual_reward
    e = E.ledger(s, tmp_path / "ledger.db", replay=E.verified_shadow, accepted_at=None)
    with pytest.raises(EscrowError, match="ACCEPTANCE_AUTHORITY_UNAVAILABLE"):
        e.reward_finalize(reward, evidence)
    assert e.balances().issued == 0


@pytest.mark.parametrize("leg", sorted(LEGS))
def test_reward_refuses_stored_evidence_whose_signature_does_not_verify(
    tmp_path: Path, actual_reward, leg: str
) -> None:
    s, evidence, reward = actual_reward
    item = evidence.works[0]
    envelope = getattr(item, leg)
    flipped = envelope.sig[:-1] + ("0" if envelope.sig[-1] != "0" else "1")
    tampered = replace(item, **{leg: envelope.model_copy(update={"sig": flipped})})
    e = E.ledger(s, tmp_path / "ledger.db", replay=E.verified_shadow)
    with pytest.raises(SignatureError):
        e.reward_finalize(reward, replace(evidence, works=(tampered,)))
    assert e.balances().issued == 0
