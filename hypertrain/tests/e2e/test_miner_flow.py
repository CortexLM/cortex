from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

from env_pin_fixtures import HOSTILE, clean_env

ROOT = Path(__file__).resolve().parents[2]


def test_miner_flow_e2e(tmp_path: Path) -> None:
    evidence = tmp_path / "evidence.json"
    # CI runs the script in a plain container: the script, not its caller, pins its own process.
    r = subprocess.run(
        [sys.executable, str(ROOT / "scripts" / "miner_flow_e2e.py")],
        env={**clean_env(), **HOSTILE, "MINER_FLOW_EVIDENCE": str(evidence)},
        capture_output=True,
        text=True,
        timeout=600,
        check=False,
    )
    assert r.returncode == 0, r.stdout[-2000:] + r.stderr[-2000:]
    body = json.loads(evidence.read_text())
    assert body["result"] == "PASS" and body["update"]["replay"] == "MATCH"
