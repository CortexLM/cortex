"""S2 / ADR 0006: kimi-k3-pro-tiny (MLA + shared-expert MoE, sigmoid top-k, aux-free bias)."""

from __future__ import annotations

import dataclasses
import json
import math

import pytest
import torch

import hypertrain.models.kimi_tiny as kt
from hypertrain.trainer.config import ModelConfig
from hypertrain.trainer.loop import Assignment, replay, train_round
from hypertrain.trainer.model import ARCH_REGISTRY, Params, forward, init_params, param_shapes

cuda = pytest.mark.skipif(not torch.cuda.is_available(), reason="needs a CUDA device (B300)")
MOE = "layers.001."


@pytest.fixture(autouse=True)
def _registered(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setitem(ARCH_REGISTRY, kt.ARCH, kt.__name__)


def _tokens(cfg: ModelConfig, b: int, t: int, seed: int = 0) -> torch.Tensor:
    g = torch.Generator().manual_seed(seed)
    return torch.randint(0, cfg.vocab, (b, t + 1), generator=g)


def _fwd_bwd(cfg: ModelConfig, tokens: torch.Tensor) -> tuple[list[torch.Tensor], Params]:
    """One loop._train_step-style pass; returns (loss, aux, grads...) and the mutated params."""
    theta = init_params(cfg)
    names = sorted(theta)
    p = {n: theta[n].detach().requires_grad_(True) for n in names}
    loss, aux = forward(cfg, p, tokens)
    gs = torch.autograd.grad(
        loss + cfg.aux_loss_coef * aux, [p[n] for n in names], allow_unused=True
    )
    zero = torch.zeros(())
    return [loss, aux, *(zero if g is None else g for g in gs)], theta


def test_profile_size_and_k2_layout() -> None:
    shapes = param_shapes(kt.PROFILE)
    total = sum(math.prod(s) for s in shapes.values())
    assert 10_000_000 <= total < 100_000_000  # "tens of millions"
    layers = range(kt.PROFILE.n_layers)
    moe = [i for i in layers if f"layers.{i:03d}.router_bias" in shapes]
    assert moe == list(layers)[kt.FIRST_DENSE :]  # K2: first layer dense, the rest MoE
    assert all(f"layers.{i:03d}.shared_w1" in shapes for i in moe)
    assert not any(n.endswith((".wk", ".wv")) for n in shapes)  # MLA, no per-head K/V weights


def test_profile_forward_backward_shapes() -> None:
    cfg = dataclasses.replace(kt.PROFILE, compute_dtype="fp32")
    out, _ = _fwd_bwd(cfg, _tokens(cfg, 1, 32))
    loss, aux, *grads = out
    assert loss.shape == aux.shape == () and torch.isfinite(loss) and float(aux.detach()) > 0
    names = sorted(param_shapes(cfg))
    for n, g in zip(names, grads, strict=True):
        if n.endswith("router_bias"):
            assert g.shape == ()  # no gradient: the bias only steers selection
        else:
            assert g.shape == param_shapes(cfg)[n] and bool(g.abs().sum() > 0), n


def test_topk_selects_on_biased_scores_and_gates_unbiased() -> None:
    scores = torch.tensor([[0.9, 0.8, 0.7, 0.1], [0.5, 0.5, 0.5, 0.5], [0.2, 0.3, 0.9, 0.6]])
    bias = torch.tensor([0.0, 0.0, 0.0, 0.75])
    experts = kt.topk_experts(scores, bias, 2)
    # rank-major: rank 0 then rank 1; token 1 ties among 0..2 -> lowest index 0
    assert experts.tolist() == [0, 3, 3, 3, 0, 2]
    want = torch.tensor([[0.9, 0.1], [0.5, 0.5], [0.6, 0.9]])
    want = want / want.sum(-1, keepdim=True) * kt.ROUTED_SCALE
    torch.testing.assert_close(kt.gate(scores, experts, 2), want)


def _mlp_inputs(cfg: ModelConfig, theta: Params, tokens: torch.Tensor) -> dict[int, torch.Tensor]:
    seen: dict[int, torch.Tensor] = {}

    def hook(layer: int, op: str, x: torch.Tensor) -> torch.Tensor:
        if op == "mlp_norm":
            seen[layer] = x.detach().clone()
        return x

    with torch.no_grad():
        forward(cfg, theta, tokens, hook=hook)
    return seen


def test_bias_update_sign_rule_applies_after_backward_only() -> None:
    cfg = kt.MICRO
    tokens = _tokens(cfg, 2, cfg.seq_len)
    theta = init_params(cfg)
    start = torch.linspace(-0.02, 0.02, cfg.n_experts)
    for i in range(kt.FIRST_DENSE, cfg.n_layers):
        theta[f"layers.{i:03d}.router_bias"] = start.clone()
    h = _mlp_inputs(cfg, theta, tokens)
    for i in range(kt.FIRST_DENSE, cfg.n_layers):  # forward alone never moves the bias
        assert torch.equal(theta[f"layers.{i:03d}.router_bias"], start)
    p = {n: x.detach().requires_grad_(True) for n, x in theta.items()}
    loss, aux = forward(cfg, p, tokens)
    torch.autograd.grad(loss + aux, [p[n] for n in sorted(p)], allow_unused=True)
    for i in range(kt.FIRST_DENSE, cfg.n_layers):
        pre = f"layers.{i:03d}."
        x = h[i].reshape(-1, cfg.d_model)
        experts = kt.topk_experts(
            torch.sigmoid(x @ theta[pre + "router"]), start, cfg.top_k_experts
        )
        load = torch.bincount(experts, minlength=cfg.n_experts).float()
        want = start + kt.BIAS_GAMMA * torch.sign(load.mean() - load)
        assert torch.equal(theta[pre + "router_bias"], want), pre
        assert not torch.equal(want, start)


def test_shared_expert_is_added_to_every_token() -> None:
    cfg = kt.MICRO
    theta = init_params(cfg)
    x = torch.randn(2, 5, cfg.d_model, generator=torch.Generator().manual_seed(1))

    def run(**zero: str) -> torch.Tensor:
        p = {**theta, **{MOE + w: torch.zeros_like(theta[MOE + w]) for w in zero.values()}}
        with torch.no_grad():
            return kt._moe(cfg, p, MOE, x, torch.float32)[0]

    full, routed, shared = run(), run(s="shared_w2"), run(r="w2")
    assert bool(shared.abs().min() > 0)  # the shared expert sees every token
    assert torch.equal(full, routed + shared)


@pytest.mark.parametrize(
    "cfg",
    [
        kt.MICRO,
        dataclasses.replace(kt.MICRO, compute_dtype="bf16"),
        dataclasses.replace(kt.PROFILE, compute_dtype="fp32"),
    ],
    ids=["micro-fp32", "micro-bf16", "profile-fp32"],
)
def test_two_passes_same_seed_are_bitwise_identical(cfg: ModelConfig) -> None:
    tokens = _tokens(cfg, 2, min(cfg.seq_len, 32))
    (a, ta), (b, tb) = _fwd_bwd(cfg, tokens), _fwd_bwd(cfg, tokens)
    assert all(torch.equal(x, y) for x, y in zip(a, b, strict=True))
    assert all(torch.equal(ta[n], tb[n]) for n in ta)  # includes the updated router_bias


def _round(doc: dict[str, object]) -> tuple[object, ...]:
    cfg = kt.train_config(doc)
    n = cfg.inner.H * cfg.inner.micro_batch
    a = Assignment(run_id=kt.run_manifest_id(doc), w=0, sample_ids=tuple(range(n)))
    get = kt.synthetic_sample(cfg)
    res = train_round(cfg, init_params(cfg.model), a, get)
    rep = replay(cfg, init_params(cfg.model), a, get, res.leaf_digests, res.delta_hash)
    return res.leaves_root, res.delta_hash, res.final_theta_hash, rep.result, res.final_theta


def test_run_manifest_is_unsigned_strict_and_replays() -> None:
    doc = json.loads(json.dumps(kt.run_manifest(kt.MICRO)))
    assert kt.train_config(doc).model == kt.MICRO
    assert set(doc) == {"kind", "model", "inner", "compress"}  # no key, signature or run secret
    for bad in ({**doc, "signature": "00" * 64}, {**doc, "kind": "RunManifest"}):
        with pytest.raises(ValueError, match="TEST run"):
            kt.train_config(bad)
    with pytest.raises(ValueError, match="not a kimi"):
        kt.train_config({**doc, "model": {**doc["model"], "arch": "decoder"}})
    other = {**doc, "model": {**doc["model"], "init_seed": 8}}
    assert kt.run_manifest_id(other) != kt.run_manifest_id(doc)
    first, second = _round(doc), _round(doc)
    assert first[:4] == second[:4] and first[3] == "MATCH"
    theta = first[4]
    assert isinstance(theta, dict)
    assert not torch.equal(theta[MOE + "router_bias"], torch.zeros(kt.MICRO.n_experts))


@pytest.mark.gpu
@cuda
def test_profile_two_rounds_bitwise_on_cuda() -> None:
    rounds = kt.qualify(kt.run_manifest(), "cuda", 2)
    print(json.dumps(rounds))
    assert rounds[0] == rounds[1]
