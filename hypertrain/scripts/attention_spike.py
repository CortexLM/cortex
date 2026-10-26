"""Plan 1.1 spike: choose the first deterministic fused attention backend on this GPU.

Rule (plan 1.1): first of sdpa-flash, flash-attn that gives identical fwd+bwd bytes over 20
repeats and max |out - math| < 2e-2 in bf16. Prints JSON; chosen=null means implement Triton (d).
Usage: python scripts/attention_spike.py [--out FILE] (needs CUDA; does not rent anything)
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path
from typing import Any

import hypertrain.trainer  # noqa: F401  (determinism before torch)

# isort: split
import torch

from hypertrain.trainer.attention import attention

REPEATS, TOL = 20, 2e-2
SHAPE = (2, 16, 2048, 128, 8)  # b, h, t, hd, kvh (mfu-dense-1b attention)


def _run(
    backend: str, q: torch.Tensor, k: torch.Tensor, v: torch.Tensor
) -> tuple[str, torch.Tensor]:
    qq, kk, vv = (x.detach().clone().requires_grad_() for x in (q, k, v))
    o = attention(qq, kk, vv, backend)  # type: ignore[arg-type]
    o.float().square().sum().backward()
    h = hashlib.sha256()
    for x in (o, qq.grad, kk.grad, vv.grad):
        assert x is not None
        h.update(x.detach().contiguous().view(torch.uint8).cpu().numpy().tobytes())
    return h.hexdigest(), o.detach()


def spike(device: torch.device) -> dict[str, Any]:
    b, h, t, hd, kvh = SHAPE
    g = torch.Generator(device=device).manual_seed(0)
    q = torch.randn(b, h, t, hd, generator=g, device=device, dtype=torch.bfloat16)
    k = torch.randn(b, kvh, t, hd, generator=g, device=device, dtype=torch.bfloat16)
    v = torch.randn(b, kvh, t, hd, generator=g, device=device, dtype=torch.bfloat16)
    _, ref = _run("math", q, k, v)
    out: dict[str, Any] = {
        "device": torch.cuda.get_device_name(device),
        "torch": torch.__version__,
        "capability": list(torch.cuda.get_device_capability(device)),
        "backends": {},
        "chosen": None,
    }
    for name in ("sdpa-flash", "flash-attn"):
        try:
            runs = [_run(name, q, k, v) for _ in range(REPEATS)]
        except Exception as e:  # unavailable or nondeterministic kernel raised: record and move on
            out["backends"][name] = {"error": f"{type(e).__name__}: {e}"[:300]}
            continue
        err = float((runs[0][1].float() - ref.float()).abs().max())
        stable = len({r[0] for r in runs}) == 1
        out["backends"][name] = {"bitwise_stable": stable, "max_abs_err": err}
        if stable and err < TOL and out["chosen"] is None:
            out["chosen"] = name
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", type=Path)
    a = ap.parse_args()
    if not torch.cuda.is_available():
        print(json.dumps({"error": "cuda unavailable"}))
        return 2
    res = spike(torch.device("cuda", 0))
    text = json.dumps(res, indent=1)
    if a.out:
        a.out.write_text(text)
    print(text)
    return 0 if res["chosen"] else 1


if __name__ == "__main__":
    sys.exit(main())
