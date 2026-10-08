"""GET-only: budget snapshot + pick 3 rentable 1x5090 hosts, rewrite live-config hosts.

Usage: select_hosts.py SNAPSHOT_OUT.json [--keep MACHINE_ID ...]
Exit 0 = config updated; 3 = abort (owned instances != 0, unknown inventory, or no 3-host set).
Rules (plan todo 12 + orchestrator gate): verified on-demand 1x RTX 5090, reliability2>=0.98,
cuda_max_good>=13.0, disk_space>=40, dph_total<=0.60, distinct host_id+machine_id,
>=2 driver major branches; cheapest worst-case first.
"""

from __future__ import annotations

import datetime
import json
import sys
import time
import urllib.parse
from decimal import Decimal
from pathlib import Path

from hypertrain.gpu_ops import budget
from hypertrain.gpu_ops.journal import Journal
from hypertrain.gpu_ops.provider import Provider, list_rows, read_api_key

CFG = Path(__file__).resolve().parent / "live-config.json"

def main(argv: list[str]) -> int:
    out = Path(argv[1])
    keep = [int(x) for x in argv[argv.index("--keep") + 1 :]] if "--keep" in argv else []
    cfg = json.loads(CFG.read_text())
    raw = out.with_suffix("").with_name(out.stem + "-raw")
    raw.mkdir(mode=0o700, exist_ok=True)
    j = Journal(raw)
    p = Provider(cfg["base_url"], read_api_key(cfg["key_file"]), raw, j, False)
    acct = p.call("GET", "/api/v0/users/current/", "acct")
    inv = list_rows(p.call("GET", "/api/v1/instances/?limit=25", "inventory"))
    now = int(time.time())
    f = json.dumps({"day": {"gte": now - 30 * 86400, "lte": now}})
    qs = urllib.parse.urlencode({"select_filters": f, "limit": 500})
    ch = p.call("GET", "/api/v0/charges/?" + qs, "charges")
    q = {
        "gpu_name": {"eq": "RTX 5090"}, "num_gpus": {"eq": 1}, "rentable": {"eq": True},
        "rented": {"eq": False}, "verified": {"eq": True}, "reliability2": {"gte": 0.98},
        "cuda_max_good": {"gte": 13.0}, "disk_space": {"gte": 40}, "dph_total": {"lte": 0.6},
        "type": "on-demand", "limit": 200,
    }
    r = p.call("GET", "/api/v0/bundles/?" + urllib.parse.urlencode({"q": json.dumps(q)}), "offers")
    credit = budget.dec(acct.parsed.get("credit")) if acct.ok() else None
    chp = ch.parsed if isinstance(ch.parsed, dict) else {}
    snap = {
        "taken_utc": datetime.datetime.now(datetime.UTC).isoformat(),
        "account_id": acct.parsed.get("id") if acct.ok() else None,
        "credit_usd": str(credit),
        "inventory": {"http_status": None if inv is None else 200, "parsed": inv is not None,
                      "owned_instances": None if inv is None else len(inv)},
        "charges_last_30d": {
            "http_status": ch.status,
            "count": len(chp.get("results") or chp.get("charges") or []),
        },
        "raw_dir": str(raw),
    }
    hours = Decimal(int(cfg["hard_deadline_seconds"]) + int(cfg["hard_grace_seconds"])) / 3600
    offers = r.parsed.get("offers") if r.ok() else None
    cands = []
    for o in offers or []:
        try:
            w = budget.host_worst_case(o, hours, int(cfg["disk_gb"]), int(cfg["egress_gb"]))
        except ValueError:
            continue
        if (
            o.get("reliability2", 0) >= 0.98
            and o.get("dph_total", 9) <= 0.6
            and o.get("driver_version")
        ):
            cands.append((0 if o["machine_id"] in keep else 1, w, o["id"], o))
    cands.sort(key=lambda c: (c[0], c[1], c[2]))
    pick: list[dict[str, object]] = []
    for _, _, _, o in cands:
        if any(o["host_id"] == x["host_id"] or o["machine_id"] == x["machine_id"] for x in pick):
            continue
        majors = {str(x["driver_version"]).split(".")[0] for x in pick}
        majors |= {o["driver_version"].split(".")[0]}
        if len(pick) == 2 and len(majors) < 2:
            continue
        pick.append(o)
        if len(pick) == 3:
            break
    keys = (
        "id", "machine_id", "host_id", "dph_total", "driver_version", "geolocation",
        "reliability2",
    )
    snap["offers_seen"] = None if offers is None else len(offers)
    snap["picked"] = [{k: o.get(k) for k in keys} for o in pick]
    out.write_text(json.dumps(snap, indent=1))
    print(json.dumps(snap))
    if credit is None or inv is None or len(inv) != 0 or len(pick) != 3:
        return 3
    cfg["hosts"] = [
        {"role": f"h{i}", "machine_id": o["machine_id"], "max_dph_total": 0.6}
        for i, o in enumerate(pick)
    ]
    cfg["provenance"] = (
        f"plan todo 12; hosts picked {snap['taken_utc']} by "
        f"experiments/gpu_phase_a/select_hosts.py (rules in its docstring), snapshot {out}"
    )
    CFG.write_text(json.dumps(cfg, indent=1) + "\n")
    return 0

if __name__ == "__main__":
    sys.exit(main(sys.argv))
