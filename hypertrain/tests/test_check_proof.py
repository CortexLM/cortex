"""check-proof.sh: a golden replay xfail fails the gate unless the diagnostic variable is set;
an unexpected skip still fails it."""

import os
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# Keeps one test per suite (speed) and turns every test under XFAIL_PREFIX into an xfail with
# reason XFAIL_WHY (default: the _expect_golden reason, as a leaf root outside GOLDEN gives);
# FORCE_SKIP also skips the codec v2 test.
PLUGIN = """
import os

import pytest

GOLDEN_WHY = %r

def pytest_collection_modifyitems(config, items):
    config.hook.pytest_deselected(items=items[1:])
    del items[1:]

def pytest_runtest_setup(item):
    prefix = os.environ.get("XFAIL_PREFIX", "tests/trainer/test_golden_replay.py")
    if item.nodeid.startswith(prefix):
        pytest.xfail(os.environ.get("XFAIL_WHY", GOLDEN_WHY))
    if os.environ.get("FORCE_SKIP") and item.nodeid.startswith("tests/trainer/test_codec_v2.py"):
        pytest.skip("forced skip")
"""
# Fixed marker the gate greps for: the start of test_golden_replay.XFAIL_REASON.
GOLDEN_WHY = "golden leaf root differs on this CPU class; forced"


def _gate(tmp_path: Path, **env: str) -> subprocess.CompletedProcess[str]:
    (tmp_path / "force_golden_xfail.py").write_text(PLUGIN % GOLDEN_WHY)
    return subprocess.run(
        [str(ROOT / "scripts/check-proof.sh"), "-p", "force_golden_xfail"],
        capture_output=True,
        text=True,
        check=False,
        env={**os.environ, "PYTHONPATH": str(tmp_path), **env},
        timeout=600,
    )


def test_proof_gate_fails_on_golden_xfail(tmp_path: Path) -> None:
    strict = _gate(tmp_path)
    assert strict.returncode == 1, strict.stdout + strict.stderr
    assert "golden replay xfail is not a proof" in strict.stderr
    assert "check-proof: FAIL" in strict.stdout
    allowed = _gate(tmp_path, CHECK_PROOF_ALLOW_GOLDEN_XFAIL="1")
    assert allowed.returncode == 0, allowed.stdout + allowed.stderr
    assert "check-proof: PASS" in allowed.stdout
    # Reporting xfails must not drop the skip report (a second -r replaces the first).
    skipped = _gate(tmp_path, CHECK_PROOF_ALLOW_GOLDEN_XFAIL="1", FORCE_SKIP="1")
    assert skipped.returncode == 1, skipped.stdout + skipped.stderr
    assert "unexpected skip in tests/trainer/test_codec_v2.py" in skipped.stderr


def test_proof_gate_fails_on_any_golden_consumer_xfail(tmp_path: Path) -> None:
    # Matched on the reason, not the file: a consumer outside test_golden_replay.py is caught.
    marker = GOLDEN_WHY.split(";")[0]
    assert marker in (ROOT / "tests/trainer/test_golden_replay.py").read_text()
    consumer = "tests/trainer/test_act_ckpt.py"
    strict = _gate(tmp_path, XFAIL_PREFIX=consumer)
    assert strict.returncode == 1, strict.stdout + strict.stderr
    assert f"XFAIL {consumer}::" in strict.stderr
    assert "check-proof: FAIL" in strict.stdout
    allowed = _gate(tmp_path, XFAIL_PREFIX=consumer, CHECK_PROOF_ALLOW_GOLDEN_XFAIL="1")
    assert allowed.returncode == 0, allowed.stdout + allowed.stderr
    # Another xfail reason is not a golden mismatch and must not trip the golden rule.
    other = _gate(tmp_path, XFAIL_PREFIX=consumer, XFAIL_WHY="known unrelated bug")
    assert other.returncode == 0, other.stdout + other.stderr
    assert "check-proof: PASS" in other.stdout
