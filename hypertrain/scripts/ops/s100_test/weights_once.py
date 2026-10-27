"""ONE get_weights call on the TEST challenge, refused unless the run's rewards are payable now.

docker exec -i hypertrain-test-challenge python -I - RUN_ID < weights_once.py
The first answer per epoch is final (store.weights), so an answer given before the issued rewards
vest freezes that epoch unpaid. The call is made only when, at epoch_at, the run has at least one
row escrow.payable() would pay (the query of hypertrain.ledger.escrow_v2.payable) and no matured
reward origin still holds units in reward_pending (vesting not applied yet, or disputed).
The internal bearer is read by path and never printed; the answer holds public hotkeys only.
"""

from __future__ import annotations

import json
import os
import sqlite3
import sys
import time
from pathlib import Path

import httpx

run_id = sys.argv[1]
child = Path("/data/runs", run_id)
root = child if (child / "challenge.db").is_file() else Path("/data")
db = sqlite3.connect((root / "challenge.db").as_uri() + "?mode=ro", uri=True)
manifest = json.loads(
    db.execute("SELECT manifest FROM runs WHERE run_id=?", (run_id,)).fetchone()[0]
)
beacon = manifest["training"]["beacon"]
epoch_at = int(time.time())
now = (epoch_at - beacon["genesis_time"]) // beacon["period"] + 1  # store._pay_v2's drand round
payable = db.execute(
    "SELECT u.origin,u.owner,o.hotkey,u.units FROM escrow_units u "
    "JOIN escrow_origins o ON o.origin=u.origin WHERE u.bucket='available' "
    "AND u.units>0 AND o.hotkey IS NOT NULL AND o.mature_at<=? "
    "AND o.origin NOT IN (SELECT origin FROM escrow_shadow_finalized) "
    "AND EXISTS(SELECT 1 FROM escrow_units p WHERE p.origin=u.origin "
    "AND p.owner=u.owner AND p.bucket='reward_pending')",
    (now,),
).fetchall()
unvested = db.execute(
    "SELECT COUNT(*) FROM escrow_units u JOIN escrow_origins o ON o.origin=u.origin "
    "WHERE u.bucket='reward_pending' AND u.units>0 AND o.reward_round IS NOT NULL "
    "AND o.mature_at<=?",
    (now,),
).fetchone()[0]
out: dict[str, object] = {
    "store": str(root),
    "drand_round_at_epoch_at": now,
    "payable_rows": len(payable),
    "payable_units": sum(r[3] for r in payable),
    "matured_unvested_rows": unvested,
}
if not payable or unvested:
    out["refused"] = "nothing payable yet, or matured rewards not vested: answer would freeze"
    print(json.dumps(out))
    raise SystemExit(3)
genesis = int(os.environ.get("HYPERTRAIN_GENESIS_UNIX", "1692803367"))  # app.Config defaults
epoch = (epoch_at - genesis) // int(os.environ.get("HYPERTRAIN_EPOCH_SECONDS", "4320"))
token = Path("/run/secrets/internal.token").read_text().strip()
r = httpx.get(
    "http://127.0.0.1:8000/internal/v1/get_weights",
    params={"epoch": epoch, "epoch_at": epoch_at},
    headers={"Authorization": "Bearer " + token, "X-Platform-Challenge-Slug": "hypertrain"},
    timeout=120,
)
out.update(epoch=epoch, epoch_at=epoch_at, status=r.status_code, answer=r.json())
print(json.dumps(out, sort_keys=True))
