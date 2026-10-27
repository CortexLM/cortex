"""Beacon rounds the TEST challenge never received, and the identities that wait for one.

    docker exec -i hypertrain-test-challenge python -I - [SINCE_SECONDS [RUN_ID]] < beacon_gaps.py

The pusher can only refill rounds after the highest one the service holds, so a round skipped while
a remote reference holds the store lock stays missing. The challenge reads exact rounds: the seed of
a trial is the round after the one the previous trial was finalized in, and an identity whose seed
is missing cannot get its challenge until its window has passed (docs/adr/0012).

Reads the `beacon` table of /data/challenge.db, or of /data/runs/<run id>/challenge.db when a run
lives in a child store, with mode=ro. Prints one JSON line and nothing secret:

    now, emitted, lag      highest round held, round the drand clock is at, their difference
    since, first           the window asked for, and the first round held inside it
    missing                rounds between first and now that were never pushed (at most 40 listed)
    missing_count          how many
    stranded               identities (first 12 characters of the id) in a state that asks for a
                           challenge whose seed_beacon is one of the missing rounds

Exit 0: no round missing in the window. 3: one is missing. 4: bad arguments or no beacon table.
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
DEFAULT_SINCE, MAX_SINCE, LISTED = 1800, 86400, 40
ASKING = ("APPLIED", "PROBATION", "ACTIVE")  # Admission.challenge refuses every other state


def report(db: sqlite3.Connection, since: int, wall: float) -> dict[str, Any]:
    emitted = int(wall - GENESIS) // PERIOD + 1
    held = {
        number
        for (number,) in db.execute(
            "SELECT round FROM beacon WHERE round >= ?", (max(1, emitted - since // PERIOD),)
        )
    }
    now = db.execute("SELECT MAX(round) FROM beacon").fetchone()[0] or 0
    first = min(held, default=None)
    missing = [] if first is None else [r for r in range(first, now + 1) if r not in held]
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
        "since": since,
        "first": first,
        "missing": missing[:LISTED],
        "missing_count": len(missing),
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
    root = child if child and (child / "challenge.db").is_file() else data
    path = root / "challenge.db"
    if not path.is_file():
        print(f"no challenge.db under {root}", file=sys.stderr)
        return 4
    db = sqlite3.connect(path.as_uri() + "?mode=ro", uri=True)
    try:
        out = report(db, since, time.time() if wall is None else wall)
    except sqlite3.Error as error:
        print(f"cannot read the beacon table: {type(error).__name__}", file=sys.stderr)
        return 4
    finally:
        db.close()
    print(json.dumps(out, sort_keys=True))
    return 3 if out["missing_count"] else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
