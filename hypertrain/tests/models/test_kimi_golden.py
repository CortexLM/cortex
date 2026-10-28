"""Known answers for kimi-k3-pro-tiny MICRO (ADR 0006).

The init digest is host independent (Philox integers and float64 to float32 casts) and, like the
dispatch-order test, a hard assertion. The round-root test can only xfail, on an unknown CPU
class: it uses the scheme and the XFAIL_REASON of tests/trainer/test_golden_replay.py, so a root
outside GOLDEN is a reported xfail that scripts/check-proof.sh turns into a failed gate.
"""

from __future__ import annotations

import dataclasses
import hashlib
import sys
from pathlib import Path

import pytest
import torch

import hypertrain.models.kimi_tiny as kt
from env_pin_fixtures import TEST_PIN
from hypertrain.trainer.model import Params, init_params

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "trainer"))
from test_golden_replay import XFAIL_REASON, _cpu  # noqa: E402

MOE = "layers.001."

# init_params(MICRO with this init_seed and init_std), see _digest. Seed 7 is MICRO as it is;
# seed 8 takes the init_std of PROFILE, so an init_std that init_params hard-codes cannot match
# both.
INIT_DIGEST = {
    (7, 0.02): "1d2b7e4508eb58516d407ec9a75d095359d725fc8d72cf850576fd7d0cb0e6c2",
    (8, 0.006): "6e6659015b32a378028256bbcde1ce61e7ee49e2215fb4d7e8844ef45edada80",
}

# One MICRO round per dtype (kt.qualify, 1 round). Each entry is one dispatch setting:
# first the AVX2-bound one CI uses (ATEN_CPU_CAPABILITY=avx2 MKL_CBWR=AVX2 ONEDNN_MAX_CPU_ISA=AVX2),
# then the default dispatch of the AMD EPYC 9575F dev host (avx512). A bf16 delta_hash is the same
# for both.
GOLDEN: dict[str, list[dict[str, str]]] = {
    "fp32": [
        {
            "leaves_root": "7f01e9f9f176f387e976d28f489725d2345441c3d89d5b3a4e52195e6b28c982",
            "delta_hash": "338b78bf95a737c9cae7702590e349159cdac38c6d03318502208ac86413485d",
            "state_root": "57eb8d10aeaae999ea57d5b056e2e099dfce4fd2ce35ecc6fee5bf9519614a47",
        },
        {
            "leaves_root": "826fb8143c1dbaa9c637c60fa8790359a0b0e72347756415893173afe622dcd7",
            "delta_hash": "3ac224d33be31f2dc362245ae3acc8ebb4037e3925853388d471381b68b01943",
            "state_root": "df75cec0452e640746e1801d7ec71b685451f780caeea9a03eee26435e1b1624",
        },
    ],
    "bf16": [
        {
            "leaves_root": "684efad67a62df69071f99c57f5c0219447cfcc01d31970c085157d1673b177e",
            "delta_hash": "794cbdfbaff8d1388ecfbd45b9d921475cd0b333d0fc2dab14bc1711ef5c6b0f",
            "state_root": "5d0e74db107501dfae4b3dcb584b14123bc4f0da4b19cddaadfd4c2350149e3c",
        },
        {
            "leaves_root": "b4465e3cd3494d08df36812a341664825654de7203324a4b5b94f2126754f4af",
            "delta_hash": "794cbdfbaff8d1388ecfbd45b9d921475cd0b333d0fc2dab14bc1711ef5c6b0f",
            "state_root": "43c0257efc958dd80151b955abcb5374577308f36377cd6b08a97bdf5cfdbe8b",
        },
    ],
}


def _digest(p: Params) -> str:
    """sha256 over (name, dtype, shape, little-endian float32 bytes) of the sorted tensors."""
    h = hashlib.sha256()
    for name in sorted(p):
        h.update(f"{name}|{p[name].dtype}|{tuple(p[name].shape)}|".encode())
        h.update(p[name].numpy().astype("<f4").tobytes())
    return h.hexdigest()


@pytest.mark.parametrize("seed,std", sorted(INIT_DIGEST))
def test_init_params_digest_is_pinned_per_seed_and_std(seed: int, std: float) -> None:
    cfg = dataclasses.replace(kt.MICRO, init_seed=seed, init_std=std)
    assert _digest(init_params(cfg)) == INIT_DIGEST[seed, std]


@pytest.mark.parametrize("dtype", ["fp32", "bf16"])
def test_micro_round_roots_unchanged(dtype: str, request: pytest.FixtureRequest) -> None:
    doc = kt.run_manifest(dataclasses.replace(kt.MICRO, compute_dtype=dtype))
    (res,) = kt.qualify(doc, "cpu", 1, TEST_PIN)
    roots = {k: res[k] for k in ("leaves_root", "delta_hash", "state_root")}
    if roots not in GOLDEN[dtype]:
        pytest.xfail(f"{XFAIL_REASON}; test={request.node.nodeid} roots={roots} cpu={_cpu()}")


@pytest.mark.parametrize("b,t", [(2, 16), (3, 7)], ids=["train-shape", "odd-shape"])
def test_dispatch_permutation_is_the_stable_argsort_of_the_expert_ids(
    b: int, t: int, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Rows keep token order inside one expert's chunk. That order feeds the GEMM, so it is part of
    the output bits (an unstable dispatch sort changes the MICRO round roots). The tie order of
    torch.sort(stable=False) is implementation-defined and differs from the stable one on CPU for
    these shapes, so the check needs no pinned root and cannot fail on correct code on any CPU."""
    cfg = kt.MICRO
    theta = init_params(cfg)
    x = torch.randn(b, t, cfg.d_model, generator=torch.Generator().manual_seed(3))
    routed: list[torch.Tensor] = []
    perms: list[torch.Tensor] = []
    topk_experts, permute_rows = kt.topk_experts, kt.permute_rows

    def spy_topk(scores: torch.Tensor, bias: torch.Tensor, k: int) -> torch.Tensor:
        routed.append(topk_experts(scores, bias, k))
        return routed[-1]

    def spy_permute(rows: torch.Tensor, perm: torch.Tensor) -> torch.Tensor:
        perms.append(perm)
        return permute_rows(rows, perm)

    monkeypatch.setattr(kt, "topk_experts", spy_topk)
    monkeypatch.setattr(kt, "permute_rows", spy_permute)
    with torch.no_grad():
        kt._moe(cfg, theta, MOE, x, torch.float32)
    experts = routed[0].tolist()
    assert perms[0].tolist() == sorted(range(len(experts)), key=lambda i: (experts[i], i))
