"""The cadence `challenge_image.sh beacon` writes, against a store lock held by a remote reference.

`beacon_model` drives the real `Pusher.run` through the outage (docs/adr/0012). What it must show:
no round between the first and the last one the service holds is skipped (a trial's seed is the
round after the one it was finalized in, and the challenge reads that exact round), and the pusher
does not give up while the lock lasts. Which numbers the script writes is read from the compose
file it leaves behind, so an edit of the script moves these tests.
"""

from __future__ import annotations

import pytest

from .beacon_model import simulate
from .sandbox import BEACON

OFFSETS = (0.0, 0.7, 1.4, 2.1, 2.8, 3.5, 5.0, 7.0)  # where in a drand period the lock begins
OLD = {"interval": 0.2, "catch_up": 3, "max_failures": 5}  # what the script wrote before ADR 0012


@pytest.fixture
def shipped(box, backup) -> dict[str, float]:
    box.run("beacon", backup, mutates={BEACON})
    entrypoint = box.live_yaml()["services"]["beacon"]["entrypoint"]
    return {
        "interval": float(entrypoint[9]),
        "catch_up": int(entrypoint[11]),
        "max_failures": int(entrypoint[13]),
    }


@pytest.mark.parametrize("lock", [13, 19, 43, 60, 120])
def test_a_lock_of_up_to_two_minutes_skips_no_round_and_the_gate_reopens(shipped, lock):
    for offset in OFFSETS:
        got = simulate(lock, offset=offset, **shipped)
        assert (got.exit_code, got.holes) == (0, 0), (lock, offset, got)
        assert got.final_lag <= 2, (lock, offset, got)  # the gate is open again at the end
        # The price of never skipping: the pusher refills oldest first and the gate stays closed
        # for up to about four times the lock (ADR 0012 section 3).
        assert got.gate_closed_after_lock <= 4 * lock + 20, (lock, offset, got)


def test_the_pusher_outlasts_a_lock_of_five_minutes(shipped):
    for offset in OFFSETS:
        got = simulate(300, offset=offset, **shipped)
        assert got.exit_code == 0 and got.timeouts < shipped["max_failures"], (offset, got)


def test_the_model_sees_what_was_wrong_with_the_old_cadence():
    skipped = [simulate(13, offset=o, **OLD).holes for o in OFFSETS]
    assert (
        max(skipped) >= 1
    )  # catch-up 3: a 13 s lock already hides rounds the pusher never refills
    assert simulate(60, **OLD).exit_code == 1  # 5 failures: gone after about 50 s of lock
    # each of the two numbers matters on its own
    assert simulate(60, interval=0.2, catch_up=40, max_failures=5).exit_code == 1
    assert simulate(120, interval=0.2, catch_up=20, max_failures=30).holes > 0


def test_the_model_reproduces_two_measurements_of_s13(shipped):
    # "5 of its 10 allowed failures per 60 s of lock"
    assert {simulate(60, offset=o, **shipped).timeouts for o in OFFSETS} == {5}
    # catch-up 3 after a 13 s lock (cache kept): the first answer came 20 s after the reference
    closed = [
        simulate(13, offset=o, **{**OLD, "max_failures": 30}).gate_closed_after_lock
        for o in OFFSETS
    ]
    assert 12 <= sum(closed) / len(closed) <= 28
