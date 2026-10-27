"""Disputes that nobody can decide: which open turns the server may close, and what it records."""

from __future__ import annotations

from pathlib import Path

import pytest

from hypertrain.auditor.island_bisect import RefereeEvidence
from hypertrain.challenge.disputes_v2 import Turn

from .test_disputes_v2 import ack_turn, service

# Wire-visible evidence tags: a rename is a change of bytes, so they are literals here.
HORIZON = "bounded-infrastructure-exclusion"
FROZEN = "unadjudicable-paused-transcript"
ABANDONED = "abandoned-turn"
FINAL_OP = {"paused": True, "level": "op", "ctx": [1, 0], "interval": (1, 2)}


def instants(turn: Turn, T: int) -> dict[str, int]:
    return {
        "early": 2,
        "grace-end": turn.turn_deadline + T,
        "grace-over": turn.turn_deadline + T + 1,
        "horizon": turn.absolute_deadline,
        "past-horizon": turn.absolute_deadline + 1,
    }


def clamped(turn: Turn) -> dict[str, int]:
    """A late move: bisect() clamps turn_deadline to the absolute horizon."""
    return {"turn_deadline": turn.absolute_deadline}


@pytest.mark.parametrize(
    "update,at,expected",
    [
        pytest.param({}, "grace-end", None, id="in-grace"),
        pytest.param({}, "grace-over", ABANDONED, id="after-grace"),
        pytest.param({}, "early", None, id="before-turn-deadline"),
        pytest.param({}, "past-horizon", HORIZON, id="past-horizon-wins-over-grace"),
        pytest.param(clamped, "horizon", None, id="clamped-turn-at-horizon"),
        pytest.param(clamped, "past-horizon", HORIZON, id="clamped-turn-past-horizon"),
        pytest.param({"paused": True}, "early", FROZEN, id="step-pause"),
        pytest.param({"paused": True, "interval": (0, 1)}, "early", FROZEN, id="step-pause-narrow"),
        pytest.param(
            {"paused": True, "level": "layer", "ctx": [1]}, "early", FROZEN, id="layer-pause"
        ),
        pytest.param({**FINAL_OP, "interval": (0, 2)}, "early", FROZEN, id="op-pause-wide"),
        pytest.param(FINAL_OP, "early", None, id="final-op-pause"),
        pytest.param(FINAL_OP, "horizon", None, id="final-op-pause-at-horizon"),
        pytest.param(FINAL_OP, "past-horizon", HORIZON, id="final-op-pause-past-horizon"),
        pytest.param(
            {**FINAL_OP, "paused": False}, "grace-end", None, id="final-op-unpaused-in-grace"
        ),
    ],
)
def test_blocked_cause_table(tmp_path: Path, update, at: str, expected: str | None) -> None:
    # Given: a funded contest opened at round 1; the table varies only its shape and the round.
    d, _, _, _, raw = service(tmp_path)
    turn = d.open(raw, now=1)
    T = d.escrow.manifest.training.verify.T.dispute_per_level
    candidate = turn.model_copy(update=update(turn) if callable(update) else update)
    # When / Then
    assert d.blocked_cause(candidate, instants(turn, T)[at]) == expected


def test_resolved_turn_is_never_blocked(tmp_path: Path) -> None:
    # Given: a contest the horizon has already closed.
    d, _, _, _, raw = service(tmp_path)
    turn = d.open(raw, now=1)
    closed = d.exclude(turn.dispute_id, now=turn.absolute_deadline + 1)
    assert closed.resolution is not None
    # When / Then: no later round makes it closable again.
    assert d.blocked_cause(closed, closed.absolute_deadline + 5) is None


def test_acknowledged_silence_is_abandoned_all_the_same(tmp_path: Path) -> None:
    # Given: the expected party acknowledged the turn, then went silent.
    d, f, _, _, raw = service(tmp_path)
    turn = d.open(raw, now=1)
    ack_turn(d, f.HOT, 2)
    T = d.escrow.manifest.training.verify.T.dispute_per_level
    # When / Then: an acknowledgement does not extend the grace window.
    assert d.blocked_cause(turn, turn.turn_deadline + T) is None
    assert d.blocked_cause(turn, turn.turn_deadline + T + 1) == ABANDONED


@pytest.mark.parametrize(
    "extra,tag",
    [
        pytest.param({}, HORIZON, id="default"),
        pytest.param({"cause": FROZEN}, FROZEN, id="frozen"),
        pytest.param({"cause": ABANDONED}, ABANDONED, id="abandoned"),
    ],
)
def test_exclusion_evidence_names_its_cause(tmp_path: Path, extra: dict, tag: str) -> None:
    # Given: a paused contest, the one shape exclude() accepts before the horizon.
    d, _, _, _, raw = service(tmp_path)
    turn = d.open(raw, now=1)
    d.pause(turn.dispute_id)
    # When
    closed = d.exclude(turn.dispute_id, now=2, **extra)
    # Then: the omitted cause keeps the bytes exclude() has always written.
    assert closed.resolution is not None
    body = d.db.execute(
        "SELECT body FROM dispute_evidence_v2 WHERE hash=?", (closed.resolution.evidence_hash,)
    ).fetchone()[0]
    assert RefereeEvidence.model_validate_json(body).op_spec == tag
