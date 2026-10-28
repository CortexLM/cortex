"""ADR 0011 part 3: the real launcher starts island workers under the manifest's pins, refuses
a host that cannot honor them before anything is charged, and the worker refuses a spawn that
skipped them."""

from __future__ import annotations

import json
import os
import subprocess
from pathlib import Path
from typing import Any

import pytest
from test_island_launch_v2 import staged_job

from env_pin_fixtures import CPU_AVX2, HOSTILE, ambient, honest_observation, needs_avx2
from hypertrain.miner import island_launch
from hypertrain.miner.island_launch import (
    CapacityAttempt,
    EnvPinRefused,
    IslandFailure,
    _launch_unbounded,
    island_env,
    launch_island,
    launch_window,
    validate_artifacts,
)
from hypertrain.protocol.env_pins import EnvPin, EnvPinUnhonorable


def published_summary(directory: Path) -> dict[str, Any]:
    summary = json.loads((directory / "rank-0/summary.json").read_bytes())
    assert isinstance(summary, dict)
    return summary


def roots(directory: Path) -> str:
    """Every commitment of the round: leaves, delta, final state and EF."""
    return json.dumps(published_summary(directory)["commitments"], sort_keys=True)


def launch_under(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path, ambient_values: dict[str, str], **job: Any
) -> Path:
    ambient(monkeypatch, **ambient_values)
    return launch_island(staged_job(tmp_path, **job), tmp_path, backend="cpu").directory


@needs_avx2
def test_pinned_roots_do_not_depend_on_the_ambient(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # A launcher that forgot the pin would run the worker under the hostile ambient, which the
    # worker refuses: this is red if the launcher stops applying it.
    runs = {
        name: launch_under(monkeypatch, tmp_path / name, values)
        for name, values in (("clean", {}), ("hostile", HOSTILE))
    }
    assert len({roots(d) for d in runs.values()}) == 1, "2 distinct commitment tuples"
    for directory in runs.values():
        assert published_summary(directory)["env"] == honest_observation(CPU_AVX2)


@pytest.mark.parametrize("entry", ["launch_island", "launch_window", "launch_unbounded"])
def test_a_host_that_cannot_honor_the_pin_is_refused_before_anything_starts(
    entry: str, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    def refuse(pin: EnvPin, backend: str) -> dict[str, Any]:
        raise EnvPinUnhonorable("ATen CPU capability: manifest 'AVX2', live 'DEFAULT'")

    def forbidden(*args: object, **kwargs: object) -> None:
        raise AssertionError("started after the pin was refused")

    def charge() -> bool:
        raise AssertionError("capacity charged after the pin was refused")

    monkeypatch.setattr(island_launch, "require_honorable", refuse)
    monkeypatch.setattr(island_launch, "run_capacity_argv", forbidden)
    monkeypatch.setattr(subprocess, "Popen", forbidden)
    job = staged_job(tmp_path, pinned=CPU_AVX2)
    capacity = CapacityAttempt("77" * 32, "66" * 32, tmp_path / "runtime.lock", charge)
    rerun = tmp_path / "rerun/published"
    (rerun / "rank-0").mkdir(parents=True)
    (rerun / "rank-0/summary.json").write_text('{"backend": "cpu"}')
    entries = {
        "launch_island": lambda: launch_island(job, tmp_path, backend="cpu", capacity=capacity),
        "launch_window": lambda: launch_window(job, rerun, 1),
        "launch_unbounded": lambda: _launch_unbounded(job, tmp_path / "attempt", "cpu", {}, None),
    }
    with pytest.raises(IslandFailure, match="cannot be honored on this host: ATen CPU") as refused:
        entries[entry]()
    assert type(refused.value) is EnvPinRefused
    assert not (tmp_path / "published").exists()


def test_capacity_runner_receives_the_pinned_environment(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # The capacity branch never goes through _launch_unbounded: its pins come from island_env.
    seen: dict[str, str] = {}

    def stop(
        argv: list[str], runtime: Path, deadline: int, capacity: object, **kwargs: Any
    ) -> None:
        seen.update(kwargs["env"])
        raise IslandFailure("stopped after reading the environment")

    ambient(monkeypatch, **HOSTILE)
    monkeypatch.setattr(
        island_launch, "require_honorable", lambda pin, backend: honest_observation(pin)
    )
    monkeypatch.setattr(island_launch, "run_capacity_argv", stop)
    capacity = CapacityAttempt("77" * 32, "66" * 32, tmp_path / "runtime.lock", lambda: True)
    job = staged_job(tmp_path, pinned=CPU_AVX2)
    with pytest.raises(IslandFailure, match="stopped after reading"):
        launch_island(job, tmp_path, backend="cpu", capacity=capacity)
    assert {name: seen.get(name) for name in HOSTILE} == {
        "ATEN_CPU_CAPABILITY": "avx2",
        "HT_ATTN_BACKEND": "math",
        "MKL_CBWR": "AVX2",
        "OMP_DYNAMIC": "FALSE",
        "ONEDNN_MAX_CPU_ISA": "AVX2",
        "HT_FP8": None,
        "HT_MOE_GROUPED": None,
        "OMP_THREAD_LIMIT": None,
    }


def test_worker_refuses_a_spawn_that_skipped_the_pins(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # Backstop: whatever launches the ranks, a worker that was not given the pins refuses.
    ambient(monkeypatch, **HOSTILE)
    monkeypatch.setattr(island_launch, "island_env", lambda job, **kwargs: dict(os.environ))
    monkeypatch.setattr(island_launch, "pinned_environ", lambda base, pin: dict(base))
    job = staged_job(tmp_path, pinned=CPU_AVX2)
    with pytest.raises(
        IslandFailure, match="environment pin mismatch: .*MKL_CBWR: manifest 'AVX2'"
    ):
        launch_island(job, tmp_path, backend="cpu")
    assert not (tmp_path / "published").exists()


@needs_avx2
def test_direct_unbounded_caller_gets_the_pin(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # The call pattern of scripts/network_gpu_operation.py: the caller's own environment.
    ambient(monkeypatch, **HOSTILE)
    job = staged_job(tmp_path, pinned=CPU_AVX2)
    publication = tmp_path / "published"
    publication.mkdir()
    for relative in job.object_paths.values():
        (publication / relative).write_bytes((tmp_path / relative).read_bytes())
    (publication / "job.json").write_bytes(job.model_dump_json().encode())
    _launch_unbounded(job, publication, "cpu", dict(os.environ, HT_ISLAND_TRACE="1"), None)
    validate_artifacts(job, publication)
    assert published_summary(publication)["env"] == honest_observation(CPU_AVX2)


@needs_avx2
def test_window_rerun_keeps_the_pin(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    # A carry job: re-running a window of a reset job refuses in the worker, pinned or not.
    job = staged_job(tmp_path, pinned=CPU_AVX2, state_policy="carry")
    ambient(monkeypatch)
    published = launch_island(job, tmp_path, backend="cpu").directory
    ambient(monkeypatch, **HOSTILE)
    launch_window(job, published, 1)
    assert all((published / f"rank-{rank}/windows/1.json").is_file() for rank in (0, 1))


def test_island_env_replaces_drops_and_keeps(tmp_path: Path) -> None:
    job = staged_job(tmp_path, pinned=CPU_AVX2)
    base = {"PATH": "/usr/bin", "MKL_CBWR": "COMPATIBLE", "HT_FP8": "1", "OMP_THREAD_LIMIT": "2"}
    snapshot = dict(base)
    env = island_env(job, trace=True, window=3, base=base)
    assert env == {
        "PATH": "/usr/bin",
        **CPU_AVX2["present"],
        "CUBLAS_WORKSPACE_CONFIG": ":4096:8",
        "OMP_NUM_THREADS": "1",
        "CUDA_DISABLE_PTX_JIT": "1",
        "HT_ISLAND_TRACE": "1",
        "HT_ISLAND_WINDOW": "3",
    }
    assert base == snapshot
