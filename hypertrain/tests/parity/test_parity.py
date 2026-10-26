from __future__ import annotations

import hypertrain.trainer  # noqa: F401  (determinism before torch)

# isort: split

import json
import os
import sys
from dataclasses import replace
from pathlib import Path
from typing import Any

import numpy as np
import pytest
import torch

from hypertrain.parity import MARGINS, item_bootstrap_ci90, power_check, tost_paired
from hypertrain.parity.bench import NON_INFORMATIVE, ByteLM, evaluate, token_logprobs

ROOT = Path(__file__).resolve().parents[2]
SIM = ROOT / "experiments" / "sim"
sys.path.insert(0, str(SIM))

import parity_grid as pg  # noqa: E402
from diloco import ARMS, ChunkedShards, SimSpec, simulate, simulate_streaming  # noqa: E402

SUMMARY_KEYS = {"cells", "gap", "ci90", "tost_result", "promotion_list"}
D = MARGINS["loss_rel"]


def _arms(shift: float, sd: float = 0.002, n: int = 5) -> tuple[list[float], list[float]]:
    rng = np.random.default_rng(7)
    ctrl = 3.0 + rng.normal(0, 0.05, n)
    arm = ctrl * (1 + shift) * (1 + rng.normal(0, sd, n))
    return list(arm), list(ctrl)


def _rel(a: list[float], c: list[float]) -> list[float]:
    return [(x - y) / y for x, y in zip(a, c, strict=True)]


def test_tost_equal_arms_equivalent() -> None:
    t = tost_paired(_rel(*_arms(0.0)), D)
    assert t.equivalent and t.non_inferior
    assert -D < t.ci90[0] < t.ci90[1] < D
    assert t.verdict.startswith("equivalent at delta=0.005")


def test_tost_two_percent_shift_not_equivalent() -> None:
    t = tost_paired(_rel(*_arms(0.02)), D)
    assert not t.equivalent and not t.non_inferior
    assert t.ci90[0] > D
    assert t.verdict.startswith("equivalence not demonstrated at delta=0.005")
    assert "90% CI" in t.verdict and "noise" not in t.verdict


def test_tost_exact_bounds_and_degenerate_inputs() -> None:
    t = tost_paired([0.001, 0.002, 0.003], D)
    hw = 2.920 * 0.001 / 3**0.5
    assert t.ci90 == pytest.approx((0.002 - hw, 0.002 + hw))
    assert not tost_paired([0.0], D).equivalent
    assert not tost_paired([0.0, float("nan"), 0.0], D).equivalent
    with pytest.raises(ValueError):
        tost_paired([0.0, 0.0], 0.0)


def test_power_check_warns_on_small_n_large_sigma() -> None:
    big = power_check(sd_diff=0.01, n=3, delta=D)
    assert big.warning is not None and "underpowered" in big.warning
    assert big.ci90_half_width > D and big.power_at_zero == 0.0
    small = power_check(sd_diff=0.0005, n=3, delta=D)
    assert small.warning is None and small.power_at_zero >= 0.8
    mmlu = power_check(sd_diff=1.0, n=3, delta=MARGINS["mmlu"])
    assert mmlu.warning is not None and mmlu.n_required and mmlu.n_required > 3


def test_item_bootstrap_brackets_true_difference() -> None:
    rng = np.random.default_rng(0)
    ctrl = (rng.random((3, 400)) < 0.5).astype(int)
    same = item_bootstrap_ci90(ctrl, ctrl)
    assert same == (0.0, (0.0, 0.0))
    arm = ctrl.copy()
    arm[:, :40] = 1 - arm[:, :40]
    m, (lo, hi) = item_bootstrap_ci90(arm, ctrl)
    assert lo <= m <= hi and m == pytest.approx(((arm - ctrl) * 100).mean())


def _tiny_model() -> tuple[object, dict[str, torch.Tensor]]:
    from hypertrain.trainer.model import init_params

    cfg = pg.build_cfg(pg.SCALES["smoke"], 0, 4).model
    return cfg, init_params(cfg)


def test_token_logprobs_match_training_loss() -> None:
    from hypertrain.trainer.model import forward

    cfg, theta = _tiny_model()
    ids = list(range(40, 90))
    lp = token_logprobs(cfg, theta, ids)  # type: ignore[arg-type]
    nll = -lp.gather(1, torch.tensor(ids[1:])[:, None]).mean()
    ce, _ = forward(cfg, theta, torch.tensor([ids]))  # type: ignore[arg-type]
    assert torch.allclose(nll, ce, atol=1e-6)
    import hypertrain.trainer.model as mm

    assert mm.F is torch.nn.functional


def test_benchmark_runner_smoke_writes_flagged_json(tmp_path: Path) -> None:
    cfg, theta = _tiny_model()
    items = [
        {
            "task": "toy_mc",
            "id": "a",
            "context": "The sky is",
            "choices": ["blue", "loud"],
            "gold": 0,
        },
        {
            "task": "toy_mc",
            "id": "b",
            "context": "Two plus two is",
            "choices": ["four", "red"],
            "gold": 0,
        },
        {"task": "toy_gsm", "id": "c", "question": "What is 2+3?", "answer": "#### 5"},
    ]
    src = tmp_path / "items.jsonl"
    src.write_text("\n".join(json.dumps(x) for x in items) + "\n")
    out = tmp_path / "bench.json"
    doc = evaluate(cfg, theta, src, out, {"checkpoint": "tiny-init"})  # type: ignore[arg-type]
    disk = json.loads(out.read_text())
    assert disk == doc and disk["scale_flag"] == NON_INFORMATIVE
    assert set(disk["tasks"]) == {"toy_mc", "toy_gsm"}
    assert disk["tasks"]["toy_mc"]["n"] == 2 and set(disk["tasks"]["toy_mc"]["items"]) == {"a", "b"}
    assert 0.0 <= disk["bench_mean"] <= 100.0
    ll, _ = ByteLM(cfg, theta).loglikelihood("The sky is", " blue")  # type: ignore[arg-type]
    assert ll < 0


def _fake_rows(shift_cell: str | None = None) -> dict[str, dict[str, object]]:
    cells = pg.design_doc()["cells"]
    rng = np.random.default_rng(1)
    base = {s: 3.0 + 0.05 * s for s in pg.SEEDS}
    rows: dict[str, dict[str, object]] = {}
    for lr in pg.DP_INNER_LRS:
        for s in pg.SEEDS:
            k = f"DP_lr{lr:g}|s{s}"
            rows[k] = {
                "key": k,
                "status": "DONE",
                "heldout_loss": base[s],
                "diverged": False,
                "heldout_loss_lawa": None,
            }
    for cid, c in cells.items():
        f = 1 + 0.002 * c["M"] ** 0.5 + (0.01 if cid == shift_cell else 0.0)
        if c.get("stage") == "A":
            f += 0.003 * abs(c["eta"] - 0.6)
        for s in pg.SEEDS:
            v = base[s] * f * (1 + rng.normal(0, 2e-4))
            rows[f"{cid}|s{s}"] = {
                "key": f"{cid}|s{s}",
                "status": "DONE",
                "heldout_loss": v,
                "heldout_loss_lawa": v * 0.999,
                "diverged": False,
            }
    rows["B_muon_M2_H30|s1"] = {"key": "B_muon_M2_H30|s1", "status": "CENSORED"}
    return rows


def test_summary_keys_and_failure_injection() -> None:
    doc = pg.design_doc()
    s = pg.summarize(doc, _fake_rows())
    assert SUMMARY_KEYS <= set(s)
    assert s["eta_trend"]["8"]["best_eta"] == 0.6
    assert "B_muon_M2_H30" not in s["gap"]
    assert s["hierarchy_test"]["tests"]
    assert all(set(v) >= {"equivalent", "ci90", "verdict"} for v in s["tost_result"].values())
    inj = pg.summarize(doc, _fake_rows(), ("DP", 0.01))
    t = inj["tost_result"]["INJECT[DP+0.01]"]
    assert not t["equivalent"] and t["verdict"].startswith("equivalence not demonstrated")
    assert inj["tost_result"]["INJECT[DP+0]"]["equivalent"]


def test_promotion_requires_two_sigma_win() -> None:
    rows = _fake_rows()
    for s in pg.SEEDS:
        r = rows[f"B_hier_R2x4_Hr1_Hg30|s{s}"]
        r["heldout_loss"] = float(r["heldout_loss"]) * 0.98  # type: ignore[arg-type]
    s = pg.summarize(pg.design_doc(), rows)
    assert [p["cell"] for p in s["promotion_list"]][:1] == ["B_hier_R2x4_Hr1_Hg30"]
    assert all(p["beats_flat_2sigma"] for p in s["promotion_list"])


def test_design_is_preregistered_and_bounded() -> None:
    doc = pg.design_doc()
    reg = json.loads((SIM / "parity_prereg.json").read_text())
    assert reg["design_sha256"] == doc["design_sha256"]
    assert doc["n_cells"] <= 48 and len(doc["seeds"]) >= 3
    assert doc["wall_clock_cap_hours"] <= 12 and doc["max_processes"] <= 64
    assert doc["margins"] == MARGINS


def test_streaming_p1_equals_flat_and_variants_run() -> None:
    sc = pg.SCALES["smoke"] | {"steps": 12}
    cfg = pg.build_cfg(sc, 0, 2)
    tr = ChunkedShards(Path(os.environ["HYPERTRAIN_DATA_DIR"]) / "train", sc["seq_len"])
    spec = SimSpec(
        M=2,
        H=3,
        steps=12,
        outer="nesterov",
        outer_lr=0.6,
        outer_momentum=0.9,
        arm=ARMS["A0"],
        seed=0,
    )
    flat = simulate(spec, cfg, tr, tr.n, lawa=(3, 2))
    st1 = simulate_streaming(spec, cfg, tr, tr.n, fragments=1)
    assert flat["theta_hash"] == st1["theta_hash"]
    assert flat["lawa_k"] == 2 and flat["lawa_theta"] is not None
    sp = replace(spec, H=6)
    a = simulate_streaming(sp, cfg, tr, tr.n, fragments=3)
    b = simulate_streaming(sp, cfg, tr, tr.n, fragments=3, overlap=1)
    assert not a["diverged"] and not b["diverged"] and a["theta_hash"] != b["theta_hash"]


def test_sim_device_proxy_matches_cpu() -> None:
    """Device path rehearsal: params on a CUDA-like proxy (.numpy() raises, host mixing raises)
    give the same bytes as CPU for every simulator entry point the grid uses."""
    sys.path.insert(0, str(ROOT / "tests" / "trainer"))
    from diloco import anneal_single, data_parallel, heldout_loss
    from fake_device import DevTensor, FakeDeviceMode, to_device

    from hypertrain.trainer.config import CompressConfig
    from hypertrain.trainer.model import init_params

    sc = pg.SCALES["smoke"] | {"steps": 12}
    cfg = pg.build_cfg(sc, 0, 2)
    tr = ChunkedShards(Path(os.environ["HYPERTRAIN_DATA_DIR"]) / "train", sc["seq_len"])
    base = SimSpec(
        M=2, H=3, steps=12, outer="nesterov", outer_lr=0.6, outer_momentum=0.9,
        arm=ARMS["A0"], seed=0,
    )  # fmt: skip
    sparse = replace(
        base, outer="sparseloco", outer_lr=1.0, sparse=CompressConfig("sparseloco", 0.05, 2, 0.95)
    )
    hier = replace(base, M=4, R=2, K=2, steps=12)
    ids = [0, 7, 13]

    def run(dev: bool) -> list[Any]:
        th = (
            (lambda: {n: to_device(x) for n, x in init_params(cfg.model).items()})
            if dev
            else (lambda: init_params(cfg.model))
        )
        out = []
        for spec in (base, sparse, hier):
            r = simulate(spec, cfg, tr, tr.n, th(), lawa=(3, 2))
            out += [r["theta_hash"], heldout_loss(cfg, r["theta"], ids, tr)]
        r = simulate(replace(base, steps=9), cfg, tr, tr.n, th())
        out.append(anneal_single(replace(base, steps=9), cfg, tr, tr.n, r, 12)["theta_hash"])
        s = simulate_streaming(replace(base, H=6), cfg, tr, tr.n, 3, 1, theta0=th())
        out.append(s["theta_hash"])
        d = data_parallel(base, cfg, tr, tr.n, th())
        out.append(d["theta_hash"])
        if dev:
            assert all(isinstance(t, DevTensor) for t in d["theta"].values())
            assert all(isinstance(t, DevTensor) for t in s["theta"].values())
        return out

    ref = run(False)
    with FakeDeviceMode(default_on_device=True):
        got = run(True)
    assert got == ref


def test_sim_inner_matches_trainer() -> None:
    import diloco

    from hypertrain.protocol.messages import f32val
    from hypertrain.trainer.loop import Assignment
    from hypertrain.trainer.loop import train_round as full
    from hypertrain.trainer.model import init_params
    from hypertrain.trainer.optim import init_state

    sc = pg.SCALES["smoke"] | {"steps": 12}
    cfg = diloco.inner_cfg(pg.build_cfg(sc, 0, 2), 6, "carry", 0)
    tr = ChunkedShards(Path(os.environ["HYPERTRAIN_DATA_DIR"]) / "train", sc["seq_len"])
    th = init_params(cfg.model)
    carry = init_state(replace(cfg.inner, state_policy="reset"), th)
    a = Assignment("5e" * 32, 0, tuple(range(12)))
    ref = full(cfg, th, a, tr, carry=carry)
    got = diloco.train_round(cfg, th, a, tr, carry=carry)
    assert diloco.state_hash(got.final_theta) == ref.final_theta_hash
    assert diloco.state_hash(got.final_state.m) == diloco.state_hash(ref.final_state.m)
    assert got.loss == f32val(ref.leaves[-1].preimage.loss_f32)


def test_batched_replicas_match_per_replica() -> None:
    import diloco

    sc = pg.SCALES["smoke"] | {"steps": 12}
    tr = ChunkedShards(Path(os.environ["HYPERTRAIN_DATA_DIR"]) / "train", sc["seq_len"])
    spec = SimSpec(
        M=4, H=3, R=2, K=2, steps=12, outer="nesterov", outer_lr=0.6, outer_momentum=0.9,
        arm=ARMS["A0"], seed=0,
    )  # fmt: skip
    for inner in ("adamw", "muon"):
        cfg = pg.build_cfg(sc, 0, 8, inner)
        got = []
        for b in (False, True):
            diloco.BATCHED = b
            try:
                got.append(
                    diloco.heldout_loss(cfg, simulate(spec, cfg, tr, tr.n)["theta"], [0, 9], tr)
                )
            finally:
                diloco.BATCHED = False
        assert abs(got[1] - got[0]) / got[0] < 1e-5


def test_real_summary_if_present() -> None:
    p = ROOT / "experiments" / "results" / "summary.json"
    if not p.exists():
        pytest.skip("full grid summary not produced yet")
    s = json.loads(p.read_text())
    assert SUMMARY_KEYS <= set(s) and s["mode"] == "full"
    assert (ROOT / "experiments" / "results" / "parity_grid.csv").exists()
