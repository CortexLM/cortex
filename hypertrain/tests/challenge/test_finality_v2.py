"""A retired round is closed with the model unchanged; it can never settle or be excluded."""

from __future__ import annotations

from dataclasses import replace

import pytest

from hypertrain.aggregator.core import FinalityError
from hypertrain.challenge import finality_v2 as f
from hypertrain.protocol.messages import Finalize

OLD, NEW, EVIDENCE = "aa" * 32, "bb" * 32, "cc" * 32


def status(w: int = 0, **kw) -> f.RoundStatus:
    base = dict(
        run_id="run",
        w=w,
        applied=False,
        disposition="PENDING",
        unresolved_disputes=0,
        unresolved_audits=3,
        rollback_complete=False,
        state_hash=OLD,
        resolution_hash=None,
    )
    return f.RoundStatus(**{**base, **kw})


def retired(w: int = 0, **kw) -> f.RoundStatus:
    return status(
        w,
        applied=True,
        disposition="RETIRED",
        unresolved_audits=0,
        state_hash=NEW,
        resolution_hash=EVIDENCE,
        **kw,
    )


@pytest.mark.parametrize("disputes", [0, 1], ids=["no-dispute", "open-dispute"])
def test_retire_closes_a_pending_round_with_the_model_unchanged(disputes):
    done = f.retire(status(unresolved_disputes=disputes), NEW, EVIDENCE)
    assert done == retired(unresolved_disputes=disputes)
    assert done.closed is (disputes == 0)  # an open dispute keeps it open until Part 1 closes it


@pytest.mark.parametrize(
    "refused",
    [
        status(applied=True),
        status(applied=True, disposition="SETTLED", unresolved_audits=0, resolution_hash=EVIDENCE),
        status(disposition="EXCLUDED", resolution_hash=EVIDENCE, rollback_complete=True),
        retired(),
    ],
    ids=["applied", "settled", "excluded", "retired"],
)
def test_only_a_pending_unapplied_round_can_be_retired(refused):
    with pytest.raises(FinalityError, match="pending, unapplied"):
        f.retire(refused, NEW, EVIDENCE)


def test_a_retired_round_cannot_settle():
    final = Finalize(w=0, final_theta_hash_w1=NEW, included=[], entitlements_root=OLD)
    with pytest.raises(FinalityError, match="cannot settle"):
        f.settle(retired(), final, EVIDENCE)


def test_a_retired_round_cannot_be_excluded():
    with pytest.raises(FinalityError, match="cannot rewrite model history"):
        f.exclude(retired(), EVIDENCE)


def test_the_rounds_after_a_retired_one_open_and_apply_as_after_an_applied_one():
    # Given: round 0 retired, round 1 applied and still being audited
    history = [f.retire(status(), NEW, EVIDENCE), status(1, applied=True)]
    # Then: the retired round neither blocks w+2 nor counts as a speculative round
    f.require_open("run", 2, history)
    f.require_apply("run", 2, history)
    # and it is an applied predecessor for the round that follows it
    f.require_apply("run", 1, history[:1])
    # while a pending round 0 is none of these
    with pytest.raises(FinalityError, match="w\\+2 blocked"):
        f.require_open(
            "run", 2, [replace(history[0], applied=False, disposition="PENDING"), *history[1:]]
        )
