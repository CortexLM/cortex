from __future__ import annotations

from dataclasses import replace

import pytest

from hypertrain.aggregator.core import FinalityError
from hypertrain.challenge.finality_v2 import (
    RoundStatus,
    exclude,
    finish_rollback,
    require_apply,
    require_open,
    settle,
)
from hypertrain.challenge.trust_v2 import FundedStatus, audit_floor, nominal_q, select_audit
from hypertrain.protocol.example import example_manifest
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import Finalize, Rollback, f32hex
from hypertrain.protocol.messages_v2 import EconomicsPolicyV2, RosterEntryV2

H = "11" * 32
K = Keypair(bytes([1]) * 32).ss58


def econ() -> EconomicsPolicyV2:
    return EconomicsPolicyV2(
        ledger_mode="test",
        G_max_units=100,
        R_collectible_units=100,
        S_min_units=1000,
        beta_ppm=1_000_000,
        gammaV_units=0,
        s_lower_ppm=1_000_000,
        q_floor=1_000_000,
        genesis_allocation_hash=H,
        reward_authority=K,
        round_reward_units=100,
        max_total_issuance=100000,
        shadow_bootstrap_rounds=12,
    )


def test_economic_floor_when_origin_backed_funding_sufficient() -> None:
    # Given
    r = RosterEntryV2(
        hotkey=K,
        slot=0,
        q_i=f32hex(1),
        admission_id=H,
        coldkey_group="owner",
        state="ACTIVE",
        eligible_weight=1 << 22,
    )
    f = FundedStatus(H, K, H, "test", 1000, 100, H, True)
    # When
    floor = audit_floor(
        econ(),
        example_manifest().verify,
        r,
        f,
        run_id=H,
        clean_finalizations=12,
        reference_reward_units=100,
    )
    # Then
    assert floor.q_ppm == 1_000_000
    assert floor.required_locked_units == 1000
    assert floor.influence_allowed


@pytest.mark.parametrize(
    "fault",
    [
        "unfunded",
        "unknown_bound",
        "zero_detection",
        "economic_infeasible",
        "uncollectible_reward",
        "shadow",
        "premature_active",
    ],
)
def test_influence_rejects_when_economic_or_admission_bound_missing(fault: str) -> None:
    # Given
    p = econ()
    r = RosterEntryV2(
        hotkey=K,
        slot=0,
        q_i=f32hex(1),
        admission_id=H,
        coldkey_group="owner",
        state="ACTIVE",
        eligible_weight=1 << 22,
    )
    f = FundedStatus(H, K, H, "test", 1000, 100, H, True)
    clean = 12
    match fault:
        case "unfunded":
            f = replace(f, locked_units=0)
        case "unknown_bound":
            f = replace(f, conservative_bound=False)
        case "zero_detection":
            p = p.model_copy(update={"s_lower_ppm": 0})
        case "economic_infeasible":
            p = p.model_copy(update={"G_max_units": 1101})
        case "uncollectible_reward":
            p = p.model_copy(update={"G_max_units": 1100})
            f = replace(f, collectible_reward_units=0)
        case "shadow":
            r = r.model_copy(update={"state": "PROBATION"})
            clean = 3
        case "premature_active":
            clean = 11
    # When
    floor = audit_floor(
        p,
        example_manifest().verify,
        r,
        f,
        run_id=H,
        clean_finalizations=clean,
        reference_reward_units=100,
    )
    # Then
    assert not floor.influence_allowed


def test_ceiling_economics_when_fractional_penalty_conversion() -> None:
    # Given
    p = econ().model_copy(
        update={"G_max_units": 101, "beta_ppm": 333_333, "s_lower_ppm": 500_000, "S_min_units": 0}
    )
    r = RosterEntryV2(
        hotkey=K,
        slot=0,
        q_i=f32hex(1),
        admission_id=H,
        coldkey_group="owner",
        state="ACTIVE",
        eligible_weight=1 << 22,
    )
    f = FundedStatus(H, K, H, "test", 600, 0, H, True)
    # When
    floor = audit_floor(
        p,
        example_manifest().verify,
        r,
        f,
        run_id=H,
        clean_finalizations=12,
        reference_reward_units=0,
    )
    # Then: ceil(101/(.5*.333333)) =607, not606.
    assert floor.required_locked_units == 607
    assert not floor.influence_allowed


@pytest.mark.parametrize(
    "clean,expected", [(0, 1000000), (3, 1000000), (4, 500000), (7, 500000), (8, 250000)]
)
def test_nominal_schedule_when_clean_finalizations_advance(clean: int, expected: int) -> None:
    # Given / When / Then
    assert nominal_q("PROBATION", clean) == expected


@pytest.mark.parametrize("q,selected", [(0, False), (1000000, True)])
def test_audit_selection_when_zero_or_one_probability(q: int, selected: bool) -> None:
    # Given / When
    selection = select_audit(H, 0, H, K, q)
    # Then
    assert selection.selected is selected
    assert selection.final_transition is selected
    assert selection.compression_and_ef is selected
    assert selection.mode == ("anchored-full" if selected else None)


def status(w: int) -> RoundStatus:
    return RoundStatus(H, w, True, "PENDING", 0, 1, False, H, None)


def test_third_round_blocked_when_first_unresolved() -> None:
    # Given
    history = [status(0), status(1)]
    require_open(H, 1, history[:1])
    require_apply(H, 1, history[:1])
    # When / Then
    with pytest.raises(FinalityError):
        require_open(H, 2, history)


def test_open_resumes_when_authenticated_settlement_complete() -> None:
    # Given
    s = replace(status(0), unresolved_audits=0)
    final = Finalize(w=0, final_theta_hash_w1=H, included=[K], entitlements_root=H)
    s = settle(s, final, H)
    # When / Then
    require_open(H, 2, [s, status(1)])
    assert s.closed


def test_exclusion_waits_for_rollback_when_update_applied() -> None:
    # Given
    s = exclude(replace(status(0), unresolved_audits=0), H)
    # When / Then
    with pytest.raises(FinalityError):
        require_open(H, 2, [s, status(1)])
    assert not s.closed


def test_open_resumes_when_complete_two_round_rollback_verified() -> None:
    # Given
    s = exclude(replace(status(0), unresolved_audits=0), H)
    repair = Rollback(
        w=0,
        excluded=[K],
        old_theta_hash_w2=H,
        new_theta_hash_w2="22" * 32,
        new_outer_state_hash="33" * 32,
        recomputed=["agg_w", "step_w", "agg_w1", "step_w1"],
        cause_hashes=[H],
    )
    # When
    s = finish_rollback(s, repair, "22" * 32, "33" * 32)
    # Then
    require_open(H, 2, [s, status(1)])
    assert s.closed


def test_finalized_model_history_preserved_when_late_fraud() -> None:
    # Given
    s = replace(status(0), disposition="SETTLED", unresolved_audits=0, resolution_hash=H)
    # When / Then
    with pytest.raises(FinalityError):
        exclude(s, "22" * 32)


@pytest.mark.parametrize("fault", ["missing", "duplicate", "dispute", "partial_rollback"])
def test_finality_fails_closed_when_incomplete_history_or_settlement(fault: str) -> None:
    # Given
    history = [replace(status(0), disposition="SETTLED", unresolved_audits=0), status(1)]
    match fault:
        case "missing":
            history.pop(0)
        case "duplicate":
            history[1] = history[0]
        case "dispute":
            history[0] = replace(history[0], unresolved_disputes=1)
        case "partial_rollback":
            history[0] = replace(history[0], disposition="EXCLUDED", rollback_complete=False)
    # When / Then
    with pytest.raises(FinalityError):
        require_apply(H, 2, history)
