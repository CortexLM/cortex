"""S3 compile, S4 FP8, S11 codec v2: default paths stay golden; flagged paths are repeatable."""

from __future__ import annotations

import dataclasses

import pytest

import hypertrain.trainer  # noqa: F401  (determinism before torch)

# isort: split
import torch
from test_golden_replay import _expect_golden
from trainer_fixtures import assignment, make_cfg, sample, without_pin_check

from hypertrain.trainer.codec_v2 import decode_v2
from hypertrain.trainer.compress import decompress, validate_payload
from hypertrain.trainer.loop import replay, train_round
from hypertrain.trainer.model import fp8_enabled, init_params, param_shapes


def _run(cfg):  # type: ignore[no-untyped-def]
    return train_round(cfg, init_params(cfg.model), assignment(cfg), sample(cfg))


@pytest.mark.parametrize("codec", ["dense-int8", "sparseloco"])
def test_codec_v2_roundtrip_same_ef(codec: str) -> None:
    cfg = make_cfg(codec=codec)
    v1 = _run(cfg)
    cfg2 = dataclasses.replace(cfg, compress=dataclasses.replace(cfg.compress, codec_version=2))
    v2 = _run(cfg2)
    assert v2.leaves_root == v1.leaves_root and v2.ef_out_hash == v1.ef_out_hash
    assert decode_v2(v2.delta_payload) == v1.delta_payload
    shapes = param_shapes(cfg.model)
    assert validate_payload(v2.delta_payload, shapes, max_payload_bytes=1 << 24) == codec
    a, b = decompress(v1.delta_payload)[1], decompress(v2.delta_payload)[1]
    assert all(torch.equal(a[n], b[n]) for n in a)
    rep = replay(
        cfg2, init_params(cfg.model), assignment(cfg), sample(cfg), v2.leaf_digests, v2.delta_hash
    )
    assert rep.result == "MATCH"


def test_fp8_off_by_default(
    monkeypatch: pytest.MonkeyPatch, request: pytest.FixtureRequest
) -> None:
    monkeypatch.delenv("HT_FP8", raising=False)
    cfg = make_cfg(dtype="bf16")
    assert not fp8_enabled(cfg.model)
    _expect_golden("loop-dense-adamw-bf16", _run(cfg).leaves_root, request)


def test_fp8_flag_repeatable(monkeypatch: pytest.MonkeyPatch) -> None:
    without_pin_check(monkeypatch)
    monkeypatch.setenv("HT_FP8", "1")
    cfg = make_cfg(dtype="bf16")
    try:
        roots = {_run(cfg).leaves_root for _ in range(2)}
    except RuntimeError as e:  # CPU builds without an fp8 _scaled_mm kernel
        pytest.skip(f"fp8 _scaled_mm unavailable here: {e}")
    assert len(roots) == 1


def test_compile_flag_repeatable(monkeypatch: pytest.MonkeyPatch) -> None:
    without_pin_check(monkeypatch)
    monkeypatch.setenv("HT_COMPILE", "1")
    cfg = make_cfg(dtype="fp32")
    roots = {_run(cfg).leaves_root for _ in range(2)}
    assert len(roots) == 1
