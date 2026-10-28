"""In-container GPU probe for scripts/b300/run_host.sh (step c): torch build, devices, determinism.

Records the torch build (version, CUDA, compiled arch list), every visible device (name,
capability, SM count, memory) and a bitwise determinism smoke. The smoke runs, on each device and
several times, a fixed fp32 and bf16 matmul, the production attention geometry (bf16, 16 q heads,
8 kv heads, head dim 128, forward and backward) and one kimi-k3-pro-tiny MICRO round (bf16, eager
MLA and the MoE sort). The attention backend is the one select_backend() picks for that device,
HT_ATTN_BACKEND included: "math" on sm_103 until a backend is qualified there, flash-SDPA on
RTX 5090. It passes only when every repeat on every device yields the same digests, the three
MICRO roots included. Inputs are generated on the CPU, so all devices see the same bytes.
A manifest pins its environment and a trainer refuses a process that does not honor the pin, so
the kimi round runs in a child process started under the pin of the device's class (CPU class
avx2; GPU class sm_103, sm_120 or none, from the device's compute capability). This process stays
unpinned: pinning HT_ATTN_BACKEND here would make every device answer "math" to the smoke above.
A device whose compute capability has no row in hypertrain.protocol.env_pins.GPU_CLASSES cannot
be pinned and fails the probe with that error.
A kernel error (a RuntimeError, e.g. no kernel image for the device) is recorded as the digest
value; any other exception is a defect and surfaces.

The image must contain hypertrain.protocol.env_pins.class_pin and hypertrain.models.kimi_tiny.
One built before them (the default pin of run_host.sh predates both) fails here: without
class_pin the script does not load, without kimi_tiny the child dies with ImportError (cannot
import name 'kimi_tiny') and the probe ends with a ChildProcessError that says so. Pass --image of
a newer build.

Usage (inside the GPU image, HT_* env as the miner runs it):
  python scripts/b300/gpu_probe.py --out /results/gpu_probe.json --expect-gpus 8 --expect-cc 10.3
Exit 0 when every check passes, 1 otherwise. When the probe raises (any exception,
KeyboardInterrupt and SystemExit included) the JSON at --out becomes
{"ok": false, "error": "<Type>: <message>"} and the exception still ends the process non-zero. A
kill (docker stop) or bad arguments leave an old file in place: run_host.sh removes it first.
"""

from __future__ import annotations

import argparse
import dataclasses
import hashlib
import json
import os
import subprocess
import sys
from pathlib import Path
from typing import Any

from hypertrain.trainer import DETERMINISM  # must precede torch

# isort: split
import torch

from hypertrain.protocol.env_pins import EnvPin, class_pin, gpu_label, pinned_environ
from hypertrain.trainer.attention import Backend, attention, chosen_paths, select_backend

GEOMETRY = {"batch": 1, "heads": 16, "kv_heads": 8, "seq": 256, "head_dim": 128}
KIMI_CPU_CLASS = "avx2"
KIMI_ROUND_FLAG = "--kimi-round"  # the child form of this script, see kimi_round()
KIMI_ROUND_TIMEOUT = 1200  # seconds: a MICRO round takes seconds, the bound only ends a hang


def digest(t: torch.Tensor) -> str:
    return hashlib.sha256(t.detach().contiguous().cpu().view(torch.uint8).numpy()).hexdigest()


def kimi_pin(device: torch.device) -> EnvPin:
    """The pin of the device's class: its compute capability for CUDA, none for cpu and meta."""
    capability = torch.cuda.get_device_capability(device) if device.type == "cuda" else None
    return EnvPin.from_body(class_pin(KIMI_CPU_CLASS, gpu_label(capability)))


def kimi_round(device: torch.device) -> dict[str, str]:
    """The roots of one kimi-k3-pro-tiny MICRO round (bf16) on ``device``, run by a child.

    The child is this script (``KIMI_ROUND_FLAG``) started under ``pinned_environ`` of the
    device's pin, which its trainer verifies. A RuntimeError of the round (a kernel error) is a
    device result and is raised here as one; a child that dies otherwise is a defect.
    """
    pin = kimi_pin(device)
    argv = [sys.executable, __file__, KIMI_ROUND_FLAG, str(device), json.dumps(pin.body())]
    done = subprocess.run(  # noqa: S603 - this interpreter and this script, arguments built here
        argv,
        env=pinned_environ(os.environ, pin),
        capture_output=True,
        text=True,
        timeout=KIMI_ROUND_TIMEOUT,
        check=False,
    )
    if done.returncode != 0:
        raise ChildProcessError(
            f"kimi round on {device} exited {done.returncode}: {done.stderr[-1000:].strip()}"
        )
    reply = json.loads(done.stdout)
    if "error" in reply:
        raise RuntimeError(reply["error"])
    roots: dict[str, str] = reply["round"]
    return roots


def kimi_child(device: str, pin_body: str) -> int:
    """The child of kimi_round: run the round here, in a process that honors the pin it is given."""
    # Imported here: an image without the module must fail through main()'s JSON, not while this
    # script loads.
    from hypertrain.models import kimi_tiny

    doc = kimi_tiny.run_manifest(dataclasses.replace(kimi_tiny.MICRO, compute_dtype="bf16"))
    try:
        (res,) = kimi_tiny.qualify(doc, device, 1, EnvPin.from_body(json.loads(pin_body)))
    except RuntimeError as e:
        print(json.dumps({"error": str(e)}))
    else:
        print(json.dumps({"round": res}))
    return 0


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
        res = kimi_round(device)
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
    sys.exit(kimi_child(*sys.argv[2:]) if sys.argv[1:2] == [KIMI_ROUND_FLAG] else main())
