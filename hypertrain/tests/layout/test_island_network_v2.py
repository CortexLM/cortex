"""Island network: independent DP arithmetic, EP collectives, identity hooks and device checks."""

from __future__ import annotations

import importlib.util
from pathlib import Path

import numpy as np
import pytest
import torch
from reference import reference_round

import hypertrain.trainer  # noqa: F401
from hypertrain.auditor.replay import pack_state
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.island import TraceContext, emulate, train_island
from hypertrain.trainer.loop import Assignment, train_round
from hypertrain.trainer.model import init_params


def helpers():
    spec = importlib.util.spec_from_file_location(
        "l1_launch_fixtures", Path(__file__).parents[1] / "miner/test_island_launch_v2.py"
    )
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


@pytest.mark.parametrize(
    "n,ep,zero1", [(1, 1, False), (2, 1, True), (4, 1, False), (2, 2, True), (4, 2, True)]
)
def test_same_layout_hooks_and_reference(n: int, ep: int, zero1: bool) -> None:
    w = helpers().tiny_manifest(n, ep, zero1)
    cfg = TrainConfig.from_manifest_v2(w)
    lay = w.training.reference_spec.layout
    theta = init_params(cfg.model)
    a = Assignment(w.run_id(), 0, tuple(range(w.training.batch_samples())))

    def get(i: int):
        return (np.arange(5, dtype=np.uint32) + i) % 16

    plain = emulate(n, lambda c: train_island(cfg, lay, c, theta, a, get))
    traces: list[list[tuple[TraceContext, bytes]]] = [[] for _ in range(n)]

    def hook(ctx: TraceContext, x: torch.Tensor) -> torch.Tensor:
        raw = x.detach().cpu().contiguous().reshape(-1).view(torch.uint8).numpy().tobytes()
        traces[ctx.rank].append((ctx, raw))
        return x

    traced = emulate(n, lambda c: train_island(cfg, lay, c, theta, a, get, hook=hook))
    for actual, expected in zip(traced, plain, strict=True):
        assert actual.leaf_digests == expected.leaf_digests
        assert actual.delta_payload == expected.delta_payload
        assert pack_state(actual.final_theta, actual.final_state) == pack_state(
            expected.final_theta, expected.final_state
        )
    assert all(any(ctx.op == "attn" for ctx, _ in trace) for trace in traces)
    assert all(any(ctx.op.startswith("optimizer.") for ctx, _ in trace) for trace in traces)
    again: list[list[tuple[TraceContext, bytes]]] = [[] for _ in range(n)]

    def capture(ctx: TraceContext, x: torch.Tensor) -> torch.Tensor:
        raw = x.detach().cpu().contiguous().reshape(-1).view(torch.uint8).numpy().tobytes()
        again[ctx.rank].append((ctx, raw))
        return x

    emulate(n, lambda c: train_island(cfg, lay, c, theta, a, get, hook=capture))
    assert traces == again
    if ep == 1:
        oracle = reference_round(cfg, lay, theta, a, get)
        assert plain[0].delta_hash == oracle["delta_hash"]
        assert plain[0].leaves_root == oracle["leaves_root"]
    else:
        assert any(ctx.op == "all_to_all.output" for ctx, _ in traces[0])
    if n == 1:
        assert plain[0].leaf_digests == train_round(cfg, theta, a, get).leaf_digests


@pytest.mark.parametrize("ep", [1, 2])
def test_decoder_bf16_same_layout_cpu(ep: int) -> None:
    from hypertrain.protocol.messages_v2 import RunManifestV2

    # Given: identical signed N2 BF16 layouts, not a CUDA oracle.
    body = helpers().tiny_manifest(2, ep).body()
    body["training"]["model"]["compute_dtype"] = "bf16"
    w = RunManifestV2.model_validate(body)
    cfg = TrainConfig.from_manifest_v2(w)
    theta = init_params(cfg.model)
    a = Assignment(w.run_id(), 0, tuple(range(w.training.batch_samples())))
    # When: each logical rank executes its assigned rows.
    outputs = emulate(
        2,
        lambda c: train_island(
            cfg,
            w.training.reference_spec.layout,
            c,
            theta,
            a,
            lambda i: (np.arange(5, dtype=np.uint32) + i) % 16,
        ),
    )
    # Then: every leaf/full-state/EF/delta agrees within the exact layout.
    assert outputs[0].leaf_digests == outputs[1].leaf_digests
    assert outputs[0].delta_payload == outputs[1].delta_payload
    assert pack_state(outputs[0].final_theta, outputs[0].final_state) == pack_state(
        outputs[1].final_theta, outputs[1].final_state
    )
    assert pack_state(outputs[0].ef_out) == pack_state(outputs[1].ef_out)


def test_independent_reference_kills_divide_by_one(monkeypatch: pytest.MonkeyPatch) -> None:
    from hypertrain.trainer import island

    w = helpers().tiny_manifest(2)
    cfg = TrainConfig.from_manifest_v2(w)
    lay = w.training.reference_spec.layout
    theta = init_params(cfg.model)
    a = Assignment(w.run_id(), 0, tuple(range(w.training.batch_samples())))

    def get(i: int):
        return (np.arange(5, dtype=np.uint32) + i) % 16

    reference = reference_round(cfg, lay, theta, a, get)
    original = island._reduce

    def wrong(comm, geo, grads, reduction):
        return {k: x * lay.n_gpus for k, x in original(comm, geo, grads, reduction).items()}

    monkeypatch.setattr(island, "_reduce", wrong)
    mutated = emulate(2, lambda c: train_island(cfg, lay, c, theta, a, get))[0]
    assert mutated.leaves_root != reference["leaves_root"]


def test_collective_scalar_device_is_explicit() -> None:
    from hypertrain.trainer.island import Geometry, _grad_norm, _mean_loss

    w = helpers().tiny_manifest(1)
    cfg = TrainConfig.from_manifest_v2(w)
    lay = w.training.reference_spec.layout

    class DeviceComm:
        rank = 0
        world = 1

        def all_gather(self, x: torch.Tensor) -> list[torch.Tensor]:
            assert x.device.type == "cpu"
            return [x]

    red = init_params(cfg.model)
    with torch.device("meta"):
        assert _grad_norm(DeviceComm(), Geometry(cfg.model, lay, 0), red) > 0
        assert _mean_loss(DeviceComm(), 1.0, torch.device("cpu")) == 1
        pack_state(red)


def test_cancel_boundary_refuses_execution() -> None:
    w = helpers().tiny_manifest(2)
    cfg = TrainConfig.from_manifest_v2(w)

    def cancelled() -> None:
        raise RuntimeError("cancelled lease")

    with pytest.raises(RuntimeError, match="cancelled lease"):
        emulate(
            2,
            lambda c: train_island(
                cfg,
                w.training.reference_spec.layout,
                c,
                init_params(cfg.model),
                Assignment(w.run_id(), 0, tuple(range(4))),
                lambda i: np.zeros(5, np.uint32),
                cancel=cancelled,
            ),
        )


def test_ep_gradient_replica_sum_independent_reference() -> None:
    from hypertrain.trainer.island import Geometry, _reduce

    w = helpers().tiny_manifest(4, 2, True)
    cfg = TrainConfig.from_manifest_v2(w)
    lay = w.training.reference_spec.layout

    def run(comm):
        geo = Geometry(cfg.model, lay, comm.rank)
        full = init_params(cfg.model)
        grads = {k: torch.full_like(geo.local(k, x), comm.rank + 1) for k, x in full.items()}
        result = _reduce(comm, geo, grads, "all_gather")
        for k, x in result.items():
            expected = (4 if comm.rank % 2 == 0 else 6) / 4 if geo.is_expert(k) else 10 / 4
            assert torch.equal(x, torch.full_like(x, expected))
        return True

    assert emulate(4, run) == [True] * 4


def test_zero_owner_rank_participates_without_host_device_fallback() -> None:
    from hypertrain.trainer.island import Geometry, gather_pieces

    # Given: one optimizer piece, four ranks; three ranks own no state.
    w = helpers().tiny_manifest(4)
    cfg = TrainConfig.from_manifest_v2(w)
    lay = w.training.reference_spec.layout
    name = "emb.weight"
    expected = torch.ones((cfg.model.vocab, cfg.model.d_model), device="cpu")

    def run(comm):
        geo = Geometry(cfg.model, lay, comm.rank)
        values = {name: expected} if comm.rank == 0 else {}
        # When: every rank joins the same real fixed-order gather.
        got = gather_pieces(comm, geo, values, [(name, 0)], lambda _: 0, expected.device)
        # Then: empty-owner ranks receive the exact tensor, no inferred CPU allocation.
        assert torch.equal(got[(name, 0)], expected)
        return True

    assert emulate(4, run) == [True] * 4
