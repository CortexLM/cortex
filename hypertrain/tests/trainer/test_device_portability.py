from __future__ import annotations

import pytest
import torch
from fake_device import DevTensor, FakeDeviceMode, to_device
from trainer_fixtures import assignment, make_cfg, sample

from hypertrain.trainer.compress import compress, decompress
from hypertrain.trainer.config import CompressConfig
from hypertrain.trainer.loop import replay, train_round
from hypertrain.trainer.model import init_params


@pytest.mark.parametrize("moe", [False, True], ids=["dense", "moe"])
@pytest.mark.parametrize("default_device", [True, False], ids=["default_dev", "explicit"])
def test_train_round_on_device_proxy_matches_cpu(moe: bool, default_device: bool) -> None:
    cfg = make_cfg(moe=moe, dtype="bf16", codec="sparseloco" if moe else "dense-int8")
    a, get = assignment(cfg), sample(cfg)
    ref = train_round(cfg, init_params(cfg.model), a, get)
    with FakeDeviceMode(default_device):
        theta0 = {k: to_device(v) for k, v in init_params(cfg.model).items()}
        res = train_round(cfg, theta0, a, get)
        rep = replay(cfg, theta0, a, get, ref.leaf_digests, ref.delta_hash)
    assert all(isinstance(t, DevTensor) for t in res.final_theta.values())
    assert all(isinstance(t, DevTensor) for t in res.ef_out.values())
    assert res.leaf_digests == ref.leaf_digests
    assert res.leaves_root == ref.leaves_root
    assert res.delta_hash == ref.delta_hash and res.ef_out_hash == ref.ef_out_hash
    assert rep.result == "MATCH"


@pytest.mark.parametrize("codec", ["dense-int8", "sparseloco"])
def test_codec_under_default_device(codec: str) -> None:
    cfg = (
        CompressConfig(codec, topk_frac=0.1, bits=2) if codec == "sparseloco" else CompressConfig()
    )
    delta = {"w": torch.linspace(-1, 1, 700, device="cpu")}
    ef = {"w": torch.zeros(700, device="cpu")}
    payload, ef_out = compress(cfg, delta, ef)
    _, ref = decompress(payload)
    with FakeDeviceMode(default_on_device=True):
        assert torch.zeros(1).device.type == "meta"
        dev_payload, _ = compress(cfg, {"w": to_device(delta["w"])}, {"w": to_device(ef["w"])})
        _, dec = decompress(payload)
    assert dev_payload == payload
    assert dec["w"].device.type == "cpu" and torch.equal(dec["w"], ref["w"])
