"""s100_test/cpu_miners.py in CUDA mode (HT_MINER_DEVICE=cuda), miner CLI and nvidia-smi faked.

The module reads the variable once at import, so each test loads a fresh copy of the script.
"""

from __future__ import annotations

import importlib.util
import json
import stat
import subprocess
import sys
from pathlib import Path
from types import ModuleType

import pytest

SCRIPT = Path(__file__).resolve().parents[3] / "scripts/ops/s100_test/cpu_miners.py"
DIGEST = "sha256:" + "ab" * 32


def load(monkeypatch: pytest.MonkeyPatch, device: str | None) -> ModuleType:
    for name in ("HT_MINER_DEVICE", "HT_IMAGE_DIGEST", "ATEN_CPU_CAPABILITY"):
        monkeypatch.delenv(name, raising=False)
    if device is not None:
        monkeypatch.setenv("HT_MINER_DEVICE", device)
    spec = importlib.util.spec_from_file_location(f"cpu_miners_{device}", SCRIPT)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def seeds(tmp_path: Path, n: int = 2) -> tuple[Path, Path]:
    d = tmp_path / "seeds"
    d.mkdir()
    for i in range(n):
        for role in ("hot", "cold"):
            (d / f"miner{i}-{role}.seed").write_text("00" * 32)
    origins = tmp_path / "origins.json"
    origins.write_text(json.dumps([f"{i:064x}" for i in range(n)]))
    return d, origins


@pytest.mark.parametrize(("device", "written"), [("cuda", "cuda"), (None, "cpu")])
def test_setup_writes_the_device_of_the_mode_into_every_miner_toml(
    monkeypatch, tmp_path, device, written
):
    m = load(monkeypatch, device)
    sd, origins = seeds(tmp_path)
    root = tmp_path / "mine"
    run_id = "cd" * 32
    m.setup([str(root), "https://relay.invalid/test-master", run_id, "5Owner", DIGEST, str(sd),
             str(origins), "2"])  # fmt: skip
    for i in range(2):
        toml = (root / f"m{i}" / "miner.toml").read_text()
        assert f'device = "{written}"' in toml
        assert f'image_digest = "{DIGEST}"' in toml and f'run_id = "{run_id}"' in toml
        assert stat.S_IMODE((root / f"m{i}" / "hot.seed").stat().st_mode) == 0o600


@pytest.mark.parametrize("command", ["admit", "round"])
def test_a_cuda_miner_needs_the_manifest_digest_and_no_aten_pin(monkeypatch, command):
    m = load(monkeypatch, "cuda")
    for bad in (None, "", "sha256:" + "AB" * 32, "sha256:abc", "ab" * 32):
        if bad is None:
            monkeypatch.delenv("HT_IMAGE_DIGEST", raising=False)
        else:
            monkeypatch.setenv("HT_IMAGE_DIGEST", bad)
        with pytest.raises(SystemExit, match="HT_IMAGE_DIGEST"):
            getattr(m, "admit" if command == "admit" else "round_")(["unused", "0"])
    monkeypatch.setenv("HT_IMAGE_DIGEST", DIGEST)
    m.require_capability()  # no ATEN_CPU_CAPABILITY, and no refusal


def test_a_cpu_miner_still_needs_the_aten_pin_and_ignores_the_digest(monkeypatch):
    m = load(monkeypatch, None)
    monkeypatch.setenv("HT_IMAGE_DIGEST", DIGEST)
    with pytest.raises(SystemExit, match="ATEN_CPU_CAPABILITY"):
        m.require_capability()
    monkeypatch.setenv("ATEN_CPU_CAPABILITY", "avx2")
    m.require_capability()


@pytest.mark.parametrize(
    ("device", "retried"), [("cuda", True), (None, False)], ids=["cuda", "cpu"]
)
def test_a_500_is_retried_only_for_a_cuda_run(monkeypatch, device, retried):
    m = load(monkeypatch, device)
    err = "INFO ...\nChallengeHTTPError: 500 cuda executor gave up"
    assert m.transient("", err) is retried
    for always in ("ChallengeHTTPError: 503 paused", "ReadTimeout", "TRIAL_CAPACITY"):
        assert m.transient("", always)
    assert not m.transient("", "ChallengeHTTPError: 409 CHALLENGE_EXPIRED")


EXHAUSTED_404 = "ChallengeHTTPError: 404: trial input object unavailable"


@pytest.mark.parametrize("device", ["cuda", None], ids=["cuda", "cpu"])
def test_only_the_404_of_a_trial_input_the_miner_gave_up_on_is_transient(monkeypatch, device):
    m = load(monkeypatch, device)
    assert m.transient("", "INFO ...\n2026-10-27 22:26:42,001 ERROR " + EXHAUSTED_404)
    for refused in (
        "ChallengeHTTPError: 404: unknown admission",
        "ChallengeHTTPError: 404: ",
        "ChallengeHTTPError: 403: trial input object unavailable",
        "ChallengeHTTPError: 409 CHALLENGE_EXPIRED",
    ):
        assert not m.transient("", refused)


def scripted_cli(m, monkeypatch, answers: list[tuple[int, str, str]]) -> list[str]:
    """A miner CLI that answers `answers` in order, the last one for ever; the stages it ran."""
    stages: list[str] = []

    def fake(root, i, name, args, **extra):
        stages.append(name)
        assert len(stages) < 200, "driver retried without bound"
        return answers[min(len(stages), len(answers)) - 1]

    monkeypatch.setattr(m, "cli", fake)
    monkeypatch.setattr(m.time, "sleep", lambda _: None)
    return stages


def test_a_trial_whose_input_404_outlasted_the_miner_gets_another_driver_pass(
    monkeypatch, tmp_path
):
    m = load(monkeypatch, None)
    (tmp_path / "m0").mkdir()
    stages = scripted_cli(
        m,
        monkeypatch,
        [(2, "", "ERROR " + EXHAUSTED_404), (0, '{"proof": 1}', ""), (0, "{}", "")],
    )
    m.trial(tmp_path, 0, 5)
    assert stages == ["probe-v2", "probe-v2", "proof-v2"]
    assert (tmp_path / "m0/proof-5.json").read_text() == '{"proof": 1}'


def test_a_404_that_never_clears_stops_the_driver_after_its_40_passes(monkeypatch, tmp_path):
    m = load(monkeypatch, None)
    (tmp_path / "m0").mkdir()
    stages = scripted_cli(m, monkeypatch, [(2, "", "ERROR " + EXHAUSTED_404)])
    with pytest.raises(SystemExit, match="probe-v2 miner 0 trial 5: retries exhausted"):
        m.trial(tmp_path, 0, 5)
    assert stages == ["probe-v2"] * 40


def test_any_other_404_stops_the_driver_at_once(monkeypatch, tmp_path):
    m = load(monkeypatch, None)
    (tmp_path / "m0").mkdir()
    stages = scripted_cli(m, monkeypatch, [(2, "", "ERROR ChallengeHTTPError: 404: unknown run")])
    with pytest.raises(SystemExit, match="unknown run"):
        m.trial(tmp_path, 0, 5)
    assert stages == ["probe-v2"]


def fake_smi(tmp_path: Path, monkeypatch, readings: list[str]) -> Path:
    """nvidia-smi that prints the next reading on each call (the last one repeats)."""
    bindir, state = tmp_path / "bin", tmp_path / "calls"
    bindir.mkdir()
    state.write_text("0")
    script = bindir / "nvidia-smi"
    script.write_text(
        "#!/bin/sh\nn=$(cat {s}); echo $((n+1)) > {s}\ncase $n in {cases} esac\n".format(
            s=state,
            cases=" ".join(f"{i}) printf '{r}\\n';;" for i, r in enumerate(readings[:-1]))
            + f" *) printf '{readings[-1]}\\n';;",
        )
    )
    script.chmod(0o755)
    monkeypatch.setenv("PATH", f"{bindir}:/usr/bin:/bin")
    return state


def test_wait_gpu_idle_waits_for_foreign_memory_to_leave(monkeypatch, tmp_path):
    m = load(monkeypatch, "cuda")
    calls = fake_smi(tmp_path, monkeypatch, ["270000", "270000", "2048"])
    pauses: list[float] = []
    monkeypatch.setattr(m.time, "sleep", pauses.append)
    m.wait_gpu_idle()
    assert pauses == [10, 10] and calls.read_text().strip() == "3"


def test_wait_gpu_idle_gives_up_inside_the_challenge_window(monkeypatch, tmp_path):
    m = load(monkeypatch, "cuda")
    fake_smi(tmp_path, monkeypatch, ["270000"])
    clock = iter(range(0, 1000, 100))
    monkeypatch.setattr(m.time, "monotonic", lambda: next(clock))
    monkeypatch.setattr(m.time, "sleep", lambda _: None)
    with pytest.raises(SystemExit, match=r"still busy after 240s: 270000 MiB"):
        m.wait_gpu_idle()


def test_wait_gpu_idle_reads_the_busiest_gpu_of_several(monkeypatch, tmp_path):
    m = load(monkeypatch, "cuda")
    fake_smi(tmp_path, monkeypatch, ["0\\n3000", "0\\n0"])
    monkeypatch.setattr(m.time, "sleep", lambda _: None)
    m.wait_gpu_idle()
    assert int(Path(tmp_path / "calls").read_text()) == 2


@pytest.mark.parametrize(
    ("device", "stage", "gated"),
    [
        ("cuda", "probe-v2", True),
        ("cuda", "run-v2", True),
        ("cuda", "join-v2", False),
        ("cuda", "status-v2", False),
        (None, "probe-v2", False),
        (None, "run-v2", False),
    ],
)
def test_only_the_gpu_stages_of_a_cuda_miner_wait_for_the_gpu(
    monkeypatch, tmp_path, device, stage, gated
):
    m = load(monkeypatch, device)
    waited: list[str] = []
    monkeypatch.setattr(m, "wait_gpu_idle", lambda: waited.append(stage))
    monkeypatch.setattr(
        m.subprocess, "run", lambda *a, **k: subprocess.CompletedProcess(a, 0, "{}", "")
    )
    (tmp_path / "m0").mkdir()
    assert m.cli(tmp_path, 0, stage, [stage])[0] == 0
    assert waited == ([stage] if gated else [])
    assert f'"stage": "{stage}"' in (tmp_path / "stages.tsv").read_text()


def test_admit_waits_for_the_gpu_before_a_join_starts_the_challenge_window(monkeypatch, tmp_path):
    m = load(monkeypatch, "cuda")
    monkeypatch.setenv("HT_IMAGE_DIGEST", DIGEST)
    order: list[str] = []
    monkeypatch.setattr(m, "wait_gpu_idle", lambda: order.append("wait"))
    monkeypatch.setattr(m, "join", lambda root, i: order.append(f"join{i}"))
    monkeypatch.setattr(m, "status", lambda root, i: {"record": {"clean_count": 4, "state": "x"}})
    assert m.admit([str(tmp_path), "1:4,0:4"]) == 0
    assert order == ["wait", "join1", "wait", "join0"]


def test_a_lapsed_window_stops_the_driver_at_once_and_shows_the_operator_step(
    monkeypatch, tmp_path
):
    # probe-v2 waited for the operator's reseed and exited with status 3 (RELAY-03): asking again
    # from here would only start another wait.
    m = load(monkeypatch, None)
    (tmp_path / "m0").mkdir()
    line = (
        "ERROR ChallengeExpired: CHALLENGE_EXPIRED: admission ab has no window left after 61 asks "
        "over 600 s; an operator must reseed it: POST /v2/runs/r/admin/join/ab/reseed"
    )
    stages = scripted_cli(m, monkeypatch, [(3, "", line)])
    with pytest.raises(SystemExit, match="must reseed it: POST /v2/runs/r/admin/join/ab/reseed"):
        m.trial(tmp_path, 0, 5)
    assert stages == ["probe-v2"]
