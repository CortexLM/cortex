#!/usr/bin/env python3
"""Turn a Harbor jobs directory into Proof ``report.json``.

``primary_value`` is the mean of **every** trial that has a measured
reward: a finite ``verifier_result.rewards.reward`` in ``result.json``,
or a finite number in ``verifier/reward.txt`` when JSON is missing
(timeout / kill can leave ``finished_at=null`` with rewards already on
disk). A trial with neither is **no measurement** — never another
field's value (score, accuracy, job-level aggregates) as a substitute.

Zero measured trials → exit 2, no report (do not invent a primary_value).
A nonzero Harbor exit is **not** fail-closed by itself: already-measured
trials still score; ``harbor_exit`` stays in evidence.
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


def allowed_task_names(tasks_dir: Path) -> frozenset[str]:
    """Directory names on the filtered task copy (the only scorable ids)."""
    if not tasks_dir.is_dir():
        return frozenset()
    return frozenset(p.name for p in tasks_dir.iterdir() if p.is_dir())


def trial_matches_allow(name: str, allow: frozenset[str]) -> bool:
    """Harbor trial ids are ``<task>`` or ``<task>__<attempt>``."""
    if not allow:
        return True
    if name in allow:
        return True
    sep = name.rfind("__")
    if sep > 0 and name[:sep] in allow:
        return True
    return False


def collect_trials(
    jobs_dir: Path, allow: frozenset[str] | None = None
) -> list[dict[str, Any]]:
    """Load every measured trial. Do not cap here — the cap is evidence only.

    Prefer ``result.json`` ``verifier_result.rewards.reward``. If that is
    absent (incomplete Harbor: timeout/kill, ``finished_at=null``), use
    ``verifier/reward.txt`` in the same trial directory. One row per trial
    dir — never double-count JSON + txt.

    When ``allow`` is a non-empty set, trials whose name is not a filtered
    task (or ``task__attempt``) are dropped so a script that ran the
    unfiltered pack cannot score excluded ids.
    """
    by_dir: dict[str, dict[str, Any]] = {}
    if not jobs_dir.is_dir():
        return []

    def add(trial_dir: Path, reward: float, name: str) -> None:
        key = str(trial_dir)
        if key in by_dir:
            return
        by_dir[key] = {"name": name, "reward": reward}

    for result_path in sorted(jobs_dir.rglob("result.json")):
        obj = _load_json(result_path)
        reward = trial_reward(obj)
        if reward is None:
            continue
        trial_dir = result_path.parent
        add(trial_dir, reward, _trial_name(obj, trial_dir))

    for reward_path in sorted(jobs_dir.rglob("reward.txt")):
        if reward_path.parent.name != "verifier":
            continue
        trial_dir = reward_path.parent.parent
        if str(trial_dir) in by_dir:
            continue
        reward = reward_from_txt(reward_path)
        if reward is None:
            continue
        add(trial_dir, reward, trial_dir.name)

    rows = [by_dir[k] for k in sorted(by_dir)]
    if not allow:
        return rows
    return [
        t
        for t in rows
        if trial_matches_allow(str(t["name"]), allow)
    ]


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
        help="filtered task copy; trials not named after these dirs are dropped",
    )
    args = parser.parse_args(argv)

    jobs_dir = Path(args.jobs_dir)
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
            "(need verifier_result.rewards.reward or verifier/reward.txt); "
            "refusing to invent a primary_value"
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
