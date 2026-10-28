"""ADR 0011 part 3: the documented presets are sufficient for the real TEST model, per compute
dtype: under the pin a hostile ambient gives the roots of the CI environment, the one the deployed
goldens (tests/models/test_kimi_golden.py) are bound to. A fresh interpreter per case because ATen,
MKL and oneDNN read their variables once.

Mutant guards, not red-before tests: they pass on any tree that has the table. They kill a preset
that loses a key the model's roots depend on (``CPU_FP32`` without ``MKL_CBWR``, ``CPU_BF16``
without ``ONEDNN_MAX_CPU_ISA``) and a protocol.md that drifts from the presets it documents."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

import pytest

from env_pin_fixtures import CPU_BF16, CPU_FP32, HOSTILE, child, clean_env, needs_avx2
from hypertrain.protocol.env_pins import EnvPin, pinned_environ

# .github/workflows/hypertrain-ci.yml: the environment every golden root was recorded under.
CI_ENVIRONMENT = {
    "ATEN_CPU_CAPABILITY": "avx2",
    "MKL_CBWR": "AVX2",
    "ONEDNN_MAX_CPU_ISA": "AVX2",
}
KIMI_ROOTS = """
import dataclasses, json, sys
import hypertrain.trainer
import hypertrain.models.kimi_tiny as kt

doc = kt.run_manifest(dataclasses.replace(kt.MICRO, compute_dtype=sys.argv[1]))
(result,) = kt.qualify(doc, "cpu", 1)
print(json.dumps([result[key] for key in ("leaves_root", "delta_hash", "state_root")]))
"""


def roots(env: dict[str, str], dtype: str) -> list[str]:
    done = child(KIMI_ROOTS, env, dtype)
    assert done.returncode == 0, done.stderr
    return list(json.loads(done.stdout))


@needs_avx2
@pytest.mark.parametrize(("dtype", "preset"), [("fp32", CPU_FP32), ("bf16", CPU_BF16)])
def test_kimi_roots_follow_the_dtype_presets(dtype: str, preset: dict[str, Any]) -> None:
    reference = roots(clean_env() | CI_ENVIRONMENT, dtype)
    if roots(clean_env(), dtype) == reference:
        pytest.skip("this host's default already is the CI environment: nothing to pin")
    pinned_hostile = pinned_environ(clean_env() | HOSTILE, EnvPin.from_body(preset))
    assert roots(pinned_hostile, dtype) == reference


def test_protocol_doc_carries_the_presets_it_is_tested_with() -> None:
    doc = (Path(__file__).parents[2] / "docs/protocol.md").read_text()
    assert json.dumps(CPU_FP32) in doc
    bf16_only = {k: v for k, v in CPU_BF16["present"].items() if k not in CPU_FP32["present"]}
    assert json.dumps(bf16_only)[1:-1] in doc  # the one key CPU_BF16 adds to CPU_FP32
