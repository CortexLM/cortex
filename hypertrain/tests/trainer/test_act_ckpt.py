"""HT_ACT_CKPT=1 recomputes layers in backward with the same kernels: leaf roots stay golden."""

from __future__ import annotations

import pytest

import hypertrain.trainer  # noqa: F401  (determinism before torch)

# isort: split
from test_golden_replay import GOLDEN
from trainer_fixtures import assignment, make_cfg, sample

from hypertrain.trainer.loop import train_round
from hypertrain.trainer.model import init_params


@pytest.mark.parametrize("moe", [False, True])
@pytest.mark.parametrize("dt", ["fp32", "bf16"])
def test_act_ckpt_bitwise(monkeypatch: pytest.MonkeyPatch, moe: bool, dt: str) -> None:
    monkeypatch.setenv("HT_ACT_CKPT", "1")
    cfg = make_cfg(moe=moe, opt="adamw", dtype=dt)
    res = train_round(cfg, init_params(cfg.model), assignment(cfg), sample(cfg))
    assert res.leaves_root in GOLDEN[f"loop-{'moe' if moe else 'dense'}-adamw-{dt}"]
