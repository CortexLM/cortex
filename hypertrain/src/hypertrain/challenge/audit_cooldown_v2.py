"""Bounded, self-releasing cooldown for an auditor whose lease expired unanswered.

Everything is counted in verified beacon rounds: no wall clock, no timer. The store keeps the
returned record as records_v2(kind='auditor-cooldown', id=<auditor hotkey>) and asks `cooling_until`
whether it still refuses the auditor. The schedule is one lease, then two leases (the absolute
window), then stays at two; a strike is forgotten after one quiet lease. An auditor that has
withheld `WITHHELD_STRIKES` signatures of a verdict the master stored as a MISMATCH is barred
for `STRIKE_BAR_ABSOLUTE_WINDOWS` absolute windows instead (ADR 0011 Part 9).
"""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from typing import Any

from pydantic import JsonValue

# The ordinary schedule caps at two leases: the third withheld MISMATCH is the first it cannot
# lengthen.
WITHHELD_STRIKES = 3
# Strictly longer than that cap, or a bar would add nothing to a second ordinary silence.
STRIKE_BAR_ABSOLUTE_WINDOWS = 2
# A job whose MISMATCH was withheld may open one window beyond the pinned `max_attempts`.
WITHHELD_EXTRA_WINDOWS = 1


def cooling_until(record: Mapping[str, Any] | None, now: int) -> int | None:
    """The round the refusal ends at, or None when the auditor may lease (now >= until)."""
    if record is None or now >= record["until"]:
        return None
    return int(record["until"])


def others_free(
    auditors: Sequence[str], me: str, records: Mapping[str, Mapping[str, Any]], now: int
) -> bool:
    """Whether some other configured auditor is outside its cooldown. Configured is not live: a
    configured auditor that never polls counts as free, so the guarantee is only that the last
    configured auditor outside a cooldown is never refused."""
    return any(cooling_until(records.get(a), now) is None for a in auditors if a != me)


def next_record(
    previous: Mapping[str, Any] | None,
    *,
    job_id: str,
    cause: str,
    now: int,
    lease_rounds: int,
    absolute_rounds: int,
    free: bool,
    withheld: int = 0,
) -> dict[str, JsonValue]:
    """The refusal after one more silent lease; `free` False means this is the last free auditor,
    whose strike is recorded but whose refusal is zero rounds. `withheld` counts the withheld
    MISMATCHes of this auditor, this lapse included: from `WITHHELD_STRIKES` on it is the bar."""
    strikes = 1
    if previous is not None and now < previous["until"] + lease_rounds:
        strikes = int(previous["strikes"]) + 1
    barred = withheld >= WITHHELD_STRIKES
    length = (
        STRIKE_BAR_ABSOLUTE_WINDOWS * absolute_rounds
        if barred
        else min(lease_rounds * strikes, absolute_rounds)
    )
    return {
        "job_id": job_id,
        "reason": "AUDITOR_WITHHELD" if barred else cause,
        "strikes": strikes,
        "since": now,
        "until": now + length if free else now,
    }
