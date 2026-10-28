"""Multi-GPU MFU harness (plan-scale S6/S7): one island round per rank under torchrun, replayed.

Same evidence shape as run_mfu.py (check_gpu_evidence.py --kind mfu), with n_gpus = dp_size.
Each rank drives train_island over DistComm (nccl on cuda, gloo on cpu); torch.distributed
reductions stay blocked (forbid_reductions), every collective is all_gather / all_to_all.
--reduction maps to the S6 paths: zero1 = bucketed reduce-scatter + ZeRO-1 optimizer shards,
zero2 = + gradient shards, zero3 = + parameter shards. Step time is the max over ranks.
Usage: torchrun --standalone --nproc-per-node N scripts/run_mfu_dist.py --profile mfu-dense-1b-dp8 \
       --device cuda --reduction zero2 --act-ckpt [--trace DIR] [--nccl-pcie] \\
       --out OUT.json
--trace profiles 3 steps after warmup of the first round (never the replay) into DIR/rank{r}.json
plus DIR/summary.json (per-rank CUDA/CPU time buckets, step times, memory).
--nccl-pcie setdefaults PCIe-only NCCL knobs; collectives are copies, so numerics are unaffected.
Each rank pins its environment as run_mfu.py does (honor_pin): --compile (HT_COMPILE=1) is refused,
because no environment class holds it.
"""

from __future__ import annotations

import argparse
import json
import os
import re
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

from run_mfu import PROFILES, honor_pin, manifest_body, summarize  # isort: skip

from hypertrain.trainer.attention import chosen_paths, select_backend
from hypertrain.trainer.island import DistComm, check_launch, island_from_manifest, train_island
from hypertrain.trainer.loop import Assignment
from hypertrain.trainer.model import init_params
from hypertrain.trainer.perf import MFU_PEAK_RTX5090_BF16, flops_per_token

REDUCTIONS = {
    "zero1": "bucketed",
    "zero1o": "overlap",
    "zero2": "zero2",
    "zero2o": "zero2_overlap",
    "zero3": "zero3",
    "zero3o": "zero3_overlap",
}
NCCL_PCIE = {
    "NCCL_P2P_DISABLE": "1",
    "NCCL_ALGO": "Ring",
    "NCCL_PROTO": "Simple",
    "NCCL_MIN_NCHANNELS": "4",
    "NCCL_MAX_NCHANNELS": "8",
    "NCCL_BUFFSIZE": "8388608",
}
# ponytail: name-regex buckets, first match wins; fine for a summary, use NSight for exact phases.
BUCKETS = [
    ("nccl", re.compile(r"nccl", re.I)),
    ("attention", re.compile(r"attention|flash|softmax|bmm", re.I)),
    ("gemm", re.compile(r"gemm|matmul|cutlass|sm90|sm100|sm120|xmma", re.I)),
    ("optimizer", re.compile(r"foreach|adam|muon|optim", re.I)),
]
TRACE_STEPS = 3


def bucket_of(name: str) -> str:
    return next((b for b, rx in BUCKETS if rx.search(name)), "other")


def trace_summary(
    prof: Any, stamps: list[float], mem_end: list[int], device: Any
) -> dict[str, Any]:
    dev_us = {b: 0.0 for b, _ in BUCKETS} | {"other": 0.0}
    cpu_us = dict(dev_us)
    for e in prof.key_averages():
        b = bucket_of(e.key)
        dev_us[b] += e.self_device_time_total
        cpu_us[b] += e.self_cpu_time_total
    cuda = device.type == "cuda"
    return {
        "profiled_steps": TRACE_STEPS,
        "cuda_time_us": dev_us,
        "cpu_time_us": cpu_us,
        "host_us_per_step": sum(cpu_us.values()) / TRACE_STEPS,
        "step_seconds": [b - a for a, b in zip(stamps, stamps[1:], strict=False)],
        "peak_mem_gib": torch.cuda.max_memory_allocated(device) / 2**30 if cuda else None,
        "mem_at_step_end_gib": [m / 2**30 for m in mem_end],
    }


def one_round(
    body: dict[str, Any],
    device: torch.device,
    reduction: str,
    trace: Path | None = None,
    warmup: int = 0,
) -> dict[str, Any]:
    cfg, lay, run_id = island_from_manifest(body)
    comm = DistComm()
    # dp1 profiles (mfu-dense-1b) carry zero1=False: a single rank has nothing to shard, so it
    # runs the plain all_gather path (a self-copy) whatever --reduction says.
    single = comm.world == 1
    check_launch(lay, comm.world, dp_size=comm.world, ep_size=1, zero1=not single)
    n = cfg.inner.H * cfg.inner.micro_batch * cfg.inner.grad_accum * comm.world
    rows = np.random.default_rng(0).integers(
        0, cfg.model.vocab, (n, cfg.model.seq_len + 1), dtype=np.uint32
    )
    theta = {k: v.to(device) for k, v in init_params(cfg.model).items()}
    sync = torch.cuda.synchronize if device.type == "cuda" else (lambda: None)
    stamps: list[float] = []
    prof: Any = None
    mine_trace: dict[str, Any] = {}
    mem_end: list[int] = []
    # Peak inside training steps only: max_memory_allocated also counts the round-end clones
    # (full theta/state, delta, error feedback, compress), which do not limit micro_batch.
    step_peak = [0]
    if trace is not None:
        from torch.profiler import ProfilerActivity, profile, schedule

        acts = [ProfilerActivity.CPU]
        if device.type == "cuda":
            acts.append(ProfilerActivity.CUDA)
        # step k+1 starts after the k-th after_step; record steps warmup+1..warmup+3
        prof = profile(
            activities=acts, schedule=schedule(wait=warmup, warmup=0, active=TRACE_STEPS, repeat=1)
        )
        prof.start()

    def after_step(t: int, _theta: Any) -> None:
        sync()
        stamps.append(time.perf_counter())
        if device.type == "cuda":
            step_peak[0] = max(step_peak[0], torch.cuda.max_memory_allocated(device))
            torch.cuda.reset_peak_memory_stats(device)
        if prof is not None:
            if warmup <= len(stamps) - 1 < warmup + TRACE_STEPS and device.type == "cuda":
                mem_end.append(torch.cuda.memory_allocated(device))
            prof.step()

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
        reduction="all_gather" if single else REDUCTIONS[reduction],
        after_step=after_step,
    )
    if prof is not None and trace is not None:
        prof.stop()
        r = dist.get_rank()
        trace.mkdir(parents=True, exist_ok=True)
        prof.export_chrome_trace(str(trace / f"rank{r}.json"))
        span = stamps[max(warmup - 1, 0) : warmup + TRACE_STEPS]
        mine_trace = trace_summary(prof, span if warmup else [t0, *span], mem_end, device)
        if device.type == "cuda":  # per-step resets above would hide the in-step peak
            mine_trace["peak_mem_gib"] = (
                max(step_peak[0], torch.cuda.max_memory_allocated(device)) / 2**30
            )
    mine: dict[str, Any] = {
        "step_seconds": [b - a for a, b in zip([t0, *stamps], stamps, strict=False)],
        "leaves_root": res.leaves_root,
        "peak_mem_gib": round(max(step_peak[0], torch.cuda.max_memory_allocated(device)) / 2**30, 3)
        if device.type == "cuda"
        else None,
        "step_peak_mem_gib": round(step_peak[0] / 2**30, 3) if device.type == "cuda" else None,
    }
    if prof is not None:
        mine["trace"] = mine_trace
    ranks: list[Any] = [None] * comm.world
    dist.all_gather_object(ranks, mine)  # outside train_island: host metadata, not verified tensors
    return {
        "run_id": run_id,
        "leaves_root": ranks[0]["leaves_root"],
        "rank_leaves_roots": [r["leaves_root"] for r in ranks],
        "step_seconds": [max(s) for s in zip(*(r["step_seconds"] for r in ranks), strict=True)],
        "rank_peak_mem_gib": [r["peak_mem_gib"] for r in ranks],
        "rank_step_peak_mem_gib": [r["step_peak_mem_gib"] for r in ranks],
        "tokens_per_step": cfg.inner.micro_batch
        * cfg.inner.grad_accum
        * cfg.model.seq_len
        * comm.world,
        "flops_per_token": flops_per_token(cfg.model),
        "n_gpus": lay.n_gpus,
        "dp_size": lay.dp_size,
        "attention_backend": select_backend(device),
        "attention_paths": chosen_paths(),
        **({"trace": [r["trace"] for r in ranks]} if prof is not None else {}),
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
    ap.add_argument(
        "--compile", action="store_true", help="sets HT_COMPILE=1, which the run's pin refuses"
    )
    ap.add_argument("--trace", type=Path, help="profile 3 steps of round 1 into this dir")
    ap.add_argument("--nccl-pcie", action="store_true", help="setdefault PCIe-only NCCL knobs")
    ap.add_argument("--micro-batch", type=int, help="override the profile's per-rank micro_batch")
    a = ap.parse_args()
    if a.nccl_pcie:
        for k, v in NCCL_PCIE.items():
            os.environ.setdefault(k, v)
    # Allocator layout only (numerics unchanged). Live: the 1B dp2 step OOMed on a 31.36 GiB
    # host with 29.87 GiB allocated plus ~480 MiB of fragmented reserve.
    os.environ.setdefault("PYTORCH_CUDA_ALLOC_CONF", "expandable_segments:True")
    if a.act_ckpt:
        os.environ["HT_ACT_CKPT"] = "1"
    if a.compile:
        os.environ["HT_COMPILE"] = "1"
    spec = json.loads(PROFILES.read_text())["profiles"][a.profile]
    if a.micro_batch:
        spec["inner"]["micro_batch"] = a.micro_batch
        os.environ["HT_MFU_MICRO_BATCH"] = str(a.micro_batch)  # lands in engine_flags
    local = int(os.environ.get("LOCAL_RANK", "0"))
    if a.device == "cuda":
        torch.cuda.set_device(local)
        device = torch.device("cuda", local)
    else:
        device = torch.device("cpu")
    pin = honor_pin(device)  # restarts this rank under the pin, so before any process group
    dist.init_process_group("nccl" if a.device == "cuda" else "gloo")
    try:
        body = manifest_body(spec, pin, device)
        run = one_round(body, device, a.reduction, a.trace, a.warmup)
        replay = one_round(body, device, a.reduction)
        if dist.get_rank() != 0:
            return 0
        if a.trace is not None:
            per_rank = {f"rank{r}": t for r, t in enumerate(run.pop("trace"))}
            (a.trace / "summary.json").write_text(json.dumps(per_rank, indent=1))
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
            "nccl_env": {k: v for k, v in sorted(os.environ.items()) if k.startswith("NCCL_")},
        }
        ev["tokens_per_s"] = ev["vendor"]["tokens_per_s"]
        ev["mfu"] = ev["vendor"]["mfu"]
        mems = [m for m in run["rank_peak_mem_gib"] if m is not None]
        ev["max_peak_mem_gib"] = max(mems) if mems else None
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
