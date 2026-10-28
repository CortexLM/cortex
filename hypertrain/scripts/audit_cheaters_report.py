"""Render the evidence of the audit-cheaters suite as the cheat x outcome table (ADR 0009).

The service-level tests of tests/audit_cheaters append one JSON object per scenario to the file
named by $AUDIT_CHEATERS_EVIDENCE (see ac_world.record). This script reads that file and prints a
markdown table: per cheat double and per stage, where the first refusal happened, the audit
verdict, what the ledger did to the cheater and to the honest miners, and the id of the hole a
strict xfail reproduces. Run from hypertrain/:

    AUDIT_CHEATERS_FULL=1 AUDIT_CHEATERS_EVIDENCE=evidence.jsonl \\
        AUDIT_CHEATERS_SNAPSHOT_DIR=<dir> uv run --frozen pytest -q tests/audit_cheaters
    uv run --frozen python scripts/audit_cheaters_report.py evidence.jsonl

It reads recorded facts only; it never drives a miner.
"""

from __future__ import annotations

import json
import sys
from collections.abc import Iterable, Mapping, Sequence
from pathlib import Path
from typing import Any

HEADER = (
    "| cheat | double | stage | first refusal | audit | cheater after the round | honest miners "
    "| hole |\n|---|---|---|---|---|---|---|---|"
)


def load(path: Path) -> list[dict[str, Any]]:
    rows = []
    for number, line in enumerate(path.read_text().splitlines(), 1):
        if not line.strip():
            continue
        try:
            row = json.loads(line)
        except json.JSONDecodeError as error:
            raise SystemExit(f"{path}:{number}: not JSON ({error.msg})") from None
        if isinstance(row, dict):
            rows.append(row)
    return rows


def first(row: Mapping[str, Any], *keys: str) -> Any:
    return next((row[k] for k in keys if row.get(k) is not None), None)


def intake(row: Mapping[str, Any]) -> str:
    """The first refusal, or the fact that the master accepted what the cheater sent."""
    value = first(row, "intake")
    steps = value if isinstance(value, list) else [value]
    last = next((s for s in reversed(steps) if isinstance(s, Mapping)), None)
    if last is None:
        return "-"
    status = last.get("status")
    if status == "UPLOADED" or status == 200:
        return "accepted"
    if status == "withdrawn":
        return f"never sent {last.get('stage')}"
    if status == "refused":
        where = last.get("endpoint") or "?"
        return f"refused {last.get('http_status')} {where}: {last.get('detail')}"
    if status == "error":
        return f"unhandled: {last.get('error')}"
    http = last.get("http_status") or last.get("status")
    return f"{http} {last.get('endpoint') or ''} {last.get('detail') or ''}".strip()


def audit(row: Mapping[str, Any]) -> str:
    verdict = first(row, "audit_verdict", "verdict")
    pair = row.get("audit")
    if verdict is None and isinstance(pair, list) and pair:
        verdict = pair[0]
    leaf = first(row, "first_bad_leaf")
    if leaf is None and isinstance(pair, list) and len(pair) > 1:
        leaf = pair[1]
    if verdict is None:
        stalled = first(row, "stalled_at")
        return f"none (stalled at {stalled})" if stalled else "-"
    return f"{verdict}" + ("" if leaf is None else f" at leaf {leaf}")


def standing(row: Mapping[str, Any]) -> str:
    admission = first(row, "cheater_admission", "absent_admission", "admission")
    parts = []
    reason = first(row, "excluded_reason")
    if isinstance(row.get("excluded"), Mapping):
        reason = reason or row["excluded"].get("reason")
    if reason:
        parts.append(f"excluded {reason}")
    if isinstance(admission, Mapping):
        parts.append(f"{admission.get('state')}, {admission.get('strikes', 0)} strike(s)")
        if admission.get("pending_dispute"):
            parts.append("pending dispute")
    weight = first(row, "cheater_weight", "absent_weight")
    if weight is not None:
        parts.append(f"weight {number(weight)}")
    return "; ".join(parts) or "-"


def number(value: Any) -> str:
    return f"{value:g}" if isinstance(value, int | float) else json.dumps(value, sort_keys=True)


def honest(row: Mapping[str, Any]) -> str:
    weights = first(row, "honest_weights")
    effect = first(row, "honest_effect", "honest_unaffected", "honest_unsanctioned")
    parts = []
    if isinstance(weights, list) and weights:
        parts.append("weights " + "/".join(number(w) for w in weights))
    elif isinstance(weights, Mapping) and weights:
        parts.append("weights " + "/".join(number(w) for w in weights.values()))
    if effect is True:
        parts.append("unsanctioned")
    elif isinstance(effect, Mapping):
        strikes = effect.get("strikes") or [0]
        parts.append(f"strikes {max(strikes)}")
    elif isinstance(effect, str):
        parts.append(effect)
    return "; ".join(parts) or "-"


def stage(row: Mapping[str, Any]) -> str:
    named = first(row, "stage")
    if named:
        return str(named)
    if row.get("coldkey_effect") is not None or first(row, "trial_outcome") is not None:
        return "probation"
    return "round"


def cells(row: Mapping[str, Any]) -> dict[str, str]:
    return {
        "intake": intake(row),
        "audit": audit(row),
        "standing": standing(row),
        "honest": honest(row),
        "hole": str(first(row, "hole") or ""),
    }


def merge(rows: Iterable[Mapping[str, Any]]) -> dict[str, str]:
    """Several tests record the same cheat at the same stage: keep, per column, the first
    informative cell (a '-' says nothing) and the union of the hole ids."""
    merged: dict[str, str] = {"intake": "-", "audit": "-", "standing": "-", "honest": "-"}
    holes: list[str] = []
    for row in rows:
        for column, cell in cells(row).items():
            if column == "hole":
                if cell and cell not in holes:
                    holes.append(cell)
            elif merged[column] == "-" and cell != "-":
                merged[column] = cell
    merged["hole"] = ", ".join(holes)
    return merged


def render(rows: Iterable[Mapping[str, Any]]) -> str:
    """One line per (cheat, double, stage), ordered by cheat number."""
    groups: dict[tuple[int, str, str], list[Mapping[str, Any]]] = {}
    for r in rows:
        name = first(r, "name", "double")
        if r.get("cheat") is None or not name:
            continue
        groups.setdefault((int(r["cheat"]), str(name), stage(r)), []).append(r)
    lines = [HEADER]
    for (cheat, name, where), members in sorted(groups.items()):
        c = merge(members)
        lines.append(
            f"| {cheat} | {name} | {where} | {c['intake']} | {c['audit']} | {c['standing']} "
            f"| {c['honest']} | {c['hole']} |"
        )
    return "\n".join(lines)


def main(argv: Sequence[str] | None = None) -> int:
    args = list(sys.argv[1:] if argv is None else argv)
    if len(args) != 1:
        print("usage: audit_cheaters_report.py EVIDENCE.jsonl", file=sys.stderr)
        return 2
    print(render(load(Path(args[0]))))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
