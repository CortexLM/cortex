"""Inference bridge for a networkless topic guest.

The agent loop and tool selection stay in the guest. The host performs a bounded
provider HTTP request with its credential and never executes model-generated code.
"""

from __future__ import annotations

import asyncio
import time
from collections.abc import Awaitable, Callable
from typing import Any

from pydantic import Field, ValidationError, model_validator

from .errors import BudgetExceeded, InvalidResponse
from .models import AgentLimits, StrictModel, canonical_bytes
from .provider import Completion, ModelProvider


class InferenceRequest(StrictModel):
    messages: list[dict[str, Any]] = Field(min_length=1, max_length=1024)
    tools: list[dict[str, Any]] = Field(min_length=1, max_length=16)
    max_tokens: int = Field(ge=64, le=32_768)
    timeout_seconds: float = Field(gt=0, le=7200)
    max_response_bytes: int = Field(ge=1024, le=1_048_576)

    @model_validator(mode="after")
    def bounded_payload(self) -> InferenceRequest:
        if len(canonical_bytes(self)) > 524_288:
            raise ValueError("inference payload exceeds byte limit")
        return self


class ProxyModelProvider:
    def __init__(
        self,
        *,
        model: str,
        exchange: Callable[[dict[str, Any]], Awaitable[dict[str, Any]]],
    ) -> None:
        self.model = model
        self.exchange = exchange

    async def complete(
        self,
        *,
        messages: list[dict[str, Any]],
        tools: list[dict[str, Any]],
        max_tokens: int,
        timeout_seconds: float,
        max_response_bytes: int,
    ) -> Completion:
        request = InferenceRequest(
            messages=messages,
            tools=tools,
            max_tokens=max_tokens,
            timeout_seconds=timeout_seconds,
            max_response_bytes=max_response_bytes,
        )
        response = await self.exchange(
            {"type": "inference", "request": request.model_dump(mode="json")}
        )
        try:
            return Completion.model_validate(response["completion"])
        except (KeyError, ValidationError):
            raise InvalidResponse("host returned an invalid inference response") from None


class InferenceBroker:
    """One broker per host-attested job, shared by all recursive guest agents.

    A transport must authenticate/bind the callback to the VM and job, then route
    every inference callback for that job to this same instance. It must close
    that callback session after worker restart; never recreate a fresh broker for
    an already running job. Durable guest budgets apply independently on resume.
    """

    def __init__(self, provider: ModelProvider, limits: AgentLimits) -> None:
        self.provider = provider
        self.limits = limits
        self.deadline = time.monotonic() + limits.wall_seconds
        self.calls = 0
        self.tokens = 0
        self._blocked = False
        self._lock = asyncio.Lock()

    def block(self) -> None:
        """Host callback invokes this when an attested preflight check turns red."""
        self._blocked = True

    async def complete(self, request: InferenceRequest) -> Completion:
        async with self._lock:
            remaining = self.deadline - time.monotonic()
            if self._blocked or remaining <= 0 or self.calls >= self.limits.max_calls:
                raise BudgetExceeded("host inference budget exhausted")
            prompt_bound = (
                len(canonical_bytes({"messages": request.messages, "tools": request.tools})) + 1024
            )
            completion_bound = min(request.max_tokens, self.limits.completion_tokens)
            reserved = prompt_bound + completion_bound
            if self.tokens + reserved > self.limits.max_tokens:
                raise BudgetExceeded("host inference token budget exhausted")
            self.calls += 1
            self.tokens += reserved
            completion = await self.provider.complete(
                messages=request.messages,
                tools=request.tools,
                max_tokens=completion_bound,
                timeout_seconds=min(remaining, request.timeout_seconds),
                max_response_bytes=min(request.max_response_bytes, self.limits.max_response_bytes),
            )
            if (
                completion.prompt_tokens > prompt_bound
                or completion.completion_tokens > completion_bound
            ):
                raise BudgetExceeded("provider exceeded host token reservation")
            self.tokens += completion.prompt_tokens + completion.completion_tokens - reserved
            return completion
