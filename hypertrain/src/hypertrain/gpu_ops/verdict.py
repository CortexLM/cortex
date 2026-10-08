"""Phase A verdict from rescued per-run results (bitwise comparison only)."""

from __future__ import annotations

from typing import Any


def compute_verdict(runs: list[dict[str, Any]], n_hosts: int, runs_per_host: int) -> dict[str, Any]:
    honest = [r for r in runs if r["name"] != "neg"]
    neg = [r for r in runs if r["name"] == "neg"]
    missing = [f"{r['role']}/{r['name']}" for r in runs if not r.get("result")]
    if missing or len(honest) != n_hosts * runs_per_host or len(neg) != 1:
        return {"verdict": "CENSORED", "reason": "missing runs: " + ",".join(missing)}
    machines = {r["machine_id"] for r in honest}
    ref = honest[0]["result"]
    mismatches = []
    for r in honest[1:]:
        res = r["result"]
        if (
            res["leaves"] != ref["leaves"]
            or res["leaves_root"] != ref["leaves_root"]
            or res["delta_hash"] != ref["delta_hash"]
        ):
            first = next(
                (
                    i
                    for i, (a, b) in enumerate(zip(ref["leaves"], res["leaves"], strict=False))
                    if a != b
                ),
                None,
            )
            mismatches.append({"run": f"{r['role']}/{r['name']}", "first_divergent_leaf": first})
    n = neg[0]["result"]
    expected = n.get("expected_first_divergent_leaf")
    neg_first = next(
        (i for i, (a, b) in enumerate(zip(ref["leaves"], n["leaves"], strict=False)) if a != b),
        None,
    )
    neg_ok = expected is not None and neg_first == expected
    off_gate = [
        f"{r['role']}/{r['name']}"
        for r in runs
        if r["result"].get("env", {}).get("device") != "cuda"
        or r["result"].get("env", {}).get("sm_count") != 170
    ]
    if mismatches and not off_gate:
        verdict = "FAIL"
    elif off_gate:
        verdict = "CENSORED"
    elif not neg_ok or len(machines) != n_hosts:
        verdict = "CENSORED"
    else:
        verdict = "PASS"
    return {
        "verdict": verdict,
        "reference_leaves_root": ref["leaves_root"],
        "reference_delta_hash": ref["delta_hash"],
        "mismatches": mismatches,
        "off_hardware_gate": off_gate,
        "distinct_machine_ids": len(machines),
        "negative_control": {
            "expected_first_divergent_leaf": expected,
            "observed": neg_first,
            "ok": neg_ok,
        },
    }
