#!/usr/bin/env python3
"""Host RCA helper: extract ``primary_value`` / trial counts from orch ``job.out``.

A metal copy of this helper looked for ``primary_value`` on the wrong
object and printed empty ``cv=``. Orchestrator ``POST /v1/vms/{id}/jobs``
answers with adjacent-tagged ``VmJobOutput`` nested under
``RunJobResponse.output``:

    {"output": {"output": "baseline", "body": {CustomRunReport…}}}

Evaluated jobs wrap the report once more (``body.report``). This script
unwraps those layers (and ``RlmToHost::Done``) so ``cv=`` / ``n_measured``
print. It does **not** invent a score: incomplete / missing primary stays
unset and exits 2.

Copy from this tree onto a retained jail; do not treat a metal-only copy as
canonical. Pass ``--jobdir <any-dir>`` (or a path to ``job.out``); there is
no baked ``JOBDIR`` default.
"""

from __future__ import annotations

import argparse
import json
import math
import sys
from pathlib import Path
from typing import Any

VM_OUTPUT_TAGS = frozenset(
    {"baseline", "evaluated", "inspected", "rules", "archived"}
)


def _is_finite_number(value: Any) -> bool:
    return isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value)


def _as_dict(value: Any) -> dict[str, Any] | None:
    return value if isinstance(value, dict) else None


def unwrap_vm_job_output(obj: dict[str, Any]) -> dict[str, Any] | None:
    """Adjacent-tagged ``{output: "<tag>", body: …}`` → report dict."""
    tag = obj.get("output")
    body = obj.get("body")
    if not isinstance(tag, str) or tag not in VM_OUTPUT_TAGS:
        return None
    if tag == "archived":
        return None
    body_d = _as_dict(body)
    if body_d is None:
        return None
    if tag == "evaluated":
        report = _as_dict(body_d.get("report"))
        return report or body_d
    if tag == "baseline":
        return body_d
    return body_d


def extract_report(obj: Any) -> dict[str, Any] | None:
    """Best-effort report dict from orch ``job.out`` / guest ``report.json``."""
    d = _as_dict(obj)
    if d is None:
        return None
    if _is_finite_number(d.get("primary_value")) and "output" not in d:
        return d
    # RlmToHost::Done { type: "done", output: VmJobOutput }
    if isinstance(d.get("type"), str) and d["type"].lower() == "done":
        inner = _as_dict(d.get("output"))
        if inner is not None:
            got = extract_report(inner)
            if got is not None:
                return got
    # RunJobResponse { output: VmJobOutput } or nested {output: {output, body}}
    nested = _as_dict(d.get("output"))
    if nested is not None:
        tagged = unwrap_vm_job_output(nested)
        if tagged is not None:
            if _is_finite_number(tagged.get("primary_value")):
                return tagged
            deeper = extract_report(tagged)
            if deeper is not None:
                return deeper
        got = extract_report(nested)
        if got is not None:
            return got
    tagged = unwrap_vm_job_output(d)
    if tagged is not None:
        if _is_finite_number(tagged.get("primary_value")):
            return tagged
        return extract_report(tagged)
    if _is_finite_number(d.get("primary_value")):
        return d
    return None


def trial_rewards(report: dict[str, Any]) -> list[float]:
    evidence = report.get("evidence")
    if not isinstance(evidence, dict):
        return []
    trials = evidence.get("trials")
    if not isinstance(trials, list):
        return []
    out: list[float] = []
    for row in trials:
        if not isinstance(row, dict):
            continue
        value = row.get("reward")
        if _is_finite_number(value):
            out.append(float(value))
    return out


def coeff_of_variation(rewards: list[float]) -> float | None:
    if len(rewards) < 2:
        return 0.0 if rewards else None
    mean = sum(rewards) / float(len(rewards))
    if mean == 0.0:
        if all(r == 0.0 for r in rewards):
            return 0.0
        return None
    var = sum((r - mean) ** 2 for r in rewards) / float(len(rewards))
    return math.sqrt(var) / abs(mean)


def format_cv(cv: float | None) -> str:
    if cv is None:
        return ""
    return f"{cv:.6g}"


JOB_OUT_NAME = "job.out"


def load_job_out(path: Path) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError, UnicodeError) as exc:
        raise SystemExit(f"summarize_job: cannot parse {path}: {exc}") from exc


def job_out_mtime(path: Path) -> float:
    try:
        return path.stat().st_mtime
    except OSError:
        return 0.0


def job_out_unwraps(path: Path) -> bool:
    try:
        obj = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError, UnicodeError):
        return False
    report = extract_report(obj)
    return report is not None and _is_finite_number(report.get("primary_value"))


def select_job_out(found: list[Path]) -> Path:
    """Newest ``job.out`` that unwraps a finite primary; never a baked job id."""
    ranked = sorted(
        found,
        key=lambda p: (
            job_out_unwraps(p),
            job_out_mtime(p),
            -len(p.parts),
            str(p),
        ),
        reverse=True,
    )
    return ranked[0]


def find_job_out(jobdir: Path) -> Path:
    """Locate ``job.out`` under any retained jail / job directory."""
    if jobdir.is_file():
        return jobdir
    if not jobdir.is_dir():
        raise SystemExit(f"summarize_job: jobdir is not a file or directory: {jobdir}")
    found: list[Path] = []
    try:
        for path in jobdir.rglob(JOB_OUT_NAME):
            if path.is_file() and path not in found:
                found.append(path)
                if len(found) >= 64:
                    break
    except OSError as exc:
        raise SystemExit(f"summarize_job: cannot walk {jobdir}: {exc}") from exc
    if not found:
        raise SystemExit(f"summarize_job: no {JOB_OUT_NAME} under {jobdir}")
    return select_job_out(found)


def summarize(obj: Any) -> dict[str, Any]:
    report = extract_report(obj)
    if report is None or not _is_finite_number(report.get("primary_value")):
        return {
            "ok": False,
            "primary_value": None,
            "n_measured": 0,
            "cv": None,
            "reason": "no finite primary_value in nested job.out",
        }
    rewards = trial_rewards(report)
    evidence = report.get("evidence") if isinstance(report.get("evidence"), dict) else {}
    n_measured = evidence.get("n_measured") if isinstance(evidence, dict) else None
    if not isinstance(n_measured, int):
        n_measured = len(rewards)
    return {
        "ok": True,
        "primary_value": float(report["primary_value"]),
        "n_measured": n_measured,
        "cv": coeff_of_variation(rewards),
        "n_trials": len(rewards),
        "harbor_exit": evidence.get("harbor_exit") if isinstance(evidence, dict) else None,
    }


def print_run_log(summary: dict[str, Any]) -> None:
    """Metal run.log lines: ``cv=`` / ``primary_value=`` / ``n_measured=``."""
    pv = summary.get("primary_value")
    n = summary.get("n_measured")
    cv = format_cv(summary.get("cv") if isinstance(summary.get("cv"), float) else None)
    print(f"primary_value={pv if pv is not None else ''}")
    print(f"n_measured={n if n is not None else ''}")
    print(f"cv={cv}")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "job_out",
        nargs="?",
        help="path to orch job.out (JSON) or a directory that contains one. Default: stdin",
    )
    parser.add_argument(
        "--jobdir",
        type=Path,
        help="any retained jail / job directory; finds job.out inside (no baked JOBDIR)",
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="print the extracted summary as JSON",
    )
    args = parser.parse_args(argv)
    if args.job_out and args.jobdir:
        print("summarize_job: pass job.out or --jobdir, not both", file=sys.stderr)
        return 2
    if args.job_out:
        obj = load_job_out(find_job_out(Path(args.job_out)))
    elif args.jobdir:
        obj = load_job_out(find_job_out(args.jobdir))
    else:
        try:
            obj = json.load(sys.stdin)
        except json.JSONDecodeError as exc:
            print(f"summarize_job: stdin is not JSON: {exc}", file=sys.stderr)
            return 2
    summary = summarize(obj)
    if args.json:
        print(json.dumps(summary, indent=2, sort_keys=True))
    else:
        print_run_log(summary)
        if not summary["ok"]:
            print(f"summarize_job: {summary.get('reason')}", file=sys.stderr)
    return 0 if summary["ok"] else 2


if __name__ == "__main__":
    raise SystemExit(main())
