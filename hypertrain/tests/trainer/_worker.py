"""Standalone child process: run one round and print leaves/delta hashes as JSON.

Usage: python _worker.py <dense|moe> <fp32|bf16> <codec> [flip_batch_index]
Imports hypertrain.trainer before torch so determinism setup runs first.
"""

from __future__ import annotations

import json
import sys

import hypertrain.trainer as trainer_pkg
from hypertrain.trainer.loop import train_round
from hypertrain.trainer.model import init_params

sys.path.insert(0, __file__.rsplit("/", 1)[0])
from trainer_fixtures import assignment, make_cfg, sample  # noqa: E402

arch, dtype, codec = sys.argv[1:4]
cfg = make_cfg(moe=arch == "moe", dtype=dtype, codec=codec)
a = assignment(cfg, flip=int(sys.argv[4]) if len(sys.argv) > 4 else None)
res = train_round(cfg, init_params(cfg.model), a, sample(cfg))
print(
    json.dumps(
        {
            "torch_preloaded": trainer_pkg.DETERMINISM["torch_preloaded"],
            "leaves_root": res.leaves_root,
            "leaves": [x.digest.hex() for x in res.leaves],
            "delta_hash": res.delta_hash,
            "ef_out_hash": res.ef_out_hash,
            "final_theta_hash": res.final_theta_hash,
        }
    )
)
