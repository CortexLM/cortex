"""GET-only: budget snapshot + pick N rentable 8x5090 hosts, quote admission, rewrite live-config.

Usage: select_hosts.py SNAPSHOT_OUT.json [--hosts N] [--config CFG.json] [--exclude MID,MID]
Exit 0 = config updated and the admission quote fits; 3 = abort (owned instances != 0, unknown
inventory/credit, fewer than N hosts, or worst case over cap/credit -> CENSORED, ask the user).
Rules (plan todo 16): verified on-demand 8x RTX 5090, reliability2>=0.98, cuda_max_good>=13.0,
disk_space>=disk_gb, distinct host_id+machine_id, >=2 driver versions; cheapest worst case first.
Run it chained with the launcher in ONE shell (offers churn within ~40 min).
"""

from __future__ import annotations

import datetime
import json
import sys
import urllib.parse
from decimal import Decimal
from pathlib import Path
from typing import Any

from hypertrain.gpu_ops import budget
from hypertrain.gpu_ops.journal import Journal
from hypertrain.gpu_ops.provider import Provider, list_rows, read_api_key

KEYS = (
    "id",
    "machine_id",
    "host_id",
    "dph_total",
    "storage_cost",
    "inet_up_cost",
    "inet_down_cost",
    "driver_version",
    "geolocation",
    "reliability2",
    "cuda_max_good",
    "disk_space",
    "inet_up",
    "inet_down",
)


def pick(
    offers: list[dict[str, Any]], cfg: dict[str, Any], n: int, hours: Decimal
) -> list[dict[str, Any]]:
    cands = []
    for o in offers:
        try:
            w = budget.host_worst_case(o, hours, int(cfg["disk_gb"]), int(cfg["egress_gb"]))
        except ValueError:
            continue
        if (
            o.get("num_gpus") == cfg["num_gpus"]
            and o.get("reliability2", 0) >= cfg.get("min_reliability", 0.98)
            and o.get("inet_down", 0) >= cfg.get("min_inet_down_mbps", 0)
            and o.get("cuda_max_good", 0) >= 13.0
            and o.get("disk_space", 0) >= cfg["disk_gb"]
            and o.get("driver_version")
            and type(o.get("id")) is int
        ):
            cands.append((w, o["id"], o))
    cands.sort(key=lambda c: (c[0], c[1]))
    out: list[dict[str, Any]] = []
    for _, _, o in cands:
        if any(o["host_id"] == x["host_id"] or o["machine_id"] == x["machine_id"] for x in out):
            continue
        if (
            len(out) == n - 1
            and len({x["driver_version"] for x in out} | {o["driver_version"]}) < 2
        ):
            continue
        out.append(o)
        if len(out) == n:
            break
    return out


def main(argv: list[str]) -> int:
    out = Path(argv[1])
    n = int(argv[argv.index("--hosts") + 1]) if "--hosts" in argv else 2
    cfg_path = (
        Path(argv[argv.index("--config") + 1])
        if "--config" in argv
        else Path(__file__).resolve().parent / "live-config.json"
    )
    cfg = json.loads(cfg_path.read_text())
    raw = out.with_suffix("").with_name(out.stem + "-raw")
    raw.mkdir(mode=0o700, exist_ok=True)
    p = Provider(cfg["base_url"], read_api_key(cfg["key_file"]), raw, Journal(raw), False)
    acct = p.call("GET", "/api/v0/users/current/", "acct")
    inv = list_rows(p.call("GET", "/api/v1/instances/?limit=25", "inventory"))
    q = {
        "gpu_name": {"eq": "RTX 5090"},
        "num_gpus": {"eq": cfg["num_gpus"]},
        "rentable": {"eq": True},
        "rented": {"eq": False},
        "verified": {"eq": True},
        "reliability2": {"gte": cfg.get("min_reliability", 0.98)},
        "cuda_max_good": {"gte": 13.0},
        "inet_down": {"gte": cfg.get("min_inet_down_mbps", 0)},
        "type": "on-demand",
        "limit": 200,
    }
    r = p.call("GET", "/api/v0/bundles/?" + urllib.parse.urlencode({"q": json.dumps(q)}), "offers")
    credit = budget.dec(acct.parsed.get("credit")) if acct.ok() else None
    snap0 = json.loads(Path(cfg["budget_snapshot"]).read_text())
    hours = Decimal(int(cfg["hard_deadline_seconds"]) + int(cfg["hard_grace_seconds"])) / 3600
    offers = r.parsed.get("offers") if r.ok() else None
    exclude = (
        {int(x) for x in argv[argv.index("--exclude") + 1].split(",") if x}
        if "--exclude" in argv
        else set()
    )
    usable = [o for o in offers or [] if isinstance(o, dict) and o.get("machine_id") not in exclude]
    chosen = pick(usable, cfg, n, hours)
    plan = None
    if credit is not None and len(chosen) == n:
        plan = budget.evaluate(
            snapshot0_credit=Decimal(str(snap0["credit_usd"])),
            current_credit=credit,
            offers=chosen,
            hard_deadline_seconds=int(cfg["hard_deadline_seconds"])
            + int(cfg["hard_grace_seconds"]),
            disk_gb=int(cfg["disk_gb"]),
            egress_gb=int(cfg["egress_gb"]),
            phase_cap=Decimal(str(cfg["phase_cap_usd"])),
        )
    snap = {
        "taken_utc": datetime.datetime.now(datetime.UTC).isoformat(),
        "account_id": acct.parsed.get("id") if acct.ok() else None,
        "account_matches_snapshot0": acct.ok() and acct.parsed.get("id") == snap0["account_id"],
        "credit_usd": str(credit),
        "inventory": {
            "parsed": inv is not None,
            "owned_instances": None if inv is None else len(inv),
        },
        "offers_seen": None if offers is None else len(offers),
        "picked": [{k: o.get(k) for k in KEYS} for o in chosen],
        "phase_b_cap_usd": str(
            min(budget.GLOBAL_CAP_USD, Decimal(str(snap0["credit_usd"])))
            - Decimal(str(cfg["phase_a_settled_debit_usd"]))
        ),
        "admission_quote": plan,
        "raw_dir": str(raw),
    }
    out.write_text(json.dumps(snap, indent=1))
    print(json.dumps(snap))
    if (
        inv is None
        or inv
        or plan is None
        or not plan["admit"]
        or not snap["account_matches_snapshot0"]
    ):
        return 3
    cfg["hosts"] = [
        {
            "role": f"h{i}",
            "machine_id": o["machine_id"],
            "max_dph_total": float(Decimal(str(o["dph_total"])) * Decimal("1.05")),
        }
        for i, o in enumerate(chosen)
    ]
    cfg["provenance"] = (
        f"plan todo 16; hosts picked {snap['taken_utc']} by select_hosts.py, snapshot {out}"
    )
    cfg_path.write_text(json.dumps(cfg, indent=1) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
