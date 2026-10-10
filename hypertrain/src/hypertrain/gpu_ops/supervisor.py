"""Persistent deadline supervisor (separate process, own session; pattern r14 supervisor.py).

Holds supervisor.lock for life, journals supervisor_ready, prints READY. Exits once
transaction_closed is journaled. At the admitted deadline it blocks new creates, rescues, then
DELETEs (forced) and requires parsed absence; 429/unknown never counts as absent.
"""

from __future__ import annotations

import fcntl
import json
import os
import select
import sys
import time
import traceback
from pathlib import Path

from hypertrain.gpu_ops.journal import Journal
from hypertrain.gpu_ops.launcher import EXIT, Orchestrator
from hypertrain.gpu_ops.provider import Provider, read_api_key


def supervise(
    run_dir: Path, cfg: dict[str, object], j: Journal, deadline: float, hard: float
) -> int:
    tick = float(cfg.get("supervisor_check_seconds", 30))  # type: ignore[arg-type]
    if cfg.get("cleanup_contract") == "network-v2":
        first = j.all("create_intent")
        if first:
            hard = min(float(r["unix"]) for r in first) + 3600
            deadline = min(deadline, hard - 420)
    parent = j.last("transaction_open")
    if cfg.get("cleanup_contract") == "network-v2":
        opens = j.all("transaction_open")
        parent = opens[0] if opens else None
    parent_fd = None
    if cfg.get("cleanup_contract") == "network-v2" and parent is not None:
        try:
            parent_fd = os.pidfd_open(int(parent["pid"]))
        except ProcessLookupError:
            j.append("supervisor_parent_exit")
            deadline = time.time()
    while time.time() < deadline:
        if j.last("transaction_closed"):
            if parent_fd is not None:
                os.close(parent_fd)
            return 0
        if cfg.get("cleanup_contract") == "network-v2":
            first = j.all("create_intent")
            if first:
                hard = min(float(r["unix"]) for r in first) + 3600
                deadline = min(deadline, hard - 420)
        timeout = min(tick, max(0.0, deadline - time.time()))
        if parent_fd is not None:
            if select.select([parent_fd], [], [], timeout)[0]:
                j.append("supervisor_parent_exit")
                break
        else:
            time.sleep(timeout)
    if parent_fd is not None:
        os.close(parent_fd)
    if j.last("transaction_closed"):
        return 0
    j.append("supervisor_deadline_reached", deadline_unix=deadline)
    opened = j.last("transaction_open")
    assert opened is not None
    p = Provider(
        str(cfg["base_url"]), read_api_key(str(cfg["key_file"])), run_dir, j, bool(opened["live"])
    )
    orch = Orchestrator(dict(cfg), j, p, run_dir, actor="supervisor")
    while True:
        if cfg.get("cleanup_contract") == "network-v2":
            starts = j.all("supervisor_cleanup_started")
            start = min(float(r["unix"]) for r in starts) if starts else time.time()
            orch.cfg["cleanup_deadline_unix"] = min(hard, start + 420)
        result = orch.cleanup(force=True)
        if result["all_absent"]:
            if cfg.get("cleanup_contract") == "network-v2" and not result.get("custody_complete"):
                j.append("supervisor_censored", cleanup=result)
                orch.close(EXIT["failed_cleaned"], "CENSORED_RESCUE")
                return 4
            orch.close(EXIT["failed_cleaned"], "supervisor_deadline_cleanup")
            return 0
        if time.time() >= hard:
            j.append("supervisor_liability_open", cleanup=result)
            return 4
        if cfg.get("cleanup_contract") == "network-v2":
            j.append("supervisor_liability_open", cleanup=result)
            return 4
        time.sleep(float(cfg.get("poll_seconds", 15)))  # type: ignore[arg-type]


def main(run_dir_arg: str) -> int:
    run_dir = Path(run_dir_arg).resolve()
    lock = open(run_dir / "supervisor.lock", "a")
    try:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        print("ALREADY_RUNNING", flush=True)
        return 5
    cfg = json.loads((run_dir / "config.json").read_text())
    j = Journal(run_dir)
    admitted = j.last("admitted")
    if not admitted:
        print("NOT_ADMITTED", flush=True)
        return 5
    deadline = float(admitted["deadline_unix"])
    hard = deadline + float(cfg.get("hard_grace_seconds", 1800))
    if cfg.get("cleanup_contract") == "network-v2":
        first = j.all("create_intent")
        if first:
            hard = min(float(r["unix"]) for r in first) + 3600
            deadline = min(deadline, hard - 420)
    j.append("supervisor_ready", supervisor_pid=os.getpid(), deadline_unix=deadline)
    print("READY", flush=True)
    logf = open(run_dir / "supervisor.log", "a")
    os.chmod(run_dir / "supervisor.log", 0o600)
    os.dup2(logf.fileno(), 1)
    os.dup2(logf.fileno(), 2)
    while True:
        try:
            return supervise(run_dir, cfg, j, deadline, hard)
        except Exception as e:  # never die silently: journal, then resume toward cleanup
            logf.write(traceback.format_exc())
            logf.flush()
            j.append("supervisor_exception", error=f"{type(e).__name__}:{e}"[:300])
            if cfg.get("cleanup_contract") == "network-v2":
                j.append("supervisor_liability_open", cleanup=None)
                return 4
            if time.time() >= hard:
                j.append("supervisor_liability_open", cleanup=None)
                return 4
            time.sleep(min(float(cfg.get("poll_seconds", 15)), max(0.0, hard - time.time())))


if __name__ == "__main__":
    sys.exit(main(sys.argv[1]))
