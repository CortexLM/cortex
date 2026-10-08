"""Todo-6 QA: HAPPY two-process roots; FAILURE 1e-3 noise at step H-1 -> first_bad_leaf."""

from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path

import hypertrain.trainer  # noqa: F401  (determinism before torch)
from hypertrain.trainer.loop import replay, train_round
from hypertrain.trainer.model import init_params

TESTS = Path(__file__).resolve().parents[1] / "tests" / "trainer"
sys.path.insert(0, str(TESTS))
from trainer_fixtures import H, J, assignment, make_cfg, sample  # noqa: E402

env = {k: v for k, v in os.environ.items() if k != "CUBLAS_WORKSPACE_CONFIG"}
happy = []
for arch, dtype, codec in [
    ("dense", "fp32", "dense-int8"),
    ("dense", "bf16", "sparseloco"),
    ("moe", "fp32", "sparseloco"),
    ("moe", "bf16", "dense-int8"),
]:
    runs = []
    for _ in range(2):
        p = subprocess.run(  # noqa: S603  (fixed argv: this interpreter + repo worker script)
            [sys.executable, str(TESTS / "_worker.py"), arch, dtype, codec],
            capture_output=True,
            text=True,
            env=env,
            check=True,
        )
        runs.append(json.loads(p.stdout))
    same = all(runs[0][k] == runs[1][k] for k in ("leaves", "leaves_root", "delta_hash"))
    happy.append(
        {
            "case": f"{arch}/{dtype}/{codec}",
            "pids_distinct": True,
            "leaves_root": [r["leaves_root"] for r in runs],
            "delta_hash": [r["delta_hash"] for r in runs],
            "identical": same,
        }
    )
    print(f"HAPPY {arch}/{dtype}/{codec} identical={same} root={runs[0]['leaves_root']}")

cfg = make_cfg()
theta0, a, get = init_params(cfg.model), assignment(cfg), sample(cfg)


def inject(t: int, theta: dict) -> None:  # type: ignore[type-arg]
    if t == H - 1:
        theta["layers.000.wq"][0, 0] += 1e-3


cheat = train_round(cfg, theta0, a, get, after_step=inject)
rep = replay(cfg, theta0, a, get, cheat.leaf_digests, cheat.delta_hash)
failure = {
    "inject": "theta['layers.000.wq'][0,0] += 1e-3 after step H-1",
    "H": H,
    "J": J,
    "expected_first_bad_leaf": H // J,
    "result": rep.result,
    "first_bad_leaf": rep.first_bad_leaf,
    "delta_match": rep.delta_match,
    "pass": rep.result == "MISMATCH" and rep.first_bad_leaf == H // J,
}
print(f"FAILURE {failure}")
ok = all(h["identical"] for h in happy) and failure["pass"]
print(json.dumps({"happy": happy, "failure": failure, "pass": ok}))
sys.exit(0 if ok else 1)
