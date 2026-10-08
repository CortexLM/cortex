"""Functional decoder LM (dense or MoE) over a flat ``{name: fp32 tensor}`` parameter dict.

MoE routing is deterministic: router top-k uses a stable descending sort (ties -> lowest expert
index), capacity drops follow (rank, token) order, and dispatch/combine are pure permutations
(custom autograd, gather in both directions) followed by a fixed-order sum over the k slots,
so no index_add/scatter_add is used anywhere.
"""

from __future__ import annotations

import math
from collections.abc import Callable

import torch
import torch.nn.functional as F
from torch import Tensor

from hypertrain.trainer.config import ModelConfig
from hypertrain.trainer.rng import rng_ctr, uniform_f64

Params = dict[str, Tensor]


def compute_dtype(cfg: ModelConfig) -> torch.dtype:
    return torch.bfloat16 if cfg.compute_dtype == "bf16" else torch.float32


def param_shapes(cfg: ModelConfig) -> dict[str, tuple[int, ...]]:
    d, hd = cfg.d_model, cfg.d_model // cfg.n_heads
    kv = cfg.n_kv_heads * hd
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
    if name.startswith("emb."):
        return 0
    if name.startswith("layers."):
        return int(name.split(".")[1]) * n_stages // cfg.n_layers
    return n_stages - 1


def init_params(cfg: ModelConfig) -> Params:
    """Uniform(-a, a), a = std*sqrt(3), Philox keyed by (init_seed, tensor index); norms = 1."""
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


def _rope(cfg: ModelConfig, t: int, dt: torch.dtype, device: torch.device) -> tuple[Tensor, Tensor]:
    """cos/sin tables computed on the host (f64) then moved: identical bytes on every device."""
    hd = cfg.d_model // cfg.n_heads
    cpu = torch.device("cpu")
    inv = 1.0 / (cfg.rope_theta ** (torch.arange(0, hd, 2, dtype=torch.float64, device=cpu) / hd))
    ang = torch.arange(t, dtype=torch.float64, device=cpu)[:, None] * inv[None, :]
    return ang.cos().to(dt).to(device), ang.sin().to(dt).to(device)


def _apply_rope(x: Tensor, cos: Tensor, sin: Tensor) -> Tensor:
    x1, x2 = x[..., 0::2], x[..., 1::2]
    return torch.stack((x1 * cos - x2 * sin, x1 * sin + x2 * cos), dim=-1).flatten(-2)


def _attn(cfg: ModelConfig, p: Params, pre: str, x: Tensor, dt: torch.dtype) -> Tensor:
    b, t, d = x.shape
    h, kvh = cfg.n_heads, cfg.n_kv_heads
    hd = d // h
    q = (x @ p[pre + "wq"].to(dt)).view(b, t, h, hd).transpose(1, 2)
    k = (x @ p[pre + "wk"].to(dt)).view(b, t, kvh, hd).transpose(1, 2)
    v = (x @ p[pre + "wv"].to(dt)).view(b, t, kvh, hd).transpose(1, 2)
    cos, sin = _rope(cfg, t, dt, x.device)
    q, k = _apply_rope(q, cos, sin), _apply_rope(k, cos, sin)
    if kvh != h:
        k = k[:, :, None].expand(b, kvh, h // kvh, t, hd).reshape(b, h, t, hd)
        v = v[:, :, None].expand(b, kvh, h // kvh, t, hd).reshape(b, h, t, hd)
    s = (q @ k.transpose(-1, -2)).float() / math.sqrt(hd)
    mask = torch.ones(t, t, dtype=torch.bool, device=x.device).triu(1)
    a = s.masked_fill(mask, float("-inf")).softmax(-1).to(dt)
    o = (a @ v).transpose(1, 2).reshape(b, t, d)
    return o @ p[pre + "wo"].to(dt)


def _swiglu(x: Tensor, w1: Tensor, w3: Tensor, w2: Tensor) -> Tensor:
    return (F.silu(x @ w1) * (x @ w3)) @ w2


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


MoeFn = Callable[[ModelConfig, Params, str, Tensor, torch.dtype], tuple[Tensor, Tensor]]


def forward(
    cfg: ModelConfig, p: Params, tokens: Tensor, moe: MoeFn | None = None
) -> tuple[Tensor, Tensor]:
    """tokens int64 [B, T+1] -> (mean CE loss fp32, aux loss fp32); ``moe`` swaps the MoE block."""
    moe_fn = moe or _moe
    dt = compute_dtype(cfg)
    inp, tgt = tokens[:, :-1], tokens[:, 1:]
    x = embed(p["emb.weight"], inp).to(dt)
    aux = torch.zeros((), dtype=torch.float32, device=tokens.device)
    for i in range(cfg.n_layers):
        pre = f"layers.{i:03d}."
        x = x + _attn(cfg, p, pre, _rms(x, p[pre + "attn_norm"]), dt)
        h = _rms(x, p[pre + "mlp_norm"])
        if cfg.is_moe:
            m, a = moe_fn(cfg, p, pre, h, dt)
            x, aux = x + m, aux + a
        else:
            x = x + _swiglu(h, p[pre + "w1"].to(dt), p[pre + "w3"].to(dt), p[pre + "w2"].to(dt))
    logits = (_rms(x, p["norm.weight"]) @ p["head.weight"].to(dt)).float()
    loss = F.cross_entropy(logits.reshape(-1, cfg.vocab), tgt.reshape(-1))
    return loss, aux
