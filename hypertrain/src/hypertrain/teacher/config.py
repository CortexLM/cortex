"""Teacher config (TOML). Endpoint, model, key source and parameters are all user-chosen."""

from __future__ import annotations

import tomllib
from dataclasses import dataclass, field, fields
from pathlib import Path
from typing import Any
from urllib.parse import urlparse


class TeacherError(RuntimeError):
    """Configuration, transport or budget failure. Messages never contain the API key."""


@dataclass(frozen=True)
class TeacherConfig:
    base_url: str = "https://openrouter.ai/api/v1"
    model: str = "deepseek/deepseek-v4.1-flash"
    api_key_env: str = "OPENROUTER_API_KEY"
    api_key_file: str | None = None  # wins over api_key_env when set
    headers: dict[str, str] = field(default_factory=dict)  # e.g. HTTP-Referer, X-Title
    temperature: float = 0.0
    top_logprobs: int = 20
    max_concurrency: int = 8
    timeout_s: float = 60.0
    retries: int = 5
    backoff_s: float = 1.0
    max_usd: float = 10.0
    # used when the response carries no usage.cost; USD per token
    price_prompt: float = 0.0000000356
    price_completion: float = 0.000001
    # Verbatim request fields; {} omits. provider None = require_parameters on openrouter.ai.
    reasoning: dict[str, Any] = field(default_factory=lambda: {"enabled": False})
    provider: dict[str, Any] | None = None
    rotations: int = 0  # 0 = all K+1 cyclic rotations, else a cap
    cache_dir: str = "teacher-cache"

    def provider_field(self) -> dict[str, Any]:
        if self.provider is not None:
            return self.provider
        host = urlparse(self.base_url).hostname or ""
        return {"require_parameters": True} if host.endswith("openrouter.ai") else {}

    def api_key(self) -> str:
        import os

        if self.api_key_file:
            key = Path(self.api_key_file).read_text().strip()
        else:
            key = os.environ.get(self.api_key_env, "").strip()
        if not key:
            src = self.api_key_file or f"${self.api_key_env}"
            raise TeacherError(f"no API key found in {src}")
        return key


def load_config(path: Path | None = None, **overrides: Any) -> TeacherConfig:
    raw: dict[str, Any] = {}
    if path is not None:
        raw = tomllib.loads(Path(path).read_text())
    names = {f.name for f in fields(TeacherConfig)}
    bad = sorted(set(raw) - names)
    if bad:
        raise TeacherError(f"unknown config keys: {bad}")
    raw.update({k: v for k, v in overrides.items() if v is not None})
    cfg = TeacherConfig(**raw)
    if not cfg.base_url.startswith(("http://", "https://")):
        raise TeacherError("base_url must be http(s)")
    if not 0 <= cfg.top_logprobs <= 20 or cfg.top_logprobs < 1:
        raise TeacherError("top_logprobs must be in 1..20")
    if cfg.max_concurrency < 1 or cfg.retries < 0 or cfg.max_usd < 0 or cfg.rotations < 0:
        raise TeacherError("bad numeric config value")
    return cfg
