"""Call the live InferenceOffer as the RLM judge backend.

Direct auth comes from harvest-pod `teacher.env` (`OPENAI_API_KEY` /
`PROOF_INFERENCE_API_KEY`). Controller-enabled proxy mode keeps the key in
the proxy alone. The key is never read from request.json or logged.
"""

from __future__ import annotations

import json
import os
import urllib.error
import urllib.request
from typing import Any

from .contract import ContractError
from .request import HarvestRequest

_KEY_ENV = ("PROOF_INFERENCE_API_KEY", "OPENAI_API_KEY")


def load_judge_api_key() -> str:
    for name in _KEY_ENV:
        value = os.environ.get(name, "").strip()
        if value:
            return value
    raise ContractError("inference API key missing; refuse scoring")


def judge_url(base_url: str, mode: str) -> str:
    origin = base_url.strip().rstrip("/")
    if mode == "embeddings":
        suffix = "/embeddings"
    elif mode == "completions":
        suffix = "/completions"
    else:
        suffix = "/chat/completions"
    if origin.endswith(suffix):
        return origin
    return origin + suffix


def _payload(request: HarvestRequest) -> dict[str, Any]:
    max_out = max(1, min(int(request.max_output_tokens or 16), 64))
    claim = request.claim[:2048]
    if request.mode == "embeddings":
        return {"model": request.model_ref, "input": claim}
    if request.mode == "completions":
        return {"model": request.model_ref, "prompt": claim, "max_tokens": max_out}
    return {
        "model": request.model_ref,
        "max_tokens": max_out,
        "messages": [
            {
                "role": "system",
                "content": "You are the Cortex Proof RLM judge. Reply with one short ack.",
            },
            {"role": "user", "content": claim},
        ],
    }


def call_judge(request: HarvestRequest, api_key: str, *, timeout_s: float = 60.0) -> None:
    """POST through the controller proxy or with direct auth; fail closed."""
    proxy = os.environ.get("PROOF_JUDGE_PROXY") == "1"
    if proxy and (
        request.mode not in ("chat", "completions")
        or request.base_url not in (
            "http://proof-judge:8080/v1",
            "http://proof-judge:8080/v1" + (
                "/completions" if request.mode == "completions" else "/chat/completions"
            ),
        )
    ):
        raise ContractError("judge proxy URL or mode not permitted; refuse scoring")
    key = api_key.strip()
    if not proxy and not key:
        raise ContractError("inference API key missing; refuse scoring")
    url = judge_url(request.base_url, request.mode)
    body = json.dumps(_payload(request), separators=(",", ":")).encode("utf-8")
    req = urllib.request.Request(url, data=body, method="POST")
    if not proxy:
        req.add_header("Authorization", f"Bearer {key}")
    req.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(req, timeout=timeout_s) as resp:
            status = int(getattr(resp, "status", 200) or 200)
            if status < 200 or status >= 300:
                raise ContractError(f"judge HTTP {status}")
            _ = resp.read(256)
    except ContractError:
        raise
    except urllib.error.HTTPError as exc:
        raise ContractError(f"judge HTTP {exc.code}") from None
    except Exception as exc:  # noqa: BLE001
        raise ContractError(f"judge request failed: {type(exc).__name__}") from None


def require_judge(request: HarvestRequest) -> None:
    call_judge(request, "" if os.environ.get("PROOF_JUDGE_PROXY") == "1" else load_judge_api_key())
