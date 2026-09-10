#!/usr/bin/env python3
"""Turn a Harbor jobs directory into Proof ``report.json``.

``primary_value`` is the mean of trial ``verifier_result.rewards.reward``
values that are finite numbers. A trial with no such field is **no
measurement** — never another field's value (score, accuracy, job-level
aggregates) as a substitute. Zero measured trials → exit 2, no report.

Evidence includes ``trials`` and a redacted ``harbor_run_tail``. Secret
values from the owner secrets dir and miner BYOK dir are blanked.
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


def _load_json(path: Path) -> Any | None:
    try:
        if path.stat().st_size > 8 * 1024 * 1024:
            return None
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError, UnicodeError):
        return None


def collect_trials(jobs_dir: Path) -> list[dict[str, Any]]:
    trials: list[dict[str, Any]] = []
    if not jobs_dir.is_dir():
        return trials
    for result_path in sorted(jobs_dir.rglob("result.json")):
        if len(trials) >= MAX_EVIDENCE_TRIALS:
            break
        obj = _load_json(result_path)
        reward = trial_reward(obj)
        if reward is None:
            continue
        name = obj.get("trial_name") if isinstance(obj, dict) else None
        if not isinstance(name, str) or not name:
            name = result_path.parent.name
        trials.append({"name": name, "reward": reward})
    return trials


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
) -> dict[str, Any]:
    primary = mean_reward(trials)
    return {
        "primary_value": primary,
        "claim_holds": True,
        "evidence": {
            "trials": trials,
            "n_measured": len(trials),
            "mean_reward": primary,
            "harbor_exit": harbor_exit,
            "harbor_run_tail": log_tail,
            "agent": redact(agent, load_redact_values()),
            "agent_source": agent_source,
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
    args = parser.parse_args(argv)

    jobs_dir = Path(args.jobs_dir)
    trials = collect_trials(jobs_dir)
    if not trials:
        _fail(
            f"no measured Harbor trials under {jobs_dir} "
            "(need verifier_result.rewards.reward); refusing to invent a primary_value"
        )
    secrets = load_redact_values()
    log_path = Path(args.log) if args.log else None
    report = build_report(
        trials,
        read_tail(log_path, secrets),
        args.harbor_exit,
        args.agent,
        args.agent_source,
    )
    out = Path(args.output)
    out.parent.mkdir(parents=True, exist_ok=True)
    dumped = json.dumps(report, indent=2, sort_keys=True)
    dumped = redact(dumped, secrets)
    out.write_text(dumped + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
