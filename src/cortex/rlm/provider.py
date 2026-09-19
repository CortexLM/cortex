"""Bounded OpenRouter client with explicit, injectable HTTP transport."""

from __future__ import annotations

import asyncio
import json
import os
import stat
from pathlib import Path
from typing import Any, Protocol

import httpx
from pydantic import BaseModel, ConfigDict, Field, ValidationError

from .errors import InvalidResponse, ProviderError
from .models import safe_model_id


class FunctionCall(BaseModel):
    model_config = ConfigDict(extra="forbid", strict=True)
    name: str = Field(min_length=1, max_length=128)
    arguments: str = Field(max_length=65_536)


class ToolCall(BaseModel):
    model_config = ConfigDict(extra="forbid", strict=True)
    id: str = Field(min_length=1, max_length=128)
    type: str
    function: FunctionCall
    index: int | None = Field(default=None, ge=0, exclude=True)


class Completion(BaseModel):
    model_config = ConfigDict(extra="forbid", strict=True)
    tool_calls: list[ToolCall] = Field(min_length=1, max_length=16)
    content: str | None = None
    prompt_tokens: int = Field(ge=0)
    completion_tokens: int = Field(ge=0)


class ModelProvider(Protocol):
    @property
    def model(self) -> str: ...

    async def complete(
        self,
        *,
        messages: list[dict[str, Any]],
        tools: list[dict[str, Any]],
        max_tokens: int,
        timeout_seconds: float,
        max_response_bytes: int,
    ) -> Completion: ...


def _read_key(path: Path) -> str:
    fd: int | None = None
    try:
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        info = os.fstat(fd)
        if not stat.S_ISREG(info.st_mode) or info.st_mode & 0o077 or info.st_size > 4096:
            raise ProviderError("OpenRouter key file must be a private regular file")
        with os.fdopen(fd, "r", encoding="utf-8") as stream:
            fd = None
            key = stream.read(4097).strip()
        if not key or len(key) > 4096 or any(char.isspace() for char in key):
            raise ProviderError("OpenRouter key file is invalid")
        return key
    except (OSError, UnicodeError):
        raise ProviderError("OpenRouter key file is unavailable") from None
    finally:
        if fd is not None:
            os.close(fd)


def _no_duplicate_keys(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("duplicate JSON key")
        result[key] = value
    return result


def strict_json(value: str | bytes) -> Any:
    return json.loads(
        value,
        object_pairs_hook=_no_duplicate_keys,
        parse_constant=lambda _value: (_ for _ in ()).throw(ValueError("invalid number")),
    )


class OpenRouterClient:
    def __init__(
        self,
        *,
        model: str,
        api_key_file: str | Path,
        client: httpx.AsyncClient | None = None,
    ) -> None:
        self.model = safe_model_id(model)
        self.api_key_file = Path(api_key_file)
        self._client = client

    async def complete(
        self,
        *,
        messages: list[dict[str, Any]],
        tools: list[dict[str, Any]],
        max_tokens: int,
        timeout_seconds: float,
        max_response_bytes: int,
    ) -> Completion:
        key = await asyncio.to_thread(_read_key, self.api_key_file)
        payload = {
            "model": self.model,
            "messages": messages,
            "tools": tools,
            "tool_choice": "required",
            "max_tokens": max_tokens,
            "temperature": 0,
            "provider": {"allow_fallbacks": False, "require_parameters": True},
        }
        try:
            async with asyncio.timeout(timeout_seconds):
                if self._client is None:
                    async with httpx.AsyncClient(follow_redirects=False) as client:
                        body = await self._request(client, payload, key, max_response_bytes)
                else:
                    body = await self._request(self._client, payload, key, max_response_bytes)
        except (httpx.HTTPError, TimeoutError):
            raise ProviderError("OpenRouter request failed") from None
        try:
            envelope = strict_json(body)
            choices = envelope["choices"]
            if len(choices) != 1 or choices[0]["finish_reason"] != "tool_calls":
                raise ValueError("expected a completed tool call")
            message = choices[0]["message"]
            if message["role"] != "assistant":
                raise ValueError("invalid role")
            usage = envelope["usage"]
            completion = Completion.model_validate(
                {
                    "tool_calls": message["tool_calls"],
                    "content": message.get("content"),
                    "prompt_tokens": usage["prompt_tokens"],
                    "completion_tokens": usage["completion_tokens"],
                }
            )
            if (
                completion.prompt_tokens + completion.completion_tokens != usage["total_tokens"]
                or completion.completion_tokens > max_tokens
                or any(call.type != "function" for call in completion.tool_calls)
                or len({call.id for call in completion.tool_calls}) != len(completion.tool_calls)
            ):
                raise ValueError("invalid usage or tool calls")
            return completion
        except (KeyError, TypeError, ValueError, ValidationError, RecursionError):
            raise InvalidResponse("OpenRouter returned an invalid tool response") from None

    async def _request(
        self,
        client: httpx.AsyncClient,
        payload: dict[str, Any],
        key: str,
        max_response_bytes: int,
    ) -> bytes:
        async with client.stream(
            "POST",
            "https://openrouter.ai/api/v1/chat/completions",
            headers={"Authorization": f"Bearer {key}"},
            json=payload,
            follow_redirects=False,
            timeout=None,  # complete() bounds the entire operation by the shared job deadline
        ) as response:
            if response.status_code != 200:
                raise ProviderError(f"OpenRouter request rejected (HTTP {response.status_code})")
            data = bytearray()
            async for chunk in response.aiter_bytes():
                if len(data) + len(chunk) > max_response_bytes:
                    raise InvalidResponse("OpenRouter response exceeds byte limit")
                data.extend(chunk)
            return bytes(data)
