"""Harness-owned metrics. Never agent-authored, never simulated.

Without a scoring runtime (torch) this module refuses rather than inventing
NLL or tokens/sec. That is the contract-only image: it can still enforce
fabric and inspect a recipe, but it cannot emit a document the control
plane would pay on.
"""

from __future__ import annotations

import hashlib
import os
from pathlib import Path
from typing import Any

from .contract import ContractError
from .request import HarvestRequest

SCORED_SPLITS = ("web_ood", "code_ood", "math_ood", "longctx", "multilingual_ood")


def require_runtime() -> None:
    try:
        import torch  # noqa: F401
        import transformers  # noqa: F401
    except ImportError as exc:
        raise ContractError(
            f"no model runtime: {exc}; this image cannot score (contract-only builds refuse)"
        ) from exc


def _shard_text(rec: dict[str, Any]) -> str:
    """Load packed shard bytes. Records carry a content hash, never the text.

    Operator primes `PROOF_HOLDOUT_STORE/<content_sha256>`. Missing bytes are
    a 503, not an invented NLL.
    """
    digest = str(rec.get("content_sha256") or "").strip().lower()
    if len(digest) != 64:
        raise ContractError(f"record {rec.get('id')} has a malformed content_sha256")
    store = Path(os.environ.get("PROOF_HOLDOUT_STORE", "/opt/proof-eval/holdout"))
    path = store / digest
    if not path.is_file():
        raise ContractError(
            f"holdout shard {digest[:12]}… is not in PROOF_HOLDOUT_STORE; refuse scoring"
        )
    return path.read_text(encoding="utf-8", errors="replace")


def measure(request: HarvestRequest, artifact_dir: str | None) -> dict[str, Any]:
    """Measure holdout NLL + optional throughput.

    A missing runtime is a failed run, not a zero. Hash-derived numbers are
    forbidden here: they would be a sim fallback inside the live image.
    """
    require_runtime()
    import torch
    from transformers import AutoModelForCausalLM, AutoTokenizer

    proxy = (request.proxy_model or request.model_ref or "").strip()
    if not (artifact_dir or proxy):
        raise ContractError("no model to measure")
    try:
        tok = AutoTokenizer.from_pretrained(proxy, trust_remote_code=True)
        model = AutoModelForCausalLM.from_pretrained(
            artifact_dir or proxy,
            torch_dtype=torch.bfloat16 if torch.cuda.is_available() else torch.float32,
            trust_remote_code=True,
        )
    except Exception as exc:  # noqa: BLE001
        raise ContractError(f"no model: {exc}") from exc
    model.eval()
    device = torch.device("cuda" if torch.cuda.is_available() else "cpu")
    model.to(device)

    split_nll: dict[str, list[float]] = {s: [] for s in SCORED_SPLITS}
    texts = []
    for rec in request.holdout:
        split = str(rec.get("split") or rec.get("task") or "web_ood")
        if split not in split_nll:
            split = "web_ood"
        texts.append((split, _shard_text(rec)))

    nlls: list[float] = []
    tokens = 0
    import time

    t0 = time.perf_counter()
    with torch.no_grad():
        for split, text in texts:
            enc = tok(text, return_tensors="pt", truncation=True, max_length=1024)
            enc = {k: v.to(device) for k, v in enc.items()}
            out = model(**enc, labels=enc["input_ids"])
            nll = float(out.loss.detach().cpu())
            split_nll[split].append(nll)
            nlls.append(nll)
            tokens += int(enc["input_ids"].numel())
    wall = max(time.perf_counter() - t0, 1e-6)
    mean = sum(nlls) / max(len(nlls), 1)
    per_split = {
        name: (sum(vals) / len(vals) if vals else mean) for name, vals in split_nll.items()
    }
    tps = tokens / wall if request.family == "throughput" else None
    return {
        "holdout_nll": mean,
        "split_nll": per_split,
        "public_nll": None,
        "tokens_per_sec": tps,
        "step_latency_ms": None,
        "wall_s": int(wall) if request.family == "throughput" else None,
        "custom_value": None,
        "canary_nll": None,
        "artifact_fingerprint": hashlib.sha256((artifact_dir or proxy).encode()).hexdigest()[:16],
    }
