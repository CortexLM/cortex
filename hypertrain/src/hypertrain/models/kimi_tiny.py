"""``kimi-k3-pro-tiny``: Kimi K2-family (DeepSeek-V3-like) test architecture (ADR 0006).

Architecture only: random init, no weights, no tokenizer. Multi-head Latent Attention with a
decoupled RoPE key, one dense first layer, then MoE layers with one shared expert and sigmoid
top-k routing balanced by a selection-only bias (auxiliary-loss-free, DeepSeek-V3 sign rule) plus
the DeepSeek-V3 sequence-wise balance loss as ``aux``.

Deterministic like ``trainer.model``: top-k is ``route`` (stable descending sort, ties -> lowest
expert index) with capacity = token count (no drops), dispatch/combine are ``permute_rows``
permutations followed by a fixed-order k-sum, attention is the eager math path.

The bias ``layers.NNN.router_bias`` is a parameter-dict tensor, so leaves, deltas and replay bind
it. Forward reads it detached: its gradient is None and AdamW leaves the 1-D zero-moment tensor
bitwise unchanged. The backward of ``_BalanceTap`` applies ``b += gamma * sign(mean_load - load)``
in place, so a batch is routed with the bias from before its own load (one update per backward
pass = per micro-batch). Single device only: the island trainer is refused (its EP ``moe``), and
``trainer.unit_order`` with world > 1 would update a per-rank bias (no load all-reduce).
"""

from __future__ import annotations

import hypertrain.trainer  # noqa: F401  (pins determinism before torch loads)

# isort: split
import argparse
import dataclasses
import functools
import hashlib
import json
import math
from collections.abc import Callable
from dataclasses import dataclass
from typing import Any

import numpy as np
import numpy.typing as npt
import torch
import torch.nn.functional as F
from torch import Tensor

from hypertrain.protocol.jcs import canonicalize
from hypertrain.trainer.config import CompressConfig, InnerConfig, ModelConfig, TrainConfig
from hypertrain.trainer.model import (
    Params,
    _apply_rope,
    _rms,
    _swiglu,
    compute_dtype,
    embed,
    permute_rows,
    route,
)
from hypertrain.trainer.rng import rng_ctr, uniform_f64

ARCH = "kimi-k3-pro-tiny"
N_SHARED = 1  # Kimi K2 n_shared_experts [A config.json]
FIRST_DENSE = 1  # Kimi K2 first_k_dense_replace [A config.json]
ROUTED_SCALE = 2.827  # Kimi K2 routed_scaling_factor [A config.json]
BIAS_GAMMA = 1e-3  # DeepSeek-V3 bias update speed [A arXiv 2412.19437]
TEST_RUN_KIND = "hypertrain-test-run-v1"
Hook = Callable[[int, str, Tensor], Tensor]

# Profile shape: ~77M parameters (1-2 GPUs), 32 routed experts top-4 + 1 shared.
PROFILE = ModelConfig(
    n_layers=6,
    d_model=512,
    n_heads=8,
    n_kv_heads=8,
    d_ff=128,
    vocab=32768,
    seq_len=512,
    n_experts=32,
    top_k_experts=4,
    capacity_factor=1.0,
    rope_theta=50000.0,
    init_std=0.006,
    aux_loss_coef=1e-4,
    compute_dtype="bf16",
    arch=ARCH,
)
# CPU test shape of the same architecture; never evidence about PROFILE.
MICRO = dataclasses.replace(
    PROFILE,
    n_layers=3,
    d_model=32,
    n_heads=4,
    n_kv_heads=4,
    d_ff=16,
    vocab=64,
    seq_len=16,
    n_experts=8,
    top_k_experts=2,
    init_seed=7,
    init_std=0.02,
    compute_dtype="fp32",
)


@dataclass(frozen=True)
class Dims:
    """MLA/MoE widths derived from ModelConfig with the Kimi K2 ratios per head dim (ADR 0006)."""

    head: int  # qk_nope_head_dim = v_head_dim (K2 128)
    rope: int  # qk_rope_head_dim = head / 2 (K2 64)
    q_rank: int  # q_lora_rank = 12 * head (K2 1536)
    kv_rank: int  # kv_lora_rank = 4 * head (K2 512)
    dense_ff: int  # first-layer FFN = (top_k + shared) * expert ff (K2 18432 = 9 * 2048)


def dims(cfg: ModelConfig) -> Dims:
    head = cfg.d_model // cfg.n_heads
    if cfg.arch != ARCH:
        raise ValueError(f"not a {ARCH} config")
    if cfg.n_kv_heads != cfg.n_heads or head % 4:
        raise ValueError("MLA needs n_kv_heads == n_heads and a head dim divisible by 4")
    if not cfg.is_moe or cfg.n_layers <= FIRST_DENSE:
        raise ValueError(f"{ARCH} needs n_experts > 1 and n_layers > {FIRST_DENSE}")
    return Dims(head, head // 2, 12 * head, 4 * head, (cfg.top_k_experts + N_SHARED) * cfg.d_ff)


def param_shapes(cfg: ModelConfig) -> dict[str, tuple[int, ...]]:
    dm, h, e, ff = dims(cfg), cfg.n_heads, cfg.n_experts, cfg.d_ff
    d = cfg.d_model
    shapes: dict[str, tuple[int, ...]] = {"emb.weight": (cfg.vocab, d)}
    for i in range(cfg.n_layers):
        p = f"layers.{i:03d}."
        shapes |= {
            p + "attn_norm": (d,),
            p + "wq_a": (d, dm.q_rank),
            p + "q_norm": (dm.q_rank,),
            p + "wq_b": (dm.q_rank, h * (dm.head + dm.rope)),
            p + "wkv_a": (d, dm.kv_rank + dm.rope),
            p + "kv_norm": (dm.kv_rank,),
            p + "wkv_b": (dm.kv_rank, h * 2 * dm.head),
            p + "wo": (h * dm.head, d),
            p + "mlp_norm": (d,),
        }
        if i < FIRST_DENSE:
            shapes |= {p + "w1": (d, dm.dense_ff), p + "w3": (d, dm.dense_ff)}
            shapes |= {p + "w2": (dm.dense_ff, d)}
            continue
        shapes |= {
            p + "router": (d, e),
            p + "router_bias": (e,),
            p + "w1": (e, d, ff),
            p + "w3": (e, d, ff),
            p + "w2": (e, ff, d),
            p + "shared_w1": (d, N_SHARED * ff),
            p + "shared_w3": (d, N_SHARED * ff),
            p + "shared_w2": (N_SHARED * ff, d),
        }
    shapes |= {"norm.weight": (d,), "head.weight": (d, cfg.vocab)}
    return shapes


def stage_of(name: str, cfg: ModelConfig, n_stages: int) -> int:
    if name.startswith("emb."):
        return 0
    if name.startswith("layers."):
        return int(name.split(".")[1]) * n_stages // cfg.n_layers
    return n_stages - 1


def init_params(cfg: ModelConfig) -> Params:
    """Decoder scheme: Philox uniform(+-std*sqrt(3)) per sorted tensor index; norms 1, bias 0."""
    out: Params = {}
    a = cfg.init_std * math.sqrt(3.0)
    for idx, (name, shape) in enumerate(sorted(param_shapes(cfg).items())):
        if name.endswith("norm") or name.endswith("norm.weight"):
            out[name] = torch.ones(shape, dtype=torch.float32)
            continue
        if name.endswith("router_bias"):
            out[name] = torch.zeros(shape, dtype=torch.float32)
            continue
        u = uniform_f64(math.prod(shape), rng_ctr(f"init:{cfg.init_seed}", 0, 0, idx))
        out[name] = torch.from_numpy((u * 2.0 - 1.0) * a).to(torch.float32).reshape(shape)
    return out


@functools.lru_cache(maxsize=16)
def _rope(
    theta: float, dim: int, t: int, dt: torch.dtype, device: torch.device
) -> tuple[Tensor, Tensor]:
    """cos/sin for the decoupled rope part, computed on the host in f64 (same bytes per device)."""
    cpu = torch.device("cpu")
    inv = 1.0 / (theta ** (torch.arange(0, dim, 2, dtype=torch.float64, device=cpu) / dim))
    ang = torch.arange(t, dtype=torch.float64, device=cpu)[:, None] * inv[None, :]
    return ang.cos().to(dt).to(device), ang.sin().to(dt).to(device)


def _mla(cfg: ModelConfig, p: Params, pre: str, x: Tensor, dt: torch.dtype) -> Tensor:
    """DeepSeek-V2/V3 MLA: low-rank q and joint kv latents, one shared rope key per token."""
    dm, (b, t, _), h = dims(cfg), x.shape, cfg.n_heads
    q = _rms(x @ p[pre + "wq_a"].to(dt), p[pre + "q_norm"]) @ p[pre + "wq_b"].to(dt)
    q_nope, q_pe = q.view(b, t, h, dm.head + dm.rope).transpose(1, 2).split((dm.head, dm.rope), -1)
    c_kv, k_pe = (x @ p[pre + "wkv_a"].to(dt)).split((dm.kv_rank, dm.rope), -1)
    kv = _rms(c_kv, p[pre + "kv_norm"]) @ p[pre + "wkv_b"].to(dt)
    k_nope, v = kv.view(b, t, h, 2 * dm.head).transpose(1, 2).split((dm.head, dm.head), -1)
    cos, sin = _rope(cfg.rope_theta, dm.rope, t, dt, x.device)
    q = torch.cat((q_nope, _apply_rope(q_pe, cos, sin)), -1)
    k_pe = _apply_rope(k_pe[:, None], cos, sin).expand(b, h, t, dm.rope)
    k = torch.cat((k_nope, k_pe), -1)
    s = (q @ k.transpose(-1, -2)).float() * (dm.head + dm.rope) ** -0.5
    mask = torch.ones(t, t, dtype=torch.bool, device=x.device).triu(1)
    o = s.masked_fill(mask, float("-inf")).softmax(-1).to(dt) @ v
    return o.transpose(1, 2).reshape(b, t, h * dm.head) @ p[pre + "wo"].to(dt)


class _BalanceTap(torch.autograd.Function):
    """Identity on router logits; its backward applies the DeepSeek-V3 bias rule in place."""

    @staticmethod
    def forward(
        ctx: torch.autograd.function.FunctionCtx, logits: Tensor, bias: Tensor, load: Tensor
    ) -> Tensor:
        # Plain attributes, not save_for_backward: the bias is mutated after this forward.
        ctx.bias, ctx.load = bias, load  # type: ignore[attr-defined]
        return logits.clone()

    @staticmethod
    def backward(
        ctx: torch.autograd.function.FunctionCtx, grad: Tensor
    ) -> tuple[Tensor, None, None]:
        bias: Tensor = ctx.bias  # type: ignore[attr-defined]
        load = ctx.load.to(torch.float32)  # type: ignore[attr-defined]
        with torch.no_grad():
            bias.add_(torch.sign(load.mean() - load), alpha=BIAS_GAMMA)
        return grad, None, None


def topk_experts(scores: Tensor, bias: Tensor, k: int) -> Tensor:
    """[k*T] rank-major experts: top-k of scores + bias per token (stable sort, ties -> lowest
    index); capacity = token count, so nothing is dropped."""
    return route(scores.detach() + bias.detach(), k, scores.shape[0])[0]


def gate(scores: Tensor, experts: Tensor, k: int) -> Tensor:
    """[T, k] UNBIASED selected scores, normalised, times ROUTED_SCALE (K2 noaux_tc)."""
    n, e = scores.shape
    onehot = F.one_hot(experts.view(k, n).t(), e).to(scores.dtype)
    picked = (onehot * scores[:, None, :]).sum(-1)
    return picked / picked.sum(-1, keepdim=True) * ROUTED_SCALE


def _moe(
    cfg: ModelConfig, p: Params, pre: str, x: Tensor, dt: torch.dtype
) -> tuple[Tensor, Tensor]:
    b, t, d = x.shape
    n, k, e = b * t, cfg.top_k_experts, cfg.n_experts
    flat = x.reshape(n, d)
    logits = flat.float() @ p[pre + "router"]
    bias = p[pre + "router_bias"]
    experts = topk_experts(torch.sigmoid(logits.detach()), bias, k)
    load = torch.bincount(experts, minlength=e)
    scores = torch.sigmoid(_BalanceTap.apply(logits, bias, load))
    weight = gate(scores, experts, k).t().reshape(-1).to(dt)
    seg = torch.sort(experts, stable=True).indices
    rows = permute_rows(flat.repeat(k, 1), seg)
    w1, w3, w2 = (p[pre + w].to(dt) for w in ("w1", "w3", "w2"))
    outs = [
        _swiglu(chunk, w1[i], w3[i], w2[i])
        for i, chunk in enumerate(torch.split(rows, load.tolist()))
    ]
    y = permute_rows(torch.cat(outs, 0), torch.argsort(seg, stable=True)) * weight[:, None]
    routed = y.view(k, n, d).sum(0)
    s1, s3, s2 = (p[pre + "shared_" + w].to(dt) for w in ("w1", "w3", "w2"))
    shared = _swiglu(flat, s1, s3, s2)
    # DeepSeek-V3 sequence-wise balance loss: mean over sequences of sum_i f_i * P_i.
    seq = torch.arange(n, device=x.device).repeat(k) // t
    hits = torch.bincount(seq * e + experts, minlength=b * e).view(b, e).float()
    prob = (scores / scores.sum(-1, keepdim=True)).view(b, t, e).mean(1)
    aux = (hits * (e / (k * t)) * prob).sum(-1).mean()
    return (routed + shared).view(b, t, d), aux


def _run(cfg: ModelConfig, p: Params, tokens: Tensor, hook: Hook) -> tuple[Tensor, Tensor]:
    dt = compute_dtype(cfg)
    inp, tgt = tokens[:, :-1], tokens[:, 1:]
    hook(-1, "input", tokens)
    x = hook(0, "embed", embed(p["emb.weight"], inp).to(dt))
    aux = torch.zeros((), dtype=torch.float32, device=tokens.device)
    for i in range(cfg.n_layers):
        pre = f"layers.{i:03d}."
        h = hook(i, "attn_norm", _rms(x, p[pre + "attn_norm"]))
        x = hook(i, "attn_residual", x + hook(i, "attn", _mla(cfg, p, pre, h, dt)))
        h = hook(i, "mlp_norm", _rms(x, p[pre + "mlp_norm"]))
        if i < FIRST_DENSE:
            m = _swiglu(h, p[pre + "w1"].to(dt), p[pre + "w3"].to(dt), p[pre + "w2"].to(dt))
        else:
            m, a = _moe(cfg, p, pre, h, dt)
            aux = aux + a
        x = hook(i, "mlp_residual", x + hook(i, "mlp", m))
    norm = hook(cfg.n_layers, "final_norm", _rms(x, p["norm.weight"]))
    logits = hook(cfg.n_layers, "head", (norm @ p["head.weight"].to(dt)).float())
    loss = F.cross_entropy(logits.reshape(-1, cfg.vocab), tgt.reshape(-1))
    return hook(cfg.n_layers, "loss", loss), aux


def _identity(layer: int, op: str, x: Tensor) -> Tensor:
    return x


def forward(
    cfg: ModelConfig,
    p: Params,
    tokens: Tensor,
    moe: Callable[..., tuple[Tensor, Tensor]] | None = None,
    hook: Hook | None = None,
) -> tuple[Tensor, Tensor]:
    """tokens int64 [B, T+1] -> (mean CE fp32, sequence-wise balance loss fp32)."""
    if moe is not None:
        raise ValueError(f"{ARCH} has its own MoE; a moe override (island EP) is unsupported")
    return _run(cfg, p, tokens, hook or _identity)


def traced_forward(
    cfg: TrainConfig, p: Params, tokens: Tensor, hook: Hook
) -> tuple[Tensor, Tensor]:
    """Dispute-replay entry point (auditor.bisect): the same eager ``_run`` as training."""
    return _run(cfg.model, p, tokens, hook)


def run_manifest(model: ModelConfig = PROFILE) -> dict[str, Any]:
    """Unsigned TEST run document for this profile. Not a protocol RunManifest: ModelSpec.arch is
    a frozen protocol Literal (ADR 0001), so no network run can carry it yet (ADR 0006)."""
    fields = dataclasses.asdict(model)
    if (fields.pop("od"), fields.pop("profile"), fields.pop("precision_policy")) != (
        None,
        "",
        "default",
    ):
        raise ValueError(f"{ARCH} uses no OD spec, reference profile or precision policy")
    return {
        "kind": TEST_RUN_KIND,
        "model": fields,
        "inner": {"lr": 3e-3, "H": 8, "J": 4, "micro_batch": 2, "warmup": 2},
        "compress": {"codec": "dense-int8"},
    }


def run_manifest_id(doc: dict[str, Any]) -> str:
    return hashlib.sha256(canonicalize(doc)).hexdigest()


def train_config(doc: dict[str, Any]) -> TrainConfig:
    """Strict parse of a TEST run document (exact keys, this arch, single stage and device)."""
    if set(doc) != {"kind", "model", "inner", "compress"} or doc["kind"] != TEST_RUN_KIND:
        raise ValueError("not a hypertrain TEST run document")
    model = ModelConfig(**doc["model"])
    dims(model)
    inner = InnerConfig(**doc["inner"])
    if inner.grad_accum != 1:  # the bias rule runs per backward pass = per step only at ga 1
        raise ValueError(f"{ARCH} TEST runs need grad_accum == 1")
    return TrainConfig(model, inner, CompressConfig(**doc["compress"]))


def synthetic_sample(cfg: TrainConfig) -> Callable[[int], npt.NDArray[np.uint32]]:
    """Philox token ids: the profile needs no tokenizer or dataset download."""

    def get(i: int) -> npt.NDArray[np.uint32]:
        u = uniform_f64(cfg.model.seq_len + 1, rng_ctr("kimi-tiny-data", 0, i, 0))
        return np.floor(u * cfg.model.vocab).astype(np.uint32)

    return get


def qualify(doc: dict[str, Any], device: str, rounds: int) -> list[dict[str, str]]:
    """Run ``rounds`` identical rounds on ``device``; the B300 qualification compares the hashes
    within a host (bitwise) and across hosts."""
    from hypertrain.trainer.loop import Assignment, train_round

    cfg = train_config(doc)
    run_id = run_manifest_id(doc)
    n = cfg.inner.H * cfg.inner.micro_batch * cfg.inner.grad_accum
    a = Assignment(run_id=run_id, w=0, sample_ids=tuple(range(n)))
    out = []
    for _ in range(rounds):
        theta = {k: v.to(device) for k, v in init_params(cfg.model).items()}
        res = train_round(cfg, theta, a, synthetic_sample(cfg))
        out.append(
            {
                "run_id": run_id,
                "leaves_root": res.leaves_root,
                "delta_hash": res.delta_hash,
                "state_root": res.final_theta_hash,
            }
        )
    return out


def main() -> None:
    ap = argparse.ArgumentParser(description=f"{ARCH} TEST-run qualification (no key, no network)")
    ap.add_argument("--device", default="cuda")
    ap.add_argument("--rounds", type=int, default=2)
    ap.add_argument("--dtype", choices=("bf16", "fp32"), default=PROFILE.compute_dtype)
    args = ap.parse_args()
    doc = run_manifest(dataclasses.replace(PROFILE, compute_dtype=args.dtype))
    results = qualify(doc, args.device, args.rounds)
    info: dict[str, Any] = {"device": args.device, "dtype": args.dtype, "torch": torch.__version__}
    if torch.device(args.device).type == "cuda":  # B300 = capability [10, 3] (sm_103)
        i = torch.cuda.current_device()
        info |= {
            "gpu": torch.cuda.get_device_name(i),
            "capability": list(torch.cuda.get_device_capability(i)),
            "arch_list": torch.cuda.get_arch_list(),
        }
    print(json.dumps({**info, "rounds": results}, indent=1))
    if len({json.dumps(r, sort_keys=True) for r in results}) != 1:
        raise SystemExit("MISMATCH: rounds differ on this host")


if __name__ == "__main__":
    main()
