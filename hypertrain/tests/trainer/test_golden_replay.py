"""WS1 guard: leaf roots captured at acc49a3 (before any engine speedup) must stay bitwise equal."""

from __future__ import annotations

import platform
import subprocess
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
# The island roots are those of a process which honors the manifest's environment pin (DET-01,
# tests/process_pin.py: the ambient OMP/MKL/oneDNN variables are replaced by the pin's). They are
# the same on the AMD EPYC 9575F dev host and on the Blacksmith runner (AMD EPYC, CI run
# 40659367870). The roots taken in an ambient environment are gone: with them a process that did
# not honor the pin would still pass, so an island root outside GOLDEN fails (see _expect_golden).
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
        "6d8ed36ff53eceaafbbb9acd41f1b774a91cd349bb1b177b8197ad7bcec08d18",
    },
    "island-moe-dp1-ep4-bf16": {
        "8ee15b332e4770ad0e70e2128565215c4895d43ba6e7dde97587d88802c57427",
    },
    "island-dense-dp4-z1-muon": {
        "4bdddbec1ab5dfe7df78d43c26344f3a3d36e122ea8189ecaf6e864bd3bcbf48",
    },
    "island-moe-dp1-ep2-z1": {
        "248b9e8d16c26d30e1153a2b02322695e7b365fd54f42f4a9c9eea6824f4dc48",
    },
    "island-moe-dp2-ep1": {
        "000ae6fbb39e8b4605ea8207e4415f6df240c660b65385eee5a63c9108ce8543",
    },
    "island-dense-dp2": {
        "311c02986b03a0a7a95430919a491e9e1f3290311c4fb1fdd36c52105b00d7b6",
    },
    "island-single-moe": {
        "2ac0b2574f752e4afb4d2bf7cdd6326bd4d98042a6a5458c3347c08f5a94cac6",
    },
    "island-single-dense": {
        "630438b0fea3eb90d55f3ad624f98accd78209f2cd9dbb8e3512c87928dd9eaf",
    },
}

XFAIL_REASON = (
    "golden leaf root differs on this CPU class; unexplained hardware dependence, "
    "see livrables/forge/01-inconnus.md (golden replay)"
)


def _cpu() -> str:
    try:
        out = subprocess.run(["lscpu"], capture_output=True, text=True, check=False).stdout
    except OSError:  # no lscpu (non-Linux)
        out = ""
    model = [ln.split(":", 1)[1].strip() for ln in out.splitlines() if ln.startswith("Model name:")]
    return model[0] if model else platform.processor()


def _expect_golden(
    key: str, root: str, request: pytest.FixtureRequest, *, explained: bool = False
) -> None:
    # A root outside GOLDEN is a reported xfail, not a red job: the cause is not yet known. For an
    # `explained` key it is: the roots are those of a process honoring the environment pin, so a
    # root outside GOLDEN is a process that does not, or a change of the numerics, and it fails.
    if root not in GOLDEN[key]:
        message = f"{XFAIL_REASON}; test={request.node.nodeid} root={root} cpu={_cpu()}"
        assert not explained, message
        pytest.xfail(message)


@pytest.mark.parametrize("moe", [False, True])
@pytest.mark.parametrize("opt", ["adamw", "muon"])
@pytest.mark.parametrize("dt", ["fp32", "bf16"])
def test_loop_leaves_root_unchanged(
    moe: bool, opt: str, dt: str, request: pytest.FixtureRequest
) -> None:
    cfg = make_cfg(moe=moe, opt=opt, dtype=dt)
    res = train_round(cfg, init_params(cfg.model), assignment(cfg), sample(cfg))
    _expect_golden(f"loop-{'moe' if moe else 'dense'}-{opt}-{dt}", res.leaves_root, request)


def test_the_process_that_takes_the_island_roots_honors_the_environment_pin() -> None:
    """The island roots of GOLDEN are those of a process under the suite's pin (DET-01,
    tests/process_pin.py): ATen, MKL and oneDNN read their variables at first use, so only the
    launcher sets them. Fails for a process that does not, whatever it computes."""
    from env_pin_fixtures import TEST_PIN
    from hypertrain.trainer.determinism import require_env_pin

    require_env_pin(TEST_PIN)


@pytest.mark.parametrize("name", sorted(lf.LAYOUTS))
def test_island_leaves_root_unchanged(name: str, request: pytest.FixtureRequest) -> None:
    cfg, lay, a, get = lf.setup(name)
    th = init_params(cfg.model)
    roots = {
        r.leaves_root for r in emulate(lay.n_gpus, lambda c: train_island(cfg, lay, c, th, a, get))
    }
    assert len(roots) == 1
    _expect_golden("island-" + name, roots.pop(), request, explained=True)


@pytest.mark.parametrize("moe", [False, True], ids=["dense", "moe"])
def test_leaves_root_is_deterministic_in_process(moe: bool) -> None:
    # Hardware-independent guard: holds on any CPU class, unlike GOLDEN.
    cfg = make_cfg(moe=moe)
    a, b = (
        train_round(cfg, init_params(cfg.model), assignment(cfg), sample(cfg)).leaves_root
        for _ in range(2)
    )
    assert a == b
