from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "models"))

import pytest
import torch
from od_fixtures import assignment, cfg_of, fresh_carry, text_sample

from hypertrain.auditor.bisect import Executor, Fault, run_dispute
from hypertrain.auditor.replay import tensor_root
from hypertrain.protocol.keys import Keypair
from hypertrain.trainer.loop import train_round
from hypertrain.trainer.model import init_params

N = 2
HON = Keypair(b"\x07" * 32).ss58
CHEAT = Keypair(b"\x08" * 32).ss58


@pytest.fixture(scope="module")
def world():
    cfg = cfg_of()
    return cfg, init_params(cfg.model), assignment(cfg), text_sample(cfg)


def test_fault_in_od_encoder_layer_bisected(world):
    cfg, theta, a, g = world
    f = Fault(2, 1, "mlp")
    honest = Executor(HON, cfg, theta, a, g, carry=fresh_carry(theta))
    cheat = Executor(CHEAT, cfg, theta, a, g, fault=f, carry=fresh_carry(theta))
    ref = Executor("referee", cfg, theta, a, g, carry=fresh_carry(theta))
    r = run_dispute("ab" * 32, cfg, honest, cheat, ref, N, (0, cfg.inner.H))
    assert (r.step, r.layer, r.op) == (2, 1, "mlp")
    assert r.resolution.loser == CHEAT


def test_fault_in_embed_and_tail(world):
    cfg, theta, a, g = world
    for f, want in [
        (Fault(1, 0, "embed"), "embed"),
        (Fault(3, cfg.model.n_layers, "head"), "head"),
    ]:
        ref = Executor("referee", cfg, theta, a, g, carry=fresh_carry(theta))
        r = run_dispute(
            "cd" * 32,
            cfg,
            Executor(CHEAT, cfg, theta, a, g, fault=f, carry=fresh_carry(theta)),
            Executor(HON, cfg, theta, a, g, carry=fresh_carry(theta)),
            ref,
            N,
            (0, cfg.inner.H),
        )
        assert (r.step, r.layer, r.op) == (f.step, f.layer, want)
        assert r.resolution.loser == CHEAT


def test_honest_executor_state_equals_committed_leaves(world):
    cfg, theta, a, g = world
    res = train_round(cfg, theta, a, g, carry=fresh_carry(theta))
    ex = Executor(HON, cfg, theta, a, g, carry=fresh_carry(theta))
    ex.state(cfg.inner.H)
    assert tensor_root(*ex.state(cfg.inner.H)) == tensor_root(res.final_theta, res.final_state)
    assert all(
        torch.equal(ex.state(cfg.inner.H)[0][n], res.final_theta[n]) for n in res.final_theta
    )
