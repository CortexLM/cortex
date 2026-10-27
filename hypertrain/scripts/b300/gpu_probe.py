"""In-container GPU probe for scripts/b300/run_host.sh (step c): torch build, devices, determinism.

Records the torch build (version, CUDA, compiled arch list), every visible device (name,
capability, SM count, memory) and a bitwise determinism smoke. The smoke runs, on each device and
several times, a fixed fp32 and bf16 matmul, the production attention geometry (bf16, 16 q heads,
8 kv heads, head dim 128, forward and backward) and one kimi-k3-pro-tiny MICRO round (bf16, eager
MLA and the MoE sort). The attention backend is the one select_backend() picks for that device,
HT_ATTN_BACKEND included: "math" on sm_103 until a backend is qualified there, flash-SDPA on
RTX 5090. It passes only when every repeat on every device yields the same digests, the three
MICRO roots included. Inputs are generated on the CPU, so all devices see the same bytes.
A kernel error (a RuntimeError, e.g. no kernel image for the device) is recorded as the digest
value; any other exception is a defect and surfaces.

Usage (inside the GPU image, HT_* env as the miner runs it):
  python scripts/b300/gpu_probe.py --out /results/gpu_probe.json --expect-gpus 8 --expect-cc 10.3
Exit 0 when every check passes, 1 otherwise. The JSON at --out is rewritten on every run. When the
probe raises (any exception, KeyboardInterrupt and SystemExit included) it holds
{"ok": false, "error": "<Type>: <message>"} and the exception still ends the process non-zero.
"""

from __future__ import annotations

import argparse
import dataclasses
import hashlib
import json
import sys
from pathlib import Path
from typing import Any

from hypertrain.trainer import DETERMINISM  # must precede torch

# isort: split
import torch

from hypertrain.models import kimi_tiny
from hypertrain.trainer.attention import Backend, attention, chosen_paths, select_backend

GEOMETRY = {"batch": 1, "heads": 16, "kv_heads": 8, "seq": 256, "head_dim": 128}


def digest(t: torch.Tensor) -> str:
    return hashlib.sha256(t.detach().contiguous().cpu().view(torch.uint8).numpy()).hexdigest()


def workload(device: torch.device) -> dict[str, str]:
    """Digests of one smoke pass on ``device``; a RuntimeError becomes ``error: <message>``."""
    g = torch.Generator().manual_seed(0)
    a, b = (torch.randn(1024, 1024, generator=g) for _ in range(2))
    n, h, kvh, t, hd = (GEOMETRY[k] for k in ("batch", "heads", "kv_heads", "seq", "head_dim"))
    q = torch.randn(n, h, t, hd, generator=g)
    k, v = (torch.randn(n, kvh, t, hd, generator=g) for _ in range(2))
    grad = torch.randn(n, h, t, hd, generator=g)
    out: dict[str, str] = {}
    try:
        out["matmul_fp32"] = digest(a.to(device) @ b.to(device))
        out["matmul_bf16"] = digest(a.to(device, torch.bfloat16) @ b.to(device, torch.bfloat16))
    except RuntimeError as e:
        out["matmul"] = f"error: {e}"
    # The backend production training takes on this device (HT_ATTN_BACKEND included), not a
    # fixed one: on sm_103 that is "math", the numerics the later steps will actually run.
    backend: Backend = select_backend(device)
    try:
        qkv = [x.to(device, torch.bfloat16).requires_grad_() for x in (q, k, v)]
        o = attention(qkv[0], qkv[1], qkv[2], backend)
        o.backward(grad.to(device, torch.bfloat16))
        out[f"{backend}_out"] = digest(o)
        for name, x in zip(("dq", "dk", "dv"), qkv, strict=True):
            assert x.grad is not None
            out[f"{backend}_{name}"] = digest(x.grad)
    except RuntimeError as e:
        out[backend] = f"error: {e}"
    try:
        # One kimi-k3-pro-tiny MICRO round in the miner's dtype: eager MLA and the MoE sort, which
        # the attention smoke above never touches. Its roots must agree across repeats and devices.
        doc = kimi_tiny.run_manifest(dataclasses.replace(kimi_tiny.MICRO, compute_dtype="bf16"))
        (res,) = kimi_tiny.qualify(doc, str(device), 1)
        for root in ("leaves_root", "delta_hash", "state_root"):
            out[f"kimi_micro_{root}"] = res[root]
    except RuntimeError as e:
        out["kimi_micro"] = f"error: {e}"
    if device.type == "cuda":
        torch.cuda.synchronize(device)
    return out


def evaluate(runs: dict[str, list[dict[str, str]]]) -> dict[str, Any]:
    """Bitwise verdict over ``{device: [digests per repeat]}``: one reference, every run equal."""
    passes = [(dev, i, r) for dev, rs in sorted(runs.items()) for i, r in enumerate(rs)]
    if not passes:
        return {"ok": False, "reference": None, "mismatches": ["no run"], "errors": []}
    ref = passes[0][2]
    errors = sorted(
        {
            f"{dev}: {key}: {val}"
            for dev, _, r in passes
            for key, val in r.items()
            if val.startswith("error:")
        }
    )
    mismatches = [f"{dev} repeat {i}" for dev, i, r in passes if r != ref]
    return {
        "ok": not errors and not mismatches,
        "reference": ref,
        "mismatches": mismatches,
        "errors": errors,
    }


def probe(repeats: int, device_type: str) -> dict[str, Any]:
    cuda = device_type == "cuda" and torch.cuda.is_available()
    arch_flags = getattr(torch._C, "_cuda_getArchFlags", lambda: None)()
    devices = [torch.device("cuda", i) for i in range(torch.cuda.device_count())] if cuda else []
    info = []
    for d in devices:
        p = torch.cuda.get_device_properties(d)
        info.append(
            {
                "index": d.index,
                "name": p.name,
                "capability": [p.major, p.minor],
                "sm_count": p.multi_processor_count,
                "total_memory": p.total_memory,
                "select_backend": select_backend(d),
            }
        )
    runs = {
        str(d): [workload(d) for _ in range(repeats)] for d in (devices or [torch.device("cpu")])
    }
    return {
        "kind": "b300-gpu-probe",
        "torch": torch.__version__,
        "torch_cuda": torch.version.cuda,
        "arch_flags": arch_flags,
        "arch_list": arch_flags.split() if arch_flags else [],
        "cuda_available": torch.cuda.is_available(),
        "device_count": len(devices),
        "devices": info,
        "geometry": GEOMETRY,
        "repeats": repeats,
        "determinism": DETERMINISM,
        "attention_paths": chosen_paths(),
        "runs": runs,
        "smoke": evaluate(runs),
    }


def checks(ev: dict[str, Any], gpus: int, cc: tuple[int, int] | None) -> dict[str, bool]:
    caps = {tuple(d["capability"]) for d in ev["devices"]}
    return {
        "torch_cu130": ev["torch"] == "2.14.0+cu130" and ev["torch_cuda"] == "13.0",
        "gpu_count": ev["device_count"] == gpus,
        "capability": cc is None or caps == {cc},
        "same_sm_count": len({d["sm_count"] for d in ev["devices"]}) <= 1,
        "bitwise_smoke": ev["smoke"]["ok"],
    }


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("--repeats", type=int, default=3)
    ap.add_argument("--expect-gpus", type=int, default=8)
    ap.add_argument("--expect-cc", default="10.3", help="major.minor, or 'any'")
    ap.add_argument("--device", choices=["cuda", "cpu"], default="cuda")
    a = ap.parse_args()
    cc: tuple[int, int] | None = None
    if a.expect_cc != "any":
        major, minor = a.expect_cc.split(".")
        cc = (int(major), int(minor))
    ev: dict[str, Any] = {"kind": "b300-gpu-probe", "ok": False}
    try:
        ev |= probe(a.repeats, a.device)
        ev["checks"] = checks(ev, a.expect_gpus, cc)
        ev["ok"] = all(ev["checks"].values())
    except BaseException as e:  # incl. KeyboardInterrupt: the run still fails, the file says so
        ev["ok"] = False
        ev["error"] = f"{type(e).__name__}: {e}"
        raise
    finally:
        # Always replaces the file: a stale ok:true from an earlier run must not survive this one.
        a.out.write_text(json.dumps(ev, indent=1, sort_keys=True))
    print(json.dumps({"ok": ev["ok"], "checks": ev["checks"], "arch_list": ev["arch_list"]}))
    return 0 if ev["ok"] else 1


if __name__ == "__main__":
    sys.exit(main())
