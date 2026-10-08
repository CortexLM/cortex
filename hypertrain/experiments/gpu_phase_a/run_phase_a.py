"""One Phase A run: tiny BF16 MoE round via the trainer public API; writes leaves + env to JSON.

Determinism env (CUBLAS_WORKSPACE_CONFIG) is pinned by hypertrain.trainer before torch import.
Usage: run_phase_a.py --profile phase_a|tiny --device cuda|cpu --shard PATH --shard-sha256 HEX
       --out OUT.json [--negative] [--image-digest sha256:...]
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import platform
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

from hypertrain.trainer import DETERMINISM  # must precede torch (sets CUBLAS_WORKSPACE_CONFIG)

# isort: split
import numpy as np
import torch

from hypertrain.protocol.example import example_manifest
from hypertrain.protocol.messages import RunManifest
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.loop import Assignment, train_round
from hypertrain.trainer.model import init_params, param_shapes

HERE = Path(__file__).resolve().parent
EXPECTED_SM_COUNT = 170
EXPECTED_TORCH = "2.14.0+cu130"
NVIDIA_SMI = ["nvidia-smi", "--query-gpu=driver_version,uuid,pci.bus_id", "--format=csv,noheader"]


def manifest_for(profile: dict[str, Any], image_digest: str | None) -> RunManifest:
    body: dict[str, Any] = dict(example_manifest().body())
    model = {**body["model"], **profile["model"]}
    inner = {**body["inner"], **profile["inner"]}
    body["model"], body["inner"] = model, inner
    if image_digest:
        body["reference_spec"] = {**body["reference_spec"], "image_digest": image_digest}
    cfg0 = TrainConfig.from_manifest(dict(body, model=dict(model, param_count=1)))
    model["param_count"] = sum(math.prod(s) for s in param_shapes(cfg0.model).values())
    return RunManifest.model_validate(body)


def shard_reader(path: Path, sha: str, seq_len: int) -> Any:
    data = path.read_bytes()
    if hashlib.sha256(data).hexdigest() != sha:
        raise SystemExit(f"shard sha256 mismatch: {path}")
    rows = np.frombuffer(data, dtype="<u4").reshape(-1, 1025)

    def get(i: int) -> np.ndarray[Any, np.dtype[np.uint32]]:
        return np.ascontiguousarray(rows[i, : seq_len + 1]).astype(np.uint32)

    return get, rows.shape[0]


def env_record(device: str) -> dict[str, Any]:
    rec: dict[str, Any] = {
        "torch": torch.__version__,
        "cuda_runtime": torch.version.cuda,
        "python": platform.python_version(),
        "CUBLAS_WORKSPACE_CONFIG": os.environ.get("CUBLAS_WORKSPACE_CONFIG"),
        "CUDA_DISABLE_PTX_JIT": os.environ.get("CUDA_DISABLE_PTX_JIT"),
        "determinism": DETERMINISM,
        "device": device,
    }
    if device == "cuda":
        p = torch.cuda.get_device_properties(0)
        rec |= {
            "gpu_name": p.name,
            "sm_count": p.multi_processor_count,
            "capability": list(torch.cuda.get_device_capability(0)),
        }
        q = subprocess.run(  # noqa: S603, S607  (fixed nvidia-smi query on the rented host)
            NVIDIA_SMI,
            capture_output=True,
            text=True,
            check=False,
        )
        rec["nvidia_smi"] = q.stdout.strip()
        rec["driver_version"] = q.stdout.split(",")[0].strip() if q.returncode == 0 else None
    return rec


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--profile", required=True)
    ap.add_argument("--device", choices=["cuda", "cpu"], required=True)
    ap.add_argument("--shard", type=Path, required=True)
    ap.add_argument("--shard-sha256", required=True)
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("--negative", action="store_true")
    ap.add_argument("--image-digest")
    a = ap.parse_args()
    spec = json.loads((HERE / "phase_a.json").read_text())["profiles"][a.profile]
    env = env_record(a.device)
    if a.device == "cuda":
        if env["torch"] != EXPECTED_TORCH or env["sm_count"] != EXPECTED_SM_COUNT:
            a.out.write_text(json.dumps({"error": "hardware_or_torch_gate", "env": env}))
            return 3
        torch.set_default_device("cuda")
    manifest = manifest_for(spec, a.image_digest)
    cfg = TrainConfig.from_manifest(manifest.body())
    get, n_rows = shard_reader(a.shard, a.shard_sha256, cfg.model.seq_len)
    n = cfg.inner.H * cfg.inner.micro_batch * cfg.inner.grad_accum
    ids = list(range(n))
    neg = spec["negative_control"]
    expected = None
    if a.negative:
        per = cfg.inner.micro_batch * cfg.inner.grad_accum
        ids[(neg["step"] - 1) * per + neg["slot"]] = neg["replacement_id"]
        expected = math.ceil(neg["step"] / cfg.inner.J)
    if max(ids) >= n_rows:
        raise SystemExit("shard too small for assignment")
    run_id = manifest.run_id()
    theta = init_params(cfg.model)
    if a.device == "cuda":
        theta = {k: v.to("cuda") for k, v in theta.items()}
    t0 = time.time()
    res = train_round(cfg, theta, Assignment(run_id, 0, tuple(ids)), get)
    out = {
        "profile": a.profile,
        "negative": a.negative,
        "expected_first_divergent_leaf": expected,
        "run_id": run_id,
        "manifest_sha256": hashlib.sha256(
            json.dumps(manifest.body(), sort_keys=True).encode()
        ).hexdigest(),
        "param_count": manifest.model.param_count,
        "leaves": [d.hex() for d in res.leaf_digests],
        "leaves_root": res.leaves_root,
        "delta_hash": res.delta_hash,
        "final_theta_hash": res.final_theta_hash,
        "seconds": round(time.time() - t0, 3),
        "env": env,
        "argv": sys.argv,
    }
    a.out.write_text(json.dumps(out, indent=1, sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main())
