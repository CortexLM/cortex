"""Persistent deadline supervisor (separate process, own session; pattern r14 supervisor.py).

Holds supervisor.lock for life, journals supervisor_ready, prints READY. Exits once
transaction_closed is journaled. At the admitted deadline it blocks new creates, rescues, then
DELETEs (forced) and requires parsed absence; 429/unknown never counts as absent.
"""

from __future__ import annotations

import fcntl
import json
import os
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
    while time.time() < deadline:
        if j.last("transaction_closed"):
            return 0
        time.sleep(min(tick, max(0.0, deadline - time.time())))
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
        result = orch.cleanup(force=True)
        if result["all_absent"]:
            orch.close(EXIT["failed_cleaned"], "supervisor_deadline_cleanup")
            return 0
        if time.time() >= hard:
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
            if time.time() >= hard:
                j.append("supervisor_liability_open", cleanup=None)
                return 4
            time.sleep(min(float(cfg.get("poll_seconds", 15)), max(0.0, hard - time.time())))


if __name__ == "__main__":
    sys.exit(main(sys.argv[1]))
