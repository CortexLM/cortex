"""launch_cuda_run.sh refuses without an explicit mode and fails closed in preflight."""

import os
import subprocess
from collections.abc import Mapping
from pathlib import Path

import pytest

SCRIPT = Path(__file__).parents[1] / "scripts/launch_cuda_run.sh"
SOURCE = Path(__file__).parents[1] / "src/hypertrain"
# The bundle step of the launcher: from the line that makes $BUNDLE to its last check.
BUNDLE_STEP_FIRST = "BUNDLE=$STATE/bundle"
BUNDLE_STEP_LAST = 'die "bundle data/store.py differs from the working tree"'
UPLOAD_CALL_SITES = ('_retrying("POST object', '_retrying("PUT object')
GIT_CONFIG = ("-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false")


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


def _real_sources() -> dict[str, str]:
    return {n: (SOURCE / n).read_text() for n in ("miner/core.py", "data/store.py")}


def _core_without_upload_call_site() -> str:
    core = _real_sources()["miner/core.py"]
    for literal in UPLOAD_CALL_SITES:
        core = core.replace(literal, '_retrying("UPLOAD object')
    return core


def _bundle_step(
    tmp_path: Path,
    miner: Mapping[str, str],
    *,
    base: Mapping[str, str] | None = None,
    working: Mapping[str, str] | None = None,
) -> subprocess.CompletedProcess[str]:
    """Run the launcher's bundle step, lifted as text, with bash and the script's shell options.

    MINER_SRC holds the files given in `miner`, the real ones for the rest. REPO is a throwaway git
    repo: its FIX_BASE commit holds `base` (default: the miner's files) and its working tree
    `working` (default: `base`), which is where the step takes the transport fixes it patches in."""
    lines = SCRIPT.read_text().splitlines()
    first = lines.index(BUNDLE_STEP_FIRST)
    last = next(i for i, line in enumerate(lines) if BUNDLE_STEP_LAST in line)
    repo = tmp_path / "repo"
    miner = {**_real_sources(), **miner}
    base = {**miner, **(base or {})}
    working = {**base, **(working or {})}
    for root, files in ((tmp_path / "miner_src", miner), (repo / "src/hypertrain", base)):
        for name, content in files.items():
            (root / name).parent.mkdir(parents=True, exist_ok=True)
            (root / name).write_text(content)
    git = ["git", "-C", str(repo), *GIT_CONFIG]
    for args in (["init", "-q"], ["add", "-A"], ["commit", "-q", "-m", "base"]):
        subprocess.run([*git, *args], check=True, capture_output=True)
    for name, content in working.items():
        (repo / "src/hypertrain" / name).write_text(content)
    script = (
        'set -euo pipefail\ndie() { echo "FAIL bundle: $*"; exit 1; }\n'
        'FIX_BASE=$(git -C "$REPO" rev-parse HEAD)\n' + "\n".join(lines[first : last + 1]) + "\n"
    )
    env = {
        **os.environ,
        "STATE": str(tmp_path / "state"),
        "MINER_SRC": str(tmp_path / "miner_src"),
        "REPO": str(repo),
    }
    return subprocess.run(
        ["bash", "-c", script], capture_output=True, text=True, env=env, timeout=60, check=False
    )


def _bundled(tmp_path: Path, name: str) -> str:
    return (tmp_path / "state/bundle/hypertrain" / name).read_text()


def test_the_bundle_step_accepts_the_miner_source_of_this_tree(tmp_path: Path) -> None:
    """Preflight greps the miner source for fixed strings and dies when one is gone: a rename in
    miner/core.py (the PUT retry call, 2026-10-28) blocked every launch and no test noticed."""
    result = _bundle_step(tmp_path, {})
    assert result.returncode == 0, result.stdout + result.stderr


@pytest.mark.parametrize(
    ("call_site", "accepted"),
    [
        pytest.param('_retrying("POST object " + digest, put)', True, id="post"),
        pytest.param('_retrying("UPLOAD object " + digest, put)', False, id="renamed"),
    ],
)
def test_the_upload_check_follows_the_retried_upload_call_site(
    tmp_path: Path, call_site: str, accepted: bool
) -> None:
    # The real file with its upload call sites renamed away: its GET retries stay, so a check that
    # accepted any _retrying call would accept the rename too.
    core = _core_without_upload_call_site() + f"\n    return {call_site}\n"
    result = _bundle_step(tmp_path, {"miner/core.py": core})
    assert result.returncode == (0 if accepted else 1), result.stdout + result.stderr
    assert ("bundle lacks the upload retry fix" in result.stdout) is (not accepted)


def test_the_working_tree_fix_is_patched_in_when_the_miner_source_lacks_the_upload_retry(
    tmp_path: Path,
) -> None:
    renamed = _core_without_upload_call_site()
    result = _bundle_step(
        tmp_path,
        {"miner/core.py": renamed},
        working={"miner/core.py": _real_sources()["miner/core.py"]},
    )
    assert result.returncode == 0, result.stdout + result.stderr
    assert '_retrying("PUT object' in _bundled(tmp_path, "miner/core.py")


@pytest.mark.parametrize("verb", ["PUT", "POST"])
def test_a_miner_source_with_the_upload_retry_is_not_patched(tmp_path: Path, verb: str) -> None:
    # The repo diff (renamed -> real) cannot apply to a source that already has a retried upload:
    # a launcher that patched anyway would die here, whichever call site the source has.
    real = _real_sources()["miner/core.py"]
    core = real.replace('_retrying("PUT object', f'_retrying("{verb} object')
    result = _bundle_step(
        tmp_path,
        {"miner/core.py": core},
        base={"miner/core.py": _core_without_upload_call_site()},
        working={"miner/core.py": real},
    )
    assert result.returncode == 0, result.stdout + result.stderr
    assert _bundled(tmp_path, "miner/core.py") == core


@pytest.mark.parametrize(
    ("name", "marker", "refusal"),
    [
        pytest.param("data/store.py", "def _parts", "the parallel part fetch", id="part-fetch"),
        pytest.param("miner/core.py", "UPLOAD_WORKERS = ", "the parallel part PUT", id="part-put"),
        pytest.param("miner/core.py", "def _local_input", "the theta-only input reuse", id="reuse"),
    ],
)
def test_each_other_bundle_check_refuses_a_miner_source_without_its_fix(
    tmp_path: Path, name: str, marker: str, refusal: str
) -> None:
    content = _real_sources()[name].replace(marker, marker[:5] + "_x" + marker[5:])
    result = _bundle_step(tmp_path, {name: content})
    assert result.returncode == 1
    assert "bundle lacks " + refusal in result.stdout


def test_a_bundle_store_that_differs_from_the_working_tree_is_refused(tmp_path: Path) -> None:
    store = _real_sources()["data/store.py"]
    result = _bundle_step(
        tmp_path, {"data/store.py": store + "\n# local edit\n"}, base={"data/store.py": store}
    )
    assert result.returncode == 1
    assert "bundle data/store.py differs from the working tree" in result.stdout
