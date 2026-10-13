"""Run a staged, content-authenticated IslandJobV1; no provider actions.

Usage: uv run --frozen python experiments/gpu_network_v2/run.py JOB_DIR [cpu|cuda]
"""

from __future__ import annotations

import sys
from pathlib import Path

from hypertrain.miner.island_launch import launch_island
from hypertrain.protocol.envelope_v2 import load_json
from hypertrain.protocol.messages_v2 import IslandJobV1


def run(directory: Path, device: str = "cuda") -> Path:
    job = IslandJobV1.model_validate(load_json((directory / "job.json").read_bytes()))
    match device:
        case "cpu":
            return launch_island(job, directory, backend="cpu", trace=True).directory
        case "cuda":
            return launch_island(job, directory, backend="cuda", trace=True).directory
        case _:
            raise ValueError("device must be cpu or cuda")


if __name__ == "__main__":
    print(run(Path(sys.argv[1]), sys.argv[2] if len(sys.argv) > 2 else "cuda"))
