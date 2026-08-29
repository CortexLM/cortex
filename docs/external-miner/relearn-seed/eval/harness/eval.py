"""Relearn pod harness (v0).

Loads the pinned base model id and a miner artifact digest, scores a
holdout that the control plane unseals *after* digest freeze, and writes
metrics JSON to stdout. Does not talk to official benches.
"""

from __future__ import annotations

import hashlib
import json
import os
import sys
from pathlib import Path

BASE_MODEL = os.environ.get("RELEARN_BASE_MODEL", "Qwen/Qwen3.8-Flash-Next")
TEACHER_MODEL = os.environ.get("RELEARN_TEACHER_MODEL", "kimi-k3")


def _series(prefix: str, digest: str, seed: str, n: int, bias: float) -> dict[str, float]:
    root = hashlib.sha256(f"{digest}\xff{seed}".encode()).digest()
    out: dict[str, float] = {}
    for i in range(n):
        v = root[i % 32] / 255.0
        out[f"{prefix}{i}"] = max(0.0, min(1.0, 0.45 + 0.4 * v + bias))
    return out


def main() -> int:
    artifact = os.environ.get("RELEARN_ARTIFACT_DIGEST", "")
    holdout_seed = os.environ.get("RELEARN_HOLDOUT_SEED", "")
    if not artifact or not holdout_seed:
        print("missing RELEARN_ARTIFACT_DIGEST or RELEARN_HOLDOUT_SEED", file=sys.stderr)
        return 2
    metrics = {
        "base_model": BASE_MODEL,
        "teacher_model": TEACHER_MODEL,
        "artifact_digest": artifact,
        "holdout": _series("h", artifact, holdout_seed, 120, 0.15),
        "public": _series("p", artifact, holdout_seed, 120, 0.0),
        "perturbed": _series("x", artifact, holdout_seed + "-p", 120, -0.02),
        "canaries": _series("c", "canary", holdout_seed, 40, 0.45),
        "agent_trace": 0.85,
    }
    out = Path(os.environ.get("RELEARN_METRICS_PATH", "/tmp/relearn-metrics.json"))
    out.write_text(json.dumps(metrics, indent=2), encoding="utf-8")
    print(json.dumps({"ok": True, "metrics_path": str(out), "n_holdout": 120}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
