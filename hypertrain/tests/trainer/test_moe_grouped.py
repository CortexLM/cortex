"""Prompt 1.8: opt-in grouped-GEMM MoE (HT_MOE_GROUPED=1) that skips dropped tokens."""

from __future__ import annotations

import pytest

import hypertrain.trainer  # noqa: F401  (determinism before torch)

# isort: split
import torch
from trainer_fixtures import assignment, make_cfg, sample

from hypertrain.trainer.loop import train_round
from hypertrain.trainer.model import _moe, _moe_grouped, init_params


@pytest.mark.parametrize("cf", [0.5, 1.0, 2.0])
def test_grouped_matches_reference_value_and_grad(cf: float) -> None:
    cfg = make_cfg(moe=True).model
    cfg = cfg.__class__(**{**cfg.__dict__, "capacity_factor": cf})
    p = {n: x.requires_grad_(True) for n, x in init_params(cfg).items()}
    x = torch.randn(2, 16, cfg.d_model, generator=torch.Generator().manual_seed(3))
    x.requires_grad_(True)
    pre = "layers.000."
    names = [x, *(p[pre + w] for w in ("router", "w1", "w2", "w3"))]
    outs = []
    for fn in (_moe, _moe_grouped, _moe_grouped):
        y, aux = fn(cfg, p, pre, x, torch.float32)
        outs.append((y, aux, *torch.autograd.grad((y.square().sum() + aux), names)))
    assert all(torch.equal(a, b) for a, b in zip(outs[1], outs[2], strict=True))  # repeatable
    for a, b in zip(outs[0], outs[1], strict=True):
        torch.testing.assert_close(a, b, rtol=1e-5, atol=1e-6)


def test_grouped_round_trains_repeatably(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("HT_MOE_GROUPED", "1")
    cfg = make_cfg(moe=True)
    roots = {
        train_round(cfg, init_params(cfg.model), assignment(cfg), sample(cfg)).leaves_root
        for _ in range(2)
    }
    assert len(roots) == 1
