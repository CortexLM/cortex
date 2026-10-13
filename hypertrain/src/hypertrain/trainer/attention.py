"""Causal attention with native GQA and a pure backend selector (plan 1.1).

Backends, tried in this order by the GPU spike (scripts/attention_spike.py):
  sdpa-flash : torch SDPA flash kernel with enable_gqa (no K/V expansion)
  flash-attn : flash_attn_func(..., deterministic=True), only if the package is installed
  math       : the reference path of model._attn (CPU, fallback, and the bitwise oracle)
A backend is used for training only once it is listed in QUALIFIED_BACKENDS for the device and
torch version; until the spike qualifies one on RTX 5090 the selector returns "math", so
production numerics are unchanged. The selected name is recorded by callers for replay.

HT_ATTN_BACKEND=<name> forces a backend (MFU A/B runs and the spike); the run then replays only
under the same override, which run_mfu.py records next to its leaf root.
The math path keeps the explicit K/V expansion: a broadcast (native-GQA) math path differs from it
by 1 ulp in dK for some shapes, so it cannot be the bitwise oracle. Native GQA lives in the fused
backends (enable_gqa / flash-attn MQA), which never materialise expanded K/V.
"""

from __future__ import annotations

import math
import os
from typing import Literal

import torch
import torch.nn.functional as F
from torch import Tensor

Backend = Literal["math", "sdpa-flash", "flash-attn"]
BACKENDS: tuple[Backend, ...] = ("sdpa-flash", "flash-attn", "math")
# (device capability, torch version) -> qualified backend. Filled only from spike evidence.
QUALIFIED_BACKENDS: dict[tuple[tuple[int, int], str], Backend] = {
    # RTX 5090: out/dq/dk/dv bitwise equal over 4 processes x 3 repeats, 8 ms vs 56 ms math at
    # b4 h16 kvh8 T2048 hd128 bf16 (tests/trainer/data/attn_qualify_sm120.json).
    ((12, 0), "2.14.0+cu130"): "sdpa-flash",
}

_MASKS: dict[tuple[int, torch.device], Tensor] = {}


def _causal_mask(t: int, device: torch.device) -> Tensor:
    key = (t, device)
    if key not in _MASKS:
        _MASKS[key] = torch.ones(t, t, dtype=torch.bool, device=device).triu(1)
    return _MASKS[key]


def select_backend(device: torch.device) -> Backend:
    """Pure function of (HT_ATTN_BACKEND, device capability, torch version, qualification)."""
    forced = os.environ.get("HT_ATTN_BACKEND")
    if forced:
        if forced not in BACKENDS:
            raise ValueError(f"HT_ATTN_BACKEND must be one of {BACKENDS}")
        return forced
    if device.type != "cuda":
        return "math"
    cap = torch.cuda.get_device_capability(device)
    return QUALIFIED_BACKENDS.get((cap, torch.__version__), "math")


def attention(q: Tensor, k: Tensor, v: Tensor, backend: Backend = "math") -> Tensor:
    """Causal attention. q [b, h, t, hd]; k, v [b, kvh, t, hd] with h % kvh == 0."""
    b, h, t, hd = q.shape
    kvh = k.shape[1]
    if backend == "sdpa-flash":
        from torch.nn.attention import SDPBackend, sdpa_kernel

        with sdpa_kernel([SDPBackend.FLASH_ATTENTION]):
            return F.scaled_dot_product_attention(q, k, v, is_causal=True, enable_gqa=kvh != h)
    if backend == "flash-attn":
        from flash_attn import flash_attn_func  # type: ignore[import-not-found]

        o = flash_attn_func(
            q.transpose(1, 2), k.transpose(1, 2), v.transpose(1, 2), causal=True, deterministic=True
        )
        return o.transpose(1, 2)
    if kvh != h:
        k = k[:, :, None].expand(b, kvh, h // kvh, t, hd).reshape(b, h, t, hd)
        v = v[:, :, None].expand(b, kvh, h // kvh, t, hd).reshape(b, h, t, hd)
    s = (q @ k.transpose(-1, -2)).float() / math.sqrt(hd)
    a = s.masked_fill(_causal_mask(t, q.device), float("-inf")).softmax(-1).to(q.dtype)
    return a @ v
