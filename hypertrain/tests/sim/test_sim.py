from __future__ import annotations

import hypertrain.trainer  # noqa: F401  (determinism before torch)

# isort: split

import json
import subprocess
import sys
from pathlib import Path

import pytest
import torch

ROOT = Path(__file__).resolve().parents[2]
SIM = ROOT / "experiments" / "sim"
sys.path.insert(0, str(SIM))

from diloco import (  # noqa: E402
    ARMS,
    ChunkedShards,
    SimSpec,
    data_parallel,
    dp_ids,
    round_ids,
    simulate,
)
from state_policy import base_cfg  # noqa: E402

STEPS = 12


def _setup() -> tuple[object, ChunkedShards]:
    cfg = base_cfg(0, STEPS, seq_len=64, tiny=True)
    return cfg, ChunkedShards(ROOT / "data" / "train", 64)


def test_m1_h1_equals_data_parallel_bitwise() -> None:
    cfg, tr = _setup()
    spec = SimSpec(
        M=1, H=1, steps=STEPS, outer="average", outer_lr=1.0, outer_momentum=0.9,
        arm=ARMS["A0"], seed=0,
    )  # fmt: skip
    sim = simulate(spec, cfg, tr, tr.n)
    dp = data_parallel(spec, cfg, tr, tr.n)
    assert not sim["diverged"] and not dp["diverged"]
    assert sim["theta_hash"] == dp["theta_hash"]
    for n in dp["theta"]:
        assert torch.equal(sim["theta"][n], dp["theta"][n]), n
    # control: a different inner policy (reset each step) must break bitwise equality
    sim_reset = simulate(replace_arm(spec, "A1"), cfg, tr, tr.n)
    assert sim_reset["theta_hash"] != dp["theta_hash"]


def replace_arm(spec: SimSpec, arm: str) -> SimSpec:
    from dataclasses import replace

    return replace(spec, arm=ARMS[arm])


def test_assignment_disjoint_and_dp_uses_same_tokens() -> None:
    cfg, tr = _setup()
    spec = SimSpec(
        M=4, H=3, steps=STEPS, outer="nesterov", outer_lr=0.7, outer_momentum=0.9,
        arm=ARMS["A0"], seed=1,
    )  # fmt: skip
    per_round = [round_ids(spec, cfg, w, tr.n) for w in range(STEPS // 3)]
    flat = [i for r in per_round for s in r for i in s]
    assert len(flat) == len(set(flat))
    assert sorted(dp_ids(spec, cfg, tr.n)) == sorted(flat)


def test_simulator_deterministic_and_regions_differ() -> None:
    cfg, tr = _setup()
    spec = SimSpec(
        M=2, H=3, steps=STEPS, outer="nesterov", outer_lr=0.7, outer_momentum=0.9,
        arm=ARMS["A3"], seed=0,
    )  # fmt: skip
    a, b = simulate(spec, cfg, tr, tr.n), simulate(spec, cfg, tr, tr.n)
    assert a["theta_hash"] == b["theta_hash"]
    from dataclasses import replace

    hier = simulate(replace(spec, M=4, R=2, K=2, steps=STEPS), cfg, tr, tr.n)
    assert not hier["diverged"] and hier["sub_rounds"] == STEPS // 3
    sparse = simulate(
        replace(spec, outer="sparseloco", outer_lr=1.0, sparse=_sparse()), cfg, tr, tr.n
    )
    assert not sparse["diverged"]
    assert len({a["theta_hash"], hier["theta_hash"], sparse["theta_hash"]}) == 3


def _sparse() -> object:
    from hypertrain.trainer.config import CompressConfig

    return CompressConfig("sparseloco", topk_frac=0.05, bits=2, ef_beta=1.0)


def _smoke(tmp: Path, *extra: str) -> tuple[int, dict]:  # type: ignore[type-arg]
    p = subprocess.run(
        [sys.executable, str(SIM / "state_policy.py"), "--smoke", "--out-dir", str(tmp), *extra],
        capture_output=True,
        text=True,
        timeout=1800,
    )
    assert p.returncode == 0, p.stdout[-3000:] + p.stderr[-3000:]
    return p.returncode, json.loads((tmp / "decision_smoke.json").read_text())


@pytest.mark.parametrize("mode", ["normal", "sabotage"])
def test_smoke_grid_neg_worse_and_sabotage_fails(tmp_path: Path, mode: str) -> None:
    if mode == "normal":
        _, d = _smoke(tmp_path)
        assert d["neg_gate"]["pass"] is True, d["cells"]
        neg = d["cells"]["M2_H5"]["arms"]["NEG"]
        assert neg["mean_gap"] > 0.0025 and neg["ci95"][0] > 0
    else:
        _, d = _smoke(tmp_path, "--sabotage-outer-lr", "10")
        assert d["divergent_runs"], "lr=10 must diverge"
        for arm in ("A1", "A2", "A3"):
            assert d["arms"][arm]["verdict"] == "FAIL"
        assert d["decision"]["state_policy"] == "carry"
