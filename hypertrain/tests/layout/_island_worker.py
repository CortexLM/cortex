"""torchrun child: one island round over gloo; writes this rank's result JSON.

Usage: python -m torch.distributed.run --standalone --nproc-per-node N _island_worker.py \
    <layout name> <out dir> [all_gather|all_reduce]
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

import hypertrain.trainer as trainer_pkg
from hypertrain.trainer.island import DistComm, train_island
from hypertrain.trainer.model import init_params

sys.path.insert(0, str(Path(__file__).resolve().parent))
import torch.distributed as dist  # noqa: E402
from layout_fixtures import setup  # noqa: E402

name, out_dir = sys.argv[1], Path(sys.argv[2])
reduction = sys.argv[3] if len(sys.argv) > 3 else "all_gather"
dist.init_process_group("gloo")
try:
    cfg, lay, a, get = setup(name)
    comm = DistComm()
    res = train_island(cfg, lay, comm, init_params(cfg.model), a, get, reduction=reduction)
    (out_dir / f"rank{comm.rank}.json").write_text(
        json.dumps(
            {
                "rank": comm.rank,
                "world": comm.world,
                "torch_preloaded": trainer_pkg.DETERMINISM["torch_preloaded"],
                "ops": sorted(comm.ops),
                "leaves_root": res.leaves_root,
                "leaves": [x.digest.hex() for x in res.leaves],
                "delta_hash": res.delta_hash,
                "final_theta_hash": res.final_theta_hash,
                "ef_out_hash": res.ef_out_hash,
            }
        )
    )
finally:
    dist.destroy_process_group()
