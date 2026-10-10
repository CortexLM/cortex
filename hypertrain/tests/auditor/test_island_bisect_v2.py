"""Real same-layout trace equality and localized forward/collective/update faults."""

from __future__ import annotations

import hashlib
import importlib.util
import sys
from pathlib import Path

import numpy as np
import pytest
import torch

import hypertrain.trainer  # noqa: F401
from hypertrain.auditor.bisect import RefereeError, tensor_digest
from hypertrain.auditor.island_bisect import IslandExecution, IslandParty, adjudicate
from hypertrain.auditor.replay import pack_state
from hypertrain.protocol.keys import Keypair
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.island import TraceContext, emulate, train_island
from hypertrain.trainer.loop import Assignment
from hypertrain.trainer.model import init_params


def helpers(name: str):
    spec = importlib.util.spec_from_file_location("l5_" + name, Path(__file__).parents[1] / name)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def execution(n: int = 2, ep: int = 1, objective: str | None = None) -> IslandExecution:
    if objective:
        helper = helpers("layout/test_od_island_v2.py")
        manifest = helper.od_manifest(objective, n, True)
        cfg = TrainConfig.from_manifest_v2(manifest)
        get = helper.samples(cfg)
    else:
        manifest = helpers("miner/test_island_launch_v2.py").tiny_manifest(n, ep)
        cfg = TrainConfig.from_manifest_v2(manifest)

        def get(i: int):
            return (np.arange(5, dtype=np.uint32) + i) % 16

    return IslandExecution(
        cfg=cfg,
        layout=manifest.training.reference_spec.layout,
        assignment=Assignment(
            manifest.run_id(), 0, tuple(range(manifest.training.batch_samples()))
        ),
        theta=init_params(cfg.model),
        get_sample=get,
    )


@pytest.mark.parametrize(
    "n,ep,objective",
    [
        (2, 1, None),
        (2, 2, None),
        (4, 2, None),
        (2, 1, "mlm"),
        (2, 1, "decision"),
        (2, 1, "distill"),
    ],
)
def test_honest_traces_when_real_island_hooks(n: int, ep: int, objective: str | None) -> None:
    # Given
    run = execution(n, ep, objective)
    plain = emulate(
        n,
        lambda c: train_island(
            run.cfg,
            run.layout,
            c,
            run.theta,
            run.assignment,
            run.get_sample,
        ),
    )
    # When
    traced = emulate(n, run.trace)
    repeated = emulate(n, run.trace)
    a = IslandParty(Keypair(b"\x31" * 32).ss58, [c for _, c in traced], J=run.cfg.inner.J)
    b = IslandParty(Keypair(b"\x32" * 32).ss58, [c for _, c in repeated], J=run.cfg.inner.J)
    # Then
    for rank, (result, _) in enumerate(traced):
        assert result.leaf_digests == plain[rank].leaf_digests
        assert result.delta_payload == plain[rank].delta_payload
        assert pack_state(result.final_theta, result.final_state) == pack_state(
            plain[rank].final_theta,
            plain[rank].final_state,
        )
    assert a.entries == b.entries
    assert {entry.rank for entry in a.entries} == set(range(n))
    for level, ctx in [("step", ()), ("layer", (1,)), ("op", (1, 0))]:
        at = list(range(a.span(level, ctx) + 1))
        assert a.hashes(level, ctx, at) == b.hashes(level, ctx, at)
    assert any(e.op.startswith("optimizer.") for e in a.entries)
    assert any(e.op == "all_gather.input" for e in a.entries)
    if ep > 1:
        assert any(e.op == "ep.dispatch.permutation" for e in a.entries)
    truth = IslandParty(Keypair(b"\x33" * 32).ss58, [c for _, c in repeated], J=run.cfg.inner.J)
    assert adjudicate(("11" * 32, "22" * 32), (a, b), truth).reason == "MATCH"


@pytest.mark.parametrize("op", ["attn", "all_to_all.input", "optimizer.theta."])
def test_fault_localizes_when_actual_hook_mutates(op: str) -> None:
    # Given
    run = execution(2, 2)
    plain = emulate(2, run.trace)
    hit = [False, False]

    def fault(ctx: TraceContext, x: torch.Tensor) -> torch.Tensor:
        if (
            ctx.rank == 0
            and ctx.step == 1
            and not hit[0]
            and ctx.op.startswith(op)
            and x.numel()
            and x.is_floating_point()
        ):
            hit[0] = True
            out = x.clone()
            out.reshape(-1)[0] += 0.1
            return out
        return x

    # When
    changed = emulate(2, lambda c: run.trace(c, fault=fault))
    good = IslandParty(Keypair(b"\x41" * 32).ss58, [c for _, c in plain], J=1)
    bad = IslandParty(Keypair(b"\x42" * 32).ss58, [c for _, c in changed], J=1)
    ref = IslandParty(Keypair(b"\x43" * 32).ss58, [c for _, c in plain], J=1)
    evidence = adjudicate(("11" * 32, "22" * 32), (good, bad), ref)
    # Then
    assert hit[0] and changed[0][0].delta_hash != plain[0][0].delta_hash
    assert evidence.reason == "FRAUD" and evidence.loser == bad.hotkey
    assert "rank=0" in evidence.op_spec and "op=" + op in evidence.op_spec


def test_referee_rejects_when_agreed_prefix_unreproducible() -> None:
    # Given
    run = execution()
    traces = emulate(2, run.trace)
    a = IslandParty(Keypair(b"\x51" * 32).ss58, [c for _, c in traces], J=1)
    b = IslandParty(Keypair(b"\x52" * 32).ss58, [c for _, c in traces], J=1)
    ref = IslandParty(Keypair(b"\x53" * 32).ss58, [c for _, c in traces], J=1)
    theta = {k: x.clone() + 0.1 for k, x in run.theta.items()}
    from hypertrain.trainer.optim import init_state

    forged = pack_state(theta, init_state(run.cfg.inner, theta))
    a.states[0] = b.states[0] = forged
    # When / Then
    with pytest.raises(RefereeError, match="agreed input"):
        adjudicate(("11" * 32, "22" * 32), (a, b), ref)


def test_tensor_digest_when_cpu_encoding_is_frozen() -> None:
    # Given
    tensor = torch.arange(6, dtype=torch.bfloat16).reshape(2, 3)
    expected = hashlib.sha256(
        b"torch.bfloat16|(2, 3)|" + tensor.view(torch.uint8).numpy().tobytes()
    )
    # When / Then
    assert tensor_digest(tensor) == expected.digest()


def test_context_rejects_when_invalid_level_or_rank() -> None:
    # Given
    run = execution()
    traces = emulate(2, run.trace)
    party = IslandParty(Keypair(b"\x61" * 32).ss58, [c for _, c in traces], J=1)
    # When / Then
    with pytest.raises(RefereeError):
        party.hashes("op", (1, 999999), [0])
    with pytest.raises(AssertionError):
        party.span("unsupported", ())
    with pytest.raises(RefereeError):
        IslandParty(party.hotkey, [traces[1][1], traces[0][1]], J=1)
