"""launch_cuda_run.sh refuses without an explicit mode and fails closed in preflight."""

import os
import subprocess
from pathlib import Path

SCRIPT = Path(__file__).parents[1] / "scripts/launch_cuda_run.sh"


def _run(tmp_path: Path, *args: str) -> subprocess.CompletedProcess[str]:
    # Stub the frozen tree (checked fail-closed before preflight) and pin TOOLS under tmp_path.
    tree = tmp_path / "tree"
    (tree / "experiments/gpu_network_v2").mkdir(parents=True)
    (tree / "experiments/gpu_network_v2/orchestrate.py").touch()
    env = dict(
        os.environ,
        RD=str(tmp_path),
        STATE=str(tmp_path / "state"),
        PY="python3",
        HT_TREE=str(tree),
        TOOLS=str(tmp_path / "tools"),
    )
    return subprocess.run(
        ["bash", str(SCRIPT), *args], capture_output=True, text=True, env=env, timeout=60
    )


def test_requires_explicit_mode(tmp_path: Path) -> None:
    result = _run(tmp_path)
    assert result.returncode == 2 and "--dry-run | --yes-spend" in result.stderr


def test_preflight_names_the_first_missing_input_and_spends_nothing(tmp_path: Path) -> None:
    result = _run(tmp_path, "--yes-spend")
    assert result.returncode == 1
    assert "FAIL preflight: missing " + str(tmp_path / "tools/prepare_record.py") in result.stdout
    assert not (tmp_path / "state" / "window").exists()


def test_the_bundle_checks_of_the_launcher_still_find_what_they_look_for_in_the_miner() -> None:
    """Preflight greps the miner source for fixed strings and dies when one is gone: a rename in
    miner/core.py (the PUT retry call, 2026-10-28) blocked every launch and no test noticed."""
    import re

    core = (Path(__file__).parents[1] / "src/hypertrain/miner/core.py").read_text()
    checks = re.findall(
        r"grep -q '([^']+)' \"\$BUNDLE/hypertrain/miner/core\.py\" \|\| die", SCRIPT.read_text()
    )
    assert len(checks) >= 3  # the PUT retry, the parallel part PUT, the theta-only input reuse
    # grep -q takes a basic regex: every character is literal here but a leading ^
    missing = [
        pattern
        for pattern in checks
        if not re.search(
            ("^" if pattern.startswith("^") else "") + re.escape(pattern.removeprefix("^")),
            core,
            re.MULTILINE,
        )
    ]
    assert missing == []
