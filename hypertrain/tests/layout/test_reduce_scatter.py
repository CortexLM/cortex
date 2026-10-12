"""Prompt 1.3 / plan 1.4: bucketed reduce-scatter equals the all_gather reference bitwise."""

from __future__ import annotations

import pytest

import hypertrain.trainer  # noqa: F401  (determinism before torch)

# isort: split
import torch
from layout_fixtures import LAYOUTS, setup

import hypertrain.trainer.island as isl
from hypertrain.trainer.island import Geometry, _reduce, emulate, train_island
from hypertrain.trainer.model import init_params


@pytest.mark.parametrize("world", [2, 4, 8])
@pytest.mark.parametrize("bucket", [64, 1 << 20])
def test_reduce_bucketed_equals_reference(world: int, bucket: int) -> None:
    cfg, lay, _, _ = setup("dense-dp2")
    lay = lay.__class__(**{**lay.__dict__, "n_gpus": world, "dp_size": world})

    shapes = {n: tuple(x.shape) for n, x in init_params(cfg.model).items()}

    def run(comm: isl.Comm) -> tuple[isl.Params, isl.Params]:
        geo = Geometry(cfg.model, lay, comm.rank)
        g = torch.Generator().manual_seed(comm.rank)
        grads = {n: torch.randn(s, generator=g) for n, s in sorted(shapes.items())}
        ref = _reduce(comm, geo, grads, "all_gather")
        return ref, isl._reduce_bucketed(comm, geo, grads, bucket)

    for ref, new in emulate(world, run):
        assert all(torch.equal(ref[n], new[n]) for n in ref)


@pytest.mark.parametrize(
    "name", sorted(n for n in LAYOUTS if LAYOUTS[n]["dp"] * LAYOUTS[n]["ep"] > 1)
)
def test_island_bucketed_same_leaves(name: str, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(isl, "BUCKET_BYTES", 4096)  # force several buckets on the tiny model
    cfg, lay, a, get = setup(name)
    th = init_params(cfg.model)
    ref = emulate(lay.n_gpus, lambda c: train_island(cfg, lay, c, th, a, get))
    new = emulate(lay.n_gpus, lambda c: train_island(cfg, lay, c, th, a, get, reduction="bucketed"))
    assert {r.leaves_root for r in new} == {ref[0].leaves_root}
