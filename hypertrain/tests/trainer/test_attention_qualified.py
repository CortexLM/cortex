"""Qualified attention backend selection (evidence: tests/trainer/data/attn_qualify_sm120.json)."""

from __future__ import annotations

import hashlib
import os

import pytest

import hypertrain.trainer  # noqa: F401  (determinism before torch)

# isort: split
import torch

from hypertrain.trainer import attention as A


def _gpu(monkeypatch: pytest.MonkeyPatch, cap: tuple[int, int], ver: str) -> None:
    monkeypatch.delenv("HT_ATTN_BACKEND", raising=False)
    monkeypatch.setattr(torch.cuda, "get_device_capability", lambda d: cap)
    monkeypatch.setattr(torch, "__version__", ver)


def test_selector_picks_sdpa_flash_on_qualified_sm120(monkeypatch: pytest.MonkeyPatch) -> None:
    _gpu(monkeypatch, (12, 0), "2.14.0+cu130")
    assert A.select_backend(torch.device("cuda", 0)) == "sdpa-flash"


@pytest.mark.parametrize("cap,ver", [((12, 0), "2.14.1+cu130"), ((9, 0), "2.14.0+cu130")])
def test_selector_falls_back_to_math_when_unqualified(
    monkeypatch: pytest.MonkeyPatch, cap: tuple[int, int], ver: str
) -> None:
    _gpu(monkeypatch, cap, ver)
    assert A.select_backend(torch.device("cuda", 0)) == "math"


def test_selector_math_on_cpu_and_override_wins(monkeypatch: pytest.MonkeyPatch) -> None:
    _gpu(monkeypatch, (12, 0), "2.14.0+cu130")
    assert A.select_backend(torch.device("cpu")) == "math"
    monkeypatch.setenv("HT_ATTN_BACKEND", "math")
    assert A.select_backend(torch.device("cuda", 0)) == "math"


def test_evidence_matches_table() -> None:
    import json
    from pathlib import Path

    ev = json.loads((Path(__file__).parent / "data/attn_qualify_sm120.json").read_text())
    q = ev["qualified"]
    assert A.QUALIFIED_BACKENDS[(tuple(q["capability"]), q["torch"])] == q["backend"]
    assert all(s["backends"][q["backend"]]["cross_process_equal"] for s in ev["shapes"].values())


@pytest.mark.skipif("HT_GENUINE_CUDA_INDEX" not in os.environ, reason="needs a real GPU")
def test_qualified_backend_repeats_bitwise() -> None:
    dev = torch.device("cuda", int(os.environ["HT_GENUINE_CUDA_INDEX"]))
    g = torch.Generator(device=dev).manual_seed(0)
    q, k, v = (
        torch.randn(2, n, 256, 64, generator=g, device=dev, dtype=torch.bfloat16) for n in (8, 4, 4)
    )
    backend = A.select_backend(dev)
    digests = set()
    for _ in range(3):
        qq, kk, vv = (x.clone().requires_grad_() for x in (q, k, v))
        o = A.attention(qq, kk, vv, backend)
        grads = torch.autograd.grad(o.float().square().sum(), (qq, kk, vv))
        h = hashlib.sha256()
        for x in (o.detach(), *grads):
            h.update(x.contiguous().view(torch.uint8).cpu().numpy().tobytes())
        digests.add(h.hexdigest())
    assert len(digests) == 1
