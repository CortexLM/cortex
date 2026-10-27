"""Dispute replay on kimi-k3-pro-tiny: the auditor routes it to ``kimi_tiny.traced_forward``."""

from __future__ import annotations

from dataclasses import replace
from typing import Any

import numpy as np
import pytest
import torch
from aud_fixtures import AUDITOR, MINER
from trainer_fixtures import H, assignment, make_kimi_cfg, sample

from hypertrain.auditor import bisect
from hypertrain.auditor.bisect import (
    DisputeResult,
    Executor,
    Fault,
    point_names,
    run_dispute,
    tensor_digest,
)
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.loop import Params, stage_states, train_round
from hypertrain.trainer.model import init_params
from hypertrain.trainer.optim import OptState

HONEST, CHEAT = AUDITOR.ss58, MINER.ss58


def kimi_cfg(dtype: str = "fp32") -> TrainConfig:
    cfg = make_kimi_cfg(dtype=dtype)
    return replace(cfg, inner=replace(cfg.inner, J=1))  # one leaf per step


def _dispute(
    cfg: TrainConfig, cheat_challenges: bool, cheat_cls: type[Executor] = Executor, **cheat: Any
) -> DisputeResult:
    theta, a, g = init_params(cfg.model), assignment(cfg), sample(cfg)
    honest = Executor(HONEST, cfg, theta, a, g)
    cheater = cheat_cls(CHEAT, cfg, theta, a, g, **cheat)
    referee = Executor("referee", cfg, theta, a, g)
    parties = (cheater, honest) if cheat_challenges else (honest, cheater)
    return run_dispute("ab" * 32, cfg, *parties, referee, 2, (0, H))


@pytest.mark.parametrize("dtype", ["fp32", "bf16"])
def test_traced_step_equals_trainer_bitwise_every_step_when_kimi(dtype: str) -> None:
    cfg = kimi_cfg(dtype)
    theta, a, g = init_params(cfg.model), assignment(cfg), sample(cfg)
    ref = train_round(cfg, theta, a, g)
    ex = Executor(HONEST, cfg, theta, a, g)
    # Backward moves router_bias in place: reach the last state first, so a state that aliases
    # its successor is read after the later steps ran.
    ex.state(H)
    for t in range(H + 1):
        th, st = ex.state(t)
        assert stage_states(cfg, th, st) == list(ref.leaves[t].preimage.stages), f"step {t}"


def test_trace_records_exactly_the_points_bisect_addresses_when_kimi() -> None:
    cfg = kimi_cfg()
    a, g = assignment(cfg), sample(cfg)
    ex = Executor(HONEST, cfg, init_params(cfg.model), a, g)
    n = cfg.model.n_layers
    addressed = set(point_names(cfg, "layer", (1,)))
    for layer in range(n + 1):
        addressed |= set(point_names(cfg, "op", (1, layer)))
    digests = ex.trace(1).digests
    assert set(digests) == addressed
    # The "input" point is the token batch [B, T+1], not the shifted model input.
    tokens = torch.from_numpy(np.stack([g(i) for i in a.batch_ids(cfg, 1)]).astype(np.int64))
    assert digests[(-1, "input")] == [tensor_digest(tokens)]


# Every op output bisect can address, except "loss": a constant added to the mean cross
# entropy has no gradient, so it never reaches the layer-level hashes and cannot be localised.
# The steps cycle through 1..5 so the step does not follow the challenger/defender parity.
_MICRO = kimi_cfg()
FAULTS = [
    pytest.param(Fault(1 + i % 5, layer, op), i % 2 == 1, id=f"layer{layer}-{op}")
    for i, (layer, op) in enumerate(
        point
        for layer in range(_MICRO.model.n_layers + 1)
        for point in point_names(_MICRO, "op", (1, layer))[1:]
        if point[1] != "loss"
    )
]


@pytest.mark.parametrize("fault,cheat_challenges", FAULTS)
def test_dispute_localizes_the_fault_and_cheater_loses_when_kimi(
    fault: Fault, cheat_challenges: bool
) -> None:
    """A fault at any layer/op is found only if the model returns what its hook was handed:
    a hook whose result is dropped, or that sees another tensor, leaves the parties agreeing
    on every later point. ``update`` perturbs the first parameter (``emb.weight``)."""
    res = _dispute(kimi_cfg(), cheat_challenges, fault=fault)
    assert (res.step, res.layer, res.op) == (fault.step, fault.layer, fault.op)
    assert res.resolution.loser == CHEAT
    assert res.resolution.op_spec == f"step={fault.step};layer={fault.layer};op={fault.op}"


class BiasSkewingExecutor(Executor):
    """Runs every step honestly, then adds ``delta`` to ``router_bias[0]`` of ``layer`` right
    after the optimizer step number ``at``. The forward ops of that step are untouched."""

    def __init__(self, *args: Any, at: int, layer: int, delta: float = 1e-3) -> None:
        super().__init__(*args)
        self.at, self.name, self.delta = at, f"layers.{layer:03d}.router_bias", delta
        self.optimizer_steps = 0

    def state(self, t: int) -> tuple[Params, OptState]:
        real_step = bisect.step

        def skewing_step(inner: Any, theta: Params, grads: Params, st: OptState, lr: float) -> None:
            real_step(inner, theta, grads, st, lr)
            self.optimizer_steps += 1
            if self.optimizer_steps == self.at:
                with torch.no_grad():
                    theta[self.name][0] += self.delta

        with pytest.MonkeyPatch.context() as mp:
            mp.setattr(bisect, "step", skewing_step)
            return super().state(t)


@pytest.mark.parametrize("cheat_challenges", [False, True], ids=["cheat-defends", "cheat-opens"])
def test_dispute_localizes_a_skewed_router_bias_to_the_update_when_kimi(
    cheat_challenges: bool,
) -> None:
    """router_bias is a parameter-dict tensor moved by backward and never by AdamW: only the
    "update" digest of the step binds its value to the op level."""
    cfg = kimi_cfg()
    res = _dispute(cfg, cheat_challenges, BiasSkewingExecutor, at=4, layer=2)
    n = cfg.model.n_layers
    assert (res.step, res.layer, res.op) == (4, n, "update")
    assert res.resolution.loser == CHEAT
    assert res.resolution.op_spec == f"step=4;layer={n};op=update"
