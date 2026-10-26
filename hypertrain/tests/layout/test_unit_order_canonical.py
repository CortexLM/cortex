from __future__ import annotations

import dataclasses
import hashlib
from typing import NoReturn

import pytest
import torch
from layout_fixtures import setup

import hypertrain.models.kimi_tiny as kt
from hypertrain.protocol.messages import Layout
from hypertrain.trainer.island import emulate, train_island
from hypertrain.trainer.leaf_v2 import CHUNK, tree_hash
from hypertrain.trainer.loop import Assignment
from hypertrain.trainer.model import init_params
from hypertrain.trainer.unit_order import UnitOrderResult, train_unit_order

UNITS = 8


def _unit_run(world: int, units: int = UNITS) -> UnitOrderResult:
    cfg, _, a, get = setup("single-dense")
    n = cfg.inner.H * units * cfg.inner.micro_batch
    a = Assignment(a.run_id, a.w, tuple(1000 + 17 * i for i in range(n)))
    theta0 = init_params(cfg.model)
    res = emulate(world, lambda c: train_unit_order(cfg, c, theta0, a, get, units))
    assert all(r.leaves == res[0].leaves for r in res)
    return res[0]


def _theta_bytes(r: UnitOrderResult) -> bytes:
    return b"".join(r.final_theta[n].numpy().tobytes() for n in sorted(r.final_theta))


def test_unit_order_root_is_world_invariant() -> None:
    runs = {w: _unit_run(w) for w in (1, 2, 4, 8)}
    print(f"unit-order canonical leaves_root={runs[1].leaves_root}")
    ref = runs[1]
    assert len(ref.leaves) == 3
    for r in runs.values():
        assert r.leaves_root == ref.leaves_root
        assert _theta_bytes(r) == _theta_bytes(ref)
        assert r.delta_hash == ref.delta_hash


def test_default_rank_order_stays_layout_dependent() -> None:
    cfg, lay, a, get = setup("dense-dp2")
    theta0 = init_params(cfg.model)
    two = emulate(2, lambda c: train_island(cfg, lay, c, theta0, a, get))[0]
    cfg1 = dataclasses.replace(cfg, inner=dataclasses.replace(cfg.inner, micro_batch=4))
    lay1 = Layout(pp=lay.pp, n_gpus=1, dp_size=1, ep_size=1, zero1=False)
    one = emulate(1, lambda c: train_island(cfg1, lay1, c, theta0, a, get))[0]
    assert one.leaves_root != two.leaves_root


def test_units_must_divide_world() -> None:
    with pytest.raises(ValueError, match="multiple of world"):
        _unit_run(3)


@pytest.mark.parametrize("world", [1, 2])
def test_kimi_unit_order_is_refused_before_training(world: int) -> None:
    # Its router bias moves per backward pass from the local load: ranks would diverge.
    cfg = dataclasses.replace(setup("single-dense")[0], model=kt.MICRO)
    a = Assignment("ab" * 32, 0, tuple(range(cfg.inner.H * 2 * cfg.inner.micro_batch)))

    def trained(i: int) -> NoReturn:
        raise AssertionError("a sample was loaded")

    theta0 = init_params(cfg.model)
    with pytest.raises(ValueError, match="does not allow reduction_order 'unit'"):
        emulate(world, lambda c: train_unit_order(cfg, c, theta0, a, trained, 2))


def _spec_tree_hash(t: dict[str, torch.Tensor]) -> str:
    names = sorted(t)
    shapes = ",".join(f'["{n}",[{",".join(str(d) for d in t[n].shape)}]]' for n in names)
    stream = b"".join(t[n].numpy().astype("<f4").tobytes() for n in names)
    digests = [hashlib.sha256(b"ht-leaf-v2|[" + shapes.encode() + b"]").digest()]
    digests += [
        hashlib.sha256(stream[i : i + CHUNK]).digest() for i in range(0, len(stream), CHUNK)
    ]
    return hashlib.sha256(b"".join(digests)).hexdigest()


def test_leaf_v2_known_answer() -> None:
    small = {"b": torch.arange(3, dtype=torch.float32), "a": torch.full((2, 2), 0.5)}
    assert tree_hash(small) == _spec_tree_hash(small)
    assert tree_hash(small) == "7751a56152ec81b993601b9319bd4b7968db307375d0e05dc4929b2285deadf6"
    big = {"w": torch.arange(300_000, dtype=torch.float32)}  # 1.2 MB: two chunks
    assert tree_hash(big) == _spec_tree_hash(big)
    with pytest.raises(ValueError, match="fp32"):
        tree_hash({"x": torch.zeros(2, dtype=torch.float64)})
