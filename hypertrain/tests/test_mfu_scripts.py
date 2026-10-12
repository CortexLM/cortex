"""run_mfu.py CPU end-to-end and check_gpu_evidence.py --kind mfu recompute/tamper checks."""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

import pytest

TREE = Path(__file__).resolve().parents[1]


def _check(path: Path) -> subprocess.CompletedProcess[str]:
    cmd = [sys.executable, str(TREE / "scripts/check_gpu_evidence.py"), "--kind", "mfu", str(path)]
    return subprocess.run(cmd, capture_output=True, text=True, check=False)  # noqa: S603


@pytest.fixture(scope="module")
def evidence(tmp_path_factory: pytest.TempPathFactory) -> dict:
    out = tmp_path_factory.mktemp("mfu") / "tiny.json"
    cmd = [
        sys.executable,
        str(TREE / "scripts/run_mfu.py"),
        "--profile",
        "tiny-cpu",
        "--device",
        "cpu",
        "--warmup",
        "1",
        "--measured-peak",
        "1e12",
        "--out",
        str(out),
    ]
    r = subprocess.run(cmd, capture_output=True, text=True, check=False, cwd=TREE)  # noqa: S603
    assert r.returncode == 0, r.stderr
    return json.loads(out.read_text())


def test_cpu_run_is_schema_valid_and_replays(evidence: dict, tmp_path: Path) -> None:
    assert evidence["replay_match"] and len(evidence["step_seconds"]) == 4
    p = tmp_path / "ok.json"
    p.write_text(json.dumps(evidence))
    assert _check(p).returncode == 0


@pytest.mark.parametrize("fault", ["mfu", "replay", "measured"])
def test_checker_rejects_when_tampered(evidence: dict, tmp_path: Path, fault: str) -> None:
    ev = json.loads(json.dumps(evidence))
    if fault == "mfu":
        ev["vendor"]["mfu"] += 0.002
    if fault == "replay":
        ev["replay_match"] = False
    if fault == "measured":
        ev["measured"]["mfu"] += 0.002
    p = tmp_path / "bad.json"
    p.write_text(json.dumps(ev))
    r = _check(p)
    assert r.returncode == 1 and "INVALID" in r.stdout
