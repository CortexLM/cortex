"""Phase B worker: one DiLoCo island round (todo-11 layout) per torchrun rank; rank 0 writes JSON.

Arms (same model, assignment, layout, optimizer, compression):
  det  = the verified deterministic round: leaves, delta hash, top-k index/value set hashes.
  base = nondeterministic baseline for the overhead ratio: torch determinism flags off, dense
         gradients by NCCL all_reduce instead of all_gather + fixed-order sum, no leaf/state
         hashing. It still runs the ZeRO-1 gathers that the leaves need (bias: ratio understated).
Determinism env (CUBLAS_WORKSPACE_CONFIG) is pinned by hypertrain.trainer before torch import.
Usage: torchrun --standalone --nproc-per-node 8 run_phase_b.py --arm det|base
       --profile phase_b|tiny --device cuda|cpu --shard PATH --shard-sha256 HEX --out OUT.json
       [--image-digest sha256:...]
"""

from __future__ import annotations

import argparse
import contextlib
import datetime
import hashlib
import json
import math
import os
import platform
import struct
import subprocess
import sys
import time
from collections.abc import Callable, Iterator
from pathlib import Path
from typing import Any

from hypertrain.trainer import DETERMINISM  # must precede torch (sets CUBLAS_WORKSPACE_CONFIG)

# isort: split
import numpy as np
import torch
import torch.distributed as dist
from torch import Tensor

import hypertrain
from hypertrain.protocol.example import example_manifest
from hypertrain.trainer import island
from hypertrain.trainer.compress import decompress
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.island import Comm, DistComm, Geometry, island_from_manifest, train_island
from hypertrain.trainer.loop import Assignment, LeafRecord
from hypertrain.trainer.model import init_params, param_shapes

HERE = Path(__file__).resolve().parent
EXPECTED_SM_COUNT = 170
MEM_LIMIT_FRAC = 0.85  # probe fails if any rank reserves more of its GPU (NCCL headroom)
EXPECTED_TORCH = "2.14.0+cu130"
NVIDIA_SMI = ["nvidia-smi", "--query-gpu=driver_version,uuid,pci.bus_id", "--format=csv,noheader"]
Getter = Callable[[int], Any]


def manifest_body(profile: dict[str, Any], image_digest: str | None) -> dict[str, Any]:
    body: dict[str, Any] = json.loads(json.dumps(example_manifest().body()))
    for k in ("model", "inner", "outer"):
        body[k].update(profile[k])
    body["reference_spec"]["layout"] = dict(profile["layout"])
    if image_digest:
        body["reference_spec"]["image_digest"] = image_digest
    cfg0 = TrainConfig.from_manifest(dict(body, model=dict(body["model"], param_count=1)))
    body["model"]["param_count"] = sum(math.prod(s) for s in param_shapes(cfg0.model).values())
    return body


def shard_reader(path: Path, sha: str, seq_len: int) -> tuple[Getter, int]:
    data = path.read_bytes()
    if hashlib.sha256(data).hexdigest() != sha:
        raise SystemExit(f"shard sha256 mismatch: {path}")
    rows = np.frombuffer(data, dtype="<u4").reshape(-1, 1025)

    def get(i: int) -> np.ndarray[Any, np.dtype[np.uint32]]:
        return np.ascontiguousarray(rows[i, : seq_len + 1]).astype(np.uint32)

    return get, rows.shape[0]


def code_sha256() -> str:
    """sha256 over the hypertrain package sources actually imported (image provenance)."""
    root = Path(hypertrain.__file__).resolve().parent
    h = hashlib.sha256()
    for p in sorted(root.rglob("*.py")):
        h.update(str(p.relative_to(root)).encode() + b"\0" + p.read_bytes() + b"\0")
    return h.hexdigest()


def topk_hashes(payload: bytes) -> tuple[str, str]:
    """(index-set, value-set) sha256 of an ht-sparse-v1 payload, read from its tensor bodies."""
    codec, dec = decompress(payload)  # validates framing and every body
    if codec != "sparseloco":
        raise ValueError("Phase B expects a sparseloco payload")
    hi, hv = hashlib.sha256(), hashlib.sha256()
    off = len(b"ht-sparse-v1") + 4
    for name in sorted(dec, key=lambda s: s.encode()):
        (ln,) = struct.unpack_from("<Q", payload, off)
        off += 8 + ln
        (ndim,) = struct.unpack_from("<I", payload, off)
        off += 4 + 8 * ndim
        (blen,) = struct.unpack_from("<Q", payload, off)
        body = payload[off + 8 : off + 8 + blen]
        off += 8 + blen
        (k,) = struct.unpack_from("<Q", body)
        idx = np.frombuffer(body, "<u4", k, 16).astype(np.int64)
        flat = dec[name].reshape(-1)
        hi.update(name.encode() + b"\0" + body[16 : 16 + 4 * k])
        hv.update(name.encode() + b"\0" + flat[torch.from_numpy(idx)].numpy().tobytes())
    return hi.hexdigest(), hv.hexdigest()


class OpenComm:
    """Baseline comm: same collectives as DistComm, but not one (so reductions are allowed)."""

    def __init__(self, inner: DistComm) -> None:
        self.inner, self.rank, self.world = inner, inner.rank, inner.world

    def all_gather(self, t: Tensor) -> list[Tensor]:
        return self.inner.all_gather(t)

    def all_to_all(self, x: Tensor, send: list[int], recv: list[int]) -> Tensor:
        return self.inner.all_to_all(x, send, recv)

    def all_reduce_sum(self, t: Tensor) -> Tensor:
        return self.inner.all_reduce_sum(t)


def _fast_reduce(comm: Comm, geo: Geometry, grads: dict[str, Tensor], _r: str) -> dict[str, Tensor]:
    """DDP-style: one all_reduce bucket for replicated tensors; single-replica experts local."""
    names = sorted(grads)
    shared = [n for n in names if len(geo.replicas(geo.my_piece(n))) == comm.world]
    if any(len(geo.replicas(geo.my_piece(n))) not in (1, comm.world) for n in names):
        raise ValueError("baseline supports dp_size == 1 expert sharding only")
    out = {n: grads[n] / geo.lay.n_gpus for n in names if n not in shared}
    if shared:
        flat = comm.all_reduce_sum(torch.cat([grads[n].reshape(-1) for n in shared]))
        off = 0
        for n in shared:
            k = grads[n].numel()
            out[n] = flat[off : off + k].view_as(grads[n]) / geo.lay.n_gpus
            off += k
    return out


@contextlib.contextmanager
def baseline_mode() -> Iterator[None]:
    """Nondeterministic kernels + all_reduce + no leaf/state hashing, restored on exit."""
    saved = {n: getattr(island, n) for n in ("_reduce", "_make_leaf", "_norm", "state_hash")}
    island._reduce = _fast_reduce  # type: ignore[assignment]
    island._make_leaf = lambda *a, **k: LeafRecord(None, b"\0" * 32)  # type: ignore[arg-type]
    island._norm = lambda a, b: 0.0
    island.state_hash = lambda t: "0" * 64
    torch.use_deterministic_algorithms(False)
    torch.backends.cudnn.benchmark, torch.backends.cudnn.deterministic = True, False
    torch.backends.cudnn.allow_tf32 = torch.backends.cuda.matmul.allow_tf32 = True
    torch.backends.cuda.matmul.allow_bf16_reduced_precision_reduction = True
    try:
        yield
    finally:
        for n, f in saved.items():
            setattr(island, n, f)
        from hypertrain.trainer.determinism import setup_determinism

        setup_determinism(DETERMINISM["num_threads"])


def run_round(
    spec: dict[str, Any],
    comm: Comm,
    place: Callable[[Tensor], Tensor],
    get: Getter,
    n_rows: int,
    image_digest: str | None = None,
) -> dict[str, Any]:
    """One H-step island round on this rank; every rank returns the same hashes."""
    body = manifest_body(spec, image_digest)
    cfg, lay, run_id = island_from_manifest(body)
    n = cfg.inner.H * cfg.inner.micro_batch * cfg.inner.grad_accum * lay.n_gpus
    if n > n_rows:
        raise SystemExit("shard too small for assignment")
    theta = {k: place(v) for k, v in init_params(cfg.model).items()}
    t0 = time.perf_counter()
    res = train_island(cfg, lay, comm, theta, Assignment(run_id, 0, tuple(range(n))), get)
    seconds = time.perf_counter() - t0
    ti, tv = topk_hashes(res.delta_payload)
    tokens = n * cfg.model.seq_len
    return {
        "run_id": run_id,
        "manifest_sha256": hashlib.sha256(json.dumps(body, sort_keys=True).encode()).hexdigest(),
        "param_count": body["model"]["param_count"],
        "layout": body["reference_spec"]["layout"],
        "n_gpus": lay.n_gpus,
        "H": cfg.inner.H,
        "J": cfg.inner.J,
        "tokens": tokens,
        "seconds": round(seconds, 3),
        "tok_per_s": round(tokens / seconds, 3),
        "leaves": [d.hex() for d in res.leaf_digests],
        "leaves_root": res.leaves_root,
        "delta_hash": res.delta_hash,
        "final_theta_hash": res.final_theta_hash,
        "topk_index_sha256": ti,
        "topk_value_sha256": tv,
    }


def env_record(device: str, local_rank: int) -> dict[str, Any]:
    rec: dict[str, Any] = {
        "torch": torch.__version__,
        "cuda_runtime": torch.version.cuda,
        "python": platform.python_version(),
        "CUBLAS_WORKSPACE_CONFIG": os.environ.get("CUBLAS_WORKSPACE_CONFIG"),
        "CUDA_DISABLE_PTX_JIT": os.environ.get("CUDA_DISABLE_PTX_JIT"),
        "determinism": DETERMINISM,
        "device": device,
        "code_sha256": code_sha256(),
        "hypertrain_path": str(Path(hypertrain.__file__).resolve().parent),
    }
    if device == "cuda":
        n = torch.cuda.device_count()
        props = [torch.cuda.get_device_properties(i) for i in range(n)]
        rec |= {
            "device_count": n,
            "gpu_name": props[local_rank].name,
            "sm_count": props[local_rank].multi_processor_count,
            "sm_counts": sorted({p.multi_processor_count for p in props}),
            "capability": list(torch.cuda.get_device_capability(local_rank)),
            "nccl": ".".join(map(str, torch.cuda.nccl.version())),
        }
        q = subprocess.run(NVIDIA_SMI, capture_output=True, text=True, check=False)  # noqa: S603
        rec["nvidia_smi"] = q.stdout.strip()
        rec["driver_version"] = q.stdout.split(",")[0].strip() if q.returncode == 0 else None
    return rec


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--arm", choices=["det", "base"], required=True)
    ap.add_argument("--profile", required=True)
    ap.add_argument("--device", choices=["cuda", "cpu"], required=True)
    ap.add_argument("--shard", type=Path, required=True)
    ap.add_argument("--shard-sha256", required=True)
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("--image-digest")
    ap.add_argument("--steps", type=int, help="probe: H=steps, J=1 (memory/OOM check)")
    a = ap.parse_args()
    if os.environ.get("HT_PHASE_B_FAULT") == "oom_hang":  # test hook: OOM text, then hang
        print("torch.OutOfMemoryError: CUDA out of memory (injected)", flush=True)
        time.sleep(600)
    spec = json.loads((HERE / "phase_b.json").read_text())["profiles"][a.profile]
    if a.steps:
        spec["inner"] = {**spec["inner"], "H": a.steps, "J": 1}
    local = int(os.environ.get("LOCAL_RANK", "0"))
    if a.device == "cuda":
        torch.cuda.set_device(local)
    env = env_record(a.device, local)
    gate_ok = a.device != "cuda" or (
        env["torch"] == EXPECTED_TORCH
        and env["sm_counts"] == [EXPECTED_SM_COUNT]
        and env["device_count"] == spec["layout"]["n_gpus"]
    )
    if not gate_ok:
        if local == 0:
            a.out.write_text(json.dumps({"error": "hardware_or_torch_gate", "env": env}))
        return 3
    if a.device == "cuda":
        dist.init_process_group(
            "nccl", device_id=torch.device("cuda", local), timeout=datetime.timedelta(minutes=5)
        )
        torch.set_default_device(f"cuda:{local}")
        dev = torch.device("cuda", local)
    else:
        dist.init_process_group("gloo")
        dev = torch.device("cpu")
    try:
        cfg_seq = spec["model"]["seq_len"]
        get, n_rows = shard_reader(a.shard, a.shard_sha256, cfg_seq)
        dcomm = DistComm()
        ctx = baseline_mode() if a.arm == "base" else contextlib.nullcontext()
        with ctx:
            comm: Comm = OpenComm(dcomm) if a.arm == "base" else dcomm
            res = run_round(spec, comm, lambda t: t.to(dev), get, n_rows, a.image_digest)
        keys = ("leaves_root", "delta_hash", "final_theta_hash", "seconds")
        mem: dict[str, Any] = {}
        if a.device == "cuda":
            total = torch.cuda.get_device_properties(local).total_memory
            mem = {"peak_mem_frac": round(torch.cuda.max_memory_reserved() / total, 4)}
            mem["peak_mem_gib"] = round(torch.cuda.max_memory_allocated() / 2**30, 3)
        per_rank: list[Any] = [None] * dcomm.world
        mine = {k: res[k] for k in keys} | {"rank": dcomm.rank} | mem
        dist.all_gather_object(per_rank, mine)
        over = [r["rank"] for r in per_rank if r.get("peak_mem_frac", 0) > MEM_LIMIT_FRAC]
        if dcomm.rank == 0:
            out = {
                "arm": a.arm,
                "profile": a.profile,
                **res,
                "ranks_agree": len({(r["leaves_root"], r["delta_hash"]) for r in per_rank}) == 1,
                "per_rank": per_rank,
                "ops": sorted(dcomm.ops),
                "env": env,
                "argv": sys.argv,
                "steps_override": a.steps,
                "mem_limit_frac": MEM_LIMIT_FRAC,
                "ranks_over_mem_limit": over,
            }
            a.out.write_text(json.dumps(out, indent=1, sort_keys=True))
        if over:
            return 4
    finally:
        dist.destroy_process_group()
    return 0


if __name__ == "__main__":
    sys.exit(main())
