"""MFU benchmark harness (plan 0.3 / WS1): one deterministic island round, timed, then replayed.

Drives hypertrain.trainer.island.train_island (production path, determinism ON) on a profile from
experiments/mfu/profiles.json and writes raw per-step timings, FLOPs/token, MFU vs the vendor peak
(and vs --measured-peak when given), and the leaf root of both the run and the replay.
check_gpu_evidence.py --kind mfu recomputes MFU from the raw timings.
Usage: python scripts/run_mfu.py --profile mfu-dense-1b --device cuda --out OUT.json
       [--warmup 10] [--measured-peak FLOPS]
"""

from __future__ import annotations

import argparse
import json
import math
import os
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

from hypertrain.trainer import DETERMINISM  # must precede torch

# isort: split
import numpy as np
import torch

from hypertrain.protocol.example import example_manifest
from hypertrain.trainer.attention import select_backend
from hypertrain.trainer.island import emulate, island_from_manifest, train_island
from hypertrain.trainer.loop import Assignment
from hypertrain.trainer.model import init_params, param_shapes
from hypertrain.trainer.perf import MFU_PEAK_RTX5090_BF16, flops_per_token

PROFILES = Path(__file__).resolve().parents[1] / "experiments/mfu/profiles.json"


def manifest_body(spec: dict[str, Any]) -> dict[str, Any]:
    body: dict[str, Any] = json.loads(json.dumps(example_manifest().body()))
    for k in ("model", "inner", "outer"):
        body[k].update(spec[k])
    body["reference_spec"]["layout"] = dict(spec["layout"])
    body["model"]["param_count"] = 1
    cfg0, _, _ = island_from_manifest(body)
    body["model"]["param_count"] = sum(math.prod(s) for s in param_shapes(cfg0.model).values())
    return body


def one_round(body: dict[str, Any], device: torch.device) -> dict[str, Any]:
    cfg, lay, run_id = island_from_manifest(body)
    if lay.n_gpus != 1:
        # ponytail: 1-GPU only; the 8x run goes through torchrun + DistComm like run_phase_b.py.
        raise SystemExit("run_mfu.py supports n_gpus=1 profiles")
    n = cfg.inner.H * cfg.inner.micro_batch * cfg.inner.grad_accum
    rows = np.random.default_rng(0).integers(
        0, cfg.model.vocab, (n, cfg.model.seq_len + 1), dtype=np.uint32
    )
    theta = {k: v.to(device) for k, v in init_params(cfg.model).items()}
    sync = torch.cuda.synchronize if device.type == "cuda" else (lambda: None)
    stamps: list[float] = []

    def after_step(t: int, _theta: Any) -> None:
        sync()
        stamps.append(time.perf_counter())

    sync()
    t0 = time.perf_counter()
    res = emulate(
        1,
        lambda c: train_island(
            cfg,
            lay,
            c,
            theta,
            Assignment(run_id, 0, tuple(range(n))),
            rows.__getitem__,
            after_step=after_step,
        ),
    )[0]
    step_s = [b - a for a, b in zip([t0, *stamps], stamps, strict=False)]
    return {
        "run_id": run_id,
        "leaves_root": res.leaves_root,
        "step_seconds": step_s,
        "tokens_per_step": cfg.inner.micro_batch * cfg.inner.grad_accum * cfg.model.seq_len,
        "flops_per_token": flops_per_token(cfg.model),
        "n_gpus": lay.n_gpus,
        "attention_backend": select_backend(device),
    }


def summarize(run: dict[str, Any], warmup: int, peak: float) -> dict[str, Any]:
    timed = run["step_seconds"][warmup:]
    if not timed:
        raise SystemExit("no timed steps after warmup")
    tok_s = run["tokens_per_step"] * len(timed) / sum(timed)
    return {
        "timed_steps": len(timed),
        "tokens_per_s": tok_s,
        "mfu": tok_s * run["flops_per_token"] / (run["n_gpus"] * peak),
    }


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--profile", required=True)
    ap.add_argument("--device", choices=["cuda", "cpu"], required=True)
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("--warmup", type=int, default=10)
    ap.add_argument("--measured-peak", type=float)
    a = ap.parse_args()
    spec = json.loads(PROFILES.read_text())["profiles"][a.profile]
    device = torch.device("cuda", 0) if a.device == "cuda" else torch.device("cpu")
    body = manifest_body(spec)
    run = one_round(body, device)
    replay = one_round(body, device)
    sha = subprocess.run(["git", "rev-parse", "HEAD"], capture_output=True, text=True, check=False)  # noqa: S603, S607
    ev: dict[str, Any] = {
        "kind": "mfu",
        "profile": a.profile,
        "device": a.device,
        "git_sha": sha.stdout.strip(),
        "torch": torch.__version__,
        "determinism": DETERMINISM,
        "engine_flags": {k: v for k, v in sorted(os.environ.items()) if k.startswith("HT_")},
        "attention_backend": run["attention_backend"],
        "warmup_steps": a.warmup,
        "peak_vendor_flops": MFU_PEAK_RTX5090_BF16,
        "peak_measured_flops": a.measured_peak,
        **{
            k: run[k]
            for k in (
                "run_id",
                "n_gpus",
                "tokens_per_step",
                "flops_per_token",
                "step_seconds",
                "leaves_root",
            )
        },
        "replay_leaves_root": replay["leaves_root"],
        "replay_match": run["leaves_root"] == replay["leaves_root"],
        "vendor": summarize(run, a.warmup, MFU_PEAK_RTX5090_BF16),
    }
    if a.measured_peak:
        ev["measured"] = summarize(run, a.warmup, a.measured_peak)
    if a.device == "cuda":
        ev["gpu_name"] = torch.cuda.get_device_name(0)
        ev["peak_mem_gib"] = round(torch.cuda.max_memory_allocated() / 2**30, 3)
    a.out.write_text(json.dumps(ev, indent=1))
    print(json.dumps({k: ev[k] for k in ("profile", "replay_match", "vendor")}))
    return 0 if ev["replay_match"] else 1


if __name__ == "__main__":
    sys.exit(main())
