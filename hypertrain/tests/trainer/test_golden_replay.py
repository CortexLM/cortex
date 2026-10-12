"""WS1 guard: leaf roots captured at acc49a3 (before any engine speedup) must stay bitwise equal."""

from __future__ import annotations

import sys
from pathlib import Path

import pytest

import hypertrain.trainer  # noqa: F401  (determinism before torch)

# isort: split
from hypertrain.trainer.island import emulate, train_island
from hypertrain.trainer.loop import train_round
from hypertrain.trainer.model import init_params

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "layout"))
import layout_fixtures as lf  # noqa: E402
from trainer_fixtures import assignment, make_cfg, sample  # noqa: E402

# Per-platform leaf roots: this AMD dev host (captured at acc49a3) and the GitHub ubuntu-24.04
# runner (from CI run on ff33e57). Each config must equal one of its platform roots.
GOLDEN: dict[str, set[str]] = {
    "loop-dense-adamw-fp32": {
        "a617ad7e95065a4e862da75640347df8fa268c171f3e5c2c166f9c028fda6ed2",
        "ba33f9468c5d1e9cfaa3d786f277c7a06c264fe9e8387255c92dd5af50fd47d6",
    },
    "loop-dense-adamw-bf16": {
        "c0d68ef52616d3754cb9e805b784b45ccda5c763d2e0428512c60160c8b4669a",
        "787870235daab111b3310d61c0d094fb101eba67710d2c5799a0452250d8f47a",
    },
    "loop-dense-muon-fp32": {
        "e4973ab6657eb60e63e10ebdbf066766d3d1b4e317ad9caddf0db9e21486cc75",
        "8de251a8c495fd868e724dbfe03593208add982c826cbb564a3307273bfbb0c2",
    },
    "loop-dense-muon-bf16": {
        "66aadfd0e34e1b5cde3eb75059fe699d938508b7034c3f045307b2020e012549",
    },
    "loop-moe-adamw-fp32": {
        "be298a1da1efcd5ef2055f0e3af35aa17459d513430d344158ca50bca7e61154",
        "12a144e07f7fd81e5ff295aa446348026199579fddbdccbf4de60f87fcfb2445",
    },
    "loop-moe-adamw-bf16": {
        "fb2c965b1bb783222c6e458abea94983cdae6877f11e026dc934d55792d29240",
        "1c281757bce66a3e239f74a1e5c4bfe10d3878f3c8b2f8867a3e11d06b8953c8",
    },
    "loop-moe-muon-fp32": {
        "89625e676a17b69f4a676b1433b76c255e035fd61db5c10111819fcb9ea91f19",
        "f173b0a87a2852f5af336c87a025a36bdf0d5862f753a67fddabb7046a197ebc",
    },
    "loop-moe-muon-bf16": {
        "cbb3dfb2c9367b69cb31be4e73ffd19fe60873d60657ff781e9b9c15c28c7604",
    },
    "island-moe-dp2-ep2-z1": {
        "8dbc123f50821e4176e2bb9e103b620515880158218dd824172dc9b0bc68c7a1",
        "fb9dccd79702f7e45e29107949f3305b4ee5306138a660aae15e53844bc7c081",
    },
    "island-moe-dp1-ep4-bf16": {
        "07f99c89f6a361b1187102555391192e5533f99906029bf80f5cc94f3968790b",
        "264271770eca744fa67e3b9680234a2243b131347dbdbdefcb803d8d585250c5",
    },
    "island-dense-dp4-z1-muon": {
        "31f377e998130cbbca682601fc53ef108e64ed1ae51af477209bc9fa68fd6482",
        "047e988525dd52e070ae881fbfc347cb584168934d0af5560af6bafa464fecb2",
    },
    "island-moe-dp1-ep2-z1": {
        "953417dd98d44487da595359a37e03a3d231fce73eeaaef612458e1ea7d205a3",
        "7f124b9bb2767d6a1eb92e9239778d30a3d00d2bb87ed089ab65817021a48e20",
    },
    "island-moe-dp2-ep1": {
        "ebe848502a955cb4ff4f48701015bf7a7b8d75f603c8cb376d061af5dfed4367",
        "db4bb4829583d2d5f51a01aa10798863782362bfa0b9984bfbe88909b7724ab9",
    },
    "island-dense-dp2": {
        "188228f30d4e802e18aef1191e3b74ca351a20a423f0ca722ae24bbc0a193cfa",
        "2eac33a93ec6982a2b0183c4e7ae4389cf0bef73ffd689da2801c878aa7deffe",
    },
    "island-single-moe": {
        "d65cfa84881dbf9d8aae6351986efe1ca047040f686d4a62b9334ca83b14f3ac",
        "1dc3c00ac0f8116537417787ca2afd284a71d94a5021bb98360f8e13100490dd",
    },
    "island-single-dense": {
        "0802ed0f74637e5cc6ad2fdac5ecca383ea93d42855551cf02d95ec03a4c2e4a",
        "2240a20cd9884e6f8fc85edd37be2e9ff523e137e189bfd12d2776a589638c75",
    },
}


@pytest.mark.parametrize("moe", [False, True])
@pytest.mark.parametrize("opt", ["adamw", "muon"])
@pytest.mark.parametrize("dt", ["fp32", "bf16"])
def test_loop_leaves_root_unchanged(moe: bool, opt: str, dt: str) -> None:
    cfg = make_cfg(moe=moe, opt=opt, dtype=dt)
    res = train_round(cfg, init_params(cfg.model), assignment(cfg), sample(cfg))
    assert res.leaves_root in GOLDEN[f"loop-{'moe' if moe else 'dense'}-{opt}-{dt}"]


@pytest.mark.parametrize("name", sorted(lf.LAYOUTS))
def test_island_leaves_root_unchanged(name: str) -> None:
    cfg, lay, a, get = lf.setup(name)
    th = init_params(cfg.model)
    roots = {
        r.leaves_root for r in emulate(lay.n_gpus, lambda c: train_island(cfg, lay, c, th, a, get))
    }
    assert len(roots) == 1 and roots <= GOLDEN["island-" + name]
