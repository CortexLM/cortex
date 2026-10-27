"""Read-only view of one round of a TEST run: deadlines, beacon round, records, audit leases.

docker exec -i hypertrain-test-challenge python -I - RUN_ID W < round_status.py
Reads /data/challenge.db with mode=ro; prints public identifiers only.
"""

from __future__ import annotations

import json
import sqlite3
import sys
from pathlib import Path


def result(raw: str | None) -> object:
    if not raw:
        return None
    try:
        data = json.loads(raw)
    except ValueError:
        return raw[:40]
    return data.get("result") if isinstance(data, dict) else str(data)[:40]


run_id, w = sys.argv[1], int(sys.argv[2])
child = Path("/data/runs", run_id)  # a run created while the root hosts one lives here
root = child if (child / "challenge.db").is_file() else Path("/data")
db = sqlite3.connect((root / "challenge.db").as_uri() + "?mode=ro", uri=True)
db.row_factory = sqlite3.Row
now = db.execute("SELECT MAX(round) FROM beacon").fetchone()[0]
row = db.execute(
    "SELECT data FROM records_v2 WHERE run_id=? AND kind='round' AND id=?", (run_id, str(w))
).fetchone()
body = json.loads(row["data"])["body"] if row else {}
deadlines = {
    k: body[k]
    for k in ("d_open", "d_assign", "d_commit", "d_audit", "d_upload", "d_final")
    if k in body
}
kinds = {
    r["kind"]: r["n"]
    for r in db.execute(
        "SELECT kind, COUNT(*) AS n FROM records_v2 WHERE run_id=? AND (id=? OR id LIKE ?) "
        "GROUP BY kind",
        (run_id, str(w), f"{w}:%"),
    )
}
leases = [
    {
        "job": r["id"][:12],
        "hotkey": (r["hotkey"] or "")[:8],
        "state": r["state"],
        "attempts": r["attempts"],
        "auditor": (r["auditor"] or "")[:8],
        "created": r["created"],
        "expires": r["expires"],
        "result": result(r["result"]),
    }
    for r in db.execute(
        "SELECT * FROM audit_leases_v2 WHERE run_id=? AND w=? ORDER BY id", (run_id, w)
    )
]
print(json.dumps({"now": now, "deadlines": deadlines, "records": kinds, "leases": leases}))
