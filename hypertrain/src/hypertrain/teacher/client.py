"""OpenAI-compatible chat-completions client (httpx only) with retries and a hard USD cap."""

from __future__ import annotations

import math
import threading
import time
from collections.abc import Callable
from dataclasses import dataclass
from typing import Any

import httpx

from hypertrain.teacher.config import TeacherConfig, TeacherError


class BudgetExceeded(TeacherError):
    pass


class Budget:
    """Reserve-before-call so concurrent workers can never overshoot max_usd."""

    def __init__(self, max_usd: float) -> None:
        self.max_usd, self.spent, self._reserved, self._peak = max_usd, 0.0, 0.0, 0.0
        self._lock = threading.Lock()

    def reserve(self, est: float) -> float:
        with self._lock:
            est = max(est, self._peak)  # never assume a call is cheaper than the dearest seen
            if self.spent + self._reserved + est > self.max_usd:
                raise BudgetExceeded(
                    f"budget cap USD {self.max_usd:.6f} would be exceeded "
                    f"(spent {self.spent:.6f}, next call ~{est:.6f})"
                )
            self._reserved += est
            return est

    def settle(self, est: float, actual: float) -> None:
        with self._lock:
            self._reserved -= est
            self._peak = max(self._peak, actual)
            self.spent += actual


def estimate_tokens(text: str) -> int:
    return math.ceil(len(text) / 4)  # no tokenizer offline: ~4 chars/token


@dataclass(frozen=True)
class Completion:
    top_logprobs: list[tuple[str, float]] | None  # first answer token alternatives
    cost: float


class TeacherClient:
    def __init__(
        self,
        cfg: TeacherConfig,
        key: str,
        *,
        budget: Budget | None = None,
        transport: httpx.BaseTransport | None = None,
        sleep: Callable[[float], None] = time.sleep,
    ) -> None:
        self.cfg, self._key, self._sleep = cfg, key, sleep
        self.budget = budget or Budget(cfg.max_usd)
        self._http = httpx.Client(
            base_url=cfg.base_url.rstrip("/") + "/",
            timeout=cfg.timeout_s,
            transport=transport,
            headers={**cfg.headers, "Authorization": f"Bearer {key}"},
        )

    def _redact(self, s: str) -> str:
        return s.replace(self._key, "***") if self._key else s

    def call_cost_estimate(self, prompt: str) -> float:
        c = self.cfg
        return estimate_tokens(prompt) * c.price_prompt + 1 * c.price_completion

    def complete(self, prompt: str) -> Completion:
        c = self.cfg
        est = self.budget.reserve(self.call_cost_estimate(prompt))
        actual = 0.0
        try:
            body = {
                "model": c.model,
                "messages": [{"role": "user", "content": prompt}],
                "temperature": c.temperature,
                "max_tokens": 1,
                "logprobs": True,
                "top_logprobs": c.top_logprobs,
            }
            if c.reasoning:
                body["reasoning"] = c.reasoning
            if prov := c.provider_field():
                body["provider"] = prov
            data = self._post(body)
            usage = data.get("usage") or {}
            cost = usage.get("cost")
            if isinstance(cost, int | float):
                actual = float(cost)
            else:
                actual = (
                    usage.get("prompt_tokens", estimate_tokens(prompt)) * c.price_prompt
                    + usage.get("completion_tokens", 1) * c.price_completion
                )
            return Completion(_first_token_alts(data), actual)
        finally:
            self.budget.settle(est, actual)

    def _post(self, body: dict[str, Any]) -> dict[str, Any]:
        last = "no attempt"
        for attempt in range(self.cfg.retries + 1):
            wait = self.cfg.backoff_s * 2**attempt
            try:
                r = self._http.post("chat/completions", json=body)
            except httpx.TransportError as e:
                last = self._redact(f"{type(e).__name__}: {e}")
            else:
                if r.status_code == 429 or r.status_code >= 500:
                    last = f"HTTP {r.status_code}"
                    ra = r.headers.get("Retry-After")
                    if ra:
                        try:
                            wait = min(max(float(ra), 0.0), 120.0)
                        except ValueError:
                            pass
                elif r.status_code >= 400:
                    raise TeacherError(f"HTTP {r.status_code}: {self._redact(r.text[:200])}")
                else:
                    try:
                        out = r.json()
                    except ValueError as e:
                        raise TeacherError("response is not JSON") from e
                    if not isinstance(out, dict):
                        raise TeacherError("response is not a JSON object")
                    return out
            if attempt < self.cfg.retries:
                self._sleep(wait)
        raise TeacherError(f"giving up after {self.cfg.retries + 1} attempts: {last}")


def _first_token_alts(data: dict[str, Any]) -> list[tuple[str, float]] | None:
    try:
        content = data["choices"][0]["logprobs"]["content"]
        # first non-blank token: a leading empty/whitespace token is not the answer
        first = next(e for e in content if str(e["token"]).strip())
        alts = first["top_logprobs"]
        out = [(str(a["token"]), float(a["logprob"])) for a in alts]
    except (KeyError, IndexError, TypeError, ValueError, StopIteration):
        return None
    return out or None
