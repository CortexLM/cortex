from __future__ import annotations

from dataclasses import replace

import pytest
from aud_fixtures import AUDITOR
from trainer_fixtures import assignment, make_cfg, sample

from hypertrain.auditor.bisect import Executor
from hypertrain.trainer.loop import stage_states, train_round
from hypertrain.trainer.model import init_params


@pytest.mark.parametrize("dtype", ["fp32", "bf16"])
@pytest.mark.parametrize("moe", [False, True])
def test_traced_step_equals_trainer_bitwise_every_step(moe: bool, dtype: str) -> None:
    """Dispute tracer mirrors model.forward op by op; any unmirrored trainer change fails here."""
    cfg = make_cfg(moe=moe, dtype=dtype)
    cfg = replace(cfg, inner=replace(cfg.inner, J=1))
    theta, a, g = init_params(cfg.model), assignment(cfg), sample(cfg)
    ref = train_round(cfg, theta, a, g)
    ex = Executor(AUDITOR.ss58, cfg, theta, a, g)
    for t in range(cfg.inner.H + 1):
        th, st = ex.state(t)
        assert stage_states(cfg, th, st) == list(ref.leaves[t].preimage.stages), f"step {t}"
