"""Audit floors and influence gates consume authenticated accounting/replay facts."""

from __future__ import annotations

import hashlib
from dataclasses import dataclass
from typing import Literal, assert_never

from hypertrain.protocol.messages import VerifySpec
from hypertrain.protocol.messages_v2 import (
    AdmissionState,
    EconomicsPolicyV2,
    RosterEntryV2,
)


class TrustError(ValueError):
    """Influence lacks funded, settled, independently verified work."""


@dataclass(frozen=True, slots=True)
class FundedStatus:
    """L2 supplies collectible origin-backed locks, never available/unmatured promises."""

    run_id: str
    hotkey: str
    admission_id: str
    ledger_mode: Literal["test", "production"]
    locked_units: int
    collectible_reward_units: int
    lock_receipt_hash: str
    conservative_bound: bool


@dataclass(frozen=True, slots=True)
class SettlementStatus:
    """L5/L0 authenticated status at the transaction's current state revision."""

    run_id: str
    w: int
    hotkey: str
    unresolved_dispute: bool
    excluded: bool
    status_hash: str


@dataclass(frozen=True, slots=True)
class ReplayEvidence:
    """L1 authenticated full anchored replay outputs; not a norm-screen attestation.

    These are independently recomputed values, not hashes copied from the commit.
    L0 obtains them only from authorized replay acceptance, binding own assignment.
    """

    run_id: str
    w: int
    hotkey: str
    assignment_hash: str
    anchor_hash: str
    leaves_root: str
    delta_hash: str
    final_theta_hash: str
    ef_in_hash: str
    ef_out_hash: str
    verdict_hash: str
    result: Literal["MATCH", "MISMATCH", "WITHHELD", "BAD_PROOF", "ASSIGNMENT_VIOLATION"]
    audit_mode: Literal["anchored-full"]


@dataclass(frozen=True, slots=True)
class AuditFloor:
    q_ppm: int
    required_locked_units: int
    influence_allowed: bool


def nominal_q(state: AdmissionState, clean_finalizations: int) -> int:
    if clean_finalizations < 0:
        raise TrustError("negative clean count")
    match state:
        case "PROBATION":
            return (
                1_000_000
                if clean_finalizations < 4
                else (500_000 if clean_finalizations < 8 else 250_000)
            )
        case "ACTIVE":
            return 250_000
        case "APPLIED" | "SUSPENDED" | "BANNED":
            return 1_000_000
        case unreachable:
            assert_never(unreachable)


def audit_floor(
    policy: EconomicsPolicyV2,
    verify: VerifySpec,
    roster: RosterEntryV2,
    funding: FundedStatus,
    *,
    run_id: str,
    clean_finalizations: int,
    reference_reward_units: int,
    anomaly_ppm: int = 0,
) -> AuditFloor:
    """Ceil integer economics: q*s*beta*(R+S)>=G; phase q remains one."""
    if (
        (funding.run_id, funding.hotkey, funding.admission_id, funding.ledger_mode)
        != (run_id, roster.hotkey, roster.admission_id, policy.ledger_mode)
        or min(funding.locked_units, funding.collectible_reward_units, reference_reward_units) < 0
        or not 0 <= anomaly_ppm <= 1_000_000
    ):
        raise TrustError("accounting status/bounds differ")
    reward = min(funding.collectible_reward_units, policy.R_collectible_units)
    exposure = reward + funding.locked_units
    denominator = policy.s_lower_ppm * policy.beta_ppm * exposure
    economic = (
        (policy.G_max_units * 10**18 + denominator - 1) // denominator
        if denominator
        else (1_000_001 if policy.G_max_units else 0)
    )
    q = max(nominal_q(roster.state, clean_finalizations), anomaly_ppm, policy.q_floor, economic)
    penalty_den = min(q, 1_000_000) * policy.s_lower_ppm * policy.beta_ppm
    required_exposure = (
        (policy.G_max_units * 10**18 + penalty_den - 1) // penalty_den
        if penalty_den
        else (policy.G_max_units + exposure + 1)
    )
    required = max(
        policy.S_min_units,
        verify.s_min_units(reference_reward_units),
        required_exposure - reward,
        0,
    )
    live = (roster.state == "PROBATION" and clean_finalizations >= 4) or (
        roster.state == "ACTIVE" and clean_finalizations >= 12
    )
    allowed = (
        live
        and funding.conservative_bound
        and q <= 1_000_000
        and policy.s_lower_ppm > 0
        and funding.locked_units >= required
    )
    return AuditFloor(min(q, 1_000_000), required, allowed)


@dataclass(frozen=True, slots=True)
class AuditSelection:
    selected: bool
    mode: Literal["anchored-full"] | None
    final_transition: bool
    compression_and_ef: bool


def select_audit(
    run_id: str, w: int, signature_hash: str, hotkey: str, q_ppm: int
) -> AuditSelection:
    """Exact integer draw from the pinned beacon; q=0 is not a final-only audit."""
    if type(w) is not int or w < 0 or type(q_ppm) is not int or not 0 <= q_ppm <= 1_000_000:
        raise TrustError("invalid audit draw bounds")
    seed = f"ht-audit-v2|{run_id}|{w}|{signature_hash}|{hotkey}".encode()
    x = int.from_bytes(hashlib.sha256(seed).digest(), "big")
    selected = x * 1_000_000 < q_ppm * 2**256
    return AuditSelection(selected, "anchored-full" if selected else None, selected, selected)
