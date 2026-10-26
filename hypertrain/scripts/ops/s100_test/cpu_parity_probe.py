"""CPU island parity probe: run one exported trial job with the cpu backend, hash the publication.

Usage: cpu_parity_probe.py JOB_DIR WORK_DIR DEADLINE
(JOB_DIR from hypertrain.challenge.trial_export)
Run it on the miner host and on the challenge host (server image, --network none) and compare
the printed hashes: a cpu TEST run scores only if the miner's publication is bitwise equal to
the reference the challenge computes on its own CPU. Both sides pass the same DEADLINE (Unix
seconds, later than now) in place of the job's, so they run the same job; nothing is sent anywhere.
"""

from __future__ import annotations

import hashlib
import json
import shutil
import sys
import time
from pathlib import Path

import hypertrain.trainer  # noqa: F401  (determinism before torch)
from hypertrain.miner.island_launch import launch_island
from hypertrain.protocol.envelope_v2 import load_json
from hypertrain.protocol.messages_v2 import IslandJobV1


def main(job_dir: Path, work: Path, deadline: int) -> int:
    import torch

    job = IslandJobV1.model_validate(load_json((job_dir / "job.json").read_bytes()))
    job = IslandJobV1.model_validate({**job.model_dump(mode="json"), "deadline": deadline})
    shutil.copytree(job_dir, work)
    started = time.monotonic()
    launch_island(job, work, backend="cpu")
    elapsed = round(time.monotonic() - started, 2)
    published = work / "published"
    files = {
        str(p.relative_to(published)): hashlib.sha256(p.read_bytes()).hexdigest()
        for p in sorted(published.rglob("*"))
        if p.is_file()
    }
    print(
        json.dumps(
            {
                "torch": torch.__version__,
                "cpu_capability": torch.backends.cpu.get_cpu_capability(),
                "threads": torch.get_num_threads(),
                "seconds": elapsed,
                "files": files,
            },
            sort_keys=True,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main(Path(sys.argv[1]), Path(sys.argv[2]), int(sys.argv[3])))
