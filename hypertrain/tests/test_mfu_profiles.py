"""The row mfu-dense-0.1b of mfu_profiles.json is the D68 intermediate TEST model (ADR 0015)."""

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
