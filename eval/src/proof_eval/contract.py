"""Image contract the harvest-pod wrapper and the control plane share.

Markers are also printed by `harvest-pod` from `metrics.json` + exit 0.
This process still prints them so a local `proof-eval score` transcript is
the same shape as a harvested run.
"""

from __future__ import annotations

import json
from typing import Any

METRICS_MARKER = "PROOF_METRICS="
OK_MARKER = "PROOF_EVAL_OK"
POD_WORKDIR = "/tmp/proof_eval"
SCORE_BINARY = "/usr/bin/proof-eval"
PROOF_METRICS_SCHEMA = 1
BAKED_PROXIES_PATH = "/opt/proof-eval/baked_proxies.json"
ADAMW_SCRIPT = "/opt/proof-eval/baselines/adamw.py"

# Default proxy the scoring image is declared to contain. Must stay in the
# Qwen/Qwen3.8 family the control-plane pin locks.
DEFAULT_PROXY = "Qwen/Qwen3.8-0.6B"


class ContractError(Exception):
    """A request or document this image must refuse."""


def encode_document(document: dict[str, Any]) -> str:
    """Exactly one JSON line, no trailing newline (harvest `cat`s this)."""
    return json.dumps(document, separators=(",", ":"), ensure_ascii=True)


def decode_document(body: str) -> dict[str, Any]:
    return json.loads(body)


def marker_line(document: dict[str, Any]) -> str:
    return f"{METRICS_MARKER}{encode_document(document)}"
