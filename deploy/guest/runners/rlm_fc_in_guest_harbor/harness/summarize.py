#!/usr/bin/env python3
"""Turn a Harbor jobs directory into Proof ``report.json`` and ``results.json``.

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

Zero scored trials → exit 2, no report. With ``--allow-tasks-dir``, every
filtered task directory must have ≥1 scored trial (``task`` or
``task__attempt``); a subset mean is not a paid score. A nonzero Harbor
exit is **not** fail-closed by itself once the filtered set is complete;
``harbor_exit`` stays in evidence.

**Agent exceptions are topic policy** (``--agent-exception-policy``, from
``constraints.params.agent_exception_policy``). A trial whose ``result.json``
carries ``exception_info`` raised **during the harness (agent) phase** —
``agent_execution.started_at`` set, the verifier never started, no
``verifier_result``, no ``verifier/reward.txt`` — is the miner's harness
failing the task (a crash, an unhandled command timeout). Harbor's own
agent timeout is different: Harbor records it and still runs the verifier,
so that trial is simply **measured** (or, if the verifier then failed,
unmeasured infrastructure). Under ``fail`` (the default) it is no measurement and the run
fails closed as before. Under ``zero`` it scores **0.0** — the task was not
solved — and the exception type plus first message line land in evidence
(``agent_exception_trials``). Every other unmeasured trial — environment
build / start failure, agent setup failure, a verifier that raised, a
trial with no ``exception_info`` — is never a score under either policy:
those are not the miner's harness failing, and inventing a 0 for them
would punish the miner for the operator's infrastructure.
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
MAX_EXCEPTION_CHARS = 400
REDACTED = "[REDACTED]"
POLICY_FAIL = "fail"
POLICY_ZERO = "zero"
EXCEPTION_POLICIES = (POLICY_FAIL, POLICY_ZERO)


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


def _timing_started(obj: dict[str, Any], phase: str) -> bool:
    timing = obj.get(phase)
    if not isinstance(timing, dict):
        return False
    started = timing.get("started_at")
    return isinstance(started, str) and bool(started.strip())


def agent_phase_exception(trial_dir: Path, obj: Any) -> dict[str, str] | None:
    """The harness-phase exception a trial died of, or ``None``.

    Strict on purpose: ``exception_info`` present, the agent phase started,
    the verifier never started, no ``verifier_result``, and no
    ``verifier/reward.txt`` on disk. Anything else (environment build /
    start failure before the agent ran, a verifier that raised, a trial
    that recorded no exception) is **not** a harness failure and stays
    unmeasured under every policy.
    """
    if not isinstance(obj, dict):
        return None
    info = obj.get("exception_info")
    if not isinstance(info, dict):
        return None
    if obj.get("verifier_result") is not None:
        return None
    if not _timing_started(obj, "agent_execution"):
        return None
    if _timing_started(obj, "verifier"):
        return None
    if (trial_dir / "verifier" / "reward.txt").is_file():
        return None
    exc_type = info.get("exception_type")
    exc_type = exc_type.strip() if isinstance(exc_type, str) and exc_type.strip() else "Exception"
    message = info.get("exception_message")
    first_line = ""
    if isinstance(message, str):
        for line in message.splitlines():
            if line.strip():
                first_line = line.strip()
                break
    return {
        "exception_type": exc_type[:MAX_EXCEPTION_CHARS],
        "exception_message": first_line[:MAX_EXCEPTION_CHARS],
    }


def collect_trials(
    jobs_dir: Path,
    allow: frozenset[str] | None = None,
    agent_exception_policy: str = POLICY_FAIL,
) -> list[dict[str, Any]]:
    """Load every scored Harbor trial. Do not cap here — the cap is evidence only.

    A trial dir is **measured** only when ``result.json`` has
    ``verifier_result.rewards.reward`` **and** ``verifier/reward.txt`` matches.
    ``reward.txt`` alone is not a measurement (miner-writable forge / host would
    refuse). Job-level snapshots are not trials.

    Under ``agent_exception_policy = "zero"`` a trial that died of a
    harness-phase exception ([`agent_phase_exception`]) is **scored** 0.0
    and carries ``outcome = "agent_exception"``; under ``fail`` it is left
    out (and the coverage check fails the run closed).

    When ``allow`` is a non-empty set, trials whose name is not a filtered
    task (or ``task__attempt``) are dropped so a script that ran the
    unfiltered pack cannot score excluded ids.
    """
    if agent_exception_policy not in EXCEPTION_POLICIES:
        _fail(f"agent_exception_policy must be one of {EXCEPTION_POLICIES}, got {agent_exception_policy!r}")
    by_dir: dict[str, dict[str, Any]] = {}
    if not jobs_dir.is_dir():
        return []

    for result_path in sorted(jobs_dir.rglob("result.json")):
        obj = _load_json(result_path)
        trial_dir = result_path.parent
        key = str(trial_dir)
        if key in by_dir:
            continue
        reward = trial_complete_reward(trial_dir, obj)
        if reward is not None:
            by_dir[key] = {
                "name": _trial_name(obj, trial_dir),
                "reward": reward,
                "outcome": "measured",
            }
            continue
        if agent_exception_policy != POLICY_ZERO:
            continue
        crashed = agent_phase_exception(trial_dir, obj)
        if crashed is None:
            continue
        by_dir[key] = {
            "name": _trial_name(obj, trial_dir),
            "reward": 0.0,
            "outcome": "agent_exception",
            **crashed,
        }

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
    agent_exception_policy: str = POLICY_FAIL,
) -> dict[str, Any]:
    primary = mean_reward(trials)
    evidence_trials = trials[:MAX_EVIDENCE_TRIALS]
    measured = [t for t in trials if t.get("outcome", "measured") == "measured"]
    crashed = [t for t in trials if t.get("outcome") == "agent_exception"]
    return {
        "primary_value": primary,
        "claim_holds": True,
        "evidence": {
            "trials": evidence_trials,
            "n_scored": len(trials),
            "n_measured": len(measured),
            "n_agent_exceptions": len(crashed),
            "agent_exception_policy": agent_exception_policy,
            "agent_exception_trials": [
                {
                    "name": t["name"],
                    "exception_type": t.get("exception_type", ""),
                    "exception_message": t.get("exception_message", ""),
                }
                for t in crashed[:MAX_EVIDENCE_TRIALS]
            ],
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


def build_results(report: dict[str, Any], trials: list[dict[str, Any]], log_tail: str) -> dict[str, Any]:
    """Complete Harbor display document. Trials are never truncated here."""
    ev = report["evidence"]
    return {
        "schema_version": 1,
        "contract": "tbench-harbor-v1",
        "topic_id": os.environ.get("PROOF_TOPIC_ID", ""),
        "custom_id": os.environ.get("PROOF_CUSTOM_ID", ""),
        "submission_digest": os.environ.get("PROOF_SUBMISSION_DIGEST", ""),
        "artifact_digest": os.environ.get("PROOF_ARTIFACT_DIGEST", ""),
        "primary_value": report["primary_value"],
        "claim_holds": report["claim_holds"],
        "n_scored": ev["n_scored"],
        "n_measured": ev["n_measured"],
        "n_agent_exceptions": ev["n_agent_exceptions"],
        "mean_reward": ev["mean_reward"],
        "agent": ev.get("agent") or "harbor",
        "agent_source": ev.get("agent_source", ""),
        "harness_kind": ev.get("harness_kind", ""),
        "agent_exception_policy": ev.get("agent_exception_policy", POLICY_FAIL),
        "harbor_exit": ev.get("harbor_exit", 0),
        "trials": trials,
        "agent_exception_trials": ev.get("agent_exception_trials", []),
        "logs": {
            "harbor_run_log": "logs/harbor.run.log",
            "harbor_run_tail": log_tail,
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
        "every dir must have ≥1 scored trial",
    )
    parser.add_argument(
        "--agent-exception-policy",
        default=os.environ.get("PROOF_PARAM_AGENT_EXCEPTION_POLICY", POLICY_FAIL).strip().lower()
        or POLICY_FAIL,
        help="fail (default): a harness-phase exception is no measurement; "
        "zero: it scores 0.0 with the exception in evidence",
    )
    args = parser.parse_args(argv)
    policy = args.agent_exception_policy.strip().lower()
    if policy not in EXCEPTION_POLICIES:
        _fail(f"agent_exception_policy must be one of {EXCEPTION_POLICIES}, got {policy!r}")

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
    trials = collect_trials(jobs_dir, allow, policy)
    if not trials:
        _fail(
            f"no measured Harbor trials under {jobs_dir} "
            "(need matching verifier_result.rewards.reward and verifier/reward.txt"
            + ("; agent_exception_policy=zero found no harness-phase exception either" if policy == POLICY_ZERO else "")
            + "); refusing to invent a primary_value"
        )
    if allow:
        missing = missing_filtered_tasks(trials, allow)
        if missing:
            _fail(
                f"incomplete vs filtered task set (missing measured trials for: "
                f"{', '.join(missing)}; agent_exception_policy={policy}); "
                "refusing a partial primary_value"
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
        policy,
    )
    out = Path(args.output)
    out.parent.mkdir(parents=True, exist_ok=True)
    dumped = json.dumps(report, indent=2, sort_keys=True)
    dumped = redact(dumped, secrets)
    out.write_text(dumped + "\n", encoding="utf-8")
    results_name = os.environ.get("PROOF_PARAM_RESULTS_PATH", "").strip() or "results.json"
    results_path = out.parent / results_name
    results = build_results(report, trials, read_tail(log_path, secrets))
    results_dumped = redact(json.dumps(results, indent=2, sort_keys=True), secrets)
    results_path.write_text(results_dumped + "\n", encoding="utf-8")
    ev = report["evidence"]
    print(
        f"summarize: n_scored={ev['n_scored']} n_measured={ev['n_measured']} "
        f"n_agent_exceptions={ev['n_agent_exceptions']} (policy={policy}) "
        f"primary_value={report['primary_value']} harbor_exit={args.harbor_exit}",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
