"""The pure rules of a mismatch sanction: what a verdict proves, and when the bond burns."""

from __future__ import annotations

import pytest

from hypertrain.challenge.sanction_v2 import DisputeView, decide, due_beacon, grade

ROOT, OTHER = "aa" * 32, "bb" * 32
AUDITOR, MINER = "auditor", "miner"


@pytest.mark.parametrize(
    ("first_bad_leaf", "recomputed", "expected"),
    [
        (1, OTHER, "LOCATED"),
        (0, ROOT, "LOCATED"),
        (None, ROOT, "COMMIT_CONTRADICTS_LEAVES"),
        (None, OTHER, None),
    ],
    ids=["located", "located-at-zero", "leaves-agree-commit-lies", "audit-anomaly"],
)
def test_grade_names_what_the_verdict_proves(first_bad_leaf, recomputed, expected):
    assert grade(first_bad_leaf, recomputed, ROOT) == expected


def test_the_sanction_waits_inside_the_window_and_applies_at_its_end():
    assert decide(now=609, due=610, dispute=None, auditor=AUDITOR) == "WAIT"
    assert decide(now=610, due=610, dispute=None, auditor=AUDITOR) == "APPLY"


def test_an_open_contest_postpones_the_sanction_past_the_window():
    open_contest = DisputeView(resolved=False)
    assert decide(now=10_000, due=610, dispute=open_contest, auditor=AUDITOR) == "WAIT"


@pytest.mark.parametrize(
    ("now", "reason", "loser", "expected"),
    [
        (10_000, "FRAUD", AUDITOR, "CANCEL"),
        (10_000, "AUDITOR_FAULT", AUDITOR, "CANCEL"),
        (100, "FRAUD", AUDITOR, "CANCEL"),
        (10_000, "INFRASTRUCTURE", None, "APPLY"),
        (10_000, "PARTY_TIMEOUT", MINER, "APPLY"),
        (10_000, "FRAUD", MINER, "APPLY"),
        (10_000, "INFRASTRUCTURE", AUDITOR, "APPLY"),
        (10_000, "AUDITOR_FAULT", "another-auditor", "APPLY"),
        (100, "INFRASTRUCTURE", None, "WAIT"),
    ],
    ids=[
        "referee-for-miner",
        "witnessed-auditor-lapse",
        "cancelled-before-the-window-ends",
        "undecided",
        "miner-lapse",
        "miner-lost",
        "infrastructure-never-exonerates",
        "stranger",
        "closed-inside-the-window",
    ],
)
def test_only_a_finding_for_the_miner_cancels_the_sanction(now, reason, loser, expected):
    closed = DisputeView(resolved=True, reason=reason, loser=loser)
    assert decide(now=now, due=610, dispute=closed, auditor=AUDITOR) == expected


def test_the_window_is_one_dispute_turn():
    assert due_beacon(first_seen=100, dispute_per_level=600) == 700
    assert due_beacon(first_seen=5, dispute_per_level=7) == 12
