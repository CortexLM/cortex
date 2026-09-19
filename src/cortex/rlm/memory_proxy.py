"""Guest access to the host's durable, signed cross-topic research memory."""

from __future__ import annotations

from .errors import ToolRejected
from .knowledge import KnowledgeStore, Observation
from .models import VmContext


class SharedKnowledgeBroker:
    def __init__(self, store: KnowledgeStore, context: VmContext):
        self.store, self.context = store, context

    def handle(self, message: dict, evidence_digests: set[str]) -> dict:
        body = message.get("request", {})
        if message.get("type") == "knowledge_read":
            if set(body) != {"limit"} or type(body["limit"]) is not int:
                raise ToolRejected("invalid knowledge read")
            observations = self.store.read_verified(self.context.topic_id, limit=body["limit"])
            return {"observations": [row.model_dump(mode="json") for row in observations]}
        if message.get("type") == "knowledge_propose":
            observation = Observation.model_validate(body)
            if (
                observation.topic_id != self.context.topic_id
                or observation.source_artifact_digest != self.context.artifact_digest
                or observation.visibility != "topic_private"
                or observation.evidence_digest not in evidence_digests
            ):
                raise ToolRejected("knowledge proposal lacks this job's execution evidence")
            return {"observation_digest": self.store.propose(observation)}
        raise ToolRejected("unknown knowledge operation")


class SharedKnowledgeClient:
    def __init__(self, context: VmContext, exchange):
        self.context, self.exchange = context, exchange

    async def read_verified(self, topic_id: str, *, limit: int = 8) -> list[Observation]:
        if topic_id != self.context.topic_id or not 1 <= limit <= 16:
            raise ToolRejected("knowledge request binding mismatch")
        result = await self.exchange(
            {
                "type": "knowledge_read",
                "request": {"limit": limit},
            }
        )
        observations = [Observation.model_validate(row) for row in result["observations"]]
        if any(row.visibility != "public" and row.topic_id != topic_id for row in observations):
            raise ToolRejected("shared knowledge leaked another topic's private observation")
        return observations

    async def propose(self, observation: Observation) -> str:
        if (
            observation.topic_id != self.context.topic_id
            or observation.source_artifact_digest != self.context.artifact_digest
            or observation.visibility != "topic_private"
        ):
            raise ToolRejected("knowledge proposal binding mismatch")
        result = await self.exchange(
            {
                "type": "knowledge_propose",
                "request": observation.model_dump(mode="json"),
            }
        )
        digest = result.get("observation_digest")
        if not isinstance(digest, str) or len(digest) != 64:
            raise ToolRejected("invalid knowledge receipt")
        return digest
