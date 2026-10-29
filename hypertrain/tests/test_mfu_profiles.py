"""The rows mfu-dense-0.1b (D68, ADR 0015), the D94 run profile mfu-dense-0.03b and its larger
option mfu-dense-0.05b (ADR 0016) of mfu_profiles.json are the dense TEST models of the builder."""

from __future__ import annotations

import json
import math
from pathlib import Path

from hypertrain.trainer.config import ModelConfig
from hypertrain.trainer.model import param_shapes

PROFILES = Path(__file__).resolve().parents[1] / "scripts/mfu_profiles.json"


def test_the_intermediate_row_has_the_pinned_parameter_count() -> None:
    row = json.loads(PROFILES.read_text())["profiles"]["mfu-dense-0.1b"]["model"]

    count = sum(math.prod(s) for s in param_shapes(ModelConfig(**row)).values())

    # The D68 band (100..300 M, checked by the builder) lets d_ff 2816 (114.8 M), 12 layers
    # (125.8 M), vocab 65536 (151.0 M) and 12 kv heads (107.0 M) through; the exact count is what
    # freezes the shape.
    assert count == 100_676_352


def test_the_50m_row_has_the_pinned_parameter_count() -> None:
    row = json.loads(PROFILES.read_text())["profiles"]["mfu-dense-0.05b"]["model"]

    count = sum(math.prod(s) for s in param_shapes(ModelConfig(**row)).values())

    # The D94 band (40..70 M, checked by the builder) lets 4 layers (50.3 M), vocab 32768 (69.2 M),
    # d_ff 2816 (49.4 M) and 12 kv heads (46.4 M) through, over the admission window (ADR 0016).
    assert count == 44_045_568


def test_the_d94_run_row_has_the_pinned_parameter_count() -> None:
    row = json.loads(PROFILES.read_text())["profiles"]["mfu-dense-0.03b"]["model"]

    count = sum(math.prod(s) for s in param_shapes(ModelConfig(**row)).values())

    # Its band (25..40 M, checked by the builder) lets 2 layers at vocab 16384 (37.8 M), 4 layers
    # (37.8 M), d_ff 2816 (36.8 M), 12 kv heads (33.8 M) and 1 layer at vocab 16384 (31,459,584)
    # through: the exact count freezes the shape.
    assert count == 31_462_656
