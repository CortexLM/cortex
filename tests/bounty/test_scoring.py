"""Money-path contracts ported from the live Bounty scorer."""

import pytest

from cortex.bounty.scoring import Holdout, judge_challenger


@pytest.mark.parametrize(
    ("severity", "expected"),
    [("trivial", 62_500), ("minor", 250_000), ("major", 500_000), ("critical", 1_000_000)],
)
def test_reward_is_precision_times_mean_severity(severity, expected):
    challenger = Holdout()
    for _ in range(3):
        challenger.record("valid", severity)

    verdict = judge_challenger(Holdout(), challenger)

    assert verdict.eligible
    assert verdict.lattice == expected


@pytest.mark.parametrize(
    ("verdict", "severity", "count", "gate"),
    [
        ("valid", None, 1, "severity_evidence_missing"),
        ("duplicate", None, 4, "triage_noise"),
        ("already_fixed_not_prod", None, 4, "triage_noise"),
        ("invalid_malicious", None, 4, "penalty"),
    ],
)
def test_unpriced_noise_and_penalty_gates_cannot_be_paid(verdict, severity, count, gate):
    challenger = Holdout()
    for _ in range(3):
        challenger.record("valid", "critical")
    for _ in range(count):
        challenger.record(verdict, severity)

    result = judge_challenger(Holdout(), challenger)

    assert not result.eligible
    assert result.lattice == 0
    assert gate in result.failed


def test_impact_does_not_override_strict_precision_displacement():
    incumbent = Holdout()
    challenger = Holdout()
    for _ in range(3):
        incumbent.record("valid", "trivial")
        challenger.record("valid", "critical")

    verdict = judge_challenger(incumbent, challenger)

    assert not verdict.eligible
    assert "no_precision_win" in verdict.failed


def test_malicious_rows_without_evidence_still_count_against_precision():
    challenger = Holdout()
    for _ in range(4):
        challenger.record("valid", "critical")
    challenger.record("invalid_malicious")

    verdict = judge_challenger(Holdout(), challenger)

    assert verdict.eligible
    assert verdict.lattice == 800_000
