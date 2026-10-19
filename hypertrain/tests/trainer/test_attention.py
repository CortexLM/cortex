"""Attention backend: CPU math path bitwise equal to the pre-1.1 inline implementation."""

from __future__ import annotations

import math
import os
from collections.abc import Callable
from typing import Any

import pytest

import hypertrain.trainer  # noqa: F401  (determinism before torch)

# isort: split
import torch

from hypertrain.trainer import attention as A
from hypertrain.trainer.attention import Backend, attention, select_backend


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


@pytest.mark.parametrize("dt", [torch.float32, torch.bfloat16])
@pytest.mark.parametrize("h,kvh", [(4, 4), (8, 2), (8, 1)])
def test_sdpa_flash_repeatable_and_close_to_math(dt: torch.dtype, h: int, kvh: int) -> None:
    g = torch.Generator().manual_seed(h * 7 + kvh)
    q, k, v = (
        torch.randn(2, n, 33, 16, generator=g).to(dt).requires_grad_() for n in (h, kvh, kvh)
    )
    runs = []
    backends: tuple[Backend, ...] = ("sdpa-flash", "sdpa-flash", "math")
    for backend in backends:
        o = attention(q, k, v, backend)
        runs.append((o.detach(), *torch.autograd.grad(o.float().sum(), (q, k, v))))
    assert all(torch.equal(a, b) for a, b in zip(runs[0], runs[1], strict=True))
    tol = 2e-2 if dt == torch.bfloat16 else 1e-5
    assert (runs[0][0].float() - runs[2][0].float()).abs().max() < tol  # plan rule: forward


def test_env_override(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("HT_ATTN_BACKEND", "sdpa-flash")
    assert select_backend(torch.device("cpu")) == "sdpa-flash"
    monkeypatch.setenv("HT_ATTN_BACKEND", "bogus")
    with pytest.raises(ValueError):
        select_backend(torch.device("cpu"))


@pytest.mark.skipif("HT_GENUINE_CUDA_INDEX" not in os.environ, reason="needs a real GPU")
def test_backward_bitwise() -> None:
    from scripts.attention_spike import spike

    assert spike(torch.device("cuda", int(os.environ["HT_GENUINE_CUDA_INDEX"])))["chosen"]


def _sdpa_raising(when: Callable[[bool], bool], calls: list[bool]) -> Callable[..., torch.Tensor]:
    """F.scaled_dot_product_attention raising the RTX 5090 error when when(enable_gqa)."""
    real = A.F.scaled_dot_product_attention

    def sdpa(q: torch.Tensor, k: torch.Tensor, v: torch.Tensor, **kw: Any) -> torch.Tensor:
        calls.append(kw["enable_gqa"])
        if when(kw["enable_gqa"]):
            raise RuntimeError("No available kernel. Aborting execution.")
        return real(q, k, v, **kw)

    return sdpa


@pytest.mark.parametrize(
    "when,chosen",
    [(lambda gqa: gqa, "flash+expanded-kv"), (lambda gqa: True, "math")],  # CPU: no mem-efficient
)
def test_fallback_probed_once_when_native_kernel_missing(
    monkeypatch: pytest.MonkeyPatch,
    caplog: pytest.LogCaptureFixture,
    when: Callable[[bool], bool],
    chosen: str,
) -> None:
    calls: list[bool] = []
    monkeypatch.setattr(A, "_PATHS", {})
    monkeypatch.setattr(A.F, "scaled_dot_product_attention", _sdpa_raising(when, calls))
    g = torch.Generator().manual_seed(0)
    q, k, v = (torch.randn(2, n, 33, 16, generator=g) for n in (8, 2, 2))
    with caplog.at_level("WARNING", logger=A.__name__):
        first = attention(q, k, v, "sdpa-flash")
        probed = list(calls)
        second = attention(q, k, v, "sdpa-flash")
    assert probed[0] and len(probed) > 1  # native enable_gqa tried first, then fell back
    assert calls[len(probed) :] == ([] if chosen == "math" else [False])  # no re-probe
    assert [r.getMessage().split()[2] for r in caplog.records] == [chosen]  # logged once
    assert torch.equal(first, second)
    assert (first - attention(q, k, v)).abs().max() < 1e-5


@pytest.mark.parametrize("dt", [torch.float32, torch.bfloat16])
@pytest.mark.parametrize("h,kvh", [(8, 2), (8, 1)])
def test_expanded_kv_close_to_native_gqa(dt: torch.dtype, h: int, kvh: int) -> None:
    g = torch.Generator().manual_seed(h * 7 + kvh)
    q, k, v = (torch.randn(2, n, 33, 16, generator=g).to(dt) for n in (h, kvh, kvh))
    native = A._sdpa("flash", q, k, v)
    expanded = A._sdpa("flash+expanded-kv", q, k, v)
    tol = 2e-2 if dt == torch.bfloat16 else 1e-5
    assert (native.float() - expanded.float()).abs().max() < tol


def test_layout_rejected_after_probe_falls_back_to_math(
    monkeypatch: pytest.MonkeyPatch, caplog: pytest.LogCaptureFixture
) -> None:
    monkeypatch.setattr(A, "_REJECTED", set())
    g = torch.Generator().manual_seed(0)
    q, k = (torch.randn(2, n, 33, 16, generator=g) for n in (8, 2))
    v = torch.randn(2, 2, 16, 33, generator=g).transpose(-1, -2)  # stride(-1) != 1: no fused kernel
    with caplog.at_level("WARNING", logger=A.__name__):
        assert A._path(q.device, q.dtype, 4, 16) == "flash"  # the contiguous probe accepts flash
        for _ in range(2):
            assert torch.equal(attention(q, k, v, "sdpa-flash"), attention(q, k, v))
        contiguous = attention(q, k, v.contiguous(), "sdpa-flash")
    assert torch.equal(contiguous, A._sdpa("flash", q, k, v.contiguous()))  # rejection not cached
    assert sum("rejected" in r.getMessage() for r in caplog.records) == 1
