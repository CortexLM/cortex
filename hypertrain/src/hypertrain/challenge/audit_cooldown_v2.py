"""Bounded, self-releasing cooldown for an auditor whose lease expired unanswered.

Everything is counted in verified beacon rounds: no wall clock, no timer. The store keeps the
returned record as records_v2(kind='auditor-cooldown', id=<auditor hotkey>) and asks `cooling_until`
whether it still refuses the auditor. The schedule is one lease, then two leases (the absolute
window), then stays at two; a strike is forgotten after one quiet lease.
"""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from typing import Any

from pydantic import JsonValue


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
) -> dict[str, JsonValue]:
    """The refusal after one more silent lease; `free` False means this is the last free auditor,
    whose strike is recorded but whose refusal is zero rounds."""
    strikes = 1
    if previous is not None and now < previous["until"] + lease_rounds:
        strikes = int(previous["strikes"]) + 1
    length = min(lease_rounds * strikes, absolute_rounds)
    return {
        "job_id": job_id,
        "reason": cause,
        "strikes": strikes,
        "since": now,
        "until": now + length if free else now,
    }
