import json

import httpx

from cortex.rlm import OpenRouterClient


async def test_tool_calls_work_with_providers_without_parallel_tool_parameter(tmp_path):
    """Regression: DeepSeek v4.1 rejects routing when parallel_tool_calls is required."""
    key = tmp_path / "key"
    key.write_text("fixture-key")
    key.chmod(0o600)

    def provider(request):
        body = json.loads(request.content)
        if "parallel_tool_calls" in body and body["provider"].get("require_parameters"):
            return httpx.Response(404, json={"error": {"message": "No compatible endpoint"}})
        return httpx.Response(
            200,
            json={
                "choices": [
                    {
                        "finish_reason": "tool_calls",
                        "message": {
                            "role": "assistant",
                            "content": None,
                            "tool_calls": [
                                {
                                    "id": "call-1",
                                    "type": "function",
                                    "function": {"name": "check", "arguments": "{}"},
                                }
                            ],
                        },
                    }
                ],
                "usage": {"prompt_tokens": 10, "completion_tokens": 10, "total_tokens": 20},
            },
        )

    async with httpx.AsyncClient(transport=httpx.MockTransport(provider)) as client:
        response = await OpenRouterClient(
            model="fixture/model",
            api_key_file=key,
            client=client,
        ).complete(messages=[], tools=[], max_tokens=64, timeout_seconds=2, max_response_bytes=4096)
    assert response.tool_calls[0].function.name == "check"


async def test_openrouter_tool_index_metadata_is_accepted(tmp_path):
    key = tmp_path / "key"
    key.write_text("fixture-key")
    key.chmod(0o600)
    envelope = {
        "choices": [
            {
                "finish_reason": "tool_calls",
                "message": {
                    "role": "assistant",
                    "content": None,
                    "tool_calls": [
                        {
                            "index": 0,
                            "id": "call-1",
                            "type": "function",
                            "function": {"name": "check", "arguments": "{}"},
                        }
                    ],
                },
            }
        ],
        "usage": {"prompt_tokens": 10, "completion_tokens": 10, "total_tokens": 20},
    }
    transport = httpx.MockTransport(lambda _: httpx.Response(200, json=envelope))
    async with httpx.AsyncClient(transport=transport) as client:
        response = await OpenRouterClient(
            model="fixture/model",
            api_key_file=key,
            client=client,
        ).complete(messages=[], tools=[], max_tokens=64, timeout_seconds=2, max_response_bytes=4096)
    assert response.tool_calls[0].function.name == "check"
