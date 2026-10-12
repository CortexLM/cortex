"""S6: ZeRO-2 (grad sharding) and ZeRO-3 (param sharding) equal the reference leaves bitwise."""

from __future__ import annotations

import pytest

import hypertrain.trainer  # noqa: F401  (determinism before torch)

# isort: split
import torch
import torch.distributed as dist
from layout_fixtures import manifest_body, setup

from hypertrain.trainer.island import emulate, island_from_manifest, to_dtensor, train_island
from hypertrain.trainer.loop import Assignment
from hypertrain.trainer.model import init_params


def _dense(world: int, opt: str) -> tuple[object, ...]:
    cfg, lay, run_id = island_from_manifest(manifest_body(False, world, 1, True, opt=opt))
    get = setup("dense-dp2")[3]
    n = cfg.inner.H * cfg.inner.micro_batch * world
    return cfg, lay, Assignment(run_id, 5, tuple(1000 + 17 * i for i in range(n))), get


CASES = [("dense", w, o) for w in (2, 4, 8) for o in ("adamw", "muon")]
CASES += [("moe-dp2-ep2-z1", 4, ""), ("moe-dp1-ep2-z1", 2, "")]


@pytest.mark.parametrize("reduction", ["zero2", "zero3"])
@pytest.mark.parametrize(("name", "world", "opt"), CASES)
def test_zero23_bitwise(name: str, world: int, opt: str, reduction: str) -> None:
    cfg, lay, a, get = _dense(world, opt) if name == "dense" else setup(name)
    th = init_params(cfg.model)  # type: ignore[attr-defined]
    ref = emulate(world, lambda c: train_island(cfg, lay, c, th, a, get))  # type: ignore[arg-type]
    new = emulate(
        world,
        lambda c: train_island(cfg, lay, c, th, a, get, reduction=reduction),  # type: ignore[arg-type]
    )
    for r in new:
        assert (r.leaves_root, r.delta_hash, r.final_theta_hash) == (
            ref[0].leaves_root,
            ref[0].delta_hash,
            ref[0].final_theta_hash,
        )


def test_zero23_requires_zero1() -> None:
    cfg, lay, a, get = setup("dense-dp2")
    with pytest.raises(ValueError, match="zero1"):
        emulate(
            2,
            lambda c: train_island(cfg, lay, c, init_params(cfg.model), a, get, reduction="zero2"),
        )


@pytest.mark.skipif(not dist.is_initialized(), reason="needs a real process group (torchrun)")
def test_dtensor_layout_roundtrip() -> None:
    from torch.distributed.device_mesh import init_device_mesh

    cfg, _, _, _ = setup("dense-dp2")
    th = init_params(cfg.model)
    mesh = init_device_mesh("cpu", (dist.get_world_size(),))
    for n, d in to_dtensor(th, mesh).items():
        assert torch.equal(d.full_tensor(), th[n])
