"""Beacon rounds the TEST challenge never received, and the identities that wait for one.

    docker exec -i hypertrain-test-challenge python -I - [SINCE_SECONDS [RUN_ID]] < beacon_gaps.py

The pusher can only refill rounds after the highest one the service holds, so a round skipped while
a remote reference holds the store lock stays missing. The challenge reads exact rounds: the seed of
a trial is the round after the one the previous trial was finalized in, and an identity whose seed
is missing cannot get its challenge until its window has passed (docs/adr/0012).

Reads the `beacon` table of /data/challenge.db, or of /data/runs/<run id>/challenge.db when a run
lives in a child store, with mode=ro. Prints one JSON line and nothing secret:

    now, emitted, lag      highest round held, round the drand clock is at, their difference
    gate_open              lag is within the freshness bound (the gate refuses beyond 2 rounds)
    since, window_start    the window asked for, and the first round it covers
    first                  the first round held inside the window
    missing                rounds of the window that were never pushed (at most 40 listed); when the
                           table holds rounds from before the window, the window's own first
                           rounds count too
    missing_count          how many
    stale                  rounds exist but none inside the window: the pusher has been dead
    stranded               identities (first 12 characters of the id) in a state that asks for a
                           challenge whose seed_beacon is one of the missing rounds

Exit 0: nothing missing in the window. 3: a round is missing, or the table is stale. 4: bad
arguments, unknown run or no beacon table. With a RUN_ID the run must live in a child store or be
known to the root store: a mistyped id is not read from the wrong database.
"""

from __future__ import annotations

import json
import re
import sqlite3
import sys
import time
from pathlib import Path
from typing import Any

GENESIS, PERIOD = 1692803367, 3  # drand quicknet; the same pair challenge_image.sh uses for `lag`
LAG_BOUND = 2  # store.py `_snapshot_current_v2`: 503 "beacon infrastructure paused" beyond it
DEFAULT_SINCE, MAX_SINCE, LISTED = 1800, 86400, 40
ASKING = ("APPLIED", "PROBATION", "ACTIVE")  # Admission.challenge refuses every other state


def report(db: sqlite3.Connection, since: int, wall: float) -> dict[str, Any]:
    emitted = int(wall - GENESIS) // PERIOD + 1
    start = max(1, emitted - since // PERIOD)
    held = {
        number for (number,) in db.execute("SELECT round FROM beacon WHERE round >= ?", (start,))
    }
    now = db.execute("SELECT MAX(round) FROM beacon").fetchone()[0] or 0
    older = db.execute("SELECT 1 FROM beacon WHERE round < ? LIMIT 1", (start,)).fetchone()
    first = min(held, default=None)
    # a table that already held rounds before the window must hold the window's first ones too
    begin = start if older else first
    missing = [] if begin is None else [r for r in range(begin, now + 1) if r not in held]
    stranded: list[dict[str, Any]] = []
    has_admissions = db.execute(
        "SELECT 1 FROM sqlite_master WHERE type='table' AND name='admissions_v2'"
    ).fetchone()
    if missing and has_admissions:
        lost = set(missing)
        for admission_id, state, seed in db.execute(
            "SELECT admission_id, state, seed_beacon FROM admissions_v2"
        ):
            if state in ASKING and seed in lost:
                stranded.append(
                    {"admission": str(admission_id)[:12], "state": state, "seed_beacon": seed}
                )
    return {
        "now": now,
        "emitted": emitted,
        "lag": emitted - now,
        "gate_open": emitted - now <= LAG_BOUND,
        "since": since,
        "window_start": start,
        "first": first,
        "missing": missing[:LISTED],
        "missing_count": len(missing),
        "stale": bool(now) and not held,
        "stranded": stranded,
    }


def main(argv: list[str], data: Path = Path("/data"), wall: float | None = None) -> int:
    since = DEFAULT_SINCE
    if argv and not (argv[0].isdigit() and 1 <= int(argv[0]) <= MAX_SINCE):
        print(f"usage: [SINCE_SECONDS 1..{MAX_SINCE} [RUN_ID]]", file=sys.stderr)
        return 4
    if argv:
        since = int(argv[0])
    if len(argv) > 2 or (len(argv) == 2 and not re.fullmatch(r"[0-9a-f]{64}", argv[1])):
        print("usage: [SINCE_SECONDS [RUN_ID: 64 hex]]", file=sys.stderr)
        return 4
    child = data / "runs" / argv[1] if len(argv) == 2 else None
    hosted = child is not None and (child / "challenge.db").is_file()
    path = (child if hosted and child else data) / "challenge.db"
    if not path.is_file():
        print(f"no challenge.db at {path}", file=sys.stderr)
        return 4
    db = sqlite3.connect(path.as_uri() + "?mode=ro", uri=True)
    try:
        if len(argv) == 2 and not hosted:  # no child store: the root must be the run's host
            if not db.execute("SELECT 1 FROM runs WHERE run_id=?", (argv[1],)).fetchone():
                print(
                    f"run {argv[1][:12]} has no child store and the root does not know it",
                    file=sys.stderr,
                )
                return 4
        out = report(db, since, time.time() if wall is None else wall)
    except sqlite3.Error as error:
        print(f"cannot read the beacon table: {type(error).__name__}", file=sys.stderr)
        return 4
    finally:
        db.close()
    print(json.dumps(out, sort_keys=True))
    return 3 if out["missing_count"] or out["stale"] else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
