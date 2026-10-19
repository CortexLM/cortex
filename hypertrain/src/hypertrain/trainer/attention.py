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

sdpa-flash is a chain, not one kernel: the flash kernel rejects fp32 and some head dims (and
enable_gqa narrows it further), so the first of PATHS whose kernel accepts the inputs is used. The
choice is probed once per (device, dtype, q heads per kv head, head dim), cached, and logged once;
it is a pure function of hardware, torch build and model geometry, so ranks and reruns on the
same hardware take the same path and stay bitwise equal.
"""

from __future__ import annotations

import logging
import math
import os
from typing import Literal

import torch
import torch.nn.functional as F
from torch import Tensor
from torch.nn.attention import SDPBackend, sdpa_kernel

log = logging.getLogger(__name__)

Backend = Literal["math", "sdpa-flash", "flash-attn"]
BACKENDS: tuple[Backend, ...] = ("sdpa-flash", "flash-attn", "math")
# (device capability, torch version) -> qualified backend. Filled only from spike evidence.
QUALIFIED_BACKENDS: dict[tuple[tuple[int, int], str], Backend] = {
    # RTX 5090: out/dq/dk/dv bitwise equal over 4 processes x 3 repeats, 8 ms vs 56 ms math at
    # b4 h16 kvh8 T2048 hd128 bf16 (tests/trainer/data/attn_qualify_sm120.json).
    ((12, 0), "2.14.0+cu130"): "sdpa-flash",
}

# sdpa-flash fallback order; "+expanded-kv" repeats K/V heads and calls SDPA without enable_gqa.
PATHS = ("flash", "mem-efficient", "flash+expanded-kv", "mem-efficient+expanded-kv", "math")
_KERNELS = {"flash": SDPBackend.FLASH_ATTENTION, "mem-efficient": SDPBackend.EFFICIENT_ATTENTION}
_NO_KERNEL = ("No available kernel", "No viable backend")  # CUDA / CPU dispatcher messages
_PATHS: dict[tuple[torch.device, torch.dtype, int, int], str] = {}
_REJECTED: set[tuple[str, torch.device, torch.dtype]] = set()  # real-call rejections, logged once

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


def _sdpa(path: str, q: Tensor, k: Tensor, v: Tensor) -> Tensor:
    kernel, _, expanded = path.partition("+")
    if expanded:
        g = q.shape[1] // k.shape[1]
        k, v = k.repeat_interleave(g, 1), v.repeat_interleave(g, 1)
    with sdpa_kernel([_KERNELS[kernel]]):
        return F.scaled_dot_product_attention(
            q, k, v, is_causal=True, enable_gqa=k.shape[1] != q.shape[1]
        )


@torch.compiler.assume_constant_result  # HT_COMPILE=1 traces the cached choice, not the probe
def _path(device: torch.device, dtype: torch.dtype, group: int, hd: int) -> str:
    """First of PATHS whose kernel accepts these inputs, probed once on tiny zero tensors."""
    key = (device, dtype, group, hd)
    if key in _PATHS:
        return _PATHS[key]
    path = "math"
    # Grad enabled: training constraints apply, and no_grad replay must pick the same path.
    # Identity hooks keep the probe's saved tensors out of an enclosing HT_ACT_CKPT checkpoint.
    with torch.enable_grad(), torch.autograd.graph.saved_tensors_hooks(lambda x: x, lambda x: x):
        q = torch.zeros(1, group, 2, hd, device=device, dtype=dtype, requires_grad=True)
        kv = torch.zeros(1, 1, 2, hd, device=device, dtype=dtype, requires_grad=True)
        for candidate in PATHS[:-1]:
            if group == 1 and candidate.endswith("+expanded-kv"):
                continue
            try:
                _sdpa(candidate, q, kv, kv)
            except RuntimeError as e:
                if not str(e).startswith(_NO_KERNEL):
                    raise
                continue
            path = candidate
            break
    _PATHS[key] = path
    log.warning("attention backend: %s (%s %s, group %d, hd %d)", path, device, dtype, group, hd)
    return path


def attention(q: Tensor, k: Tensor, v: Tensor, backend: Backend = "math") -> Tensor:
    """Causal attention. q [b, h, t, hd]; k, v [b, kvh, t, hd] with h % kvh == 0."""
    b, h, t, hd = q.shape
    kvh = k.shape[1]
    if backend == "sdpa-flash":
        path = _path(q.device, q.dtype, h // kvh, hd)
        if path != "math":
            try:
                return _sdpa(path, q, k, v)
            except RuntimeError as e:
                # The probe sees contiguous tiny tensors; this call's layout (e.g. a last-dim
                # stride != 1) can still be rejected. Not cached: the path stays a function of
                # the inputs, not of call order. ponytail: straight to math, not the next of
                # PATHS; walk PATHS if a production shape ever lands here.
                if not str(e).startswith(_NO_KERNEL):
                    raise
                if (path, q.device, q.dtype) not in _REJECTED:
                    _REJECTED.add((path, q.device, q.dtype))
                    log.warning("attention backend: %s rejected %s, using math", path, e)
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
