"""check-proof.sh: a golden replay xfail fails the gate unless the diagnostic variable is set;
an unexpected skip still fails it."""

import os
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# Keeps one test per suite (speed) and turns every golden replay test into an xfail, as a
# leaf root outside GOLDEN does; FORCE_SKIP also skips the codec v2 test.
PLUGIN = """
import os

import pytest

def pytest_collection_modifyitems(config, items):
    config.hook.pytest_deselected(items=items[1:])
    del items[1:]

def pytest_runtest_setup(item):
    if item.nodeid.startswith("tests/trainer/test_golden_replay.py"):
        pytest.xfail("forced golden mismatch")
    if os.environ.get("FORCE_SKIP") and item.nodeid.startswith("tests/trainer/test_codec_v2.py"):
        pytest.skip("forced skip")
"""


def test_proof_gate_fails_on_golden_xfail(tmp_path: Path) -> None:
    (tmp_path / "force_golden_xfail.py").write_text(PLUGIN)

    def gate(**env: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [str(ROOT / "scripts/check-proof.sh"), "-p", "force_golden_xfail"],
            capture_output=True,
            text=True,
            check=False,
            env={**os.environ, "PYTHONPATH": str(tmp_path), **env},
            timeout=600,
        )

    strict = gate()
    assert strict.returncode == 1, strict.stdout + strict.stderr
    assert "golden replay xfail is not a proof" in strict.stderr
    assert "check-proof: FAIL" in strict.stdout
    allowed = gate(CHECK_PROOF_ALLOW_GOLDEN_XFAIL="1")
    assert allowed.returncode == 0, allowed.stdout + allowed.stderr
    assert "check-proof: PASS" in allowed.stdout
    # Reporting xfails must not drop the skip report (a second -r replaces the first).
    skipped = gate(CHECK_PROOF_ALLOW_GOLDEN_XFAIL="1", FORCE_SKIP="1")
    assert skipped.returncode == 1, skipped.stdout + skipped.stderr
    assert "unexpected skip in tests/trainer/test_codec_v2.py" in skipped.stderr
