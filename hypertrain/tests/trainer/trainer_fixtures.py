from __future__ import annotations

import json
from typing import Any

import numpy as np
import numpy.typing as npt

from hypertrain.protocol.example import example_manifest
from hypertrain.protocol.messages import f32hex
from hypertrain.trainer.config import CompressConfig, InnerConfig, ModelConfig, TrainConfig
from hypertrain.trainer.loop import Assignment, SampleFn
from hypertrain.trainer.rng import rng_ctr, uniform_f64

RUN_ID = "ab" * 32  # protocol run_id: 64 lowercase hex
H, J = 6, 2


def make_cfg(
    moe: bool = False,
    dtype: str = "fp32",
    codec: str = "dense-int8",
    opt: str = "adamw",
    policy: str = "reset",
    n_stages: int = 2,
    rewarmup: int = 0,
) -> TrainConfig:
    model = ModelConfig(
        n_layers=2,
        d_model=32,
        n_heads=4,
        n_kv_heads=2,
        d_ff=48,
        vocab=64,
        seq_len=16,
        n_experts=4 if moe else 0,
        top_k_experts=2,
        capacity_factor=1.0,
        init_seed=7,
        compute_dtype=dtype,
    )
    inner = InnerConfig(
        lr=3e-3,
        H=H,
        J=J,
        micro_batch=2,
        opt=opt,
        warmup=2,
        state_policy=policy,
        rewarmup_steps=rewarmup,
    )
    comp = (
        CompressConfig("sparseloco", topk_frac=0.05, bits=2, ef_beta=0.9)
        if codec == "sparseloco"
        else CompressConfig()
    )
    return TrainConfig(model=model, inner=inner, compress=comp, n_stages=n_stages)


def sample(cfg: TrainConfig) -> SampleFn:
    def get(i: int) -> npt.NDArray[np.uint32]:
        u = uniform_f64(cfg.model.seq_len + 1, rng_ctr("data", 0, i, 0))
        return np.floor(u * cfg.model.vocab).astype(np.uint32)

    return get


def assignment(cfg: TrainConfig, flip: int | None = None, w: int = 3) -> Assignment:
    n = cfg.inner.H * cfg.inner.micro_batch * cfg.inner.grad_accum
    ids = [1000 + 17 * i for i in range(n)]
    if flip is not None:
        ids[flip] = 999_999
    return Assignment(run_id=RUN_ID, w=w, sample_ids=tuple(ids))


def small_manifest_body(moe: bool = False, opt: str = "adamw", codec: str = "dense-int8") -> Any:
    """Protocol RunManifest body (example manifest) shrunk to a CPU-test-sized model."""
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
    )
    body["inner"].update(opt=opt, micro_batch=2, H=H, J=J, state_policy="reset", rewarmup_steps=0)
    body["inner"]["lr_schedule"].update(peak_lr=f32hex(3e-3), warmup=2)
    if codec == "sparseloco":
        body["outer"].update(opt="sparseloco", topk_frac=f32hex(0.05), bits=2, ef_beta=f32hex(0.9))
    body["reference_spec"]["layout"]["pp"] = 2
    return body
