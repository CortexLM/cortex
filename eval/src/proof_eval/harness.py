"""Harness-owned metrics. Never agent-authored, never simulated.

Without a scoring runtime (torch) this module refuses rather than inventing
NLL or tokens/sec. That is the contract-only image: it can still enforce
fabric and inspect a recipe, but it cannot emit a document the control
plane would pay on.
"""

from __future__ import annotations

import hashlib
import json
import math
import os
import re
from pathlib import Path
from typing import Any

from .contract import ContractError
from .request import HarvestRequest

SCORED_SPLITS = ("web_ood", "code_ood", "math_ood", "longctx", "multilingual_ood")

def _artifact_path(artifact_dir: str | None) -> Path:
    if not artifact_dir:
        raise ContractError("a local data-only artifact directory is required")
    root = Path(artifact_dir).absolute()
    if root.resolve() != root or not root.is_dir():
        raise ContractError("artifact must be a local directory without symlinks")
    # ponytail: flat safetensors + fast tokenizer exports only; expand audited formats as needed.
    allowed = {
        "config.json", "generation_config.json", "tokenizer.json", "tokenizer_config.json",
        "special_tokens_map.json", "added_tokens.json", "vocab.json", "merges.txt",
        "model.safetensors", "model.safetensors.index.json",
    }

    def check_config(value: Any) -> None:
        if isinstance(value, dict):
            for key, item in value.items():
                if key in {"auto_map", "custom_pipelines", "quantization_config"}:
                    raise ContractError(f"unsupported artifact extension: {key}")
                if key.endswith(("_file", "_path")) and item is not None:
                    if not isinstance(item, str) or item not in allowed or not (root / item).is_file():
                        raise ContractError(f"nonlocal artifact reference: {key}")
                check_config(item)
        elif isinstance(value, list):
            for item in value:
                check_config(item)

    try:
        for path in root.iterdir():
            if path.is_symlink() or not path.is_file():
                raise ContractError("artifact entries must be regular files, not links/directories")
            if path.name not in allowed and not re.fullmatch(r"model-\d{5}-of-\d{5}\.safetensors", path.name):
                raise ContractError(f"unsupported artifact file: {path.name}")
            if path.suffix == ".json":
                data = json.loads(path.read_text())
                if path.name != "tokenizer.json":
                    check_config(data)
                if path.name == "model.safetensors.index.json":
                    weights = data.get("weight_map")
                    if not isinstance(weights, dict) or not weights:
                        raise ContractError("missing safetensors weight_map")
                    for shard in weights.values():
                        if not isinstance(shard, str) or not re.fullmatch(r"model-\d{5}-of-\d{5}\.safetensors", shard) or not (root / shard).is_file():
                            raise ContractError("invalid safetensors shard path")
        for name in ("config.json", "tokenizer.json", "tokenizer_config.json"):
            if not (root / name).is_file():
                raise ContractError(f"missing local artifact {name}")
        if not any((root / name).is_file() for name in ("model.safetensors", "model.safetensors.index.json")):
            raise ContractError("local safetensors weights are required")
    except (OSError, ValueError, TypeError, AttributeError) as exc:
        raise ContractError(f"invalid artifact: {exc}") from exc
    return root


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
    digest = rec.get("content_sha256")
    if not isinstance(digest, str) or not re.fullmatch(r"[0-9a-f]{64}", digest):
        raise ContractError(f"record {rec.get('id')} has a malformed content_sha256")
    store = Path(os.environ.get("PROOF_HOLDOUT_STORE", "/opt/proof-eval/holdout"))
    path = store / digest
    if path.is_symlink() or not path.is_file():
        raise ContractError(
            f"holdout shard {digest[:12]}… is not in PROOF_HOLDOUT_STORE; refuse scoring"
        )
    try:
        body = path.read_bytes()
        if hashlib.sha256(body).hexdigest() != digest:
            raise ContractError("holdout shard content_sha256 mismatch")
        return body.decode("utf-8")
    except (OSError, UnicodeError) as exc:
        raise ContractError(f"unreadable holdout shard: {exc}") from exc


def measure(request: HarvestRequest, artifact_dir: str | None) -> dict[str, Any]:
    """Measure holdout NLL + optional throughput.

    A missing runtime is a failed run, not a zero. Hash-derived numbers are
    forbidden here: they would be a sim fallback inside the live image.
    """
    artifact = _artifact_path(artifact_dir)
    texts = []
    for rec in request.holdout:
        split = rec.get("split") or rec.get("task")
        if not isinstance(split, str) or split not in SCORED_SPLITS:
            raise ContractError(f"unknown or missing holdout split: {split!r}")
        texts.append((split, _shard_text(rec)))
    if {split for split, _ in texts} != set(SCORED_SPLITS):
        raise ContractError("missing required holdout splits")
    require_runtime()
    import torch
    from transformers import AutoModelForCausalLM, AutoTokenizer

    try:
        tok = AutoTokenizer.from_pretrained(
            str(artifact), local_files_only=True, trust_remote_code=False, use_fast=True,
        )
        if not tok.is_fast:
            raise ContractError("only local fast tokenizers are supported")
        model = AutoModelForCausalLM.from_pretrained(
            str(artifact), local_files_only=True, use_safetensors=True,
            torch_dtype=torch.bfloat16 if torch.cuda.is_available() else torch.float32,
            trust_remote_code=False,
        )
    except Exception as exc:  # noqa: BLE001
        raise ContractError(f"no model: {exc}") from exc
    model.eval()
    device = torch.device("cuda" if torch.cuda.is_available() else "cpu")
    model.to(device)

    split_nll: dict[str, list[float]] = {s: [] for s in SCORED_SPLITS}
    nlls: list[float] = []
    tokens = 0
    import time

    t0 = time.perf_counter()
    with torch.no_grad():
        for split, text in texts:
            enc = tok(text, return_tensors="pt", truncation=True, max_length=1024)
            enc = {k: v.to(device) for k, v in enc.items()}
            if enc["input_ids"].numel() < 2:
                raise ContractError("holdout requires at least two tokens per record")
            out = model(**enc, labels=enc["input_ids"])
            nll = float(out.loss.detach().cpu())
            if not math.isfinite(nll):
                raise ContractError("non-finite holdout loss")
            split_nll[split].append(nll)
            nlls.append(nll)
            tokens += int(enc["input_ids"].numel())
    wall = time.perf_counter() - t0
    if not math.isfinite(wall) or wall <= 0:
        raise ContractError("invalid measurement duration")
    mean = sum(nlls) / len(nlls)
    per_split = {
        name: sum(vals) / len(vals) for name, vals in split_nll.items()
    }
    tps = tokens / wall if request.family == "throughput" else None
    if not all(math.isfinite(value) for value in [mean, *per_split.values()]) or (tps is not None and (not math.isfinite(tps) or tps <= 0)):
        raise ContractError("non-finite measured metrics")
    if tokens <= 0:
        raise ContractError("no holdout tokens were processed; refuse scoring")
    return {
        "holdout_nll": mean,
        "split_nll": per_split,
        "public_nll": None,
        "tokens_per_sec": tps,
        "step_latency_ms": None,
        "wall_s": int(wall) if request.family == "throughput" else None,
        "custom_value": None,
        "canary_nll": None,
        "artifact_fingerprint": hashlib.sha256(str(artifact).encode()).hexdigest()[:16],
    }
