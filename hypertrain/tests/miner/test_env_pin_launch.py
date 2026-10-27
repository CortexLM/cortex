"""ADR 0011 part 3: the real launcher's island worker honors or refuses the manifest's pin."""

from __future__ import annotations

import json
from pathlib import Path

import pytest
from test_island_launch_v2 import staged_job

from env_pin_fixtures import CPU_FP32, HOSTILE, ambient, needs_avx2
from hypertrain.miner.island_launch import IslandFailure, launch_island


@needs_avx2
def test_launched_worker_records_the_pin_it_honored(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    ambient(monkeypatch, **CPU_FP32["present"])
    job = staged_job(tmp_path, pinned=CPU_FP32)
    published = launch_island(job, tmp_path, backend="cpu").directory
    summary = json.loads((published / "rank-0/summary.json").read_bytes())
    assert summary["env_pin"] == {**CPU_FP32, "cpu_capability": "AVX2"}


def test_worker_refuses_a_spawn_that_does_not_honor_the_pin(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    ambient(monkeypatch, **HOSTILE)
    job = staged_job(tmp_path, pinned=CPU_FP32)
    with pytest.raises(
        IslandFailure, match="environment pin mismatch: .*MKL_CBWR: manifest 'AVX2'"
    ):
        launch_island(job, tmp_path, backend="cpu")
    assert not (tmp_path / "published").exists()
