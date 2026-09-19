import json

import pytest
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

from cortex.rlm import (
    AgentLimits,
    AgentRequest,
    AgentTask,
    KnowledgeApproval,
    KnowledgeStore,
    Observation,
    VmContext,
)
from cortex.rlm.provider import Completion
from cortex.vm.guest import GuestIdentity
from cortex.vm.guest_server import GuestServer


class StopAfterMemory(Exception):
    pass


class InferenceBoundary:
    def __init__(self):
        self.requests = []

    async def exchange(self, message):
        self.requests.append(message)
        if len(self.requests) == 2:
            raise StopAfterMemory()
        completion = Completion.model_validate(
            {
                "tool_calls": [
                    {
                        "id": "memory-1",
                        "type": "function",
                        "function": {"name": "knowledge_read", "arguments": '{"limit": 8}'},
                    }
                ],
                "prompt_tokens": 10,
                "completion_tokens": 10,
            }
        )
        return {"completion": completion.model_dump(mode="json")}


async def test_guest_uses_durable_verified_memory_but_keeps_other_topics_private(tmp_path):
    owner = Ed25519PrivateKey.generate()
    public_key = owner.public_key().public_bytes_raw()
    path = tmp_path / "knowledge.sqlite3"
    memory = KnowledgeStore(path, owner_public_key=public_key)
    for topic_id, content in [
        ("topic-a", "Verified reusable evidence"),
        ("topic-b", "holdout-secret-from-topic-b"),
    ]:
        observation = Observation(topic_id=topic_id, content=content, evidence_digest="a" * 64)
        approval = KnowledgeApproval(
            observation_digest=memory.propose(observation),
            verification_report_digest="b" * 64,
            owner_signature="0" * 128,
        )
        memory.approve(
            approval.model_copy(
                update={"owner_signature": owner.sign(approval.signing_bytes()).hex()}
            )
        )
    memory.close()
    reopened = KnowledgeStore(path, owner_public_key=public_key)
    callback = InferenceBoundary()
    server = GuestServer(
        GuestIdentity("vm-a", "topic-a", "c" * 64, "topic"),
        workspace=tmp_path / "guest",
        callback=callback,
        knowledge=reopened,
    )
    request = AgentRequest(
        task=AgentTask(
            context=VmContext(
                topic_id="topic-a", job_id="memory-job", purpose="setup", image_digest="c" * 64
            ),
            objective="Inspect verified observations before investigating",
        )
    )

    with pytest.raises(StopAfterMemory):
        await server.handle(
            {
                "api_version": 2,
                "type": "agent",
                "request": request.model_dump(mode="json"),
                "model": "test/model",
                "limits": AgentLimits().model_dump(mode="json"),
            }
        )

    context = json.dumps(callback.requests[1])
    assert "Verified reusable evidence" in context
    assert "holdout-secret-from-topic-b" not in context
    reopened.close()
