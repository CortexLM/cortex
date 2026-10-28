"""ADR 0011 part 3: every rank of a publication declares the environment it ran in, and a
publication whose declaration differs from the manifest's pin is refused by the client before it
is screened, so the party never posts it. The server compares the same declaration again
(tests/challenge/test_admission_env_v2.py): this is the first of the two controls."""

from __future__ import annotations

import json
from collections.abc import Callable
from pathlib import Path
from typing import Any

import pytest
from test_island_launch_v2 import staged_job

from env_pin_fixtures import CPU_AVX2, honest_observation, needs_avx2
from hypertrain.miner.island_launch import EnvPinRefused, launch_island, validate_artifacts

Edit = Callable[[dict[str, Any]], None]


def env(name: str, value: str | None) -> Edit:
    return lambda seen: seen["env"].__setitem__(name, value)


TAMPERED: dict[str, tuple[Edit, str]] = {
    "isa-resolved": (lambda seen: seen.__setitem__("cpu_capability", "AVX512"), "cpu_capability"),
    "mkl": (env("MKL_CBWR", "COMPATIBLE"), "MKL_CBWR"),
    "onednn": (env("ONEDNN_MAX_CPU_ISA", "ALL"), "ONEDNN_MAX_CPU_ISA"),
    "sm": (lambda seen: seen.__setitem__("gpu", "sm_103"), "gpu"),
    "attention": (env("HT_ATTN_BACKEND", "sdpa-flash"), "HT_ATTN_BACKEND"),
    "fp8": (env("HT_FP8", "1"), "HT_FP8"),
}


@needs_avx2
def test_a_launch_publishes_what_every_rank_observed(tmp_path: Path) -> None:
    job = staged_job(tmp_path)
    artifacts = launch_island(job, tmp_path, backend="cpu")
    assert artifacts.env.model_dump() == honest_observation(CPU_AVX2)
    for rank in (0, 1):
        summary = json.loads((artifacts.directory / f"rank-{rank}/summary.json").read_bytes())
        assert summary["env"] == honest_observation(CPU_AVX2)


@needs_avx2
@pytest.mark.parametrize("fault", sorted(TAMPERED))
def test_a_publication_whose_rank_declares_another_environment_is_refused(
    tmp_path: Path, fault: str
) -> None:
    job = staged_job(tmp_path)
    artifacts = launch_island(job, tmp_path, backend="cpu")
    validate_artifacts(job, artifacts.directory)  # the untouched twin conforms
    path = artifacts.directory / "rank-1/summary.json"
    summary = json.loads(path.read_bytes())
    edit, names = TAMPERED[fault]
    edit(summary["env"])
    path.write_text(json.dumps(summary))
    with pytest.raises(
        EnvPinRefused, match=rf"rank 1 observed an environment that differs: \['{names}'\]"
    ):
        validate_artifacts(job, artifacts.directory)
