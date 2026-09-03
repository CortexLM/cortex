"""HTTP teacher / judge.

OpenAI-compatible `/chat/completions`. URL, model, and key come from
env (`RELEARN_TEACHER_API_URL`, `RELEARN_TEACHER_MODEL`,
`RELEARN_TEACHER_API_KEY`). Missing URL or key → sim. Never send miner
weights / safetensors / GGUF as the `model` or as a payload to be served.
"""

from __future__ import annotations

import json
import os
import urllib.error
import urllib.request

DEFAULT_MODEL = "kimi-k3"


class TeacherError(RuntimeError):
    pass


def teacher_model() -> str:
    return os.environ.get("RELEARN_TEACHER_MODEL", DEFAULT_MODEL).strip() or DEFAULT_MODEL


def teacher_api_url() -> str:
    return os.environ.get("RELEARN_TEACHER_API_URL", "").strip()


def teacher_api_key() -> str:
    return os.environ.get("RELEARN_TEACHER_API_KEY", "").strip()


def _looks_like_digest(model: str) -> bool:
    t = model.strip().lower().removeprefix("0x")
    return len(t) == 64 and all(c in "0123456789abcdef" for c in t)


def refuse_miner_weights(model: str, candidate: str) -> None:
    if _looks_like_digest(model):
        raise TeacherError("miner artifact digest is not a teacher model")
    lower = candidate.lower()
    if any(tok in lower for tok in ("safetensors", "gguf", "nvfp4", "ckpt")):
        raise TeacherError("miner weights are not a teacher payload")


def judge(prompt: str, candidate: str, api_url: str | None = None) -> dict:
    model = teacher_model()
    refuse_miner_weights(model, candidate)
    url = (api_url or teacher_api_url()).rstrip("/")
    key = teacher_api_key()
    if not url or not key:
        score = 1.0 if candidate.strip() else 0.0
        return {"model": model, "score": score, "backend": "sim"}
    body = json.dumps(
        {
            "model": model,
            "messages": [
                {"role": "system", "content": "Score the candidate 0..1. JSON only."},
                {"role": "user", "content": f"prompt={prompt}\ncandidate={candidate}"},
            ],
        }
    ).encode()
    headers = {"content-type": "application/json", "authorization": f"Bearer {key}"}
    req = urllib.request.Request(
        url + "/chat/completions",
        data=body,
        headers=headers,
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=60) as resp:  # noqa: S310
            payload = json.loads(resp.read().decode())
    except urllib.error.HTTPError as e:
        raise TeacherError(f"teacher HTTP {e.code}") from e
    return {"model": model, "raw": payload, "backend": "http_api"}


TEACHER_MODEL = teacher_model()
