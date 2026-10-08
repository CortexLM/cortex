"""Inner optimizers (AdamW, Muon) on fp32 master params, explicit fixed-order math, no torch.optim.

State policies at round start:
  reset   -> m = v = 0, step = 0 (bias correction restarts)
  derived -> m = 0, v = public v0 (see derive_v0); v bias correction disabled
  carry   -> m, v, step taken from the miner's previous round end state
"""

from __future__ import annotations

import math
from dataclasses import dataclass, field

import torch
from torch import Tensor

from hypertrain.trainer.config import InnerConfig

Params = dict[str, Tensor]
MUON_SUFFIXES = (".wq", ".wk", ".wv", ".wo", ".w1", ".w2", ".w3")
NS_COEFFS = (3.4445, -4.7750, 2.0315)


def uses_muon(cfg: InnerConfig, name: str) -> bool:
    return cfg.opt == "muon" and name.endswith(MUON_SUFFIXES)


@dataclass
class OptState:
    m: Params = field(default_factory=dict)
    v: Params = field(default_factory=dict)
    step: int = 0

    def clone(self) -> OptState:
        return OptState(
            {k: t.clone() for k, t in self.m.items()},
            {k: t.clone() for k, t in self.v.items()},
            self.step,
        )


def init_state(
    cfg: InnerConfig,
    params: Params,
    carry: OptState | None = None,
    v0: Params | None = None,
) -> OptState:
    names = sorted(params)
    adam_names = [n for n in names if not uses_muon(cfg, n)]
    if cfg.state_policy == "carry":
        if carry is None:
            raise ValueError("state_policy carry requires the previous round state")
        if sorted(carry.m) != names or sorted(carry.v) != adam_names:
            raise ValueError("carried state does not match the parameter set")
        return carry.clone()
    if carry is not None:
        raise ValueError(f"state_policy {cfg.state_policy} must not receive carried state")
    m = {n: torch.zeros_like(params[n]) for n in names}
    if cfg.state_policy == "derived":
        if v0 is None or sorted(v0) != adam_names:
            raise ValueError("state_policy derived requires public v0 for every AdamW tensor")
        v = {n: v0[n].clone() for n in adam_names}
    else:
        if v0 is not None:
            raise ValueError("v0 is only valid with state_policy derived")
        v = {n: torch.zeros_like(params[n]) for n in adam_names}
    return OptState(m, v, 0)


def derive_v0(outer_grad: Params, H: int, block: int = 256) -> Params:
    """v0 = blockwise mean of (g/H)^2, broadcast back over each block (coordinator-computed)."""
    out: Params = {}
    for name in sorted(outer_grad):
        g = outer_grad[name].float().reshape(-1) / H
        n = g.numel()
        pad = torch.zeros((n + block - 1) // block * block, dtype=torch.float32, device=g.device)
        pad[:n] = g * g
        cnt = torch.full((pad.numel() // block,), float(block), device=g.device)
        cnt[-1] = float(n - (cnt.numel() - 1) * block)
        means = pad.view(-1, block).sum(1) / cnt
        out[name] = means.repeat_interleave(block)[:n].reshape(outer_grad[name].shape)
    return out


def lr_at(cfg: InnerConfig, global_step: int, round_step: int) -> float:
    """WSD schedule on the global step times the per-round re-warmup ramp (round_step 0-based)."""
    s = global_step
    if s < cfg.warmup:
        base = (s + 1) / cfg.warmup
    elif s < cfg.warmup + cfg.stable:
        base = 1.0
    elif cfg.decay and s < cfg.warmup + cfg.stable + cfg.decay:
        base = 1.0 - (s - cfg.warmup - cfg.stable) / cfg.decay
    else:
        base = 0.0 if cfg.decay else 1.0
    if cfg.rewarmup_steps and round_step < cfg.rewarmup_steps:
        base *= (round_step + 1) / cfg.rewarmup_steps
    return cfg.lr * base


def newton_schulz(g: Tensor, steps: int) -> Tensor:
    """Quintic Newton-Schulz orthogonalization over the last two dims, fp32."""
    a, b, c = NS_COEFFS
    x = g.float()
    transposed = x.shape[-2] > x.shape[-1]
    if transposed:
        x = x.transpose(-1, -2)
    x = x / (torch.linalg.matrix_norm(x, keepdim=True) + 1e-7)
    for _ in range(steps):
        aa = x @ x.transpose(-1, -2)
        x = a * x + (b * aa + c * (aa @ aa)) @ x
    return x.transpose(-1, -2) if transposed else x


def global_grad_norm(grads: Params) -> float:
    total = 0.0
    for name in sorted(grads):
        total += float(grads[name].double().pow(2).sum())
    return math.sqrt(total)


@torch.no_grad()
def step(
    cfg: InnerConfig,
    params: Params,
    grads: Params,
    st: OptState,
    lr: float,
    norm: float | None = None,
) -> None:
    """Update ``params`` in place; ``norm`` overrides the clip norm (sharded island layouts)."""
    if norm is None:
        norm = global_grad_norm(grads)
    clip = min(1.0, cfg.grad_clip / (norm + 1e-6))
    st.step += 1
    bc1 = 1.0 - cfg.beta1**st.step
    bc2 = 1.0 if cfg.state_policy == "derived" else 1.0 - cfg.beta2**st.step
    for name in sorted(params):
        p, g = params[name], grads[name] * clip
        decay = cfg.wd if p.ndim >= 2 else 0.0
        if uses_muon(cfg, name):
            m = st.m[name]
            m.mul_(cfg.muon_momentum).add_(g)
            u = g + cfg.muon_momentum * m
            o = newton_schulz(u, cfg.ns_steps)
            scale = 0.2 * math.sqrt(max(p.shape[-2], p.shape[-1]))
            p.mul_(1.0 - lr * decay).sub_(o, alpha=lr * scale)
        else:
            m, v = st.m[name], st.v[name]
            m.mul_(cfg.beta1).add_(g, alpha=1.0 - cfg.beta1)
            v.mul_(cfg.beta2).addcmul_(g, g, value=1.0 - cfg.beta2)
            denom = (v / bc2).sqrt().add_(cfg.eps)
            p.mul_(1.0 - lr * decay).addcdiv_(m, denom, value=-lr / bc1)
