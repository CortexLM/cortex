"""The evidence renderer of the audit-cheaters suite (scripts/audit_cheaters_report.py).

The cheat x outcome table of ADR 0009 is rendered from recorded evidence, so the renderer must
not invent or drop a fact: a refusal keeps its endpoint and status, a stall is not a verdict, the
hole ids of several tests meet on one line, and a line without a cheat number is ignored.
"""

from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location(
    "audit_cheaters_report", ROOT / "scripts" / "audit_cheaters_report.py"
)
assert spec is not None and spec.loader is not None
report = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = report
spec.loader.exec_module(report)

CAUGHT = {
    "cheat": 3,
    "name": "forged-delta-valid-hash",
    "intake": {"status": "UPLOADED"},
    "audit_verdict": "MISMATCH",
    "first_bad_leaf": None,
    "excluded_reason": "UNVERIFIED",
    "cheater_admission": {"state": "ACTIVE", "strikes": 0},
    "cheater_weight": 0.0,
    "honest_weights": [250000.0, 250000.0],
    "hole": "AC-SANCTION",
}
REFUSED = {
    "cheat": 7,
    "name": "commit-lie-leaves-root",
    "intake": {
        "status": "refused",
        "http_status": 422,
        "endpoint": "POST /leaves",
        "detail": "leaf object does not match accepted commit",
    },
}
STALLED = {
    "cheat": 8,
    "name": "no-show-before-commit",
    "intake": {"status": "withdrawn", "stage": "commit"},
    "stalled_at": "audits",
    "absent_admission": {"state": "ACTIVE", "strikes": 0},
}


def row(table: str, name: str) -> list[str]:
    line = next(x for x in table.splitlines() if f"| {name} |" in x)
    return [cell.strip() for cell in line.strip("|").split("|")]


def test_a_caught_cheat_shows_the_verdict_the_sanction_and_the_honest_effect() -> None:
    cells = row(report.render([CAUGHT]), "forged-delta-valid-hash")
    assert cells[3:] == [
        "accepted",
        "MISMATCH",
        "excluded UNVERIFIED; ACTIVE, 0 strike(s); weight 0",
        "weights 250000/250000",
        "AC-SANCTION",
    ]


def test_a_refusal_keeps_its_endpoint_status_and_detail() -> None:
    cells = row(report.render([REFUSED]), "commit-lie-leaves-root")
    assert cells[3] == "refused 422 POST /leaves: leaf object does not match accepted commit"
    assert cells[4] == "-"


def test_a_stall_is_not_reported_as_a_verdict() -> None:
    cells = row(report.render([STALLED]), "no-show-before-commit")
    assert cells[3] == "never sent commit"
    assert cells[4] == "none (stalled at audits)"


def test_rows_of_one_cheat_and_stage_merge_and_their_hole_ids_meet() -> None:
    other = {**CAUGHT, "hole": "AC-COLDKEY", "intake": None, "audit_verdict": None}
    cells = row(report.render([CAUGHT, other]), "forged-delta-valid-hash")
    assert cells[3:5] == ["accepted", "MISMATCH"]
    assert cells[-1] == "AC-SANCTION, AC-COLDKEY"


def test_a_line_without_a_cheat_number_is_left_out_and_bad_json_is_named(tmp_path: Path) -> None:
    assert report.render([{"test": "honest-roster-of-five", "peak_rss_mb": 589}]).count("\n") == 1
    evidence = tmp_path / "evidence.jsonl"
    evidence.write_text(json.dumps(CAUGHT) + "\n{broken\n")
    with pytest.raises(SystemExit, match=r"evidence.jsonl:2: not JSON"):
        report.load(evidence)
