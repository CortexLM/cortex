"""WS8 / ADR 0006: the model registry (OpenDecision encoder, kimi-k3-pro-tiny); CPU from-scratch
round is bitwise."""

from __future__ import annotations

import dataclasses

import pytest
from od_fixtures import assignment, cfg_of, fresh_carry, text_sample

import hypertrain.models.kimi_tiny as kt
import hypertrain.models.opendecision as od
from hypertrain.models.registry import Arch
from hypertrain.trainer.loop import replay, train_round
from hypertrain.trainer.model import ARCH_REGISTRY, _arch_impl, init_params, param_shapes


def test_od_encoder_registered() -> None:
    cfg = cfg_of()
    assert ARCH_REGISTRY["od-encoder"].module == od.__name__
    assert _arch_impl(cfg.model) is od
    assert param_shapes(cfg.model) == od.param_shapes(cfg.model)


def test_kimi_registered_without_runtime_registration() -> None:
    # A torchrun worker is a fresh interpreter: only the permanent line can resolve the arch.
    assert ARCH_REGISTRY[kt.ARCH].module == kt.__name__
    assert _arch_impl(kt.MICRO) is kt
    assert param_shapes(kt.MICRO) == kt.param_shapes(kt.MICRO)


def test_registry_cannot_be_changed_at_runtime() -> None:
    # implementation_hash covers registry.py; a runtime entry or edit would bypass it.
    with pytest.raises(TypeError):
        ARCH_REGISTRY["nope"] = Arch("hypertrain.models.kimi_tiny")  # type: ignore[index]
    with pytest.raises(dataclasses.FrozenInstanceError):
        ARCH_REGISTRY[kt.ARCH].max_ranks = None  # type: ignore[misc]


def test_unknown_arch_rejected() -> None:
    bad = dataclasses.replace(cfg_of().model, arch="nope")
    with pytest.raises(ValueError, match="unknown arch"):
        param_shapes(bad)


def test_from_scratch_round_bitwise_and_replays() -> None:
    cfg = cfg_of()
    get = text_sample(cfg)
    roots = []
    for _ in range(2):
        theta = init_params(cfg.model)  # registry dispatch, fresh init
        res = train_round(cfg, theta, assignment(cfg), get, carry=fresh_carry(theta))
        roots.append((res.leaves_root, res.delta_hash))
    assert roots[0] == roots[1]
    theta = init_params(cfg.model)
    rep = replay(
        cfg,
        theta,
        assignment(cfg),
        get,
        res.leaf_digests,
        res.delta_hash,
        carry=fresh_carry(theta),
    )
    assert rep.result == "MATCH"
