"""The strike window of a no-show outlasts two full PROBATION -> ACTIVE ladders (D63, D70).

The window is `NO_SHOW_STRIKE_WINDOW_SCREENS` work screens. A first no-show puts the admission back
on probation; to be silent a second time it has to be rostered again, so the suspension tier is
only reachable if the window is longer than the way back. The numbers are the ones of ADR 0010
("Strike window of a no-show"): the screen length is read from the manifest builders that pin it,
the ladder from the measured durations below.
"""

from __future__ import annotations

import re
from pathlib import Path
from typing import get_args

import pytest

from hypertrain.challenge.admission import NO_SHOW_STRIKE_WINDOW_SCREENS, no_show_strike_window
from hypertrain.protocol.messages_v2 import AdmissionPolicyV2

ROOT = Path(__file__).parents[2]
BEACON_SECONDS = 3  # one drand round
LADDER = get_args(AdmissionPolicyV2.model_fields["clean_finalizations"].annotation)[0]  # 12 trials
# Measured (D70): twelve clean trials of the TEST replay took about 29 minutes; a production
# training round takes about ten.
TEST_LADDER_SECONDS = 29 * 60
PRODUCTION_ROUND_SECONDS = 10 * 60
# Not measured: the longest trial ADR 0015 estimates for its 101 M model (250-390 s) [H].
DENSE_TRIAL_SECONDS = 390


def pinned_screen_epoch(builder: str, pattern: str, group: int = 1) -> int:
    found = re.search(pattern, (ROOT / builder).read_text())
    assert found, f"{builder} no longer pins work_screen_epoch_rounds"
    return int(found.group(group))


# The TEST builder pins 20000 for its dense-0.1b geometry (ADR 0015) and 6000 for the others.
TEST_SCREENS = (
    "scripts/b300/d20/build_test_manifest.sh",
    r'screen_rounds = (\d+) if geometry == "dense-0.1b" else (\d+)\n',
)
# policy name -> (screen length pinned by its builder, rounds of one ladder)
POLICIES = {
    "test": (pinned_screen_epoch(*TEST_SCREENS, group=2), TEST_LADDER_SECONDS // BEACON_SECONDS),
    "test-dense-0.1b": (
        pinned_screen_epoch(*TEST_SCREENS, group=1),
        LADDER * DENSE_TRIAL_SECONDS // BEACON_SECONDS,
    ),
    "production": (
        pinned_screen_epoch("src/hypertrain/ops/manifest_v2.py", r"work_screen_epoch_rounds=(\d+)"),
        LADDER * (PRODUCTION_ROUND_SECONDS // BEACON_SECONDS),
    ),
}


@pytest.mark.parametrize(("epoch", "ladder"), POLICIES.values(), ids=POLICIES.keys())
def test_the_window_covers_at_least_two_full_ladders(epoch: int, ladder: int) -> None:
    assert no_show_strike_window(epoch) >= 2 * ladder


def test_the_numbers_of_the_adr_are_the_ones_of_the_code() -> None:
    # ADR 0010: the TEST ladder is 580 rounds, the 101 M TEST one at most 1560, the production one
    # 2400; the window is 24000 rounds, 80000 for the 101 M TEST geometry.
    assert [ladder for _, ladder in POLICIES.values()] == [580, 1560, 2400]
    assert [no_show_strike_window(epoch) for epoch, _ in POLICIES.values()] == [24000, 80000, 24000]
    assert no_show_strike_window(1) == NO_SHOW_STRIKE_WINDOW_SCREENS
