"""Plan 1.5: fused optimizer step is bitwise equal to the per-tensor reference loop."""

from __future__ import annotations

import os

import pytest

import hypertrain.trainer  # noqa: F401  (determinism before torch)

# isort: split
import torch

from hypertrain.trainer.config import InnerConfig
from hypertrain.trainer.optim import init_state, step

SHAPES = {
    "emb.weight": (64, 32),
    "layers.000.attn_norm": (32,),
    "layers.000.wq": (32, 32),
    "layers.000.wk": (32, 16),
    "layers.000.wv": (32, 16),
    "layers.000.w1": (4, 32, 48),
    "layers.000.w2": (4, 48, 32),
    "layers.001.wq": (32, 32),
    "head.weight": (32, 64),
}


@pytest.mark.parametrize("clip", [1.0, 0.37])
@pytest.mark.parametrize("opt", ["adamw", "muon"])
@pytest.mark.parametrize("policy", ["reset", "derived", "carry"])
def test_fused_equals_reference_50_steps(opt: str, policy: str, clip: float) -> None:
    cfg = InnerConfig(
        lr=3e-3, H=50, J=1, micro_batch=1, opt=opt, state_policy=policy, grad_clip=clip
    )
    g = torch.Generator().manual_seed(1)
    p0 = {n: torch.randn(s, generator=g) * 0.02 for n, s in SHAPES.items()}
    ref_cfg = InnerConfig(lr=3e-3, H=50, J=1, micro_batch=1, opt=opt)
    seed = init_state(ref_cfg, p0)
    seed.step = 3
    v0 = {n: x.abs() for n, x in seed.v.items()} if policy == "derived" else None
    carry = seed if policy == "carry" else None
    pa = {n: x.clone() for n, x in p0.items()}
    pb = {n: x.clone() for n, x in p0.items()}
    sa = init_state(cfg, pa, carry, v0)
    sb = init_state(cfg, pb, carry, v0)
    for _ in range(50):
        grads = {n: torch.randn(s, generator=g) for n, s in SHAPES.items()}
        step(cfg, pa, grads, sa, 3e-3, fused=False)
        step(cfg, pb, grads, sb, 3e-3, fused=True)
    for d in ((pa, pb), (sa.m, sb.m), (sa.v, sb.v)):
        assert sorted(d[0]) == sorted(d[1])
        assert all(torch.equal(d[0][n], d[1][n]) for n in d[0])


@pytest.mark.skipif("HT_GENUINE_CUDA_INDEX" not in os.environ, reason="needs a real GPU")
def test_step_has_no_host_sync() -> None:
    dev = torch.device("cuda", int(os.environ["HT_GENUINE_CUDA_INDEX"]))
    cfg = InnerConfig(lr=3e-3, H=1, J=1, micro_batch=1, opt="muon", grad_clip=0.37)
    p = {n: torch.randn(s, device=dev) for n, s in SHAPES.items()}
    st = init_state(cfg, p)
    grads = {n: torch.randn(s, device=dev) for n, s in SHAPES.items()}
    torch.cuda.set_sync_debug_mode("error")
    try:
        step(cfg, p, grads, st, 3e-3)
    finally:
        torch.cuda.set_sync_debug_mode("default")
