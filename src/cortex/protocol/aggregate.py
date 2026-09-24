"""Exact served Python aggregation, including CPython 3.12 summation.

The live Rust service ports this algorithm, not the historical integer
Hamilton algorithm in BUNDLE_SPEC section 6. The checked-in upstream vectors
are the authority; rounding is independent per UID, without renormalization.
"""

import math
from collections.abc import Iterable, Mapping, Sequence
from dataclasses import dataclass

from .models import (
    BOUNTY_FULL_SHARE_REPORTS,
    FULL_SHARE_SCORE,
    PROPORTIONAL_SHARES,
    Leaf,
    Score,
)
from .scale import ProtocolError, fixed, uint


def compensated_sum(values: Iterable[float]) -> float:
    result = correction = 0.0
    for value in values:
        total = result + value
        correction += (
            (result - total) + value if abs(result) >= abs(value) else (value - total) + result
        )
        result = total
    return result + correction if correction != 0.0 else result


@dataclass(frozen=True)
class ChallengeWeights:
    slug: str
    emission_percent: float
    weights: Mapping[str, float]
    ok: bool = True


@dataclass(frozen=True)
class FinalWeights:
    uids: tuple[int, ...]
    weights: tuple[float, ...]
    hotkey_weights: Mapping[str, float]

    @property
    def final_vector(self) -> tuple[tuple[int, int], ...]:
        return tuple(
            (uid, max(0, min(65535, round(weight * 65535))))
            for uid, weight in zip(self.uids, self.weights, strict=True)
        )


def aggregate_challenge_weights(
    results: Sequence[ChallengeWeights],
    hotkey_to_uid: Mapping[str, int],
    *,
    min_allowed_weights: int = 1,
    max_weight_limit: int = 65535,
) -> FinalWeights:
    """General reference API, including upstream padding/error contracts."""
    active = [result for result in results if result.ok]
    fractions = {result.slug: max(result.emission_percent, 0.0) / 100.0 for result in active}
    allocated = compensated_sum(fractions.values())
    if allocated > 1:
        fractions = {slug: share / allocated for slug, share in fractions.items()}
    scores: dict[str, float] = {}
    for result in active:
        share = fractions[result.slug]
        if share <= 0:
            continue
        cleaned = {
            key: float(value)
            for key, value in result.weights.items()
            if math.isfinite(float(value)) and float(value) > 0
        }
        total = compensated_sum(cleaned.values())
        if total <= 0:
            continue
        for key, value in cleaned.items():
            scores[key] = scores.get(key, 0.0) + share * (value / total)
    kept: dict[str, float] = {}
    by_uid: dict[int, float] = {}
    for key, score in scores.items():
        uid = hotkey_to_uid.get(key)
        if uid is None or uid == 0:
            continue
        by_uid[uid] = by_uid.get(uid, 0.0) + score
        kept[key] = score
    miner_total = compensated_sum(by_uid.values())
    if miner_total <= 1e-12:
        if max_weight_limit <= 0:
            raise ProtocolError(f"max_weight_limit={max_weight_limit} admits no positive weight")
        candidates = [0] + sorted(set(hotkey_to_uid.values()) - {0})
        needed = max(1, min_allowed_weights, math.ceil(1.0 / min(max_weight_limit / 65535, 1)))
        if len(candidates) < needed:
            raise ProtocolError(
                "cannot build a chain-valid zero-miner weight vector: "
                f"need {needed} uids (min_allowed_weights={min_allowed_weights}, "
                f"max_weight_limit={max_weight_limit}) but only {len(candidates)} "
                "usable uid(s) available"
            )
        by_uid = dict.fromkeys(candidates[:needed], 1.0 / needed)
        kept = {}
    else:
        burn = 1.0 - miner_total
        if burn > 1e-12:
            by_uid[0] = by_uid.get(0, 0.0) + burn
        total = compensated_sum(by_uid.values())
        by_uid = {uid: score / total for uid, score in by_uid.items()}
    ordered = sorted(by_uid.items())
    return FinalWeights(tuple(uid for uid, _ in ordered), tuple(w for _, w in ordered), kept)


def challenge_emission_percent(
    challenge: bytes, bps: int, raw_total: int, *, algorithm_version: int
) -> float:
    """Share a challenge pays; the unpaid remainder burns and never moves elsewhere."""
    emission_percent = bps / 100.0
    if algorithm_version == 2 and challenge == b"bounty":
        emission_percent *= min(raw_total, BOUNTY_FULL_SHARE_REPORTS) / BOUNTY_FULL_SHARE_REPORTS
    elif algorithm_version == 3:
        emission_percent *= min(raw_total, FULL_SHARE_SCORE) / FULL_SHARE_SCORE
    return emission_percent


def aggregate_leaves(
    leaves: Sequence[Leaf],
    shares: tuple[tuple[bytes, int], ...],
    uid_map: tuple[tuple[bytes, int], ...],
    *,
    algorithm_version: int = 1,
) -> FinalWeights:
    if algorithm_version not in (1, 2, 3):
        raise ProtocolError("unsupported algorithm version")
    if algorithm_version == 2 and shares != PROPORTIONAL_SHARES:
        raise ProtocolError("algorithm 2 requires proportional shares")
    if algorithm_version == 1 and shares == PROPORTIONAL_SHARES:
        raise ProtocolError("proportional shares require algorithm version 2")
    if len(dict(shares)) != len(shares) or sum(bps for _, bps in shares) != 10000:
        raise ProtocolError("emission shares must be unique and sum to 10000")
    if len(dict(uid_map)) != len(uid_map) or len({uid for _, uid in uid_map}) != len(uid_map):
        raise ProtocolError("duplicate uid mapping")
    for key, uid in uid_map:
        fixed(key, 32)
        uint(uid, 2)
    scores: dict[bytes, dict[bytes, int]] = {}
    for leaf in leaves:
        raw = leaf.score.value if isinstance(leaf.score, Score) else 0
        uint(raw, 8)
        miners = scores.setdefault(leaf.challenge_id, {})
        miners[leaf.miner_hotkey] = miners.get(leaf.miner_hotkey, 0) + raw
        uint(miners[leaf.miner_hotkey], 8)
    results = []
    for challenge, bps in sorted(shares):
        uint(bps, 2)
        miners = scores.get(challenge, {})
        weights = {key.hex(): float(value) for key, value in sorted(miners.items()) if value > 0}
        emission_percent = challenge_emission_percent(
            challenge, bps, sum(miners.values()), algorithm_version=algorithm_version
        )
        results.append(ChallengeWeights(challenge.hex(), emission_percent, weights))
    return aggregate_challenge_weights(results, {key.hex(): uid for key, uid in sorted(uid_map)})
