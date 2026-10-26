"""D18 / ADR 0006: ModelSpec.arch is a model-registry name. Existing manifests keep their hashes,
unknown archs are rejected, and v2 manifests carry kimi-k3-pro-tiny under its own geometry rule."""

from __future__ import annotations

import copy
import dataclasses
import json
import math
from pathlib import Path
from typing import Any

import pytest
from pydantic import ValidationError
from test_network_v2 import wrapper
from test_od_fields import od_body

import hypertrain.models.kimi_tiny as kt
from hypertrain.protocol.example import example_manifest
from hypertrain.protocol.messages import RunManifest, f32hex
from hypertrain.protocol.messages_v2 import RunManifestV2

# Real values captured at f45064de, before arch became a registry name (REPORT.md, ADR 0006).
GOLDEN_V1 = "ad8beb30084a2e53f817c45f1cbfdf1491162575f10f2c3d190645081b2ba2ff"  # published
GOLDEN_V1_OD = "0a32bc442fb217c4548ac8978075211c935d59e66d38b512decd2221ee9ffbf0"
GOLDEN_V2_RUN = "d0725eb4558afbd1b98e76f8b66910b21b78c2a977f84df3ad979074fc599aed"
GOLDEN_V2_TRAINING = "d7a864ceda214b127755ea7caee3768a1a8fb244643ed2a8d2365ed6f1fdb1cd"
MICRO = {
    "n_layers": 3,
    "d_model": 32,
    "n_heads": 4,
    "n_kv_heads": 4,
    "d_ff": 16,
    "vocab": 64,
    "seq_len": 16,
    "n_experts": 8,
    "top_k_experts": 2,
}


def _without_arch(training: dict[str, Any]) -> dict[str, Any]:
    out = copy.deepcopy(training)
    del out["model"]["arch"]
    return out


def kimi_training(**geometry: Any) -> dict[str, Any]:
    """Example body with a kimi geometry; param_count from the implementation, not the rule."""
    shape = MICRO | geometry
    body = example_manifest().body()
    body["model"].update(
        shape,
        arch=kt.ARCH,
        rope_theta=50000,
        init_seed=7,
        compute_dtype="fp32",
        capacity_factor=f32hex(1.0),
        aux_loss_coef=f32hex(1e-4),
        param_count=sum(
            math.prod(s) for s in kt.param_shapes(dataclasses.replace(kt.MICRO, **shape)).values()
        ),
    )
    body["reference_spec"]["driver_allowlist"] = ["cpu-test"]
    return body


def test_existing_manifests_hash_as_before_and_omitted_arch_is_the_decoder() -> None:
    fixture = json.loads((Path(__file__).parent / "fixtures/network_v1_baseline.json").read_bytes())
    published = json.loads(bytes.fromhex(fixture["messages"][0]["body_hex"]))
    for body in (published, _without_arch(published)):
        assert RunManifest.model_validate(body).run_id() == GOLDEN_V1
    assert RunManifest.model_validate(od_body()).run_id() == GOLDEN_V1_OD
    v2 = wrapper().body()
    for body in (v2, {**v2, "training": _without_arch(v2["training"])}):
        m = RunManifestV2.model_validate(body)
        assert (m.run_id(), m.training.training_hash()) == (GOLDEN_V2_RUN, GOLDEN_V2_TRAINING)
        assert m.training.model.arch == "decoder"


@pytest.mark.parametrize("arch", ["nope", "Decoder", "", "kimi-k3-pro"])
def test_unknown_arch_is_rejected_by_v1_and_v2(arch: str) -> None:
    body = example_manifest().body()
    body["model"]["arch"] = arch
    with pytest.raises(ValidationError, match="unknown arch"):
        RunManifest.model_validate(body)
    with pytest.raises(ValidationError, match="unknown arch"):
        wrapper(body)


def test_arch_must_be_a_json_string() -> None:
    body = example_manifest().body()
    body["model"]["arch"] = b"decoder"
    with pytest.raises(ValidationError, match="valid string"):
        RunManifest.model_validate(body)


def test_v1_manifest_refuses_an_arch_registered_after_v1() -> None:
    with pytest.raises(ValidationError, match="manifest_version 2"):
        RunManifest.model_validate(kimi_training())


@pytest.mark.parametrize(
    "geometry",
    [
        {},
        # The profile itself (77,001,376 parameters, bf16 in production).
        {f: getattr(kt.PROFILE, f) for f in MICRO},
        dict(n_layers=2, d_model=64, n_heads=2, n_kv_heads=2, d_ff=8, n_experts=4, top_k_experts=1),
    ],
    ids=["micro", "profile", "two-layer"],
)
def test_kimi_v2_param_count_rule_matches_the_implementation(geometry: dict[str, Any]) -> None:
    m = wrapper(kimi_training(**geometry))
    assert m.training.model.arch == kt.ARCH


@pytest.mark.parametrize(
    ("change", "error"),
    [
        (lambda b: b["reference_spec"]["layout"].update(n_gpus=2, dp_size=2), "at most 1 rank"),
        (lambda b: b["reference_spec"]["layout"].update(n_gpus=2, ep_size=2), "at most 1 rank"),
        (lambda b: b["model"].update(param_count=b["model"]["param_count"] + 1), "param_count"),
        (lambda b: b["inner"].update(grad_accum=2), "grad_accum"),
        (lambda b: b["model"].update(capacity_factor=f32hex(1.25)), "capacity"),
        (lambda b: b["model"].update(precision_policy="fp8-rowwise"), "precision"),
        (lambda b: b["reference_spec"].update(profile="od-fp32-ref-v1"), "profile"),
        (lambda b: b["model"].update(n_kv_heads=2), "geometry"),
        (lambda b: b["model"].update(d_model=24), "geometry"),  # head dim 6, not a multiple of 4
        (lambda b: b["model"].update(n_experts=1, top_k_experts=1), "geometry"),
        (lambda b: b["model"].update(n_layers=1), "geometry"),
    ],
    ids=[
        "dp2",
        "ep2",
        "param-count",
        "grad-accum",
        "capacity",
        "fp8",
        "profile",
        "gqa",
        "head-dim",
        "dense",
        "no-moe-layer",
    ],
)
def test_kimi_v2_rule_rejects(change: Any, error: str) -> None:
    body = kimi_training()
    wrapper(copy.deepcopy(body))  # the unchanged body is valid
    change(body)
    with pytest.raises(ValidationError, match=error):
        wrapper(body)
