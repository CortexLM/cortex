from __future__ import annotations

import json

import httpx
import pytest

from cortex.rlm import InvalidResponse, OpenRouterClient, ProviderError


def valid_response():
    return {
        "choices": [
            {
                "finish_reason": "tool_calls",
                "message": {
                    "role": "assistant",
                    "content": None,
                    "tool_calls": [
                        {
                            "id": "one",
                            "type": "function",
                            "function": {"name": "finish", "arguments": "{}"},
                        }
                    ],
                },
            }
        ],
        "usage": {"prompt_tokens": 2, "completion_tokens": 2, "total_tokens": 4},
    }


async def complete(tmp_path, response):
    key = tmp_path / "key"
    key.write_text("test-only-key")
    key.chmod(0o600)
    async with httpx.AsyncClient(transport=httpx.MockTransport(lambda _: response)) as client:
        provider = OpenRouterClient(model="test/model", api_key_file=key, client=client)
        return await provider.complete(
            messages=[], tools=[], max_tokens=64, timeout_seconds=1, max_response_bytes=2048
        )


@pytest.mark.parametrize("status", [301, 401, 429, 500])
async def test_http_errors_never_expose_provider_body_or_key(tmp_path, status):
    with pytest.raises(ProviderError) as caught:
        await complete(tmp_path, httpx.Response(status, text="secret-credential-and-private-input"))

    assert str(status) in str(caught.value)
    assert "secret-credential" not in str(caught.value)
    assert "test-only-key" not in str(caught.value)


async def test_plain_acknowledgment_is_not_a_verdict(tmp_path):
    response = valid_response()
    response["choices"][0] = {
        "finish_reason": "stop",
        "message": {"role": "assistant", "content": "Done"},
    }

    with pytest.raises(InvalidResponse):
        await complete(tmp_path, httpx.Response(200, json=response))


@pytest.mark.parametrize("violation", ["duplicate", "missing_usage", "truncated", "over_tokens"])
async def test_malformed_responses_fail_closed(tmp_path, violation):
    response = valid_response()
    if violation == "duplicate":
        raw = json.dumps(response).replace(
            '"prompt_tokens": 2', '"prompt_tokens": 2,"prompt_tokens": 4'
        )
    else:
        if violation == "missing_usage":
            del response["usage"]
        elif violation == "truncated":
            response["choices"][0]["finish_reason"] = "length"
        elif violation == "over_tokens":
            response["usage"] = {"prompt_tokens": 2, "completion_tokens": 65, "total_tokens": 67}
        raw = json.dumps(response)

    with pytest.raises(InvalidResponse):
        await complete(tmp_path, httpx.Response(200, content=raw))


async def test_response_byte_limit_applies_before_parsing(tmp_path):
    with pytest.raises(InvalidResponse, match="byte limit"):
        await complete(tmp_path, httpx.Response(200, content=b" " * 2049))


async def test_key_symlink_is_rejected_before_http(tmp_path):
    source = tmp_path / "source"
    source.write_text("test-only-key")
    source.chmod(0o600)
    key = tmp_path / "key"
    key.symlink_to(source)
    provider = OpenRouterClient(model="test/model", api_key_file=key)

    with pytest.raises(ProviderError, match="unavailable"):
        await provider.complete(
            messages=[], tools=[], max_tokens=64, timeout_seconds=1, max_response_bytes=2048
        )


async def test_key_rotation_is_observed_per_request(tmp_path):
    key = tmp_path / "key"
    key.write_text("first-test-key")
    key.chmod(0o600)
    observed = []

    def transport(request):
        observed.append(request.headers["authorization"])
        return httpx.Response(200, json=valid_response())

    async with httpx.AsyncClient(transport=httpx.MockTransport(transport)) as client:
        provider = OpenRouterClient(model="test/model", api_key_file=key, client=client)
        arguments = dict(
            messages=[], tools=[], max_tokens=64, timeout_seconds=1, max_response_bytes=2048
        )
        await provider.complete(**arguments)
        key.write_text("second-test-key")
        await provider.complete(**arguments)

    assert observed == ["Bearer first-test-key", "Bearer second-test-key"]


async def test_provider_uses_explicit_job_deadline_instead_of_httpx_five_second_default(tmp_path):
    key = tmp_path / "key"
    key.write_text("test-only-key")
    key.chmod(0o600)

    def transport(request):
        # The outer asyncio deadline bounds connect + upload + inference + download.
        # HTTPX's unrelated default read deadline must not abort a legitimate model run.
        assert all(value is None for value in request.extensions["timeout"].values())
        return httpx.Response(200, json=valid_response())

    async with httpx.AsyncClient(transport=httpx.MockTransport(transport)) as client:
        provider = OpenRouterClient(model="test/model", api_key_file=key, client=client)
        result = await provider.complete(
            messages=[], tools=[], max_tokens=64, timeout_seconds=180, max_response_bytes=2048
        )
    assert result.tool_calls[0].function.name == "finish"
