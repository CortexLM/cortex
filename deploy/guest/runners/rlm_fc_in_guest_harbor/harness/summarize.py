#!/usr/bin/env python3
"""Turn a Harbor jobs directory into Proof ``report.json``.

``primary_value`` is the mean of **complete** Harbor trials. A trial is
measured only when Harbor left both:

- ``result.json`` with a finite ``verifier_result.rewards.reward``, and
- ``verifier/reward.txt`` whose parsed float matches that JSON reward.

The paid value is always the JSON verifier reward — never a miner-writable
``reward.txt`` alone, and never another field (score, accuracy, job-level
aggregates). A mismatched or missing pair is **no measurement**.

A Harbor **job** snapshot (``finished_at`` / ``n_running`` / ``n_completed``,
no ``trial_name`` / ``verifier_result``) that is still running or
``finished_at``-null is fail-closed, matching host harvest.

Zero measured trials → exit 2, no report. With ``--allow-tasks-dir``, every
filtered task directory must have ≥1 complete trial (``task`` or
``task__attempt``); a subset mean is not a paid score. A nonzero Harbor
exit is **not** fail-closed by itself once the filtered set is complete;
``harbor_exit`` stays in evidence.
"""

from __future__ import annotations

import argparse
import json
import math
import os
import sys
from pathlib import Path
from typing import Any

MAX_TAIL_CHARS = 8 * 1024
MAX_EVIDENCE_TRIALS = 256
MAX_REWARD_TXT_BYTES = 64 * 1024
REDACTED = "[REDACTED]"


def _fail(msg: str, code: int = 2) -> None:
    print(msg, file=sys.stderr)
    raise SystemExit(code)


def _is_finite_number(value: Any) -> bool:
    return isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value)


def trial_reward(obj: Any) -> float | None:
    """Return the Harbor verifier reward, or None if this JSON is not a measured trial."""
    if not isinstance(obj, dict):
        return None
    verifier = obj.get("verifier_result")
    if not isinstance(verifier, dict):
        return None
    rewards = verifier.get("rewards")
    if not isinstance(rewards, dict) or "reward" not in rewards:
        return None
    value = rewards["reward"]
    if _is_finite_number(value):
        return float(value)
    return None


def reward_from_txt(path: Path) -> float | None:
    """Parse ``verifier/reward.txt`` (a single finite number). Never invent."""
    try:
        if not path.is_file() or path.stat().st_size > MAX_REWARD_TXT_BYTES:
            return None
        raw = path.read_bytes().strip()
    except OSError:
        return None
    if not raw:
        return None
    try:
        text = raw.decode("utf-8")
    except UnicodeDecodeError:
        return None
    line = ""
    for candidate in text.splitlines():
        line = candidate.strip()
        if line:
            break
    if not line:
        return None
    try:
        value = float(line)
    except ValueError:
        return None
    if math.isfinite(value):
        return float(value)
    return None


def rewards_agree(json_reward: float, txt_reward: float) -> bool:
    """Harbor JSON and verifier/reward.txt must be the same measured value."""
    return math.isclose(json_reward, txt_reward, rel_tol=0.0, abs_tol=1e-9)


def _load_json(path: Path) -> Any | None:
    try:
        if path.stat().st_size > 8 * 1024 * 1024:
            return None
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError, UnicodeError):
        return None


def _trial_name(obj: Any, trial_dir: Path) -> str:
    if isinstance(obj, dict):
        name = obj.get("trial_name")
        if isinstance(name, str) and name:
            return name
    return trial_dir.name


def is_harbor_job_snapshot(obj: Any) -> bool:
    """Job-level Harbor result.json (not a per-trial verifier payload)."""
    if not isinstance(obj, dict):
        return False
    if obj.get("trial_name") is not None or obj.get("verifier_result") is not None:
        return False
    if any(k in obj for k in ("n_running", "finished_at", "n_completed")):
        return True
    stats = obj.get("stats")
    if isinstance(stats, dict) and any(
        k in stats for k in ("n_running_trials", "n_running", "n_completed")
    ):
        return True
    return False


def _snapshot_n_running(obj: dict[str, Any]) -> int | None:
    n = obj.get("n_running")
    if isinstance(n, int) and not isinstance(n, bool) and n >= 0:
        return n
    stats = obj.get("stats")
    if not isinstance(stats, dict):
        return None
    for key in ("n_running_trials", "n_running"):
        n = stats.get(key)
        if isinstance(n, int) and not isinstance(n, bool) and n >= 0:
            return n
    return None


def _snapshot_finished_at_null(obj: dict[str, Any]) -> bool:
    if "finished_at" not in obj:
        return is_harbor_job_snapshot(obj)
    value = obj.get("finished_at")
    if value is None:
        return True
    if isinstance(value, str) and not value:
        return True
    return False


def job_snapshot_stale(obj: Any) -> bool:
    """Host harvest refuse: still running or finished_at null/empty."""
    if not is_harbor_job_snapshot(obj):
        return False
    n_running = _snapshot_n_running(obj)
    return (n_running is not None and n_running > 0) or _snapshot_finished_at_null(obj)


def refuse_stale_harbor_snapshots(jobs_dir: Path) -> None:
    """Fail-closed on an unfinished Harbor job snapshot (host harvest contract)."""
    if not jobs_dir.is_dir():
        return
    for result_path in sorted(jobs_dir.rglob("result.json")):
        obj = _load_json(result_path)
        if obj is None or not job_snapshot_stale(obj):
            continue
        n_running = _snapshot_n_running(obj) if isinstance(obj, dict) else None
        _fail(
            f"harbor job snapshot incomplete ({result_path} n_running={n_running or 0}, "
            "finished_at null); refusing to publish a stale snapshot"
        )


def allowed_task_names(tasks_dir: Path) -> frozenset[str]:
    """Directory names on the filtered task copy (the only scorable ids)."""
    if not tasks_dir.is_dir():
        return frozenset()
    return frozenset(p.name for p in tasks_dir.iterdir() if p.is_dir())


def trial_matches_allow(name: str, allow: frozenset[str]) -> bool:
    """Harbor trial ids are ``<task>`` or ``<task>__<attempt>``."""
    if not allow:
        return True
    return trial_task_id(name, allow) is not None


def trial_task_id(name: str, allow: frozenset[str]) -> str | None:
    """Map a trial name onto a filtered task directory, or None if excluded."""
    if name in allow:
        return name
    sep = name.rfind("__")
    if sep > 0 and name[:sep] in allow:
        return name[:sep]
    return None


def missing_filtered_tasks(
    trials: list[dict[str, Any]], allow: frozenset[str]
) -> list[str]:
    """Filtered task dirs with no complete measured trial."""
    covered: set[str] = set()
    for trial in trials:
        tid = trial_task_id(str(trial["name"]), allow)
        if tid is not None:
            covered.add(tid)
    return sorted(allow - covered)


def trial_complete_reward(trial_dir: Path, obj: Any) -> float | None:
    """Harbor JSON reward only when matching verifier/reward.txt is present."""
    json_reward = trial_reward(obj)
    if json_reward is None:
        return None
    txt_reward = reward_from_txt(trial_dir / "verifier" / "reward.txt")
    if txt_reward is None or not rewards_agree(json_reward, txt_reward):
        return None
    return json_reward


def collect_trials(
    jobs_dir: Path, allow: frozenset[str] | None = None
) -> list[dict[str, Any]]:
    """Load every complete Harbor trial. Do not cap here — the cap is evidence only.

    A trial dir counts only when ``result.json`` has
    ``verifier_result.rewards.reward`` **and** ``verifier/reward.txt`` matches.
    ``reward.txt`` alone is not a measurement (miner-writable forge / host would
    refuse). Job-level snapshots are not trials.

    When ``allow`` is a non-empty set, trials whose name is not a filtered
    task (or ``task__attempt``) are dropped so a script that ran the
    unfiltered pack cannot score excluded ids.
    """
    by_dir: dict[str, dict[str, Any]] = {}
    if not jobs_dir.is_dir():
        return []

    for result_path in sorted(jobs_dir.rglob("result.json")):
        obj = _load_json(result_path)
        trial_dir = result_path.parent
        reward = trial_complete_reward(trial_dir, obj)
        if reward is None:
            continue
        key = str(trial_dir)
        if key in by_dir:
            continue
        by_dir[key] = {"name": _trial_name(obj, trial_dir), "reward": reward}

    rows = [by_dir[k] for k in sorted(by_dir)]
    if not allow:
        return rows
    return [t for t in rows if trial_matches_allow(str(t["name"]), allow)]


def load_redact_values() -> list[str]:
    values: list[str] = []

    def add_file(path: Path) -> None:
        try:
            raw = path.read_bytes().strip()
        except OSError:
            return
        if not raw:
            return
        try:
            text = raw.decode("utf-8").replace("\n", "").replace("\r", "")
        except UnicodeDecodeError:
            return
        if text:
            values.append(text)

    secrets_dir = os.environ.get("PROOF_SECRETS_DIR", "")
    secret_files = os.environ.get("PROOF_SECRET_FILES", "")
    if secrets_dir:
        root = Path(secrets_dir)
        if secret_files:
            for name in secret_files.split(","):
                name = name.strip()
                if name and ".." not in name and "/" not in name:
                    add_file(root / name)
        elif root.is_dir():
            for child in root.iterdir():
                if child.is_file():
                    add_file(child)

    miner_dir = os.environ.get("PROOF_MINER_ENV_DIR", "")
    miner_names = os.environ.get("PROOF_MINER_ENV_NAMES", "")
    if miner_dir:
        root = Path(miner_dir)
        names = [n.strip() for n in miner_names.split(",") if n.strip()] if miner_names else []
        if not names and root.is_dir():
            names = [p.name for p in root.iterdir() if p.is_file()]
        for name in names:
            if ".." in name or "/" in name:
                continue
            add_file(root / name)
            env_val = os.environ.get(name, "")
            if env_val:
                values.append(env_val.replace("\n", "").replace("\r", ""))

    for env_name in (
        os.environ.get("PROOF_PARAM_MINER_BYOK", ""),
        os.environ.get("PROOF_PARAM_INFERENCE_KEY_ENV", ""),
    ):
        if env_name:
            env_val = os.environ.get(env_name, "")
            if env_val:
                values.append(env_val.replace("\n", "").replace("\r", ""))

    # Longest first so a key that is a prefix of another still redacts fully.
    uniq = sorted({v for v in values if len(v) >= 4}, key=len, reverse=True)
    return uniq


def redact(text: str, secrets: list[str]) -> str:
    out = text
    for secret in secrets:
        if secret:
            out = out.replace(secret, REDACTED)
    return out


def read_tail(path: Path | None, secrets: list[str]) -> str:
    if path is None or not path.is_file():
        return ""
    try:
        data = path.read_bytes()
    except OSError:
        return ""
    if len(data) > MAX_TAIL_CHARS:
        data = data[-MAX_TAIL_CHARS:]
    text = data.decode("utf-8", errors="replace")
    return redact(text, secrets)


def mean_reward(trials: list[dict[str, Any]]) -> float:
    rewards = [float(t["reward"]) for t in trials]
    return sum(rewards) / float(len(rewards))


def build_report(
    trials: list[dict[str, Any]],
    log_tail: str,
    harbor_exit: int,
    agent: str,
    agent_source: str,
    harness_kind: str = "",
) -> dict[str, Any]:
    primary = mean_reward(trials)
    evidence_trials = trials[:MAX_EVIDENCE_TRIALS]
    return {
        "primary_value": primary,
        "claim_holds": True,
        "evidence": {
            "trials": evidence_trials,
            "n_measured": len(trials),
            "n_evidence_trials": len(evidence_trials),
            "evidence_truncated": len(trials) > MAX_EVIDENCE_TRIALS,
            "mean_reward": primary,
            "harbor_exit": harbor_exit,
            "harbor_incomplete": harbor_exit != 0,
            "harbor_run_tail": log_tail,
            "agent": redact(agent, load_redact_values()),
            "agent_source": agent_source,
            "harness_kind": harness_kind,
        },
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--jobs-dir", required=True)
    parser.add_argument("--log", default="")
    parser.add_argument("--output", required=True)
    parser.add_argument("--harbor-exit", type=int, default=0)
    parser.add_argument("--agent", default="")
    parser.add_argument("--agent-source", default="")
    parser.add_argument("--harness-kind", default="")
    parser.add_argument(
        "--allow-tasks-dir",
        default="",
        help="filtered task copy; trials not named after these dirs are dropped; "
        "every dir must have ≥1 complete trial",
    )
    args = parser.parse_args(argv)

    jobs_dir = Path(args.jobs_dir)
    refuse_stale_harbor_snapshots(jobs_dir)
    allow: frozenset[str] | None = None
    if args.allow_tasks_dir:
        allow = allowed_task_names(Path(args.allow_tasks_dir))
        if not allow:
            _fail(
                f"allow-tasks-dir {args.allow_tasks_dir} has no task directories; "
                "refusing to invent a primary_value"
            )
    trials = collect_trials(jobs_dir, allow)
    if not trials:
        _fail(
            f"no measured Harbor trials under {jobs_dir} "
            "(need matching verifier_result.rewards.reward and verifier/reward.txt); "
            "refusing to invent a primary_value"
        )
    if allow:
        missing = missing_filtered_tasks(trials, allow)
        if missing:
            _fail(
                f"incomplete vs filtered task set (missing measured trials for: "
                f"{', '.join(missing)}); refusing a partial primary_value"
            )
    secrets = load_redact_values()
    log_path = Path(args.log) if args.log else None
    report = build_report(
        trials,
        read_tail(log_path, secrets),
        args.harbor_exit,
        args.agent,
        args.agent_source,
        args.harness_kind,
    )
    out = Path(args.output)
    out.parent.mkdir(parents=True, exist_ok=True)
    dumped = json.dumps(report, indent=2, sort_keys=True)
    dumped = redact(dumped, secrets)
    out.write_text(dumped + "\n", encoding="utf-8")
    n = len(trials)
    print(
        f"summarize: n_measured={n} primary_value={report['primary_value']} "
        f"harbor_exit={args.harbor_exit}",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
