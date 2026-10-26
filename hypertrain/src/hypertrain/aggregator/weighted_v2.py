"""Exact max-min allocation with intersecting probation and known-owner bounds."""

from __future__ import annotations

from collections.abc import Sequence
from dataclasses import dataclass
from typing import Final

import numpy as np

from hypertrain.aggregator.core import (
    OuterParams,
    OuterState,
    Params,
    centered_clip,
    copy_flags,
    names,
    outer_step,
    preclip,
)
from hypertrain.protocol.messages import f32val
from hypertrain.protocol.messages_v2 import (
    WEIGHT_QUANTUM,
    AggregationPolicyV2,
    RosterEntryV2,
    WeightAllocationV2,
    WeightedEntryV2,
)

Q: Final = WEIGHT_QUANTUM


class InsufficientEligibleWeight(ValueError):
    """INSUFFICIENT_ELIGIBLE_WEIGHT: caps cannot carry one complete update."""


@dataclass(frozen=True, slots=True)
class Candidate:
    roster: RosterEntryV2
    commit_hash: str
    delta_manifest_hash: str


def _completion_capacity(
    active: Sequence[int],
    probation: Sequence[int],
    owner: Sequence[int],
    cohort_cap: int,
) -> int:
    """Two-cohort min-cut: all-owner capacity versus active capacity plus probation cap."""
    return min(
        sum(min(r, a + p) for a, p, r in zip(active, probation, owner, strict=True)),
        sum(min(r, a) for a, r in zip(active, owner, strict=True)) + cohort_cap,
    )


def allocate_weights(
    candidates: Sequence[Candidate], policy: AggregationPolicyV2
) -> WeightAllocationV2:
    """Lexicographic max-min integer filling, UTF-8 ties, no renormalization.

    Closed-form completion probes are O(n); incremental tie checks are O(1).
    At most n nonfinal filling iterations, O(n^2 log Q) work, O(n) scratch.
    """
    if len(candidates) > 1024:
        raise InsufficientEligibleWeight("oversized candidate set")
    ordered = sorted(candidates, key=lambda c: c.roster.hotkey.encode("utf-8"))
    ids = [c.roster.hotkey for c in ordered]
    if len(ids) != len(set(ids)):
        raise InsufficientEligibleWeight("duplicate or oversized candidate set")
    if any(c.roster.state not in ("ACTIVE", "PROBATION") for c in ordered):
        raise InsufficientEligibleWeight("shadow/suspended identity supplied for live allocation")
    groups = sorted({c.roster.coldkey_group for c in ordered})
    group_index = {g: i for i, g in enumerate(groups)}
    owner_ids = [group_index[c.roster.coldkey_group] for c in ordered]
    probation_ids = [c.roster.state == "PROBATION" for c in ordered]
    caps = [min(Q, policy.owner_group_caps.get(g, Q)) for g in groups]
    ceilings = [min(policy.miner_cap_units, c.roster.eligible_weight) for c in ordered]
    weights = [0] * len(ordered)
    free = set(range(len(ordered)))

    def completion(lower: list[int]) -> tuple[list[int], list[int], list[int], int, int] | None:
        residual, active, probation = caps.copy(), [0] * len(groups), [0] * len(groups)
        total = 0
        cohort: int = policy.probation_cap_units
        for i, w in enumerate(lower):
            if w > ceilings[i]:
                return None
            g = owner_ids[i]
            residual[g] -= w
            total += w
            if probation_ids[i]:
                cohort -= w
            if i in free:
                target = probation if probation_ids[i] else active
                target[g] += ceilings[i] - w
        if total > Q or cohort < 0 or min(residual, default=0) < 0:
            return None
        if total + _completion_capacity(active, probation, residual, cohort) < Q:
            return None
        return residual, active, probation, cohort, total

    if completion(weights) is None:
        raise InsufficientEligibleWeight("INSUFFICIENT_ELIGIBLE_WEIGHT")
    while sum(weights) < Q:
        lo, hi = 0, (Q - sum(weights)) // len(free)
        # Local upper bounds are exact necessary constraints, not relaxed feasibility.
        counts = [0] * len(groups)
        residual = caps.copy()
        cohort, probation_count = int(policy.probation_cap_units), 0
        for i, w in enumerate(weights):
            residual[owner_ids[i]] -= w
            if probation_ids[i]:
                cohort -= w
            if i in free:
                hi = min(hi, ceilings[i] - w)
                counts[owner_ids[i]] += 1
                probation_count += probation_ids[i]
        hi = min(hi, *(r // count for r, count in zip(residual, counts, strict=True) if count))
        if probation_count:
            hi = min(hi, cohort // probation_count)
        while lo < hi:
            mid = (lo + hi + 1) // 2
            trial = [w + mid if i in free else w for i, w in enumerate(weights)]
            if completion(trial) is not None:
                lo = mid
            else:
                hi = mid - 1
        weights = [w + lo if i in free else w for i, w in enumerate(weights)]
        state = completion(weights)
        if state is None:
            raise InsufficientEligibleWeight("INSUFFICIENT_ELIGIBLE_WEIGHT")
        residual, active, probation, cohort, total = state
        all_capacity = sum(
            min(r, a + p) for r, a, p in zip(residual, active, probation, strict=True)
        )
        active_capacity = sum(min(r, a) for r, a in zip(residual, active, strict=True))
        for i in sorted(free):
            if total == Q:
                break
            g = owner_ids[i]
            r, a, p = residual[g], active[g], probation[g]
            a1, p1 = a - (not probation_ids[i]), p - probation_ids[i]
            cohort1 = cohort - probation_ids[i]
            all1 = all_capacity - min(r, a + p) + min(r - 1, a1 + p1)
            active1 = active_capacity - min(r, a) + min(r - 1, a1)
            if (
                weights[i] < ceilings[i]
                and r > 0
                and cohort1 >= 0
                and total + 1 + min(all1, active1 + cohort1) >= Q
            ):
                weights[i] += 1
                total += 1
                residual[g], active[g], probation[g] = r - 1, a1, p1
                cohort, all_capacity, active_capacity = cohort1, all1, active1
            else:
                free.remove(i)
                remaining = ceilings[i] - weights[i]
                if probation_ids[i]:
                    probation[g] -= remaining
                else:
                    active[g] -= remaining
                all_capacity += min(r, active[g] + probation[g]) - min(r, a + p)
                active_capacity += min(r, active[g]) - min(r, a)
        if not free and sum(weights) != Q:
            raise InsufficientEligibleWeight("INSUFFICIENT_ELIGIBLE_WEIGHT")
    return WeightAllocationV2(
        policy_hash=policy.digest(),
        entries=[
            WeightedEntryV2(
                hotkey=c.roster.hotkey,
                admission_id=c.roster.admission_id,
                probation=c.roster.state == "PROBATION",
                weight_units=w,
                commit_hash=c.commit_hash,
                delta_manifest_hash=c.delta_manifest_hash,
            )
            for c, w in zip(ordered, weights, strict=True)
        ],
    )


@dataclass(frozen=True, slots=True)
class WeightedResult:
    state: OuterState
    preclipped: tuple[str, ...]
    copy_suspicion: tuple[dict[str, str], ...]


def aggregate_weighted(
    previous: OuterState,
    deltas: dict[str, Params],
    allocation: WeightAllocationV2,
    policy: AggregationPolicyV2,
    params: OuterParams,
) -> WeightedResult:
    """Reuse qualified preclip/CClip/outer update with exact final dyadic weights."""
    if allocation.policy_hash != policy.digest() or (
        params.preclip_norm,
        params.cclip_tau,
        params.cclip_iters,
    ) != (policy.preclip_norm, policy.cclip_tau, policy.cclip_iters):
        raise InsufficientEligibleWeight("aggregation policy/parameters differ")
    if set(deltas) != {e.hotkey for e in allocation.entries}:
        raise InsufficientEligibleWeight("input set differs from allocation")
    clipped, hits = [], []
    for entry in allocation.entries:
        delta = deltas[entry.hotkey]
        if names(delta) != names(previous.theta) or any(
            a.dtype != np.float32
            or a.shape != previous.theta[n].shape
            or not bool(np.isfinite(a).all())
            for n, a in delta.items()
        ):
            raise InsufficientEligibleWeight("malformed delta")
        value, hit = preclip(delta, f32val(policy.preclip_norm))
        clipped.append(value)
        if hit:
            hits.append(entry.hotkey)
    weights = [np.float32(e.weight_units / Q) for e in allocation.entries]
    g = centered_clip(
        clipped, weights, previous.center, f32val(policy.cclip_tau), policy.cclip_iters
    )
    flags = copy_flags([(e.hotkey, d) for e, d in zip(allocation.entries, clipped, strict=True)])
    return WeightedResult(outer_step(previous, g, params), tuple(hits), tuple(flags))
