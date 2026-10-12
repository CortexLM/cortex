"""Multi-GPU MFU harness (plan-scale S6/S7): one island round per rank under torchrun, replayed.

Same evidence shape as run_mfu.py (check_gpu_evidence.py --kind mfu), with n_gpus = dp_size.
Each rank drives train_island over DistComm (nccl on cuda, gloo on cpu); torch.distributed
reductions stay blocked (forbid_reductions), every collective is all_gather / all_to_all.
--reduction maps to the S6 paths: zero1 = bucketed reduce-scatter + ZeRO-1 optimizer shards,
zero2 = + gradient shards, zero3 = + parameter shards. Step time is the max over ranks.
Usage: torchrun --standalone --nproc-per-node N scripts/run_mfu_dist.py --profile mfu-dense-1b-dp8 \
       --device cuda --reduction zero2 --act-ckpt [--compile] --out OUT.json
"""

from __future__ import annotations

import argparse
import json
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
import torch.distributed as dist

from run_mfu import PROFILES, manifest_body, summarize  # isort: skip

from hypertrain.trainer.attention import select_backend
from hypertrain.trainer.island import DistComm, check_launch, island_from_manifest, train_island
from hypertrain.trainer.loop import Assignment
from hypertrain.trainer.model import init_params
from hypertrain.trainer.perf import MFU_PEAK_RTX5090_BF16, flops_per_token

REDUCTIONS = {"zero1": "bucketed", "zero2": "zero2", "zero3": "zero3"}


def one_round(body: dict[str, Any], device: torch.device, reduction: str) -> dict[str, Any]:
    cfg, lay, run_id = island_from_manifest(body)
    comm = DistComm()
    check_launch(lay, comm.world, dp_size=comm.world, ep_size=1, zero1=True)
    n = cfg.inner.H * cfg.inner.micro_batch * cfg.inner.grad_accum * comm.world
    rows = np.random.default_rng(0).integers(
        0, cfg.model.vocab, (n, cfg.model.seq_len + 1), dtype=np.uint32
    )
    theta = {k: v.to(device) for k, v in init_params(cfg.model).items()}
    sync = torch.cuda.synchronize if device.type == "cuda" else (lambda: None)
    stamps: list[float] = []

    def after_step(t: int, _theta: Any) -> None:
        sync()
        stamps.append(time.perf_counter())

    dist.barrier()
    sync()
    t0 = time.perf_counter()
    res = train_island(
        cfg,
        lay,
        comm,
        theta,
        Assignment(run_id, 0, tuple(range(n))),
        rows.__getitem__,
        reduction=REDUCTIONS[reduction],
        after_step=after_step,
    )
    mine = {
        "step_seconds": [b - a for a, b in zip([t0, *stamps], stamps, strict=False)],
        "leaves_root": res.leaves_root,
        "peak_mem_gib": round(torch.cuda.max_memory_allocated(device) / 2**30, 3)
        if device.type == "cuda"
        else None,
    }
    ranks: list[Any] = [None] * comm.world
    dist.all_gather_object(ranks, mine)  # outside train_island: host metadata, not verified tensors
    return {
        "run_id": run_id,
        "leaves_root": ranks[0]["leaves_root"],
        "rank_leaves_roots": [r["leaves_root"] for r in ranks],
        "step_seconds": [max(s) for s in zip(*(r["step_seconds"] for r in ranks), strict=True)],
        "rank_peak_mem_gib": [r["peak_mem_gib"] for r in ranks],
        "tokens_per_step": cfg.inner.micro_batch
        * cfg.inner.grad_accum
        * cfg.model.seq_len
        * comm.world,
        "flops_per_token": flops_per_token(cfg.model),
        "n_gpus": lay.n_gpus,
        "dp_size": lay.dp_size,
        "attention_backend": select_backend(device),
    }


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--profile", required=True)
    ap.add_argument("--device", choices=["cuda", "cpu"], required=True)
    ap.add_argument("--reduction", choices=sorted(REDUCTIONS), default="zero2")
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("--warmup", type=int, default=10)
    ap.add_argument("--measured-peak", type=float)
    ap.add_argument("--act-ckpt", action="store_true", help="sets HT_ACT_CKPT=1")
    ap.add_argument("--compile", action="store_true", help="sets HT_COMPILE=1")
    a = ap.parse_args()
    if a.act_ckpt:
        os.environ["HT_ACT_CKPT"] = "1"
    if a.compile:
        os.environ["HT_COMPILE"] = "1"
    spec = json.loads(PROFILES.read_text())["profiles"][a.profile]
    local = int(os.environ.get("LOCAL_RANK", "0"))
    if a.device == "cuda":
        torch.cuda.set_device(local)
        device = torch.device("cuda", local)
    else:
        device = torch.device("cpu")
    dist.init_process_group("nccl" if a.device == "cuda" else "gloo")
    try:
        body = manifest_body(spec)
        run = one_round(body, device, a.reduction)
        replay = one_round(body, device, a.reduction)
        if dist.get_rank() != 0:
            return 0
        git = ["git", "rev-parse", "HEAD"]
        sha = subprocess.run(git, capture_output=True, text=True, check=False)  # noqa: S603, S607
        roots = {*run["rank_leaves_roots"], *replay["rank_leaves_roots"]}
        ev: dict[str, Any] = {
            "kind": "mfu",
            "profile": a.profile,
            "device": a.device,
            "reduction": a.reduction,
            "git_sha": sha.stdout.strip(),
            "torch": torch.__version__,
            "determinism": DETERMINISM,
            "engine_flags": {k: v for k, v in sorted(os.environ.items()) if k.startswith("HT_")},
            "warmup_steps": a.warmup,
            "peak_vendor_flops": MFU_PEAK_RTX5090_BF16,
            "peak_measured_flops": a.measured_peak,
            **run,
            "replay_leaves_root": replay["leaves_root"],
            "replay_match": len(roots) == 1,
            "vendor": summarize(run, a.warmup, MFU_PEAK_RTX5090_BF16),
        }
        if a.measured_peak:
            ev["measured"] = summarize(run, a.warmup, a.measured_peak)
        if a.device == "cuda":
            ev["gpu_name"] = torch.cuda.get_device_name(device)
        a.out.write_text(json.dumps(ev, indent=1))
        print(json.dumps({k: ev[k] for k in ("profile", "replay_match", "vendor")}))
        return 0 if ev["replay_match"] else 1
    finally:
        dist.destroy_process_group()


if __name__ == "__main__":
    sys.exit(main())
