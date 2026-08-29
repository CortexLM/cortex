"""Frozen GLM-5.3 judge.

HTTP API is judge-only. Never send miner weights / safetensors / GGUF
as the `model` or as the candidate payload to be *served*.
"""

from __future__ import annotations

import json
import os
import urllib.request

TEACHER_MODEL = "zai-org/GLM-5.3"


class TeacherError(RuntimeError):
    pass


def refuse_miner_weights(model: str, candidate: str) -> None:
    if model != TEACHER_MODEL:
        raise TeacherError("teacher API only accepts zai-org/GLM-5.3")
    lower = candidate.lower()
    if any(tok in lower for tok in ("safetensors", "gguf", "nvfp4", "ckpt")):
        raise TeacherError("miner weights are not a teacher payload")


def judge(prompt: str, candidate: str, api_url: str | None = None) -> dict:
    refuse_miner_weights(TEACHER_MODEL, candidate)
    url = api_url or os.environ.get("RELEARN_TEACHER_API_URL", "")
    if not url:
        # Sim fallback — deterministic, no network.
        score = 1.0 if candidate.strip() else 0.0
        return {"model": TEACHER_MODEL, "score": score, "backend": "sim"}
    body = json.dumps(
        {
            "model": TEACHER_MODEL,
            "messages": [
                {"role": "system", "content": "Score the candidate 0..1. JSON only."},
                {"role": "user", "content": f"prompt={prompt}\ncandidate={candidate}"},
            ],
        }
    ).encode()
    req = urllib.request.Request(
        url.rstrip("/") + "/chat/completions",
        data=body,
        headers={"content-type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=60) as resp:  # noqa: S310
        payload = json.loads(resp.read().decode())
    return {"model": TEACHER_MODEL, "raw": payload, "backend": "http_api"}
