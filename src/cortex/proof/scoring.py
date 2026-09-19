"""Deterministic Proof gates and per-topic payout masses."""

from __future__ import annotations

import math
from collections import defaultdict

from cortex.errors import ServiceError
from cortex.proof.models import EvaluationReport, Topic

SCORE_MAX = 1_000_000


def judge(topic: Topic, report: EvaluationReport) -> list[str]:
    if topic.baseline is None:
        raise ServiceError(503, "baseline is not sealed")
    failures = []
    if report.verdict != "clean" or not report.reproduced or not report.claim_holds:
        failures.append("unreproduced or rejected claim")
    rules = {rule.id for rule in topic.checklist}
    if set(report.rule_results) != rules or not all(report.rule_results.values()):
        failures.append("checklist rejected")
    if report.wall_seconds > topic.wall_budget_s:
        failures.append("wall budget exceeded")
    if topic.metric.family != "custom" and report.flops_used > topic.flops_budget:
        failures.append("flops budget exceeded")
    if failures:
        return failures
    primary = report.metrics.get(topic.metric.primary)
    baseline = topic.baseline.metrics[topic.metric.primary]
    if primary is None:
        raise ServiceError(503, "measured primary missing")
    improvement = primary - baseline if topic.metric.direction == "max" else baseline - primary
    threshold = (
        topic.metric.epsilon * abs(baseline) if topic.metric.relative else topic.metric.epsilon
    )
    if improvement < threshold:
        failures.append("baseline improvement floor missed")
    if topic.metric.family != "custom":
        if (
            report.executor_offer_id is None
            or report.executor_offer_commitment is None
            or report.executor_config_commitment is None
        ):
            raise ServiceError(503, "executor provenance missing")
        required = topic.eval_executor.require_offer_commitment
        if required is not None and report.executor_offer_commitment != required:
            raise ServiceError(503, "executor offer provenance mismatch")
        splits = {key for key in topic.baseline.metrics if key.startswith("split_nll/")}
        if not splits:
            raise ServiceError(503, "baseline split evidence missing")
        for key in splits:
            value = report.metrics.get(key)
            if value is None:
                raise ServiceError(503, "measured split missing")
            if value > topic.baseline.metrics[key] + topic.metric.max_regression:
                failures.append("holdout split regression")
        if topic.metric.family == "throughput":
            value = report.metrics.get("holdout_nll")
            reference = topic.baseline.metrics.get("holdout_nll")
            if value is None or reference is None:
                raise ServiceError(503, "holdout quality evidence missing")
            if value > reference + topic.metric.quality_floor_nll:
                failures.append("throughput quality floor missed")
    return failures


def allocate(mass: int, weights: dict[str, int]) -> dict[str, int]:
    total = sum(weights.values())
    if mass <= 0 or total <= 0:
        return {}
    rows = [(key, *divmod(mass * weight, total)) for key, weight in weights.items() if weight > 0]
    leftover = mass - sum(row[1] for row in rows)
    rows.sort(key=lambda row: (-row[2], row[0]))
    return {key: floor + int(index < leftover) for index, (key, floor, _) in enumerate(rows)}


def payout(
    topics: list[Topic],
    rows: list[dict],
    epoch: int,
    champions: dict[str, float] | None = None,
    *,
    frozen_topics: dict[str, Topic] | None = None,
    previous_artifacts: dict[str, set[str]] | None = None,
) -> dict[str, int]:
    """Sum open-topic masses; exact ties share, copies receive no novelty pool."""
    active = sorted((topic for topic in topics if topic.active(epoch)), key=lambda topic: topic.id)
    if not active:
        raise ServiceError(503, "no open topics")
    result: defaultdict[str, int] = defaultdict(int)
    base_mass, remainder = divmod(SCORE_MAX, len(active))
    for index, topic in enumerate(active):
        if topic.baseline is None:
            raise ServiceError(503, "baseline is not sealed")
        mass = base_mass + int(index < remainder)
        candidates: dict[str, dict] = {}
        for row in rows:
            if row["epoch"] != epoch or row["topic_id"] != topic.id or row["status"] != "accepted":
                continue
            frozen = (frozen_topics or {}).get(row["topic_digest"], topic)
            if (
                frozen.content_digest() != row["topic_digest"]
                or frozen.id != topic.id
                or frozen.metric != topic.metric
                or frozen.baseline != topic.baseline
            ):
                continue
            report = EvaluationReport.model_validate(row["report"])
            if judge(frozen, report):
                continue
            hotkey = row["hotkey"]
            candidate = {
                "primary": report.metrics[topic.metric.primary],
                "digest": report.artifact_digest,
                "duplicate": report.near_duplicate,
            }
            previous = candidates.get(hotkey)
            if previous is None or (
                candidate["primary"] > previous["primary"]
                if topic.metric.direction == "max"
                else candidate["primary"] < previous["primary"]
            ):
                candidates[hotkey] = candidate
        if not candidates:
            continue
        best = max if topic.metric.direction == "max" else min
        if topic.payout_mode == "wta":
            winning = best(row["primary"] for row in candidates.values())
            pieces = allocate(
                mass, {key: 1 for key, row in candidates.items() if row["primary"] == winning}
            )
        else:
            pieces = allocate(
                mass * topic.pass_floor_share_bps // 10000, dict.fromkeys(candidates, 1)
            )
            bar = topic.baseline.metrics[topic.metric.primary]
            champion = (champions or {}).get(topic.id)
            if champion is not None and math.isfinite(champion):
                bar = best(bar, champion)
            seen = set((previous_artifacts or {}).get(topic.id, set()))
            weights = {}
            for key, row in sorted(candidates.items()):
                delta = (
                    row["primary"] - bar
                    if topic.metric.direction == "max"
                    else bar - row["primary"]
                )
                duplicate = row["duplicate"] or row["digest"] in seen
                seen.add(row["digest"])
                scaled = min(max(delta, 0.0) * 1_000_000_000.0, float(2**64 - 1))
                weights[key] = 0 if duplicate else min(2**64 - 1, math.floor(scaled + 0.5))
            novelty = allocate(mass * (10000 - topic.pass_floor_share_bps) // 10000, weights)
            for key, value in novelty.items():
                pieces[key] = pieces.get(key, 0) + value
        for key, value in pieces.items():
            result[key] += value
    return dict(result)
