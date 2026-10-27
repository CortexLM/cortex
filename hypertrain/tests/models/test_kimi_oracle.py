"""S8 K1 / ADR 0006: kimi-k3-pro-tiny against per-token float64 oracles.

test_kimi_tiny.py compares ``_moe`` with itself (additivity, a uniform bias shift) and checks
gradients only for being non-zero. Here the routed MoE, its gradients, the MLA gradients, the
composed forward (residual wiring, norms, head, LM target, balance loss) and the compute dtype are
compared with references that share no code with kimi_tiny: Python loops over tokens, experts,
sequences and heads in float64.

fp32 code against a float64 reference needs a tolerance, and both must select the same experts:
every routing case asserts that the k-th and (k+1)-th biased scores of each token are more than
GAP apart, so a pinned seed cannot sit on a near tie.
"""

from __future__ import annotations

import dataclasses

import pytest
import torch
import torch.nn.functional as F
from torch import Tensor

import hypertrain.models.kimi_tiny as kt
from hypertrain.trainer.config import ModelConfig
from hypertrain.trainer.model import forward, init_params

MOE = "layers.001."
MOE_WEIGHTS = ("router", "w1", "w3", "w2", "shared_w1", "shared_w3", "shared_w2")
MLA_WEIGHTS = ("wq_a", "q_norm", "wq_b", "wkv_a", "kv_norm", "wkv_b", "wo")
K2_ROUTED_SCALE = 2.827  # Kimi K2 config.json routed_scaling_factor, not kt.ROUTED_SCALE
K2_FIRST_DENSE = 1  # Kimi K2 config.json first_k_dense_replace, not kt.FIRST_DENSE
GAP = 1e-3  # an fp32 score is off by ~1e-7, so no selection can flip across this margin
# Error budget, relative to the largest reference entry. Pristine fp32 against float64 measures
# <= 6e-7 on every single-block comparison (6-7 chained ops over 16..96 terms, one fp32 ulp is
# 6e-8) and <= 1.7e-6 on the gradients of the whole model (a few hundred chained ops) on the
# default, AVX2 and AVX-512 CPU kernels, so 1e-5 leaves 6x for another summation order.
# Mutants of the gate, combine, shared expert, router tap and LM target move some comparison by
# 4e-2 or more.
TOL = 1e-5


def _mismatch(name: str, got: Tensor | None, want: Tensor | None) -> str | None:
    """A message when the fp32 ``got`` leaves the budget around the float64 ``want``."""
    if want is None:  # autograd found no path in the reference: there must be none in the code
        return None if got is None else f"{name}: a gradient reaches a tensor it does not depend on"
    scale = float(want.detach().abs().max())
    if not scale > 0.0:
        return f"{name}: the reference is identically zero, nothing to compare"
    if got is None:  # autograd found no path: the gradient the reference has is missing
        return f"{name}: no gradient reaches this tensor"
    err = float((got.detach().double() - want.detach()).abs().max()) / scale
    return None if err <= TOL else f"{name}: max|code - reference| / max|reference| = {err:.1e}"


def _assert_clear_selection(scores: Tensor, bias: Tensor, k: int) -> None:
    top = (scores + bias).sort(-1, descending=True).values
    gap = float((top[:, k - 1] - top[:, k]).min())
    assert gap > GAP, f"near tie ({gap:.1e}): fp32 and float64 could select different experts"


def _rms64(v: Tensor, w: Tensor) -> Tensor:
    return v * torch.rsqrt(v.pow(2).mean(-1, keepdim=True) + 1e-6) * w


def _swiglu64(v: Tensor, w1: Tensor, w3: Tensor, w2: Tensor) -> Tensor:
    a = v @ w1
    return (a * torch.sigmoid(a) * (v @ w3)) @ w2


def _ref_moe(
    cfg: ModelConfig, p: dict[str, Tensor], x: Tensor, bias: Tensor
) -> tuple[Tensor, Tensor]:
    """One token at a time. ``p`` holds the float64 weights, ``bias`` only steers the selection."""
    b, t, d = x.shape
    e, k = cfg.n_experts, cfg.top_k_experts
    flat = x.reshape(b * t, d)
    s = torch.sigmoid(flat @ p["router"])
    _assert_clear_selection(s.detach(), bias, k)
    hits = torch.zeros(b, e, dtype=torch.float64)
    rows = []
    for tok in range(b * t):
        biased = (s[tok].detach() + bias).tolist()
        sel = sorted(range(e), key=lambda j: (-biased[j], j))[:k]  # ties -> lowest index
        gate = s[tok, sel] / s[tok, sel].sum() * K2_ROUTED_SCALE  # unbiased scores
        y = _swiglu64(flat[tok], p["shared_w1"], p["shared_w3"], p["shared_w2"])
        for rank, j in enumerate(sel):
            # f_i counts the biased selection that routed the token, as the code does (S8 K5:
            # DeepSeek-V3 eq. 18 counts the unbiased top-k; this line moves with that decision).
            hits[tok // t, j] += 1
            y = y + gate[rank] * _swiglu64(flat[tok], p["w1"][j], p["w3"][j], p["w2"][j])
        rows.append(y)
    prob = (s / s.sum(-1, keepdim=True)).view(b, t, e).mean(1)
    aux = (hits * (e / (k * t)) * prob).sum(-1).mean()
    return torch.stack(rows).view(b, t, d), aux


def _moe_case(
    cfg: ModelConfig, seed: int, b: int, t: int, starved: tuple[int, ...]
) -> tuple[dict[str, Tensor], Tensor, Tensor]:
    """Weights at O(1) scale (init_std 0.02 makes every output ~1e-3 and hides errors) and ``b``
    sequences of ``t`` tokens, so the sequence axis, the token axis and the rank-major combine all
    matter. A ``starved`` expert gets a bias that keeps every token away from it."""
    d, e, ff = cfg.d_model, cfg.n_experts, cfg.d_ff
    g = torch.Generator().manual_seed(seed)

    def rnd(*shape: int, std: float) -> Tensor:
        return torch.randn(*shape, generator=g) * std

    params = {
        "router": rnd(d, e, std=0.25),
        "router_bias": rnd(e, std=0.06),
        "w1": rnd(e, d, ff, std=0.3),
        "w3": rnd(e, d, ff, std=0.3),
        "w2": rnd(e, ff, d, std=0.3),
        "shared_w1": rnd(d, ff, std=0.3),
        "shared_w3": rnd(d, ff, std=0.3),
        "shared_w2": rnd(ff, d, std=0.3),
    }
    params["router_bias"][list(starved)] = -1.0
    return params, rnd(b, t, d, std=1.0), rnd(b, t, d, std=1.0)


# PROFILE's routing width (32 experts, top 4) at MICRO's d_model and d_ff.
WIDE = dataclasses.replace(kt.MICRO, n_experts=32, top_k_experts=4)
# Each case pins its seed to a draw that satisfies the routing preconditions asserted below.
MOE_CASES = [
    pytest.param(kt.MICRO, 388, 3, 5, (), id="micro-k2-e8"),
    pytest.param(kt.MICRO, 9, 3, 5, (2, 7), id="micro-k2-e8-experts-2-and-7-empty"),
    pytest.param(WIDE, 15073, 3, 16, (), id="wide-k4-e32"),
]


@pytest.mark.parametrize("cfg, seed, b, t, starved", MOE_CASES)
def test_moe_matches_the_per_token_float64_reference(
    cfg: ModelConfig, seed: int, b: int, t: int, starved: tuple[int, ...]
) -> None:
    k = cfg.top_k_experts
    params, x0, probe = _moe_case(cfg, seed, b, t, starved)
    bias = params["router_bias"].double()
    scores = torch.sigmoid(x0.reshape(-1, cfg.d_model).double() @ params["router"].double())
    plain = scores.sort(-1, descending=True).indices[:, :k].sort(-1).values
    steered = (scores + bias).sort(-1, descending=True).indices[:, :k].sort(-1).values
    assert bool((plain != steered).any()), "the router bias steers no token"
    load = torch.bincount(steered.flatten(), minlength=cfg.n_experts).tolist()
    empty = tuple(j for j, n in enumerate(load) if n == 0)
    # A wrong weight index for an expert that gets no token changes nothing: a case declares the
    # experts it leaves empty on purpose and every other expert must receive a token.
    assert empty == starved, f"experts without a token: {empty}, expected {starved} (load {load})"

    # _BalanceTap moves the bias in place on every backward: each pass gets fresh clones.
    def fresh() -> tuple[dict[str, Tensor], Tensor]:
        p = {MOE + n: v.clone().requires_grad_() for n, v in params.items()}
        return p, x0.clone().requires_grad_()

    names = list(MOE_WEIGHTS)
    p, x = fresh()
    out, aux = kt._moe(cfg, p, MOE, x, torch.float32)
    grads = torch.autograd.grad(
        (out * probe).sum(), [p[MOE + n] for n in names] + [x], allow_unused=True
    )
    got = {"out": out, "aux": aux} | {
        f"d {n}": g for n, g in zip(names + ["x"], grads, strict=True)
    }
    p, x = fresh()
    aux_only = kt._moe(cfg, p, MOE, x, torch.float32)[1]
    assert aux_only.requires_grad, "aux is detached from the router"
    (got["d aux / d router"],) = torch.autograd.grad(
        aux_only, [p[MOE + "router"]], allow_unused=True
    )

    r = {n: params[n].double().requires_grad_() for n in names}
    rx = x0.double().requires_grad_()
    r_out, r_aux = _ref_moe(cfg, r, rx, bias)
    r_grads = torch.autograd.grad(
        (r_out * probe.double()).sum(), [r[n] for n in names] + [rx], retain_graph=True
    )
    want = {"out": r_out, "aux": r_aux} | {
        f"d {n}": g for n, g in zip(names + ["x"], r_grads, strict=True)
    }
    (want["d aux / d router"],) = torch.autograd.grad(r_aux, [r["router"]])

    bad = [m for n, w in want.items() if (m := _mismatch(n, got[n], w))]
    assert not bad, "\n".join(bad)


def _tokens(cfg: ModelConfig, b: int) -> Tensor:
    g = torch.Generator().manual_seed(1)
    return torch.randint(0, cfg.vocab, (b, cfg.seq_len + 1), generator=g)


COMPUTE_DTYPES = {"bf16": torch.bfloat16, "fp32": torch.float32}


@pytest.mark.parametrize("name", COMPUTE_DTYPES)
def test_activations_follow_the_compute_dtype(name: str) -> None:
    dtype = COMPUTE_DTYPES[name]
    cfg = dataclasses.replace(kt.MICRO, compute_dtype=name)
    seen: dict[str, list[torch.dtype]] = {"attn": [], "mlp": []}

    def hook(layer: int, op: str, v: Tensor) -> Tensor:
        if op in seen:
            seen[op].append(v.dtype)
            assert v.dtype == dtype, f"layer {layer} {op} is {v.dtype}"
        return v

    with torch.no_grad():
        loss, aux = forward(cfg, init_params(cfg), _tokens(cfg, 2), hook=hook)
    assert seen == {"attn": [dtype] * cfg.n_layers, "mlp": [dtype] * cfg.n_layers}
    assert loss.dtype == aux.dtype == torch.float32  # the losses stay fp32 under a bf16 config


def _ref_mla(cfg: ModelConfig, p: dict[str, Tensor], x: Tensor) -> Tensor:
    """DeepSeek-V3 MLA one head at a time in float64; the widths come from the weight shapes."""
    b, t, _ = x.shape
    h, kv_rank = cfg.n_heads, p["wkv_b"].shape[0]
    rope = p["wkv_a"].shape[1] - kv_rank
    nope = p["wkv_b"].shape[1] // (2 * h)

    freq = cfg.rope_theta ** -(torch.arange(0, rope, 2, dtype=torch.float64) / rope)
    turn = torch.polar(
        torch.ones(t, rope // 2, dtype=torch.float64), torch.outer(torch.arange(t), freq)
    )

    def rotate(v: Tensor) -> Tensor:  # [b, t, rope] read as interleaved (even, odd) complex pairs
        pairs = torch.view_as_complex(v.reshape(b, t, rope // 2, 2).contiguous())
        return torch.view_as_real(pairs * turn).reshape(b, t, rope)

    q = (_rms64(x @ p["wq_a"], p["q_norm"]) @ p["wq_b"]).view(b, t, h, nope + rope)
    latent = x @ p["wkv_a"]
    kv = (_rms64(latent[..., :kv_rank], p["kv_norm"]) @ p["wkv_b"]).view(b, t, h, 2 * nope)
    k_pe = rotate(latent[..., kv_rank:])  # one rope key per token, shared by every head
    future = torch.ones(t, t, dtype=torch.bool).triu(1)
    heads = []
    for i in range(h):
        score = q[:, :, i, :nope] @ kv[:, :, i, :nope].transpose(-1, -2)
        score = score + rotate(q[:, :, i, nope:]) @ k_pe.transpose(-1, -2)
        score = (score * (nope + rope) ** -0.5).masked_fill(future, float("-inf"))
        heads.append(score.softmax(-1) @ kv[:, :, i, nope:])
    return torch.cat(heads, -1) @ p["wo"]


def test_mla_gradients_match_the_float64_reference() -> None:
    cfg = dataclasses.replace(kt.MICRO, rope_theta=10.0, init_std=0.1)  # strong rotation
    g = torch.Generator().manual_seed(5)
    theta = init_params(cfg)
    params = {n: theta[MOE + n] for n in MLA_WEIGHTS}
    for n in ("q_norm", "kv_norm"):  # all ones at init: a swapped norm weight would pass unseen
        params[n] = 1 + 0.3 * torch.randn(params[n].shape, generator=g)
    x0 = torch.randn(2, 6, cfg.d_model, generator=g)
    probe = torch.randn(2, 6, cfg.d_model, generator=g)

    p = {MOE + n: v.clone().requires_grad_() for n, v in params.items()}
    x = x0.clone().requires_grad_()
    out = kt._mla(cfg, p, MOE, x, torch.float32)
    got = torch.autograd.grad(
        (out * probe).sum(), [p[MOE + n] for n in MLA_WEIGHTS] + [x], allow_unused=True
    )

    r = {n: v.double().requires_grad_() for n, v in params.items()}
    rx = x0.double().requires_grad_()
    want = torch.autograd.grad(
        (_ref_mla(cfg, r, rx) * probe.double()).sum(), [r[n] for n in MLA_WEIGHTS] + [rx]
    )
    bad = [
        m
        for n, a, b in zip([*MLA_WEIGHTS, "x"], got, want, strict=True)
        if (m := _mismatch(f"d {n}", a, b))
    ]
    assert not bad, "\n".join(bad)


def _model_weights(cfg: ModelConfig, seed: int) -> dict[str, Tensor]:
    """Every tensor is drawn here, not by init_params, so an init change cannot move this test.
    Weights at 1/sqrt(fan_in) keep the activations O(1) (init_std 0.02 makes every output ~1e-3 and
    hides errors); norm weights sit away from 1 so a swapped or ignored norm is visible; the router
    bias is not zero."""
    g = torch.Generator().manual_seed(seed)
    theta = {}
    for name, shape in kt.param_shapes(cfg).items():
        draw = torch.randn(shape, generator=g)
        if name.endswith(("norm", "norm.weight")):
            theta[name] = 1 + 0.3 * draw
        elif name.endswith("router_bias"):
            theta[name] = 0.06 * draw
        elif name == "emb.weight":
            theta[name] = draw
        else:
            theta[name] = draw * shape[-2] ** -0.5
    return theta


def _ref_forward(cfg: ModelConfig, p: dict[str, Tensor], tokens: Tensor) -> tuple[Tensor, Tensor]:
    """The float64 blocks above composed into a model: (logits [B, T, V], balance loss)."""
    x = p["emb.weight"][tokens[:, :-1]]
    aux = torch.zeros((), dtype=torch.float64)
    for i in range(cfg.n_layers):
        pre = f"layers.{i:03d}."
        w = {n[len(pre) :]: v for n, v in p.items() if n.startswith(pre)}
        x = x + _ref_mla(cfg, w, _rms64(x, w["attn_norm"]))
        h = _rms64(x, w["mlp_norm"])
        if i < K2_FIRST_DENSE:
            x = x + _swiglu64(h, w["w1"], w["w3"], w["w2"])
        else:
            mix, layer_aux = _ref_moe(cfg, w, h, w["router_bias"])
            x, aux = x + mix, aux + layer_aux
    return _rms64(x, p["norm.weight"]) @ p["head.weight"], aux


def test_forward_matches_the_float64_composition() -> None:
    cfg = kt.MICRO
    theta = _model_weights(cfg, 6)  # a seed whose draw meets the preconditions asserted below
    tokens = _tokens(cfg, 2)
    leaves = [n for n in theta if not n.endswith("router_bias")]  # the bias has no gradient

    r = {n: v.double() for n, v in theta.items()}
    for n in leaves:
        r[n].requires_grad_()
    logits, r_aux = _ref_forward(cfg, r, tokens)
    flat = logits.reshape(-1, cfg.vocab)
    r_loss = F.cross_entropy(flat, tokens[:, 1:].reshape(-1))  # next token: shifted by one
    unshifted = F.cross_entropy(flat, tokens[:, :-1].reshape(-1))
    shift = float((r_loss - unshifted).detach().abs())
    assert shift > 0.1, f"the target shift moves the reference loss by only {shift:.1e}"

    # _BalanceTap moves the bias in place on every backward: the code gets its own clones.
    p = {n: v.clone().requires_grad_() for n, v in theta.items()}
    loss, aux = forward(cfg, p, tokens)
    assert loss.requires_grad and aux.requires_grad, "the loss or aux is detached from the weights"
    got: dict[str, Tensor | None] = {"loss": loss, "aux": aux}
    want: dict[str, Tensor | None] = {"loss": r_loss, "aux": r_aux}
    for out, y, r_y in (("loss", loss, r_loss), ("aux", aux, r_aux)):
        code = torch.autograd.grad(y, [p[n] for n in leaves], retain_graph=True, allow_unused=True)
        ref = torch.autograd.grad(r_y, [r[n] for n in leaves], retain_graph=True, allow_unused=True)
        for n, g, w in zip(leaves, code, ref, strict=True):
            got[f"d {out} / d {n}"], want[f"d {out} / d {n}"] = g, w

    bad = [m for n, w in want.items() if (m := _mismatch(n, got[n], w))]
    assert not bad, "\n".join(bad)
