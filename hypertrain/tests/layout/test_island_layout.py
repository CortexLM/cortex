from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path
from typing import Any

import pytest
import torch
from layout_fixtures import LAYOUTS, H, J, manifest_body, setup
from pydantic import ValidationError
from reference import reference_round

from hypertrain.protocol.envelope import Intake, seal
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import Layout, RunManifest
from hypertrain.trainer.island import (
    Comm,
    ForbiddenCollective,
    Geometry,
    LayoutMismatch,
    check_launch,
    emulate,
    ep_combine,
    ep_dispatch,
    island_from_manifest,
    train_island,
)
from hypertrain.trainer.loop import RoundResult, batch_hash, train_round
from hypertrain.trainer.model import init_params

HERE = Path(__file__).resolve().parent
WORKER = HERE / "_island_worker.py"
CHILD_ENV = {k: v for k, v in os.environ.items() if k != "CUBLAS_WORKSPACE_CONFIG"}


def _launch(name: str, out: Path, reduction: str = "all_gather") -> subprocess.Popen[str]:
    out.mkdir(parents=True)
    n = LAYOUTS[name]["dp"] * LAYOUTS[name]["ep"]
    cmd = [sys.executable, "-m", "torch.distributed.run", "--standalone"]
    cmd += [f"--nproc-per-node={n}", str(WORKER), name, str(out), reduction]
    return subprocess.Popen(
        cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, env=CHILD_ENV
    )


def _collect(p: subprocess.Popen[str], out: Path) -> list[dict[str, Any]]:
    _, err = p.communicate(timeout=600)
    assert p.returncode == 0, err[-4000:]
    ranks = [json.loads(f.read_text()) for f in sorted(out.glob("rank*.json"))]
    assert len(ranks) == ranks[0]["world"]
    return ranks


def _summary(res: RoundResult) -> dict[str, Any]:
    return {
        "leaves_root": res.leaves_root,
        "leaves": [x.digest.hex() for x in res.leaves],
        "delta_hash": res.delta_hash,
        "final_theta_hash": res.final_theta_hash,
        "ef_out_hash": res.ef_out_hash,
    }


def _emulated(name: str) -> list[RoundResult]:
    cfg, lay, a, get = setup(name)
    theta0 = init_params(cfg.model)
    return emulate(lay.n_gpus, lambda c: train_island(cfg, lay, c, theta0, a, get))


def _assert_same(ranks: list[dict[str, Any]], ref: dict[str, Any]) -> None:
    for r in ranks:
        assert r["torch_preloaded"] is False
        assert "all_gather" in r["ops"] and set(r["ops"]) <= {"all_gather", "all_to_all"}
        assert {k: r[k] for k in ref} == ref


def test_four_rank_gloo_rerun_and_emulation_bitwise(tmp_path: Path) -> None:
    name = "moe-dp2-ep2-z1"
    procs = [(_launch(name, tmp_path / f"run{i}"), tmp_path / f"run{i}") for i in range(2)]
    emu = _emulated(name)
    runs = [_collect(p, out) for p, out in procs]
    ref = _summary(emu[0])
    print(f"{name} leaves_root={ref['leaves_root']} delta_hash={ref['delta_hash']}")
    assert all(_summary(x) == ref for x in emu)
    assert len(ref["leaves"]) == H // J + 1
    for ranks in runs:
        assert len(ranks) == 4
        _assert_same(ranks, ref)


@pytest.mark.parametrize("name", ["moe-dp1-ep4-bf16", "dense-dp4-z1-muon", "moe-dp1-ep2-z1"])
def test_gloo_matches_emulation(name: str, tmp_path: Path) -> None:
    proc = _launch(name, tmp_path / "run")
    ref = _summary(_emulated(name)[0])
    _assert_same(_collect(proc, tmp_path / "run"), ref)


@pytest.mark.parametrize("name", ["single-moe", "single-dense"])
def test_single_rank_island_equals_todo6_train_round(name: str) -> None:
    cfg, _, a, get = setup(name)
    island = _emulated(name)[0]
    base = train_round(cfg, init_params(cfg.model), a, get)
    assert _summary(island) == _summary(base)


def test_leaf_batch_hash_covers_all_ranks_samples() -> None:
    cfg, lay, a, _ = setup("moe-dp2-ep1")
    res = _emulated("moe-dp2-ep1")[0]
    per = cfg.inner.micro_batch * cfg.inner.grad_accum * lay.n_gpus
    for leaf in res.leaves:
        ids = a.sample_ids[max(0, leaf.t - J) * per : leaf.t * per]
        assert leaf.preimage.batch_ids_sha256 == batch_hash(ids)
    assert res.leaves[1].preimage.batch_ids_sha256 != batch_hash(a.sample_ids[: J * per // 2])


def test_layout_changes_leaves() -> None:
    assert _summary(_emulated("moe-dp2-ep1")[0]) != _summary(_emulated("single-moe")[0])


@pytest.mark.parametrize(("world", "ep"), [(4, 4), (4, 2), (2, 2)])
def test_ep_all_to_all_is_inverse_permutation(world: int, ep: int) -> None:
    cfg, _, _, _ = setup("moe-dp2-ep2-z1")
    lay = Layout(pp=1, n_gpus=world, dp_size=world // ep, ep_size=ep, zero1=False)
    e = cfg.model.n_experts

    def run(c: Comm) -> tuple[bool, bool, bool]:
        geo = Geometry(cfg.model, lay, c.rank)
        g = torch.Generator().manual_seed(c.rank)
        counts = torch.randint(0, 4, (e,), generator=g)
        if c.rank == 0:
            counts[0] = 0
        n = int(counts.sum())
        expert = torch.repeat_interleave(torch.arange(e), counts)
        rows = (torch.randn(n, 3, generator=g) + c.rank * 100.0).requires_grad_(True)
        grouped, plan = ep_dispatch(c, geo, rows, counts)
        lo = geo.ep_rank * geo.eq
        tags = ep_dispatch(c, geo, expert.float()[:, None], counts)[0].view(-1)
        owned = bool(((tags >= lo) & (tags < lo + geo.eq)).all())
        back = ep_combine(c, plan, grouped * 1.0)
        back.backward(torch.full_like(back, 2.0))
        assert rows.grad is not None
        return torch.equal(back, rows), owned, torch.equal(rows.grad, torch.full_like(rows, 2.0))

    assert emulate(world, run) == [(True, True, True)] * world


def test_dp_size_change_without_manifest_update_rejected() -> None:
    body = manifest_body(**LAYOUTS["moe-dp2-ep2-z1"])
    cfg, lay, run_id = island_from_manifest(body)
    with pytest.raises(LayoutMismatch, match="dp_size"):
        check_launch(lay, 4, dp_size=4, ep_size=1)
    with pytest.raises(LayoutMismatch, match="n_gpus"):
        check_launch(lay, 2)
    _, _, a, get = setup("moe-dp2-ep2-z1")
    theta0 = init_params(cfg.model)
    with pytest.raises(LayoutMismatch):
        emulate(2, lambda c: train_island(cfg, lay, c, theta0, a, get))
    bad = json.loads(json.dumps(body))
    bad["reference_spec"]["layout"]["dp_size"] = 4
    with pytest.raises(ValidationError, match="n_gpus must equal"):
        island_from_manifest(bad)
    bad["reference_spec"]["layout"]["n_gpus"] = 8
    assert island_from_manifest(bad)[2] != run_id  # a real layout change is a new run
    for key, val in (("zero1", 1), ("dp_size", True), ("ep_size", 0), ("extra", 1)):
        mal = json.loads(json.dumps(body))
        mal["reference_spec"]["layout"][key] = val
        with pytest.raises(ValidationError):
            island_from_manifest(mal)
    plain = json.loads(json.dumps(body))
    plain["reference_spec"]["layout"] = {"pp": 2}
    with pytest.raises(ValidationError):
        island_from_manifest(plain)
    odd = json.loads(json.dumps(body))
    odd["reference_spec"]["layout"].update(n_gpus=3, dp_size=1, ep_size=3)
    with pytest.raises(ValueError, match="ep_size must divide"):
        island_from_manifest(odd)


def test_all_reduce_flag_is_rejected_by_guard(tmp_path: Path) -> None:
    proc = _launch("moe-dp1-ep2-z1", tmp_path / "run", reduction="all_reduce")
    cfg, lay, a, get = setup("moe-dp1-ep2-z1")
    theta0 = init_params(cfg.model)
    with pytest.raises(ForbiddenCollective):
        emulate(2, lambda c: train_island(cfg, lay, c, theta0, a, get, reduction="all_reduce"))
    _, err = proc.communicate(timeout=600)
    assert proc.returncode != 0
    assert "ForbiddenCollective: torch.distributed.all_reduce on verified tensors" in err
    assert not list((tmp_path / "run").glob("rank*.json"))


def test_protocol_intake_accepts_signed_island_manifest() -> None:
    body = manifest_body(**LAYOUTS["moe-dp2-ep2-z1"])
    m = RunManifest.model_validate(body)
    assert m.reference_spec.layout == Layout(pp=2, n_gpus=4, dp_size=2, ep_size=2, zero1=True)
    assert m.model_dump(mode="json") == body
    owner = Keypair(bytes([5]) * 32)
    env = seal(owner, "RunManifest", m.run_id(), body, 100)
    got = Intake(m.run_id(), {"RunManifest": owner.ss58.__eq__}).accept(env, now_round=50)
    assert isinstance(got, RunManifest) and got.run_id() == m.run_id()
    assert island_from_manifest(body)[1] == m.reference_spec.layout


@pytest.mark.parametrize("name", ["moe-dp2-ep1", "dense-dp4-z1-muon"])
def test_gloo_equals_spec_reference(name: str, tmp_path: Path) -> None:
    proc = _launch(name, tmp_path / "run")
    cfg, lay, a, get = setup(name)
    ref = reference_round(cfg, lay, init_params(cfg.model), a, get)
    ranks = _collect(proc, tmp_path / "run")
    ref = {k: ref[k] for k in ("leaves_root", "leaves", "delta_hash")}
    print(f"{name} reference leaves_root={ref['leaves_root']}")
    assert len(ranks) == lay.n_gpus > 1
    _assert_same(ranks, ref)
    assert _summary(_emulated(name)[0])["leaves_root"] == ref["leaves_root"]


@pytest.mark.parametrize("name", ["moe-dp2-ep2-z1", "moe-dp1-ep2-z1"])
def test_ep_numerics_match_spec_reference_without_ep(name: str) -> None:
    """Numerics check (not replay equality): EP island vs the spec oracle with ep_size=1."""
    cfg, lay, a, get = setup(name)
    assert cfg.model.compute_dtype == "fp32" and lay.ep_size > 1
    flat = Layout(pp=lay.pp, n_gpus=lay.n_gpus, dp_size=lay.n_gpus, ep_size=1, zero1=False)
    theta0 = init_params(cfg.model)
    ref = reference_round(cfg, flat, theta0, a, get)
    got = emulate(lay.n_gpus, lambda c: train_island(cfg, lay, c, theta0, a, get))[0]
    final: Any = ref["final_theta"]
    assert sorted(got.final_theta) == sorted(final)
    for k in sorted(final):
        torch.testing.assert_close(got.final_theta[k], final[k], atol=1e-5, rtol=1e-4, msg=k)
    assert any(not torch.equal(got.final_theta[k], theta0[k]) for k in final if ".w1" in k)
