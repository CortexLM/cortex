"""L2: OD / assign_unit manifest fields and the unchanged-run_id guarantee."""

from __future__ import annotations

import copy
from typing import Any

import pytest
from pydantic import ValidationError

from hypertrain.protocol.example import example_manifest
from hypertrain.protocol.messages import RunManifest, f32hex

GOLDEN = "ad8beb30084a2e53f817c45f1cbfdf1491162575f10f2c3d190645081b2ba2ff"
NEW_KEYS = {"od", "source", "assign_unit", "unit_sha256_root", "profile"}


def _walk(o: object) -> set[str]:
    if isinstance(o, dict):
        return set(o) | {k for v in o.values() for k in _walk(v)}
    if isinstance(o, list):
        return {k for v in o for k in _walk(v)}
    return set()


def od_body() -> dict[str, Any]:
    b = copy.deepcopy(example_manifest().body())
    b["model"] |= {
        "arch": "od-encoder",
        "n_layers": 2,
        "d_model": 64,
        "n_heads": 4,
        "n_kv_heads": 4,
        "d_ff": 256,
        "vocab": 259,
        "seq_len": 64,
        "param_count": 187_140,
        "compute_dtype": "bf16",
        "capacity_factor": f32hex(1.0),
        "aux_loss_coef": f32hex(0.0),
        "od": {
            "preset": "od-tiny",
            "head_layers": 1,
            "objective": "mlm",
            "mask_ratio": f32hex(0.3),
            "mask_seed": 7,
            "tokenizer_offset": 3,
            "warm_start": False,
            "record": None,
            "decision_rule": "log",
            "rps_weight": f32hex(1.0),
            "distill_temp": f32hex(1.0),
        },
    }
    b["dataset"]["sample_format"] = "u16[seq_len+1] token ids"
    b["reference_spec"]["profile"] = "od-bf16-det-eager-v1"
    return b


def _bad(mutate: Any) -> None:
    b = od_body()
    mutate(b)
    with pytest.raises(ValidationError):
        RunManifest.model_validate(b)


def test_golden_run_id() -> None:
    assert example_manifest().run_id() == GOLDEN


def test_old_body_has_no_new_keys() -> None:
    assert not (_walk(example_manifest().body()) & NEW_KEYS)


def test_od_manifest_round_trips() -> None:
    m = RunManifest.model_validate(od_body())
    assert m.model.od is not None
    again = RunManifest.model_validate(m.body())
    assert again == m and again.run_id() == m.run_id() != GOLDEN


def test_od_iff_encoder() -> None:
    b = od_body()
    b["model"]["arch"] = "decoder"
    with pytest.raises(ValidationError):
        RunManifest.model_validate(b)
    b = copy.deepcopy(example_manifest().body())
    b["model"]["od"] = od_body()["model"]["od"]
    with pytest.raises(ValidationError):
        RunManifest.model_validate(b)


def test_assign_unit_rules() -> None:
    def unit(b: dict[str, Any], h: int = 512, n: int = 4096) -> None:
        b["inner"] |= {"micro_batch": 32, "grad_accum": 1, "H": h, "J": 1}
        b["dataset"] |= {"n_samples": n, "assign_unit": 512, "unit_sha256_root": "a" * 64}

    b = od_body()
    unit(b)
    RunManifest.model_validate(b)  # valid: 32*512 % 512 == 0
    _bad(lambda x: unit(x, h=500))
    _bad(lambda x: unit(x, n=4100))
    _bad(lambda x: (unit(x), x["dataset"].pop("unit_sha256_root")))


def test_unit_root_without_unit_rejected() -> None:
    _bad(lambda b: b["dataset"].update(unit_sha256_root="a" * 64))


@pytest.mark.parametrize(
    "mutate",
    [
        lambda b: b["model"].update(n_kv_heads=2),
        lambda b: b["model"].update(d_ff=128),
        lambda b: b["model"].update(n_experts=2),
        lambda b: b["model"].update(top_k_experts=2),
        lambda b: b["model"].update(capacity_factor=f32hex(1.25)),
        lambda b: b["model"].update(aux_loss_coef=f32hex(0.01)),
        lambda b: b["model"].update(compute_dtype="fp32"),
        lambda b: b["reference_spec"].update(profile="od-fp32-ref-v1"),
        lambda b: b["reference_spec"].update(profile="od-bf16-det-v1"),
        lambda b: b["reference_spec"].pop("profile"),
        lambda b: b["reference_spec"]["layout"].update(zero1=True),
        lambda b: b["model"].update(vocab=260),
        lambda b: b["model"]["od"].update(head_layers=2),
        lambda b: b["model"]["od"].update(objective="decision"),
        lambda b: b["model"]["od"].update(tokenizer_offset=4),
    ],
)
def test_od_rules_reject(mutate: Any) -> None:
    _bad(mutate)


def test_od_fp32_ref_profile_ok() -> None:
    b = od_body()
    b["reference_spec"]["profile"] = "od-fp32-ref-v1"
    b["model"]["compute_dtype"] = "fp32"
    RunManifest.model_validate(b)


def test_decision_record_len_pins_seq_len() -> None:
    rec = dict(state_len=512, n_questions=4, n_options=8, opt_len=32, instr_len=64)
    b = od_body()
    b["model"]["od"] |= {"objective": "decision", "record": rec}
    b["model"]["seq_len"] = 1835
    # d_model etc. still od-tiny; only seq_len/record coupling is under test
    RunManifest.model_validate(b)
    b["model"]["seq_len"] = 1834
    with pytest.raises(ValidationError):
        RunManifest.model_validate(b)


def test_profile_on_decoder_rejected() -> None:
    b = copy.deepcopy(example_manifest().body())
    b["reference_spec"]["profile"] = "od-fp32-ref-v1"
    with pytest.raises(ValidationError):
        RunManifest.model_validate(b)
