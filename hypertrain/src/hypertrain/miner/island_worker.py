"""Torchrun worker. Import determinism before torch; no pickle or implicit CPU CUDA oracle."""

from __future__ import annotations

import hypertrain.trainer  # noqa: F401

# isort: split
import hashlib
import json
import os
import shutil
import subprocess
import sys
import time
from datetime import timedelta
from pathlib import Path

import numpy as np
import numpy.typing as npt
import torch
import torch.distributed as dist

from hypertrain.auditor.replay import pack_state, tensor_root, unpack_state
from hypertrain.gpu_ops.journal import durable_write
from hypertrain.miner.island_launch import IslandFailure, confined, gpu_uuid
from hypertrain.protocol.envelope_v2 import load_json
from hypertrain.protocol.hashing import MerkleTree
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages_v2 import IslandJobV1, RankWorkResult
from hypertrain.trainer.compress import state_hash
from hypertrain.trainer.determinism import apply_reference_env
from hypertrain.trainer.island import DistComm, TraceContext, island_from_manifest, train_island
from hypertrain.trainer.loop import Assignment
from hypertrain.trainer.optim import OptState


def execute(directory: Path, backend: str) -> None:
    job = IslandJobV1.model_validate(load_json((directory / "job.json").read_bytes()))
    cfg, lay, run_id = island_from_manifest(job.manifest.body())
    local_rank = int(os.environ["LOCAL_RANK"])
    match backend:
        case "cuda":
            if not torch.cuda.is_available() or torch.cuda.device_count() != lay.n_gpus:
                raise IslandFailure("exact CUDA device count unavailable")
            torch.cuda.set_device(local_rank)
            ref = job.manifest.training.reference_spec
            if torch.__version__ != "2.14.0+cu130":
                raise IslandFailure("unqualified CUDA torch version")
            if os.environ.get("HT_IMAGE_DIGEST") != ref.image_digest:
                raise IslandFailure("runtime image differs from manifest")
            smi = shutil.which("nvidia-smi")
            if smi is None:
                raise IslandFailure("nvidia-smi unavailable")
            mine = gpu_uuid(torch.cuda.get_device_properties(local_rank).uuid)
            pinned = os.environ.get("HT_GPU_UUIDS")
            if pinned is not None and pinned.split(",")[local_rank] != mine:
                raise IslandFailure("rank device differs from the parent's pinned GPU")
            # nvidia-smi ignores CUDA_VISIBLE_DEVICES: query this rank's device by UUID.
            driver = subprocess.run(  # noqa: S603 - PATH-resolved nvidia-smi, fixed query
                [smi, "-i", mine, "--query-gpu=driver_version", "--format=csv,noheader"],
                check=True,
                capture_output=True,
                text=True,
                timeout=10,
            ).stdout.splitlines()
            if len(driver) != 1 or driver[0].strip() not in ref.driver_allowlist:
                raise IslandFailure("unqualified CUDA driver")
            if torch.cuda.get_device_properties(local_rank).multi_processor_count != ref.sm_count:
                raise IslandFailure("unqualified CUDA SM profile")
            device = torch.device("cuda", local_rank)
            torch.set_default_device(device)
            torch.cuda.reset_peak_memory_stats(device)
            engine = "nccl"
        case "cpu":
            device = torch.device("cpu")
            engine = "gloo"
        case _:
            raise IslandFailure("unsupported execution backend")
    apply_reference_env(job.manifest.training.reference_spec.env.model_dump())
    dist.init_process_group(engine, timeout=timedelta(minutes=5))
    try:

        def blob(name: str, expected: str) -> bytes:
            raw = confined(directory, job.object_paths[name]).read_bytes()
            if hashlib.sha256(raw).hexdigest() != expected:
                raise IslandFailure(f"input hash mismatch: {name}")
            return raw

        theta, carry = unpack_state(blob("start_state", job.start_state_sha256))
        ef, _ = unpack_state(blob("ef_in", job.ef_in_sha256))
        v0, _ = unpack_state(blob("v0", job.v0_sha256))

        def move(p: dict[str, torch.Tensor]) -> dict[str, torch.Tensor]:
            return {n: x.to(device) for n, x in p.items()}

        theta, ef, v0 = move(theta), move(ef), move(v0)
        if cfg.inner.state_policy != "carry":
            if carry is not None:
                raise IslandFailure("unexpected carried optimizer")
        elif carry is None:
            raise IslandFailure("missing carried optimizer")
        else:
            carry.m, carry.v = move(carry.m), move(carry.v)
        raw = confined(directory, job.object_paths["samples"]).read_bytes()
        proofs = json.loads(confined(directory, job.object_paths["sample_proofs"]).read_bytes())
        dataset = job.manifest.training.dataset
        width = cfg.model.seq_len + 1
        dtype = "<u2" if dataset.sample_format.startswith("u16") else "<u4"
        rows = np.frombuffer(raw, dtype=dtype).reshape(len(job.sample_ids), width)
        samples = {}
        for i, row, proof in zip(job.sample_ids, rows, proofs, strict=True):
            if not MerkleTree.verify(
                row.tobytes(),
                i,
                [bytes.fromhex(p) for p in proof],
                bytes.fromhex(dataset.merkle_root),
                dataset.n_samples,
            ):
                raise IslandFailure("sample membership mismatch")
            samples[i] = row.astype(np.uint32)

        def get(i: int) -> npt.NDArray[np.uint32]:
            return samples[i]

        started = time.monotonic_ns()
        out = directory / f"rank-{dist.get_rank()}"
        out.mkdir()
        (out / "checkpoints").mkdir()

        def checkpoint(t: int, theta: dict[str, torch.Tensor], state: OptState) -> None:
            durable_write(out / "checkpoints" / f"{t}.safetensors", pack_state(theta, state))

        traces = []

        def trace(ctx: TraceContext, x: torch.Tensor) -> torch.Tensor:
            raw = x.detach().cpu().contiguous().reshape(-1).view(torch.uint8).numpy().tobytes()
            traces.append(
                {
                    "rank": ctx.rank,
                    "step": ctx.step,
                    "microbatch": ctx.microbatch,
                    "layer": ctx.layer,
                    "op": ctx.op,
                    "shape": list(x.shape),
                    "dtype": str(x.dtype),
                    "sha256": hashlib.sha256(raw).hexdigest(),
                }
            )
            return x

        def deadline() -> None:
            if time.time() >= job.deadline:
                raise IslandFailure("job deadline expired")

        result = train_island(
            cfg,
            lay,
            DistComm(),
            theta,
            Assignment(run_id, job.w, tuple(job.sample_ids), job.global_step0),
            get,
            ef_in=ef,
            carry=carry,
            v0=v0 if cfg.inner.state_policy == "derived" else None,
            hook=trace if os.environ.get("HT_ISLAND_TRACE") == "1" else None,
            cancel=deadline,
            checkpoint=checkpoint,
        )
        allocated = reserved = 0
        total = 1
        if backend == "cuda":
            torch.cuda.synchronize(device)
            allocated = torch.cuda.max_memory_allocated(device)
            reserved = torch.cuda.max_memory_reserved(device)
            total = torch.cuda.get_device_properties(device).total_memory
        work = RankWorkResult(
            rank=dist.get_rank(),
            fingerprint=hashlib.sha256(
                f"{backend}:{local_rank}:{torch.__version__}".encode()
            ).hexdigest(),
            elapsed_ns=time.monotonic_ns() - started,
            allocated_bytes=allocated,
            reserved_bytes=reserved,
            total_bytes=total,
        )
        if os.environ.get("HT_ISLAND_TRACE") == "1":
            durable_write(out / "trace.json", canonicalize(traces))
        durable_write(out / "state.safetensors", pack_state(result.final_theta, result.final_state))
        durable_write(out / "ef.safetensors", pack_state(result.ef_out))
        durable_write(out / "delta.bin", result.delta_payload)
        durable_write(
            out / "leaves.json",
            canonicalize([x.preimage.model_dump(mode="json") for x in result.leaves]),
        )
        durable_write(
            out / "summary.json",
            canonicalize(
                {
                    "job_hash": hashlib.sha256(
                        canonicalize(job.model_dump(mode="json"))
                    ).hexdigest(),
                    "backend": backend,
                    "work": work.model_dump(mode="json"),
                    "optimizer_step": result.final_state.step,
                    "commitments": {
                        "leaves_root": result.leaves_root,
                        "final_theta_hash": result.final_theta_hash,
                        "state_root": tensor_root(result.final_theta, result.final_state),
                        "ef_out_hash": state_hash(result.ef_out),
                        "delta_hash": result.delta_hash,
                        "leaves": [x.digest.hex() for x in result.leaves],
                    },
                }
            ),
        )
    finally:
        dist.destroy_process_group()


if __name__ == "__main__":
    execute(Path(sys.argv[1]), sys.argv[2])
