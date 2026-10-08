from __future__ import annotations

import json
from typing import Any

import numpy as np
import numpy.typing as npt

from hypertrain.protocol.example import example_manifest
from hypertrain.protocol.messages import Layout, f32hex
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.island import island_from_manifest
from hypertrain.trainer.loop import Assignment, SampleFn
from hypertrain.trainer.rng import rng_ctr, uniform_f64

H, J = 4, 2
LAYOUTS: dict[str, dict[str, Any]] = {
    "moe-dp2-ep2-z1": dict(moe=True, dp=2, ep=2, zero1=True, dtype="fp32"),
    "moe-dp1-ep4-bf16": dict(moe=True, dp=1, ep=4, zero1=False, dtype="bf16"),
    "dense-dp4-z1-muon": dict(moe=False, dp=4, ep=1, zero1=True, dtype="fp32", opt="muon"),
    "moe-dp1-ep2-z1": dict(moe=True, dp=1, ep=2, zero1=True, dtype="fp32"),
    "moe-dp2-ep1": dict(moe=True, dp=2, ep=1, zero1=False, dtype="fp32"),
    "dense-dp2": dict(moe=False, dp=2, ep=1, zero1=False, dtype="fp32"),
    "single-moe": dict(moe=True, dp=1, ep=1, zero1=False, dtype="fp32"),
    "single-dense": dict(moe=False, dp=1, ep=1, zero1=True, dtype="fp32"),
}


def manifest_body(
    moe: bool, dp: int, ep: int, zero1: bool, dtype: str = "fp32", opt: str = "adamw"
) -> Any:
    body: Any = json.loads(json.dumps(example_manifest().body()))
    body["model"].update(
        n_layers=2,
        d_model=32,
        n_heads=4,
        n_kv_heads=2,
        d_ff=48,
        vocab=64,
        seq_len=16,
        n_experts=4 if moe else 1,
        top_k_experts=2 if moe else 1,
        init_seed=7,
        capacity_factor=f32hex(1.0),
        compute_dtype=dtype,
    )
    body["inner"].update(opt=opt, micro_batch=2, H=H, J=J, state_policy="reset", rewarmup_steps=0)
    body["inner"]["lr_schedule"].update(peak_lr=f32hex(3e-3), warmup=2)
    body["outer"].update(opt="sparseloco", topk_frac=f32hex(0.05), bits=2, ef_beta=f32hex(0.9))
    body["reference_spec"]["layout"] = {
        "pp": 2,
        "n_gpus": dp * ep,
        "dp_size": dp,
        "ep_size": ep,
        "zero1": zero1,
    }
    return body


def setup(name: str) -> tuple[TrainConfig, Layout, Assignment, SampleFn]:
    cfg, lay, run_id = island_from_manifest(manifest_body(**LAYOUTS[name]))
    n = cfg.inner.H * cfg.inner.micro_batch * cfg.inner.grad_accum * lay.n_gpus
    a = Assignment(run_id=run_id, w=5, sample_ids=tuple(1000 + 17 * i for i in range(n)))

    def get(i: int) -> npt.NDArray[np.uint32]:
        u = uniform_f64(cfg.model.seq_len + 1, rng_ctr("data", 0, i, 0))
        return np.floor(u * cfg.model.vocab).astype(np.uint32)

    return cfg, lay, a, get
