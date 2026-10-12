"""Causal attention with native GQA and a pure backend selector (plan 1.1).

Backends, tried in this order by the GPU spike (scripts/attention_spike.py):
  sdpa-flash : torch SDPA flash kernel with enable_gqa (no K/V expansion)
  flash-attn : flash_attn_func(..., deterministic=True), only if the package is installed
  math       : the reference path of model._attn (CPU, fallback, and the bitwise oracle)
A backend is used for training only once it is listed in QUALIFIED_BACKENDS for the device and
torch version; until the spike qualifies one on RTX 5090 the selector returns "math", so
production numerics are unchanged. The selected name is recorded by callers for replay.
"""

from __future__ import annotations

import math
from typing import Literal

import torch
import torch.nn.functional as F
from torch import Tensor

Backend = Literal["math", "sdpa-flash", "flash-attn"]
BACKENDS: tuple[Backend, ...] = ("sdpa-flash", "flash-attn", "math")
# (device capability, torch version) -> qualified backend. Filled only from spike evidence.
QUALIFIED_BACKENDS: dict[tuple[tuple[int, int], str], Backend] = {}

_MASKS: dict[tuple[int, torch.device], Tensor] = {}


def _causal_mask(t: int, device: torch.device) -> Tensor:
    key = (t, device)
    if key not in _MASKS:
        _MASKS[key] = torch.ones(t, t, dtype=torch.bool, device=device).triu(1)
    return _MASKS[key]


def select_backend(device: torch.device) -> Backend:
    """Pure function of (device capability, torch version, qualification table)."""
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
