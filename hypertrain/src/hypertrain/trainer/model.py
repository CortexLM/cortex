"""Functional decoder LM (dense or MoE) over a flat ``{name: fp32 tensor}`` parameter dict.

MoE routing is deterministic: router top-k uses a stable descending sort (ties -> lowest expert
index), capacity drops follow (rank, token) order, and dispatch/combine are pure permutations
(custom autograd, gather in both directions) followed by a fixed-order sum over the k slots,
so no index_add/scatter_add is used anywhere.
"""

from __future__ import annotations

import functools
import importlib
import math
import os
from collections.abc import Callable
from types import ModuleType

import torch
import torch.nn.functional as F
from torch import Tensor
from torch.utils.checkpoint import checkpoint

from hypertrain.trainer.attention import attention, select_backend
from hypertrain.trainer.config import ModelConfig
from hypertrain.trainer.rng import rng_ctr, uniform_f64

Params = dict[str, Tensor]


# Model registry: arch name -> module exposing param_shapes/stage_of/init_params/forward.
ARCH_REGISTRY = {"od-encoder": "hypertrain.models.opendecision"}


def _arch_impl(cfg: ModelConfig) -> ModuleType:
    """Non-decoder architecture module from ARCH_REGISTRY (patch point for tests)."""
    if cfg.arch not in ARCH_REGISTRY:
        raise ValueError(f"unknown arch {cfg.arch!r}; registered: decoder, {sorted(ARCH_REGISTRY)}")
    return importlib.import_module(ARCH_REGISTRY[cfg.arch])


def compute_dtype(cfg: ModelConfig) -> torch.dtype:
    return torch.bfloat16 if cfg.compute_dtype == "bf16" else torch.float32


def param_shapes(cfg: ModelConfig) -> dict[str, tuple[int, ...]]:
    d, hd = cfg.d_model, cfg.d_model // cfg.n_heads
    kv = cfg.n_kv_heads * hd
    if cfg.arch != "decoder":
        return _arch_impl(cfg).param_shapes(cfg)
    shapes: dict[str, tuple[int, ...]] = {"emb.weight": (cfg.vocab, d)}
    for i in range(cfg.n_layers):
        p = f"layers.{i:03d}."
        shapes |= {
            p + "attn_norm": (d,),
            p + "wq": (d, d),
            p + "wk": (d, kv),
            p + "wv": (d, kv),
            p + "wo": (d, d),
            p + "mlp_norm": (d,),
        }
        if cfg.is_moe:
            e = cfg.n_experts
            shapes |= {
                p + "router": (d, e),
                p + "w1": (e, d, cfg.d_ff),
                p + "w3": (e, d, cfg.d_ff),
                p + "w2": (e, cfg.d_ff, d),
            }
        else:
            shapes |= {p + "w1": (d, cfg.d_ff), p + "w3": (d, cfg.d_ff), p + "w2": (cfg.d_ff, d)}
    shapes |= {"norm.weight": (d,), "head.weight": (d, cfg.vocab)}
    return shapes


def stage_of(name: str, cfg: ModelConfig, n_stages: int) -> int:
    if cfg.arch != "decoder":
        return _arch_impl(cfg).stage_of(name, cfg, n_stages)
    if name.startswith("emb."):
        return 0
    if name.startswith("layers."):
        return int(name.split(".")[1]) * n_stages // cfg.n_layers
    return n_stages - 1


def init_params(cfg: ModelConfig) -> Params:
    """Uniform(-a, a), a = std*sqrt(3), Philox keyed by (init_seed, tensor index); norms = 1."""
    if cfg.arch != "decoder":
        return _arch_impl(cfg).init_params(cfg)
    out: Params = {}
    a = cfg.init_std * math.sqrt(3.0)
    for idx, (name, shape) in enumerate(sorted(param_shapes(cfg).items())):
        if name.endswith("norm") or name.endswith("norm.weight"):
            out[name] = torch.ones(shape, dtype=torch.float32, device="cpu")
            continue
        n = math.prod(shape)
        u = uniform_f64(n, rng_ctr(f"init:{cfg.init_seed}", 0, 0, idx))
        out[name] = torch.from_numpy((u * 2.0 - 1.0) * a).to(torch.float32).reshape(shape)
    return out


class _Permute(torch.autograd.Function):
    @staticmethod
    def forward(ctx: torch.autograd.function.FunctionCtx, x: Tensor, perm: Tensor) -> Tensor:
        inv = torch.empty_like(perm)
        inv[perm] = torch.arange(perm.numel(), dtype=perm.dtype, device=perm.device)
        ctx.save_for_backward(inv)
        return x.index_select(0, perm)

    @staticmethod
    def backward(ctx: torch.autograd.function.FunctionCtx, grad: Tensor) -> tuple[Tensor, None]:
        (inv,) = ctx.saved_tensors  # type: ignore[attr-defined]
        return grad.index_select(0, inv), None


def permute_rows(x: Tensor, perm: Tensor) -> Tensor:
    out = _Permute.apply(x, perm)
    assert isinstance(out, Tensor)
    return out


def segment_sum_rows(values: Tensor, keys: Tensor, n_keys: int) -> Tensor:
    """out[k] = sum of values[i] with keys[i] == k, summed in ascending i (sorted segment-sum)."""
    order = torch.sort(keys, stable=True).indices
    sorted_keys = keys.index_select(0, order)
    uniq, counts = torch.unique_consecutive(sorted_keys, return_counts=True)
    sums = torch.segment_reduce(values.index_select(0, order), "sum", lengths=counts)
    out = torch.zeros((n_keys, *values.shape[1:]), dtype=values.dtype, device=values.device)
    out[uniq] = sums
    return out


class _Embed(torch.autograd.Function):
    @staticmethod
    def forward(ctx: torch.autograd.function.FunctionCtx, weight: Tensor, ids: Tensor) -> Tensor:
        ctx.save_for_backward(ids)
        ctx.n_rows = weight.shape[0]  # type: ignore[attr-defined]
        return weight.index_select(0, ids)

    @staticmethod
    def backward(ctx: torch.autograd.function.FunctionCtx, grad: Tensor) -> tuple[Tensor, None]:
        (ids,) = ctx.saved_tensors  # type: ignore[attr-defined]
        return segment_sum_rows(grad, ids, ctx.n_rows), None  # type: ignore[attr-defined]


def embed(weight: Tensor, ids: Tensor) -> Tensor:
    out = _Embed.apply(weight, ids.reshape(-1))
    assert isinstance(out, Tensor)
    return out.view(*ids.shape, weight.shape[1])


def _rms(x: Tensor, w: Tensor) -> Tensor:
    xf = x.float()
    y = xf * torch.rsqrt(xf.pow(2).mean(-1, keepdim=True) + 1e-6) * w
    return y.to(x.dtype)


@functools.lru_cache(maxsize=16)
def _rope(cfg: ModelConfig, t: int, dt: torch.dtype, device: torch.device) -> tuple[Tensor, Tensor]:
    """cos/sin tables computed on the host (f64) then moved: identical bytes on every device.
    Cached: the pageable host->device copy blocked the host once per layer and recompute."""
    hd = cfg.d_model // cfg.n_heads
    cpu = torch.device("cpu")
    inv = 1.0 / (cfg.rope_theta ** (torch.arange(0, hd, 2, dtype=torch.float64, device=cpu) / hd))
    ang = torch.arange(t, dtype=torch.float64, device=cpu)[:, None] * inv[None, :]
    return ang.cos().to(dt).to(device), ang.sin().to(dt).to(device)


def _apply_rope(x: Tensor, cos: Tensor, sin: Tensor) -> Tensor:
    x1, x2 = x[..., 0::2], x[..., 1::2]
    return torch.stack((x1 * cos - x2 * sin, x1 * sin + x2 * cos), dim=-1).flatten(-2)


E4M3_MAX = 448.0


def fp8_enabled(cfg: ModelConfig) -> bool:
    """S4 FP8 linear path: manifest precision_policy fp8-rowwise, or HT_FP8=1 for MFU A/B runs.
    OFF by default; changes numerics, so it is used for production only after GPU qualification
    (replay MATCH twice per (GPU, torch)). Falls back to the plain matmul (stub) when this torch
    has no float8 / _scaled_mm."""
    want = cfg.precision_policy == "fp8-rowwise" or os.environ.get("HT_FP8") == "1"
    return want and hasattr(torch, "float8_e4m3fn") and hasattr(torch, "_scaled_mm")


def _rowwise_fp8(x: Tensor, dim: int) -> tuple[Tensor, Tensor]:
    scale = (x.float().abs().amax(dim, keepdim=True) / E4M3_MAX).clamp(min=1e-12)
    return (x.float() / scale).to(torch.float8_e4m3fn), scale


class _Fp8Matmul(torch.autograd.Function):
    """x[M,K] @ w[K,N] with dynamic rowwise scaling (per x row, per w column) in e4m3 via
    torch._scaled_mm (the kernel torchao's float8 rowwise recipe dispatches to, so no torchao
    dependency). Backward in the compute dtype (high-precision grads, like torchao
    rowwise_with_gw_hp for grad_weight)."""

    @staticmethod
    def forward(ctx: torch.autograd.function.FunctionCtx, x: Tensor, w: Tensor) -> Tensor:
        ctx.save_for_backward(x, w)
        xq, sx = _rowwise_fp8(x, 1)
        wq, sw = _rowwise_fp8(w, 0)
        return torch._scaled_mm(
            xq, wq.t().contiguous().t(), scale_a=sx, scale_b=sw, out_dtype=x.dtype
        )

    @staticmethod
    def backward(ctx: torch.autograd.function.FunctionCtx, g: Tensor) -> tuple[Tensor, Tensor]:
        x, w = ctx.saved_tensors  # type: ignore[attr-defined]
        return g @ w.t(), x.t() @ g


def _mm(cfg: ModelConfig, x: Tensor, w: Tensor) -> Tensor:
    if not fp8_enabled(cfg):
        return x @ w
    out = _Fp8Matmul.apply(x.reshape(-1, x.shape[-1]), w)
    assert isinstance(out, Tensor)
    return out.view(*x.shape[:-1], w.shape[-1])


def _attn(cfg: ModelConfig, p: Params, pre: str, x: Tensor, dt: torch.dtype) -> Tensor:
    b, t, d = x.shape
    h, kvh = cfg.n_heads, cfg.n_kv_heads
    hd = d // h
    q = _mm(cfg, x, p[pre + "wq"].to(dt)).view(b, t, h, hd).transpose(1, 2)
    k = _mm(cfg, x, p[pre + "wk"].to(dt)).view(b, t, kvh, hd).transpose(1, 2)
    v = _mm(cfg, x, p[pre + "wv"].to(dt)).view(b, t, kvh, hd).transpose(1, 2)
    cos, sin = _rope(cfg, t, dt, x.device)
    q, k = _apply_rope(q, cos, sin), _apply_rope(k, cos, sin)
    o = attention(q, k, v, select_backend(x.device)).transpose(1, 2).reshape(b, t, d)
    return _mm(cfg, o, p[pre + "wo"].to(dt))


def _swiglu(
    x: Tensor, w1: Tensor, w3: Tensor, w2: Tensor, cfg: ModelConfig | None = None
) -> Tensor:
    if cfg is None or not fp8_enabled(cfg):
        return (F.silu(x @ w1) * (x @ w3)) @ w2
    return _mm(cfg, F.silu(_mm(cfg, x, w1)) * _mm(cfg, x, w3), w2)


def route(logits: Tensor, k: int, capacity: int) -> tuple[Tensor, Tensor]:
    """Return (experts [k*T] rank-major, keep [k*T] bool) for router logits [T, E]."""
    order = torch.sort(logits, dim=-1, descending=True, stable=True).indices[:, :k]
    experts = order.t().reshape(-1)
    seg = torch.sort(experts, stable=True).indices
    sorted_e = experts.index_select(0, seg)
    counts = torch.bincount(sorted_e, minlength=logits.shape[1])
    starts = torch.cumsum(counts, 0) - counts
    pos_sorted = torch.arange(sorted_e.numel(), device=sorted_e.device) - starts.index_select(
        0, sorted_e
    )
    keep = torch.empty_like(experts, dtype=torch.bool)
    keep[seg] = pos_sorted < capacity
    return experts, keep


def _moe(
    cfg: ModelConfig, p: Params, pre: str, x: Tensor, dt: torch.dtype
) -> tuple[Tensor, Tensor]:
    b, t, d = x.shape
    n, k, e = b * t, cfg.top_k_experts, cfg.n_experts
    flat = x.reshape(n, d)
    logits = flat.float() @ p[pre + "router"]
    capacity = math.ceil(cfg.capacity_factor * n * k / e)
    experts, keep = route(logits.detach(), k, capacity)
    onehot = F.one_hot(experts.view(k, n).t(), e).to(logits.dtype)
    sel = (onehot * logits[:, None, :]).sum(-1).softmax(-1).t().reshape(-1)
    weight = (sel * keep.to(sel.dtype)).to(dt)
    seg = torch.sort(experts, stable=True).indices
    rows = permute_rows(flat.repeat(k, 1), seg)
    counts = torch.bincount(experts, minlength=e).tolist()
    outs = [
        _swiglu(chunk, p[pre + "w1"][i].to(dt), p[pre + "w3"][i].to(dt), p[pre + "w2"][i].to(dt))
        for i, chunk in enumerate(torch.split(rows, counts))
    ]
    y = permute_rows(torch.cat(outs, 0), torch.argsort(seg, stable=True)) * weight[:, None]
    combined = y.view(k, n, d).sum(0)
    probs = logits.softmax(-1)
    frac = torch.bincount(experts[keep], minlength=e).float() / max(1, n * k)
    aux = e * (frac * probs.mean(0)).sum()
    return combined.view(b, t, d), aux


class _PadGather(torch.autograd.Function):
    """out = cat(x, 0-row)[idx]; idx is injective on real rows, so backward is the inverse gather
    cat(grad, 0-row)[inv] (no scatter_add, deterministic)."""

    @staticmethod
    def forward(
        ctx: torch.autograd.function.FunctionCtx, x: Tensor, idx: Tensor, inv: Tensor
    ) -> Tensor:
        ctx.save_for_backward(inv)
        return torch.cat([x, x.new_zeros(1, *x.shape[1:])]).index_select(0, idx)

    @staticmethod
    def backward(
        ctx: torch.autograd.function.FunctionCtx, grad: Tensor
    ) -> tuple[Tensor, None, None]:
        (inv,) = ctx.saved_tensors  # type: ignore[attr-defined]
        return (
            torch.cat([grad, grad.new_zeros(1, *grad.shape[1:])]).index_select(0, inv),
            None,
            None,
        )


def _pad_gather(x: Tensor, idx: Tensor, inv: Tensor) -> Tensor:
    out = _PadGather.apply(x, idx, inv)
    assert isinstance(out, Tensor)
    return out


def _moe_grouped(
    cfg: ModelConfig, p: Params, pre: str, x: Tensor, dt: torch.dtype
) -> tuple[Tensor, Tensor]:
    """Opt-in (HT_MOE_GROUPED=1) MoE: kept tokens packed into a static [E, capacity, d] buffer and
    run as three bmm grouped GEMMs; dropped tokens are never computed. No host sync (static
    shapes). Same routing, weights and aux as _moe; numerics differ from _moe only by GEMM
    blocking (repeatable, tolerance-tested), so it is not the replay reference.
    """
    b, t, d = x.shape
    n, k, e = b * t, cfg.top_k_experts, cfg.n_experts
    flat = x.reshape(n, d)
    logits = flat.float() @ p[pre + "router"]
    cap = math.ceil(cfg.capacity_factor * n * k / e)
    experts, keep = route(logits.detach(), k, cap)
    onehot = F.one_hot(experts.view(k, n).t(), e).to(logits.dtype)
    sel = (onehot * logits[:, None, :]).sum(-1).softmax(-1).t().reshape(-1)
    weight = (sel * keep.to(sel.dtype)).to(dt)
    seg = torch.sort(experts, stable=True).indices
    sorted_e = experts.index_select(0, seg)
    counts = torch.bincount(sorted_e, minlength=e)
    pos = torch.empty_like(experts)
    pos[seg] = torch.arange(n * k, device=x.device) - (torch.cumsum(counts, 0) - counts)[sorted_e]
    m = n * k
    slot = torch.where(keep, experts * cap + pos, e * cap)  # dropped -> pad slot
    src = torch.full((e * cap + 1,), m, dtype=torch.int64, device=x.device)
    src[slot] = torch.arange(m, device=x.device)  # pad slot entry overwritten below
    src = src[: e * cap]
    rows = flat.repeat(k, 1)
    buf = _pad_gather(rows, src, torch.where(keep, slot, e * cap)).view(e, cap, d)
    w1, w3, w2 = (p[pre + w].to(dt) for w in ("w1", "w3", "w2"))
    hid = F.silu(torch.bmm(buf, w1)) * torch.bmm(buf, w3)
    out = torch.bmm(hid, w2).view(e * cap, d)
    y = _pad_gather(out, torch.where(keep, slot, e * cap), src) * weight[:, None]
    combined = y.view(k, n, d).sum(0)
    probs = logits.softmax(-1)
    frac = torch.bincount(experts[keep], minlength=e).float() / max(1, n * k)
    aux = e * (frac * probs.mean(0)).sum()
    return combined.view(b, t, d), aux


MoeFn = Callable[[ModelConfig, Params, str, Tensor, torch.dtype], tuple[Tensor, Tensor]]
ForwardHook = Callable[[int, str, Tensor], Tensor]


def _block(
    cfg: ModelConfig, p: Params, pre: str, x: Tensor, dt: torch.dtype, moe_fn: MoeFn
) -> tuple[Tensor, Tensor]:
    """One decoder layer without trace hooks (same op order as the hooked path)."""
    x = x + _attn(cfg, p, pre, _rms(x, p[pre + "attn_norm"]), dt)
    h = _rms(x, p[pre + "mlp_norm"])
    if cfg.is_moe:
        m, a = moe_fn(cfg, p, pre, h, dt)
        return x + m, a
    m = _swiglu(h, p[pre + "w1"].to(dt), p[pre + "w3"].to(dt), p[pre + "w2"].to(dt), cfg)
    return x + m, torch.zeros((), dtype=torch.float32, device=x.device)


LAYER_KEYS = ("attn_norm", "wq", "wk", "wv", "wo", "mlp_norm", "w1", "w3", "w2")
_COMPILED: dict[tuple[ModelConfig, torch.dtype], Callable[..., Tensor]] = {}


def _compiled_layer(cfg: ModelConfig, dt: torch.dtype) -> Callable[..., Tensor]:
    """S3 (HT_COMPILE=1, dense only): one Inductor graph per (config, dtype), dynamic=False,
    taking (x, *layer weights) so all layers share it. Fusion changes rounding, so leaf roots
    differ from eager: it replays only under the same flag, recorded in run_mfu engine_flags,
    and is qualified per (GPU, torch) by two runs that MATCH. The single allowed compile site
    (tests/trainer/test_trainer_core.py::test_forbidden_constructs_absent)."""
    if (cfg, dt) not in _COMPILED:

        def layer(x: Tensor, *w: Tensor) -> Tensor:
            return _block(cfg, dict(zip(LAYER_KEYS, w, strict=True)), "", x, dt, _moe)[0]

        _COMPILED[(cfg, dt)] = torch.compile(layer, dynamic=False)
    return _COMPILED[(cfg, dt)]


def forward(
    cfg: ModelConfig,
    p: Params,
    tokens: Tensor,
    moe: MoeFn | None = None,
    hook: ForwardHook | None = None,
) -> tuple[Tensor, Tensor]:
    """tokens int64 [B, T+1] -> (mean CE loss fp32, aux loss fp32); ``moe`` swaps the MoE block."""
    if cfg.arch != "decoder":
        return _arch_impl(cfg).forward(cfg, p, tokens, moe=moe, hook=hook)

    def emit(layer: int, op: str, x: Tensor) -> Tensor:
        return x if hook is None else hook(layer, op, x)

    moe_fn = moe or (_moe_grouped if os.environ.get("HT_MOE_GROUPED") == "1" else _moe)
    dt = compute_dtype(cfg)
    inp, tgt = tokens[:, :-1], tokens[:, 1:]
    emit(-1, "input", tokens)
    x = emit(0, "embed", embed(p["emb.weight"], inp).to(dt))
    aux = torch.zeros((), dtype=torch.float32, device=tokens.device)
    for i in range(cfg.n_layers):
        pre = f"layers.{i:03d}."
        if hook is None and os.environ.get("HT_COMPILE") == "1" and not cfg.is_moe:
            fn = _compiled_layer(cfg, dt)
            w = [p[pre + k] for k in LAYER_KEYS]
            if os.environ.get("HT_ACT_CKPT") == "1" and torch.is_grad_enabled():
                x = checkpoint(fn, x, *w, use_reentrant=False)
            else:
                x = fn(x, *w)
            continue
        if hook is None:
            # HT_ACT_CKPT=1: recompute each layer in backward (same kernels, so bitwise equal).
            if os.environ.get("HT_ACT_CKPT") == "1" and torch.is_grad_enabled():
                x, a = checkpoint(_block, cfg, p, pre, x, dt, moe_fn, use_reentrant=False)
            else:
                x, a = _block(cfg, p, pre, x, dt, moe_fn)
            aux = aux + a
            continue
        h = emit(i, "attn_norm", _rms(x, p[pre + "attn_norm"]))
        attn = emit(i, "attn", _attn(cfg, p, pre, h, dt))
        x = emit(i, "attn_residual", x + attn)
        h = emit(i, "mlp_norm", _rms(x, p[pre + "mlp_norm"]))
        if cfg.is_moe:
            m, a = moe_fn(cfg, p, pre, h, dt)
            m = emit(i, "mlp", m)
            x, aux = emit(i, "mlp_residual", x + m), aux + a
        else:
            m = emit(
                i,
                "mlp",
                _swiglu(h, p[pre + "w1"].to(dt), p[pre + "w3"].to(dt), p[pre + "w2"].to(dt), cfg),
            )
            x = emit(i, "mlp_residual", x + m)
    norm = emit(cfg.n_layers, "final_norm", _rms(x, p["norm.weight"]))
    logits = emit(cfg.n_layers, "head", (norm @ p["head.weight"].to(dt)).float())
    loss = emit(
        cfg.n_layers, "loss", F.cross_entropy(logits.reshape(-1, cfg.vocab), tgt.reshape(-1))
    )
    return loss, aux
