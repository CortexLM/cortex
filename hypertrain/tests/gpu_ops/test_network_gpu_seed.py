"""Real tiny D2 inputs; no qualification result, CUDA execution or training."""

from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path

import pytest

import hypertrain.trainer  # noqa: F401
from hypertrain.auditor.replay import unpack_state
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.model import init_params

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location(
    "network_seed_builder", ROOT / "scripts/network_gpu_seed.py"
)
assert spec is not None and spec.loader is not None
seed = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = seed
spec.loader.exec_module(seed)


@pytest.fixture
def generated(tmp_path):
    public, private = tmp_path / "public", tmp_path / "private"
    digest = seed.generate(
        public, private, "sha256:" + "ab" * 32, ["candidate-driver-not-observed"], 1000, 2000
    )
    return public, private, digest


def test_real_seed_exact_profile_genesis_and_tokenizer(generated):
    import torch

    public, private, digest = generated
    job = seed.validate(public, digest)
    m = job.manifest.training
    assert m.model.param_count == 61856 and m.model.compute_dtype == "bf16"
    assert m.reference_spec.layout.model_dump() == dict(
        pp=1, n_gpus=2, dp_size=1, ep_size=2, zero1=True
    )
    assert m.inner.H == 30 and m.inner.J == 5 and m.inner.state_policy == "carry"
    theta, state = unpack_state((public / "start_state").read_bytes())
    assert sum(x.numel() for x in theta.values()) == 61856
    assert state is not None and state.step == 0 and set(state.m) == set(state.v) == set(theta)
    assert all(x.dtype == torch.float32 for x in theta.values())
    assert all(torch.count_nonzero(x) == 0 for x in (*state.m.values(), *state.v.values()))
    ef, _ = unpack_state((public / "ef_in").read_bytes())
    v0, _ = unpack_state((public / "v0").read_bytes())
    assert all(torch.count_nonzero(x) == 0 for x in ef.values()) and not v0
    expected = init_params(TrainConfig.from_manifest_v2(job.manifest).model)
    assert all(torch.equal(theta[k], expected[k]) for k in theta)
    index = json.loads((public / "seed.json").read_bytes())
    assert "NOT_CUDA_QUALIFIED" in index["claim"]
    assert not json.loads((public / "reference.json").read_bytes())["cuda_observed"]
    for role, key in index["roles"].items():
        secret = private / (role + ".seed")
        assert secret.stat().st_mode & 0o777 == 0o600
        assert Keypair(secret.read_bytes()).ss58 == key
    assert not list(public.rglob("*.seed"))


@pytest.mark.parametrize("fault", ["genesis", "samples", "profile", "signature", "hash"])
def test_seed_tampering_rejects(generated, fault):
    public, _, digest = generated
    index = json.loads((public / "seed.json").read_bytes())
    if fault == "hash":
        digest = "00" * 32
    else:
        name = {
            "genesis": "start_state",
            "samples": "samples",
            "profile": "profile.json",
            "signature": "job-envelope.json",
        }[fault]
        path = public / name
        if fault == "profile":
            body = json.loads(path.read_bytes())
            body["model"]["d_model"] = 64
            path.write_bytes(canonicalize(body))
        elif fault == "signature":
            body = json.loads(path.read_bytes())
            body["sig"] = "00" * 64
            path.write_bytes(canonicalize(body))
        else:
            raw = bytearray(path.read_bytes())
            raw[-1] ^= 1
            path.write_bytes(raw)
        index["files"][name] = sha256_hex(path.read_bytes())
        raw = canonicalize(index)
        (public / "seed.json").write_bytes(raw)
        digest = sha256_hex(raw)
    with pytest.raises(ValueError):
        seed.validate(public, digest)


def test_missing_runtime_inputs_reject_before_output(tmp_path):
    with pytest.raises(ValueError):
        seed.generate(tmp_path / "out", tmp_path / "keys", "sha256:" + "ab" * 32, [], 1000, 2000)
    assert not (tmp_path / "out").exists() and not (tmp_path / "keys").exists()
