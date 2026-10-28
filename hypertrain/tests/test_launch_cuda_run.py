"""launch_cuda_run.sh refuses without an explicit mode and fails closed in preflight."""

import hashlib
import json
import os
import re
import subprocess
import sys
from pathlib import Path

import pytest

SCRIPT = Path(__file__).parents[1] / "scripts/launch_cuda_run.sh"
SOURCE = Path(__file__).parents[1] / "src/hypertrain"
# A pattern may hold a double quote, so a grep call is matched up to the first bundle path on its
# line; bash does the quoting when the call runs.
BUNDLE_GREP = re.compile(r'grep\b[^\n]*?"\$BUNDLE/hypertrain/[^"\n]+"')
UPLOAD_CALL_SITES = ('_retrying("POST object', '_retrying("PUT object')


def _run(
    tmp_path: Path, *args: str, extra: dict[str, str] | None = None
) -> subprocess.CompletedProcess[str]:
    # Stub the frozen tree (checked fail-closed before preflight) and pin TOOLS under tmp_path.
    tree = tmp_path / "tree"
    (tree / "experiments/gpu_network_v2").mkdir(parents=True, exist_ok=True)
    (tree / "experiments/gpu_network_v2/orchestrate.py").touch()
    # The script runs its interpreter with PYTHONHOME=$RD/runtime/local.
    (tmp_path / "runtime").mkdir(exist_ok=True)
    if not (tmp_path / "runtime/local").exists():
        (tmp_path / "runtime/local").symlink_to(sys.base_prefix)
    env = dict(
        os.environ,
        RD=str(tmp_path),
        STATE=str(tmp_path / "state"),
        PY=str(Path(sys.base_prefix) / "bin/python3"),
        HT_TREE=str(tree),
        TOOLS=str(tmp_path / "tools"),
        E2E=str(tmp_path / "e2e"),
        PROXY_TOKEN=str(tmp_path / "proxy.token"),
        COORD_KEY=str(tmp_path / "coord.key"),
        **(extra or {}),
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


def _preflight(
    tmp_path: Path, ledger: str, extra: dict[str, str]
) -> subprocess.CompletedProcess[str]:
    """Dry-run the script over stub inputs: the economics policy of the given ledger mode, which
    the manifest pins by hash, and an empty file for every other input the preflight lists."""
    out = tmp_path / "manifest/out-v1"
    out.mkdir(parents=True)
    policy = json.dumps({"ledger_mode": ledger}).encode()
    (out / "policy-economics_policy_hash.json").write_bytes(policy)
    (out / "manifest.json").write_text(
        json.dumps({"network": {"economics_policy_hash": hashlib.sha256(policy).hexdigest()}})
    )
    for _ in range(64):
        done = _run(tmp_path, "--dry-run", extra=extra)
        missing = re.search(r"FAIL preflight: missing (\S+)", done.stdout)
        if missing is None:
            return done
        Path(missing[1]).parent.mkdir(parents=True, exist_ok=True)
        Path(missing[1]).touch()
    raise AssertionError("the preflight keeps naming missing inputs")


STRICT = {
    "QUAL_KNOWN_HOSTS_H0": "/data/kh0",
    "QUAL_KNOWN_HOSTS_H1": "/data/kh1",
    "QUAL_FINGERPRINT_H0": "SHA256:fp0",
    "QUAL_FINGERPRINT_H1": "SHA256:fp1",
}


@pytest.mark.parametrize(
    "ledger,extra,outcome",
    [
        ("production", {}, r"FAIL preflight: .*strict"),
        ("production", {"QUAL_LABEL_ONLY": "1"}, r"FAIL preflight: .*test-ledger"),
        ("production", {k: STRICT[k] for k in list(STRICT)[:3]}, r"FAIL preflight: .*all four"),
        ("test", {}, r"FAIL preflight: .*strict"),
        ("production", STRICT, "QUALIFICATION_TRUST mode=strict ledger=production"),
        ("test", STRICT, "QUALIFICATION_TRUST mode=strict ledger=test"),
        ("test", {"QUAL_LABEL_ONLY": "1"}, "QUALIFICATION_TRUST mode=label-only ledger=test"),
    ],
    ids=[
        "production_without_attestation_inputs",
        "production_asked_for_label_only",
        "production_with_a_missing_fingerprint",
        "test_without_an_explicit_label_only",
        "production_strict",
        "test_strict",
        "test_label_only",
    ],
)
def test_bootstrap_trust_is_decided_in_preflight_before_any_spend(
    tmp_path: Path, ledger: str, extra: dict[str, str], outcome: str
) -> None:
    # Given / When
    done = _preflight(tmp_path, ledger, extra)
    # Then: the mode is settled, or refused, before the roots check and every provider call.
    assert re.search(outcome, done.stdout), done.stdout + done.stderr
    assert not (tmp_path / "state" / "window").exists()


def test_bootstrap_stage_takes_its_trust_mode_from_the_preflight() -> None:
    # Given: the bootstrap stage of the script.
    text = SCRIPT.read_text()
    stage = text[text.index("if stage bootstrap; then") :].split("\nfi\n", 1)[0]
    # When / Then: no stage can fall back to label-only by itself.
    assert "--label-only" not in stage and '"${TRUST[@]}"' in stage


def _bundle_exits(root: Path, core: str) -> dict[str, int]:
    """Each bundle check of the launcher, run by bash and the system grep on a bundle holding this
    tree's data/store.py and the given miner/core.py: the exit status of every grep call."""
    bundle = root / "bundle"
    for name, text in (
        ("miner/core.py", core),
        ("data/store.py", (SOURCE / "data/store.py").read_text()),
    ):
        (bundle / "hypertrain" / name).parent.mkdir(parents=True, exist_ok=True)
        (bundle / "hypertrain" / name).write_text(text)
    env = {**os.environ, "BUNDLE": str(bundle)}
    return {
        call: subprocess.run(
            ["bash", "-u", "-c", call], capture_output=True, env=env, timeout=30, check=False
        ).returncode
        for call in BUNDLE_GREP.findall(SCRIPT.read_text())
    }


def test_every_bundle_check_of_the_launcher_passes_on_the_miner_source(tmp_path: Path) -> None:
    """Preflight greps the miner source for fixed strings and dies when one is gone: a rename in
    miner/core.py (the PUT retry call, 2026-10-28) blocked every launch and no test noticed."""
    exits = _bundle_exits(tmp_path, (SOURCE / "miner/core.py").read_text())
    # The upload retry, the parallel part fetch, the parallel part PUT and the theta-only input
    # reuse: fewer means the script was refactored out of this test's sight.
    assert len(exits) >= 4
    assert {call: code for call, code in exits.items() if code != 0} == {}


@pytest.mark.parametrize(
    ("call_site", "accepted"),
    [
        pytest.param('_retrying("PUT object " + digest, put)', True, id="put"),
        pytest.param('_retrying("POST object " + digest, put)', True, id="post"),
        pytest.param('_retrying("UPLOAD object " + digest, put)', False, id="renamed"),
    ],
)
def test_the_upload_check_follows_the_retried_upload_call_site_of_the_miner(
    tmp_path: Path, call_site: str, accepted: bool
) -> None:
    # The real file with its upload call sites renamed away: its GET retries stay, so a check that
    # accepted any _retrying call would accept the rename too.
    core = (SOURCE / "miner/core.py").read_text()
    for literal in UPLOAD_CALL_SITES:
        core = core.replace(literal, '_retrying("UPLOAD object')
    exits = _bundle_exits(tmp_path, core + f"\n    return {call_site}\n")
    upload = {call: code for call, code in exits.items() if "_retrying(" in call}
    # Every spot that tests the upload retry must answer alike: a decision and a final check that
    # diverged would patch a bundle that is then refused, or the reverse.
    assert upload
    assert set(upload.values()) == {0 if accepted else 1}, upload
