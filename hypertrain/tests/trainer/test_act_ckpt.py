"""HT_ACT_CKPT=1 recomputes layers in backward with the same kernels: leaf roots stay golden."""

from __future__ import annotations

import pytest

import hypertrain.trainer  # noqa: F401  (determinism before torch)

# isort: split
from test_golden_replay import _expect_golden
from trainer_fixtures import assignment, make_cfg, sample

from hypertrain.trainer.loop import train_round
from hypertrain.trainer.model import init_params


@pytest.mark.parametrize("moe", [False, True])
@pytest.mark.parametrize("dt", ["fp32", "bf16"])
def test_act_ckpt_bitwise(
    monkeypatch: pytest.MonkeyPatch, moe: bool, dt: str, request: pytest.FixtureRequest
) -> None:
    monkeypatch.setenv("HT_ACT_CKPT", "1")
    cfg = make_cfg(moe=moe, opt="adamw", dtype=dt)
    res = train_round(cfg, init_params(cfg.model), assignment(cfg), sample(cfg))
    _expect_golden(f"loop-{'moe' if moe else 'dense'}-adamw-{dt}", res.leaves_root, request)


@pytest.mark.parametrize("moe", [False, True])
@pytest.mark.parametrize("dt", ["fp32", "bf16"])
def test_act_ckpt_matches_plain_in_one_process(
    monkeypatch: pytest.MonkeyPatch, moe: bool, dt: str
) -> None:
    # Hardware-independent: GOLDEN misses on an unknown runner CPU only xfail above.
    cfg = make_cfg(moe=moe, opt="adamw", dtype=dt)
    roots = []
    for ckpt in (False, True):
        if ckpt:
            monkeypatch.setenv("HT_ACT_CKPT", "1")
        else:
            monkeypatch.delenv("HT_ACT_CKPT", raising=False)
        res = train_round(cfg, init_params(cfg.model), assignment(cfg), sample(cfg))
        roots.append(res.leaves_root)
    assert roots[0] == roots[1]
