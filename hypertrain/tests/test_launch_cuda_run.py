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
