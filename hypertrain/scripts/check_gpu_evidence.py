"""Validate a todo-12 Phase A or todo-16 Phase B (``"phase": "B"``) evidence JSON.

Exit 0 = valid; 1 = invalid (reasons printed). Any unmasked per-instance secret
(instance_api_key / jupyter_token) in the evidence, or for Phase B in its run's raw logs,
is invalid.
"""

from __future__ import annotations

import base64
import json
import re
import sys
from decimal import Decimal, InvalidOperation
from pathlib import Path
from typing import Any

HEX64 = re.compile(r"[0-9a-f]{64}")
SECRET = re.compile(r'"(instance_api_key|jupyter_token)"\s*:\s*"(?!\*\*\*")[^"]+"')
B_KEYS = (
    "leaves_root",
    "delta_hash",
    "final_theta_hash",
    "topk_index_sha256",
    "topk_value_sha256",
)


def hex64(v: Any) -> bool:
    return isinstance(v, str) and HEX64.fullmatch(v) is not None


def secrets_in(ev: dict[str, Any]) -> list[str]:
    errs = ["evidence holds an unmasked instance secret"] if SECRET.search(json.dumps(ev)) else []
    if ev.get("phase") == "B" and isinstance(ev.get("journal"), str):
        raw = Path(ev["journal"]).parent / "raw"
        for f in sorted(raw.glob("*.json")) if raw.is_dir() else []:
            try:
                text = base64.b64decode(json.loads(f.read_text())["raw_base64"]).decode(
                    "utf-8", "replace"
                )
            except (OSError, ValueError, KeyError, TypeError):
                errs.append(f"raw log unreadable: {f.name}")
                continue
            if SECRET.search(text):
                errs.append(f"raw log holds an unmasked instance secret: {f.name}")
    return errs


def check_b(ev: dict[str, Any], errs: list[str]) -> list[str]:
    verdict = ev.get("verdict")
    hosts = [h for h in ev.get("hosts") or [] if isinstance(h, dict)]
    machines = {h.get("machine_id") for h in hosts}
    if len(hosts) < 2 or None in machines or len(machines) != len(hosts):
        errs.append("need >=2 hosts with distinct machine_ids")
    if len({h.get("driver_version") for h in hosts} - {None, ""}) < 2:
        errs.append("need >=2 distinct driver versions")
    runs = [r for r in ev.get("runs") or [] if isinstance(r, dict)]
    det = {r.get("role"): r.get("result") or {} for r in runs if r.get("arm") == "det"}
    base = {r.get("role"): r.get("result") or {} for r in runs if r.get("arm") == "base"}
    roles = {h.get("role") for h in hosts}
    if set(det) != roles or set(base) != roles:
        errs.append("need one det and one base run per host")
    for role, res in det.items():
        if any(not hex64(res.get(k)) for k in B_KEYS):
            errs.append(f"{role}/det: hashes missing")
        if not isinstance(res.get("leaves"), list) or not res["leaves"]:
            errs.append(f"{role}/det: leaves missing")
        env = res.get("env") or {}
        if env.get("device") != "cuda" or env.get("sm_counts") != [170]:
            errs.append(f"{role}/det: device != cuda or sm_count != 170")
        if (
            res.get("n_gpus") != 8
            or env.get("device_count") != 8
            or res.get("ranks_agree") is not True
        ):
            errs.append(f"{role}/det: not an agreeing 8-rank island")
    oh = ev.get("overhead") or {}
    ratios = oh.get("per_host_ratio") or {}
    mx = money(oh.get("max_ratio"))
    if (
        set(ratios) != roles
        or mx is None
        or mx != max((money(v) or Decimal(0)) for v in ratios.values())
    ):
        errs.append("overhead ratios missing or inconsistent")
    elif oh.get("verdict") != (
        "PASS" if mx <= Decimal("1.5") else "WATCH" if mx <= Decimal("1.6") else "REPLAN"
    ):
        errs.append("overhead verdict does not match the 1.5/1.6 thresholds")
    if not isinstance(ev.get("network"), dict):
        errs.append("network section missing (numbers or CENSORED)")
    if errs:
        return errs
    ref = next(iter(det.values()))
    same = all(r.get(k) == ref.get(k) for r in det.values() for k in ("leaves", *B_KEYS))
    if verdict == "PASS" and not same:
        errs.append("verdict PASS but hosts differ")
    if verdict == "FAIL" and (same or not ev.get("mismatches")):
        errs.append("verdict FAIL needs differing hosts and a mismatch record")
    return errs


def money(v: Any) -> Decimal | None:
    if isinstance(v, bool) or v is None:
        return None
    try:
        d = Decimal(str(v))
    except InvalidOperation:
        return None
    return d if d.is_finite() and d >= 0 else None


def check(ev: dict[str, Any]) -> list[str]:
    errs: list[str] = []
    verdict = ev.get("verdict")
    if verdict not in ("PASS", "FAIL", "CENSORED"):
        errs.append(f"verdict invalid: {verdict!r}")
    inv = ev.get("inventory")
    if not isinstance(inv, dict) or inv.get("parsed") is not True:
        errs.append("final inventory not from a parsed instance list")
    elif type(inv.get("owned_instances")) is not int or inv["owned_instances"] != 0:
        errs.append(f"owned_instances must be 0, got {inv.get('owned_instances')!r}")
    cost = ev.get("cost")
    if not isinstance(cost, dict):
        errs.append("cost missing")
    else:
        cap = money(cost.get("cap_usd"))
        lines = cost.get("lines")
        vals = [money(x.get("usd")) if isinstance(x, dict) else None for x in lines or []]
        if cap is None or not lines or None in vals:
            errs.append("cost cap/lines missing or non-numeric")
        else:
            total = sum((v for v in vals if v is not None), Decimal(0))
            if total > cap:
                errs.append(f"cost {total} exceeds cap {cap}")
            if money(cost.get("total_usd")) != total:
                errs.append("cost total_usd != sum of lines")
    errs += secrets_in(ev)
    if verdict == "CENSORED":
        return errs
    digest = ev.get("image_digest")
    if not (
        isinstance(digest, str) and digest.startswith("sha256:") and HEX64.fullmatch(digest[7:])
    ):
        errs.append("image_digest missing")
    if not isinstance(ev.get("image"), str) or not ev["image"].endswith("@" + str(digest)):
        errs.append("image not pinned to image_digest")
    if ev.get("phase") == "B":
        return check_b(ev, errs)
    hosts = ev.get("hosts") or []
    machines = {h.get("machine_id") for h in hosts if isinstance(h, dict)}
    if len(hosts) < 3 or None in machines or len(machines) != len(hosts):
        errs.append("need >=3 hosts with distinct machine_ids")
    if any(not (isinstance(h, dict) and h.get("driver_version")) for h in hosts):
        errs.append("driver_version missing for a host")
    elif len({h.get("driver_version") for h in hosts}) < 2:
        errs.append("need >=2 distinct driver versions")
    runs = ev.get("runs") or []
    honest = [r for r in runs if isinstance(r, dict) and r.get("name") != "neg"]
    neg = [r for r in runs if isinstance(r, dict) and r.get("name") == "neg"]
    if len(honest) < 6 or len(neg) != 1:
        errs.append("need >=6 honest runs and exactly one negative control")
    for r in honest + neg:
        res = r.get("result") or {}
        tag = f"{r.get('role')}/{r.get('name')}"
        if not (isinstance(res.get("leaves_root"), str) and HEX64.fullmatch(res["leaves_root"])):
            errs.append(f"{tag}: leaves_root missing")
        if not (isinstance(res.get("delta_hash"), str) and HEX64.fullmatch(res["delta_hash"])):
            errs.append(f"{tag}: delta_hash missing")
        if not isinstance(res.get("leaves"), list) or not res["leaves"]:
            errs.append(f"{tag}: leaves missing")
        if not hex64(res.get("final_theta_hash")):
            errs.append(f"{tag}: final_theta_hash missing")
        env = res.get("env") or {}
        if env.get("sm_count") != 170:
            errs.append(f"{tag}: sm_count != 170")
        if env.get("device") != "cuda":
            errs.append(f"{tag}: device != cuda")
        if r.get("machine_id") not in machines:
            errs.append(f"{tag}: machine_id not among hosts")
    if errs:
        return errs
    ref = honest[0]["result"]
    same = all(
        r["result"][k] == ref[k]
        for r in honest
        for k in ("leaves", "leaves_root", "delta_hash", "final_theta_hash")
    )
    if verdict == "PASS":
        if not same:
            errs.append("verdict PASS but honest runs differ")
        n = neg[0]["result"]
        first = next(
            (i for i, (a, b) in enumerate(zip(ref["leaves"], n["leaves"], strict=False)) if a != b),
            None,
        )
        exp = n.get("expected_first_divergent_leaf")
        if exp is None or first != exp:
            errs.append(f"negative control diverged at {first}, expected {exp}")
    if verdict == "FAIL":
        if same:
            errs.append("verdict FAIL but honest runs identical")
        if not ev.get("bisect") and not ev.get("mismatches"):
            errs.append("verdict FAIL without mismatch/bisect record")
    return errs


def main(argv: list[str]) -> int:
    if len(argv) != 2:
        print("usage: check_gpu_evidence.py EVIDENCE.json", file=sys.stderr)
        return 2
    try:
        ev = json.loads(Path(argv[1]).read_text())
    except (OSError, ValueError) as e:
        print(f"INVALID: unreadable: {e}")
        return 1
    errs = check(ev) if isinstance(ev, dict) else ["top-level not an object"]
    for err in errs:
        print("INVALID:", err)
    if not errs:
        print("VALID", ev.get("verdict"))
    return 1 if errs else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
