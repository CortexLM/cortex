"""All replicas of one sub-round as one stacked model: params [R, ...], one forward/backward.

Same math as trainer.loop._train_step + optim.step per replica (dense fp32 decoder, carried
inner state, grad_accum 1, AdamW or Muon), but every tensor carries a leading replica axis, so
a step costs one set of kernels for all R replicas and no host sync. Reduction shapes differ
from the per-replica path, so results agree within float rounding, not bitwise (equivalence:
experiments/sim/parity_equiv.py, docs/parity.md section 5). Opt-in: HYPERTRAIN_SIM_BATCHED=1.
"""

from __future__ import annotations

import math
from collections.abc import Sequence

import numpy as np
import torch
import torch.nn.functional as F
from torch import Tensor

from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.loop import Assignment, SampleFn, _load_batch
from hypertrain.trainer.model import _apply_rope, _rope, embed
from hypertrain.trainer.optim import OptState, lr_at, newton_schulz, uses_muon

Params = dict[str, Tensor]


def _rms(x: Tensor, w: Tensor) -> Tensor:
    return x * torch.rsqrt(x.pow(2).mean(-1, keepdim=True) + 1e-6) * w[:, None, None, :]


def _mm(x: Tensor, w: Tensor) -> Tensor:
    return x @ w[:, None]


def forward(cfg: TrainConfig, p: Params, tokens: Tensor) -> Tensor:
    """tokens int64 [R, B, T+1] -> mean CE per replica [R] (fp32)."""
    m = cfg.model
    R, B, _ = tokens.shape
    inp, tgt = tokens[:, :, :-1], tokens[:, :, 1:]
    T, d, h = inp.shape[2], m.d_model, m.n_heads
    hd = d // h
    off = (torch.arange(R, device=tokens.device) * m.vocab).view(R, 1, 1)
    x = embed(p["emb.weight"].reshape(R * m.vocab, d), inp + off)
    cos, sin = _rope(m, T, torch.float32, x.device)
    mask = torch.ones(T, T, dtype=torch.bool, device=x.device).triu(1)
    for i in range(m.n_layers):
        pre = f"layers.{i:03d}."
        a = _rms(x, p[pre + "attn_norm"])
        q, k, v = (
            _mm(a, p[pre + w]).view(R, B, T, h, hd).transpose(2, 3) for w in ("wq", "wk", "wv")
        )
        q, k = _apply_rope(q, cos, sin), _apply_rope(k, cos, sin)
        s = (q @ k.transpose(-1, -2)) / math.sqrt(hd)
        o = (s.masked_fill(mask, float("-inf")).softmax(-1) @ v).transpose(2, 3)
        x = x + _mm(o.reshape(R, B, T, d), p[pre + "wo"])
        a = _rms(x, p[pre + "mlp_norm"])
        f = F.silu(_mm(a, p[pre + "w1"])) * _mm(a, p[pre + "w3"])
        x = x + _mm(f, p[pre + "w2"])
    logits = _mm(_rms(x, p["norm.weight"]), p["head.weight"])
    ce = F.cross_entropy(logits.reshape(-1, m.vocab), tgt.reshape(-1), reduction="none")
    return ce.view(R, -1).mean(1)


@torch.no_grad()
def _step(cfg: TrainConfig, p: Params, g: Params, st: OptState, lr: float) -> None:
    c = cfg.inner
    norm = torch.zeros(next(iter(g.values())).shape[0], dtype=torch.float64, device=lr_dev(g))
    for n in sorted(g):
        norm += g[n].double().pow(2).flatten(1).sum(1)
    clip = torch.clamp(c.grad_clip / (norm.sqrt() + 1e-6), max=1.0).float()
    st.step += 1
    bc1, bc2 = 1.0 - c.beta1**st.step, 1.0 - c.beta2**st.step
    for n in sorted(p):
        w, gr = p[n], g[n] * clip.view(-1, *[1] * (g[n].ndim - 1))
        decay = c.wd if w.ndim - 1 >= 2 else 0.0
        if uses_muon(c, n):
            mo = st.m[n]
            mo.mul_(c.muon_momentum).add_(gr)
            o = newton_schulz(gr + c.muon_momentum * mo, c.ns_steps)
            scale = 0.2 * math.sqrt(max(w.shape[-2], w.shape[-1]))
            w.mul_(1.0 - lr * decay).sub_(o, alpha=lr * scale)
        else:
            mo, v = st.m[n], st.v[n]
            mo.mul_(c.beta1).add_(gr, alpha=1.0 - c.beta1)
            v.mul_(c.beta2).addcmul_(gr, gr, value=1.0 - c.beta2)
            w.mul_(1.0 - lr * decay).addcdiv_(mo, (v / bc2).sqrt().add_(c.eps), value=-lr / bc1)


def lr_dev(t: Params) -> torch.device:
    return next(iter(t.values())).device


def train_many(
    cfg: TrainConfig,
    starts: Sequence[Params],
    assigns: Sequence[Assignment],
    get: SampleFn,
    carries: Sequence[OptState],
) -> list[tuple[Params, OptState, float]]:
    """H inner steps for R replicas at once -> per replica (theta, carried state, last loss)."""
    c = cfg.inner
    if c.state_policy != "carry" or c.grad_accum != 1 or c.rewarmup_steps:
        raise ValueError("batched path supports carry, grad_accum 1, no re-warmup only")
    if cfg.model.is_moe or cfg.model.arch != "decoder" or cfg.model.compute_dtype != "fp32":
        raise ValueError("batched path supports the dense fp32 decoder only")
    if cfg.model.n_kv_heads != cfg.model.n_heads:
        raise ValueError("batched path assumes n_kv_heads == n_heads")
    if len({a.global_step0 for a in assigns}) != 1 or len({s.step for s in carries}) != 1:
        raise ValueError("replicas must share the schedule position")
    names = sorted(starts[0])
    dev = lr_dev(starts[0])
    p = {n: torch.stack([s[n] for s in starts]).clone() for n in names}
    st = OptState(
        {n: torch.stack([s.m[n] for s in carries]) for n in names},
        {n: torch.stack([s.v[n] for s in carries]) for n in carries[0].v},
        carries[0].step,
    )
    H, mb = c.H, c.micro_batch
    rows = torch.stack([_load_batch(cfg, a.sample_ids, get) for a in assigns])
    rows = rows.view(len(assigns), H, mb, -1).to(dev)
    ce = torch.zeros(len(assigns), device=dev)
    for t in range(1, H + 1):
        params = {n: p[n].detach().requires_grad_(True) for n in names}
        ce = forward(cfg, params, rows[:, t - 1])
        gs = torch.autograd.grad(ce.sum(), [params[n] for n in names])
        lr = lr_at(c, assigns[0].global_step0 + t - 1, t - 1)
        _step(cfg, p, dict(zip(names, gs, strict=True)), st, lr)
    losses = [float(np.float32(x)) for x in ce.detach().cpu().tolist()]
    out = []
    for r in range(len(assigns)):
        th = {n: p[n][r] for n in names}
        s = OptState({n: st.m[n][r] for n in names}, {n: st.v[n][r] for n in st.v}, st.step)
        out.append((th, s, losses[r]))
    return out
