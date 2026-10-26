import json
import math
from pathlib import Path

import pytest

from hypertrain.trainer.config import ModelConfig
from hypertrain.trainer.model import param_shapes
from hypertrain.trainer.perf import MFU_PEAK_RTX5090_BF16, active_params, flops_per_token, mfu

ROOT = Path(__file__).resolve().parents[2]


def _phase_b() -> ModelConfig:
    m = json.loads((ROOT / "experiments/gpu_phase_b/phase_b.json").read_text())["profiles"]
    return ModelConfig(**m["phase_b"]["model"])


def test_dense_active_equals_all_params() -> None:
    cfg = ModelConfig(
        n_layers=16, d_model=2048, n_heads=16, n_kv_heads=8, d_ff=5632, vocab=32768, seq_len=2048
    )
    total = sum(math.prod(s) for s in param_shapes(cfg).values())
    assert active_params(cfg) == total
    assert flops_per_token(cfg) == 6 * total + 12 * 16 * 2048 * 2048


def test_phase_b_moe_active_params_and_audit_mfu() -> None:
    cfg = _phase_b()
    total = sum(math.prod(s) for s in param_shapes(cfg).values())
    assert total == 654_965_760  # phase_b.json provenance
    assert active_params(cfg) == 201_980_928  # audit 4.1: 201.9M
    assert flops_per_token(cfg) == 1_362_880_512
    # audit 4.1: Phase B h1 deterministic, 8 GPUs, 2000.84 tok/s -> 0.16%
    got = mfu(2000.84, flops_per_token(cfg), MFU_PEAK_RTX5090_BF16, n_gpus=8)
    assert abs(got * 100 - 0.16) < 0.01


@pytest.mark.parametrize(
    "args",
    [(0.0, 1, 1.0, 1), (1.0, 0, 1.0, 1), (1.0, 1, 0.0, 1), (1.0, 1, 1.0, 0), (-1.0, 1, 1.0, 1)],
)
def test_mfu_rejects_nonpositive(args: tuple[float, int, float, int]) -> None:
    with pytest.raises(ValueError):
        mfu(*args)


def test_non_decoder_rejected() -> None:
    cfg = _phase_b()
    with pytest.raises(ValueError):
        active_params(ModelConfig(**{**cfg.__dict__, "arch": "od-encoder"}))
