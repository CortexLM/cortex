"""The cadence `challenge_image.sh beacon` writes, against a store lock held by a remote reference.

`beacon_model` drives the real `Pusher.run` through the outage (docs/adr/0012). What it must show:
no round between the first and the last one the store holds is skipped (a trial's seed is the round
after the one it was finalized in, and the challenge reads that exact round), and the pusher does
not give up while the lock lasts. Which numbers the script writes is read from the compose file it
leaves behind, so an edit of the script moves these tests. Both layouts of the challenge are run:
the run in the root store, and the run in a child store with a lock of its own.
"""

from __future__ import annotations

import pytest

from .beacon_model import Costs, HeadFirstPusher, simulate
from .sandbox import BEACON

OFFSETS = (0.0, 0.7, 1.4, 2.1, 2.8, 3.5, 5.0, 7.0)  # where in a drand period the lock begins
LAYOUTS = ("root", "child")
# What the previous version of the script left on the TEST stack, with the failure budget S13
# reported ("5 of its 10 allowed failures"); the sandbox compose file has 5.
OLD = {"interval": 0.2, "catch_up": 3, "max_failures": 10}


@pytest.fixture
def shipped(box, backup) -> dict[str, float]:
    box.run("beacon", backup, mutates={BEACON})
    entrypoint = box.live_yaml()["services"]["beacon"]["entrypoint"]
    return {
        "interval": float(entrypoint[9]),
        "catch_up": int(entrypoint[11]),
        "max_failures": int(entrypoint[13]),
    }


@pytest.mark.parametrize("layout", LAYOUTS)
@pytest.mark.parametrize("lock", [13, 19, 43, 60, 150])
def test_a_lock_of_up_to_two_and_a_half_minutes_skips_no_round_and_the_gate_reopens(
    shipped, lock, layout
):
    for offset in OFFSETS:
        got = simulate(lock, offset=offset, layout=layout, **shipped)
        assert (got.exit_code, got.holes) == (0, 0), (lock, offset, got)
        assert got.final_lag <= 2, (lock, offset, got)  # the gate is open again at the end
        # The price of never skipping: the pusher refills oldest first and the gate stays closed
        # for up to about four times the lock (ADR 0012 section 3).
        assert got.gate_closed_after_lock <= 4 * lock + 20, (lock, offset, got)


@pytest.mark.parametrize("layout", LAYOUTS)
def test_the_pusher_outlasts_a_lock_of_five_minutes(shipped, layout):
    for offset in OFFSETS:
        got = simulate(300, offset=offset, layout=layout, **shipped)
        assert got.exit_code == 0 and got.timeouts < shipped["max_failures"], (offset, got)


@pytest.mark.parametrize("layout", LAYOUTS)
def test_the_model_sees_what_was_wrong_with_the_old_cadence(layout):
    lock = {"root": 13, "child": 60}[layout]
    skipped = [simulate(lock, offset=o, layout=layout, **OLD).holes for o in OFFSETS]
    assert max(skipped) >= 1  # catch-up 3: the pusher never refills what the lock hid
    # each of the two numbers matters on its own
    assert simulate(120, layout="root", **OLD).exit_code == 1  # 10 failures: gone after ~110 s
    assert simulate(60, interval=0.2, catch_up=60, max_failures=5).exit_code == 1
    assert simulate(120, interval=0.2, catch_up=20, max_failures=100, layout="root").holes > 0


def test_the_model_reproduces_two_measurements_of_s13(shipped):
    # "5 of its 10 allowed failures per 60 s of lock"
    assert {simulate(60, offset=o, layout="root", **shipped).timeouts for o in OFFSETS} == {5}
    # catch-up 3 after a 13 s lock (cache kept): the first answer came 20 s after the reference
    for layout in LAYOUTS:
        closed = [
            simulate(13, offset=o, layout=layout, **OLD).gate_closed_after_lock for o in OFFSETS
        ]
        assert 8 <= sum(closed) / len(closed) <= 28, layout


def test_the_shipped_cadence_keeps_no_hole_when_a_round_costs_more_but_not_when_it_costs_the_period(
    shipped,
):
    """The cadence assumes the pusher keeps up: 2.4 s a round against the 3 s period. The gate
    stays closed much longer at 2.6 s and the pusher never catches up at 2.9 s (ADR 0012)."""
    for cost in (1.0, 1.3):  # 2.0 s and 2.6 s a round
        got = simulate(60, costs=Costs(cost, cost), **shipped)
        assert (got.exit_code, got.holes, got.final_lag <= 2) == (0, 0, True), cost
    assert simulate(60, costs=Costs(1.45, 1.45), **shipped).final_lag > 2


@pytest.mark.parametrize("layout", LAYOUTS)
@pytest.mark.parametrize("lock", [13, 19, 43, 60])
def test_a_pusher_that_pushes_the_head_first_would_reopen_the_gate_within_seconds(
    shipped, lock, layout
):
    """PROPOSAL, not shipped: the evidence for the pusher change ADR 0012 asks for."""
    for offset in OFFSETS:
        got = simulate(lock, offset=offset, layout=layout, pusher=HeadFirstPusher, **shipped)
        assert (got.exit_code, got.holes) == (0, 0), (lock, offset, got)
        assert got.final_lag <= 2 and got.gate_closed_after_lock <= 10, (lock, offset, got)
