"""Controller-supplied training/FLOP evidence.

Miner-declared FLOP numbers are never accepted. The envelope must carry a
retained compute trace; the controller (and this helper) recompute the total
from that list. Missing or mismatched evidence refuses success.

`eval/baselines/adamw.py` is a parameter lock, not executable training, and
is not this envelope. A synthetic one-matmul fixture does not support every
recipe.
"""

from __future__ import annotations

import json
import os
import re
from pathlib import Path
from typing import Any

from .compute_trace import verify_trace
from .contract import ContractError

_DIGEST = re.compile(r"^[0-9a-f]{64}$")


def load_training_evidence(path: str | Path | None = None) -> dict[str, Any]:
    raw_path = path or os.environ.get("PROOF_TRAINING_EVIDENCE_FILE")
    if not raw_path or not str(raw_path).strip():
        raise ContractError("training evidence missing; refuse scoring")
    file = Path(raw_path)
    if file.is_symlink() or not file.is_file():
        raise ContractError("training evidence must be a local regular file")
    try:
        data = json.loads(file.read_text(encoding="utf-8"))
    except (OSError, ValueError) as exc:
        raise ContractError(f"unreadable training evidence: {exc}") from exc
    return verify_training_evidence(data)


def verify_training_evidence(data: Any) -> dict[str, Any]:
    if not isinstance(data, dict) or data.get("schema_version") != 1:
        raise ContractError("training evidence schema is not 1")
    script = data.get("script_digest")
    log = data.get("log_digest")
    seed = data.get("seed")
    claimed = data.get("flops_used")
    if not isinstance(script, str) or not _DIGEST.fullmatch(script):
        raise ContractError("training evidence script_digest is malformed")
    if not isinstance(log, str) or not _DIGEST.fullmatch(log):
        raise ContractError("training evidence log_digest is malformed")
    if not isinstance(seed, int) or seed < 0:
        raise ContractError("training evidence seed is malformed")
    if not isinstance(claimed, int) or claimed < 1:
        raise ContractError("training evidence flops_used is missing")
    counted = verify_trace(data.get("trace"))
    if counted != claimed:
        raise ContractError("training FLOPs do not match the retained trace")
    return {
        "schema_version": 1,
        "script_digest": script,
        "log_digest": log,
        "seed": seed,
        "flops_used": counted,
        "trace": data["trace"],
    }
