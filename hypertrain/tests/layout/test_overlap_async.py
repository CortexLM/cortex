"""Async bucketed owner reductions (zero2_overlap / zero3_overlap) equal the reference bitwise."""

from __future__ import annotations

from pathlib import Path

import pytest

import hypertrain.trainer  # noqa: F401  (determinism before torch)

# isort: split
from layout_fixtures import manifest_body, setup
from test_island_layout import _collect, _launch, _summary

import hypertrain.trainer.island as isl
from hypertrain.trainer.island import emulate, island_from_manifest, train_island
from hypertrain.trainer.loop import Assignment
from hypertrain.trainer.model import init_params

REDUCTIONS = ["zero2_overlap", "zero3_overlap", "overlap"]


@pytest.mark.parametrize("reduction", REDUCTIONS)
@pytest.mark.parametrize("world", [2, 4, 8])
def test_overlap_bitwise(world: int, reduction: str, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(isl, "BUCKET_BYTES", 4096)  # several buckets on the tiny model
    cfg, lay, run_id = island_from_manifest(manifest_body(False, world, 1, True))
    get = setup("dense-dp2")[3]
    a = Assignment(run_id, 5, tuple(1000 + 17 * i for i in range(cfg.inner.H * 2 * world)))
    th = init_params(cfg.model)
    ref = emulate(world, lambda c: train_island(cfg, lay, c, th, a, get))
    new = emulate(world, lambda c: train_island(cfg, lay, c, th, a, get, reduction=reduction))
    want = (ref[0].leaves_root, ref[0].final_theta_hash)
    assert {(r.leaves_root, r.final_theta_hash) for r in new} == {want}


@pytest.mark.parametrize("reduction", ["zero2_overlap", "zero3_overlap"])
def test_overlap_moe_bitwise(reduction: str) -> None:
    cfg, lay, a, get = setup("moe-dp2-ep2-z1")
    th = init_params(cfg.model)
    ref = emulate(4, lambda c: train_island(cfg, lay, c, th, a, get))
    new = emulate(4, lambda c: train_island(cfg, lay, c, th, a, get, reduction=reduction))
    assert {r.leaves_root for r in new} == {ref[0].leaves_root}


def test_gloo_async_zero2_overlap_equals_reference(tmp_path: Path) -> None:
    proc = _launch("moe-dp1-ep2-z1", tmp_path / "run", reduction="zero2_overlap")
    cfg, lay, a, get = setup("moe-dp1-ep2-z1")
    ref = _summary(
        emulate(2, lambda c: train_island(cfg, lay, c, init_params(cfg.model), a, get))[0]
    )
    for r in _collect(proc, tmp_path / "run"):
        assert {k: r[k] for k in ref} == ref
