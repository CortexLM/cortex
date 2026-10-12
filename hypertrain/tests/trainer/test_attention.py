"""Attention backend: CPU math path bitwise equal to the pre-1.1 inline implementation."""

from __future__ import annotations

import math
import os

import pytest

import hypertrain.trainer  # noqa: F401  (determinism before torch)

# isort: split
import torch

from hypertrain.trainer.attention import attention, select_backend


def _old(q, k, v):
    b, h, t, hd = q.shape
    kvh = k.shape[1]
    if kvh != h:
        k = k[:, :, None].expand(b, kvh, h // kvh, t, hd).reshape(b, h, t, hd)
        v = v[:, :, None].expand(b, kvh, h // kvh, t, hd).reshape(b, h, t, hd)
    s = (q @ k.transpose(-1, -2)).float() / math.sqrt(hd)
    mask = torch.ones(t, t, dtype=torch.bool).triu(1)
    return s.masked_fill(mask, float("-inf")).softmax(-1).to(q.dtype) @ v


@pytest.mark.parametrize("t", [1, 17, 2048])
@pytest.mark.parametrize("h,kvh", [(4, 4), (4, 2), (8, 1)])
def test_math_path_bitwise_equal_to_previous_when_fp32(t: int, h: int, kvh: int) -> None:
    g = torch.Generator().manual_seed(t * 31 + h + kvh)
    q = torch.randn(1, h, t, 16, generator=g, requires_grad=True)
    k = torch.randn(1, kvh, t, 16, generator=g, requires_grad=True)
    v = torch.randn(1, kvh, t, 16, generator=g, requires_grad=True)
    new, old = attention(q, k, v), _old(q, k, v)
    assert torch.equal(new, old)
    gn = torch.autograd.grad(new.sum(), (q, k, v))
    go = torch.autograd.grad(old.sum(), (q, k, v))
    assert all(torch.equal(a, b) for a, b in zip(gn, go, strict=True))


def test_selector_returns_math_on_cpu() -> None:
    assert select_backend(torch.device("cpu")) == "math"


@pytest.mark.skipif("HT_GENUINE_CUDA_INDEX" not in os.environ, reason="needs a real GPU")
def test_backward_bitwise() -> None:
    from scripts.attention_spike import spike

    assert spike(torch.device("cuda", int(os.environ["HT_GENUINE_CUDA_INDEX"])))["chosen"]
