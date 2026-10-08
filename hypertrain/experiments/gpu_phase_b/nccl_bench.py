"""Intra-host collective bandwidth (torchrun child): all_gather + all_to_all bus bandwidth.

busbw = (n-1)/n * bytes / t (nccl-tests convention; bytes = all_gather output / all_to_all input).
Usage: torchrun --standalone --nproc-per-node N nccl_bench.py --device cuda|cpu --out OUT.json
       [--sizes-mib 16,128,512] [--iters 5]
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import time
from pathlib import Path

import torch
import torch.distributed as dist


def _sync(device: str) -> None:
    if device == "cuda":
        torch.cuda.synchronize()


def bench(device: str, mib: float, iters: int) -> dict[str, float]:
    n, dev = dist.get_world_size(), torch.device(device, int(os.environ.get("LOCAL_RANK", "0")))
    numel = max(n, int(mib * 2**20) // 4 // n * n)
    x = torch.ones(numel, dtype=torch.float32, device=dev)
    gathered = torch.empty(numel * n, dtype=torch.float32, device=dev)
    a2a = torch.empty_like(x)
    out: dict[str, float] = {"mib_per_rank": numel * 4 / 2**20}
    for name, op, nbytes in (
        ("all_gather", lambda: dist.all_gather_into_tensor(gathered, x), numel * 4 * n),
        ("all_to_all", lambda: dist.all_to_all_single(a2a, x), numel * 4),
    ):
        for _ in range(2):
            op()
        _sync(device)
        dist.barrier()
        t0 = time.perf_counter()
        for _ in range(iters):
            op()
        _sync(device)
        dt = (time.perf_counter() - t0) / iters
        out[f"{name}_ms"] = dt * 1e3
        out[f"{name}_busbw_GBps"] = (n - 1) / n * nbytes / dt / 1e9
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--device", choices=["cuda", "cpu"], required=True)
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("--sizes-mib", default="16,128,512")
    ap.add_argument("--iters", type=int, default=5)
    a = ap.parse_args()
    local = int(os.environ.get("LOCAL_RANK", "0"))
    if a.device == "cuda":
        torch.cuda.set_device(local)
        dist.init_process_group("nccl", device_id=torch.device("cuda", local))
    else:
        dist.init_process_group("gloo")
    try:
        rows = [bench(a.device, float(s), a.iters) for s in a.sizes_mib.split(",")]
        if dist.get_rank() == 0:
            meta = {"world": dist.get_world_size(), "backend": dist.get_backend()}
            if a.device == "cuda":
                meta["nccl"] = ".".join(map(str, torch.cuda.nccl.version()))
            a.out.write_text(json.dumps({**meta, "rows": rows}, indent=1))
    finally:
        dist.destroy_process_group()
    return 0


if __name__ == "__main__":
    sys.exit(main())
