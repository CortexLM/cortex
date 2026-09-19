from __future__ import annotations

from dataclasses import dataclass

import pytest

from cortex.rlm import (
    AgentLimits,
    AgentRequest,
    AgentTask,
    BudgetExceeded,
    GuestRlmService,
    InferenceBroker,
    InferenceRequest,
    ProxyModelProvider,
    RunJournal,
    ToolRejected,
    VmContext,
)
from cortex.rlm.provider import Completion


class ModelBoundary:
    model = "test/model"

    def __init__(self):
        self.requests = []

    async def complete(self, **request):
        self.requests.append(request)
        return Completion.model_validate(
            {
                "tool_calls": [
                    {
                        "id": "call-1",
                        "type": "function",
                        "function": {"name": "finish", "arguments": "{}"},
                    }
                ],
                "prompt_tokens": 2,
                "completion_tokens": 2,
            }
        )


def request():
    return InferenceRequest(
        messages=[{"role": "user", "content": "Investigate topic evidence"}],
        tools=[{"type": "function", "function": {"name": "finish"}}],
        max_tokens=256,
        timeout_seconds=30.0,
        max_response_bytes=4096,
    )


async def test_guest_proxy_uses_host_credentials_and_shared_host_budget():
    provider = ModelBoundary()
    broker = InferenceBroker(provider, AgentLimits(max_calls=1))
    seen = []

    async def exchange(message):
        seen.append(message)
        completed = await broker.complete(InferenceRequest.model_validate(message["request"]))
        return {"completion": completed.model_dump(mode="json")}

    proxy = ProxyModelProvider(model=provider.model, exchange=exchange)

    completed = await proxy.complete(**request().model_dump())
    with pytest.raises(BudgetExceeded, match="host inference budget"):
        await proxy.complete(**request().model_dump())

    assert completed.completion_tokens == 2
    assert len(provider.requests) == 1
    assert "model" not in seen[0]["request"] and "api_key" not in seen[0]["request"]


async def test_host_red_rule_gate_blocks_guest_attempt_to_continue_inference():
    provider = ModelBoundary()
    broker = InferenceBroker(provider, AgentLimits())
    broker.block()

    with pytest.raises(BudgetExceeded):
        await broker.complete(request())

    assert not provider.requests


async def test_host_reserves_tokens_before_request_and_caps_guest_limits():
    provider = ModelBoundary()
    broker = InferenceBroker(provider, AgentLimits(completion_tokens=64, max_response_bytes=1024))

    await broker.complete(request())

    assert provider.requests[0]["max_tokens"] == 64
    assert provider.requests[0]["max_response_bytes"] == 1024
    assert broker.tokens == 4


@pytest.mark.parametrize("field,value", [("topic_id", "foreign"), ("kind", "experiment")])
async def test_guest_agent_rejects_wrong_topic_or_non_topic_guest(tmp_path, field, value):
    @dataclass
    class Binding:
        topic_id: str = "topic-a"
        image_digest: str = "a" * 64
        kind: str = "topic"

    binding = Binding()
    setattr(binding, field, value)
    journal = RunJournal(tmp_path / "journal")
    provider = ModelBoundary()
    service = GuestRlmService(
        binding=binding,
        provider=provider,
        executor_factory=lambda _task: None,
        journal=journal,
        limits=AgentLimits(),
    )
    agent_request = AgentRequest(
        task=AgentTask(
            context=VmContext(
                topic_id="topic-a", job_id="job-a", purpose="setup", image_digest="a" * 64
            ),
            objective="Prepare the owner topic",
        )
    )

    with pytest.raises(ToolRejected, match="bind this topic guest"):
        await service.run(agent_request)

    assert not provider.requests
    journal.close()
