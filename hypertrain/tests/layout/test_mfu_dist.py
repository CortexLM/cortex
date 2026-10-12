"""run_mfu_dist.py: profile layouts shard bitwise-equal; torchrun CPU smoke passes the checker."""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

import numpy as np
import pytest

import hypertrain.trainer  # noqa: F401  (determinism before torch)

# isort: split
from hypertrain.trainer.island import Comm, emulate, island_from_manifest, train_island
from hypertrain.trainer.loop import Assignment, RoundResult
from hypertrain.trainer.model import init_params

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
from run_mfu import PROFILES, manifest_body  # noqa: E402
from run_mfu_dist import REDUCTIONS  # noqa: E402


def _tiny_with_layout(world: int) -> dict[str, object]:
    profiles = json.loads(PROFILES.read_text())["profiles"]
    spec = dict(profiles["tiny-cpu"])
    spec["layout"] = dict(profiles["mfu-dense-1b-dp8"]["layout"], n_gpus=world, dp_size=world)
    return manifest_body(spec)


@pytest.mark.parametrize("world", [2, 4, 8])
def test_dist_reductions_bitwise(world: int) -> None:
    cfg, lay, run_id = island_from_manifest(_tiny_with_layout(world))
    n = cfg.inner.H * cfg.inner.micro_batch * world
    a = Assignment(run_id, 0, tuple(range(n)))
    shape = (n, cfg.model.seq_len + 1)
    rows = np.random.default_rng(0).integers(0, cfg.model.vocab, shape, dtype=np.uint32)
    th = init_params(cfg.model)
    ref = emulate(world, lambda c: train_island(cfg, lay, c, th, a, rows.__getitem__))
    want = (ref[0].leaves_root, ref[0].delta_hash, ref[0].final_theta_hash)
    for red in REDUCTIONS.values():

        def run(c: Comm, red: str = red) -> RoundResult:
            return train_island(cfg, lay, c, th, a, rows.__getitem__, reduction=red)

        got = emulate(world, run)
        assert {(r.leaves_root, r.delta_hash, r.final_theta_hash) for r in got} == {want}, red


@pytest.mark.parametrize("reduction", sorted(REDUCTIONS))
def test_torchrun_cpu_smoke(tmp_path: Path, reduction: str) -> None:
    out = tmp_path / "ev.json"
    cmd = [sys.executable, "-m", "torch.distributed.run", "--standalone", "--nproc-per-node", "2"]
    cmd += [str(ROOT / "scripts/run_mfu_dist.py"), "--profile", "tiny-cpu-dp2", "--device", "cpu"]
    cmd += ["--reduction", reduction, "--warmup", "1", "--act-ckpt", "--out", str(out)]
    subprocess.run(cmd, check=True, cwd=ROOT, timeout=600)  # noqa: S603
    ev = json.loads(out.read_text())
    assert (ev["n_gpus"], ev["dp_size"], ev["replay_match"]) == (2, 2, True)
    assert ev["engine_flags"]["HT_ACT_CKPT"] == "1"
    chk = [sys.executable, str(ROOT / "scripts/check_gpu_evidence.py"), "--kind", "mfu", str(out)]
    assert subprocess.run(chk, cwd=ROOT, check=False).returncode == 0  # noqa: S603
    ev["dp_size"] = 3
    out.write_text(json.dumps(ev))
    assert subprocess.run(chk, cwd=ROOT, check=False).returncode == 1  # noqa: S603
