"""CPU mining smoke: real challenge app + one miner + one auditor, one complete test-mode round.

Unlike `miner_flow_e2e.py` (in-process master, no auditor), this starts the challenge FastAPI app
as its own process, drives a full round through the e2e harness (train -> aggregate -> audit ->
finalize) with the tiny 2-layer CPU model and H/J reduced, and fails unless the round reaches
FINAL with a bitwise MATCH verdict.

Writes evidence JSON to $MINE_SMOKE_EVIDENCE (default: a fresh private temp file, path printed);
exit 0 = PASS, exit 1 = the round did not complete.
"""

from __future__ import annotations

import json
import os
import sys
import tempfile
import time
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tests" / "e2e"))
sys.path.insert(0, str(ROOT / "tests" / "trainer"))

import common  # type: ignore[import-not-found]  # noqa: E402,F401  (determinism before torch)
from common import manifest  # noqa: E402
from harness import World, key  # type: ignore[import-not-found]  # noqa: E402

H, J = 4, 2
MINER_INDEX = 0x41


def evidence_file() -> tuple[Path, int]:
    """$MINE_SMOKE_EVIDENCE (never via a symlink), else a fresh 0600 file, never a fixed name."""
    env = os.environ.get("MINE_SMOKE_EVIDENCE")
    if env:
        # No O_TRUNC: a failed start must not empty an existing file; main() truncates at write.
        fd = os.open(env, os.O_WRONLY | os.O_CREAT | os.O_NOFOLLOW, 0o600)
        os.fchmod(fd, 0o600)  # O_CREAT keeps the mode of a file that already exists
        return Path(env), fd
    fd, name = tempfile.mkstemp(prefix="mine-smoke-evidence-", suffix=".json")
    return Path(name), fd


def main() -> int:
    ev: dict[str, Any] = {"checks": {}}
    out, fd = evidence_file()
    started = time.monotonic()
    code = 0
    with tempfile.TemporaryDirectory() as d:
        w = None
        try:
            m = manifest(policy="carry", H=H, J=J, q=1.0, E=1, k_segments=2)
            k = key(MINER_INDEX)
            w = World(Path(d), m, {k: {"probation": True}})
            miner = w.miner(k)
            uploaded = w.run_all([miner], 0)
            ev["checks"]["miner_uploaded"] = uploaded == ["UPLOADED"]
            assert ev["checks"]["miner_uploaded"], uploaded
            w.aggregate(0)
            audits = w.audit(0, [miner])
            ev["audits"] = audits
            r = w.finalize(0)
            ev["finalize"] = {"status": r["status"], "json": r.get("json")}
            assert r["status"] == 200, r
            verdict = r["json"]["verdicts"][k.ss58]
            ev["checks"]["round_finalized"] = True
            ev["checks"]["verdict_match"] = verdict == "MATCH"
            assert verdict == "MATCH", f"verdict {verdict}"
            ev["round_state"] = w.view(0)["state"]
            ev["checks"]["state_final"] = ev["round_state"] == "FINAL"
            assert ev["checks"]["state_final"], ev["round_state"]
        except Exception as e:  # noqa: BLE001 - the evidence must record every failure
            ev["failed"] = f"{type(e).__name__}: {e}"
            code = 1
        finally:
            if w is not None:
                try:
                    ev["process_exit_codes"] = w.stop()
                except Exception as e:  # noqa: BLE001 - a failed cleanup fails the run, not the write
                    ev["cleanup_failed"] = f"{type(e).__name__}: {e}"
                    code = 1
    ev["elapsed_seconds"] = round(time.monotonic() - started, 2)
    ev["result"] = "PASS" if code == 0 else "FAIL"
    os.ftruncate(fd, 0)
    with os.fdopen(fd, "w") as f:
        f.write(json.dumps(ev, indent=1, sort_keys=True, default=str))
    print(
        json.dumps(
            {
                "result": ev["result"],
                "elapsed_seconds": ev["elapsed_seconds"],
                "checks": ev["checks"],
                "evidence": str(out),
                "failed": ev.get("failed"),
            },
            sort_keys=True,
        )
    )
    return code


if __name__ == "__main__":
    sys.exit(main())
