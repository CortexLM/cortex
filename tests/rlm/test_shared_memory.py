import pytest
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

from cortex.rlm.errors import ToolRejected
from cortex.rlm.knowledge import KnowledgeApproval, KnowledgeStore, Observation
from cortex.rlm.memory_proxy import SharedKnowledgeBroker, SharedKnowledgeClient
from cortex.rlm.models import VmContext


def approve(store, owner, observation):
    value = store.propose(observation)
    unsigned = KnowledgeApproval(
        observation_digest=value,
        verification_report_digest="be" * 32,
        owner_signature="00" * 64,
    )
    store.approve(
        unsigned.model_copy(
            update={
                "owner_signature": owner.sign(unsigned.signing_bytes()).hex(),
            }
        )
    )


def client(store, topic):
    context = VmContext(topic_id=topic, job_id="one-job", purpose="setup", image_digest="ab" * 32)
    broker = SharedKnowledgeBroker(store, context)

    async def exchange(message):
        return broker.handle(message, {"cd" * 32})

    return SharedKnowledgeClient(context, exchange)


async def test_two_topic_guests_share_approved_public_knowledge_after_host_restart(tmp_path):
    owner = Ed25519PrivateKey.from_private_bytes(bytes([13]) * 32)
    path = tmp_path / "shared.sqlite3"
    store = KnowledgeStore(path, owner_public_key=owner.public_key().public_bytes_raw())
    public = Observation(
        topic_id="topic-a",
        content="Verified general technique",
        evidence_digest="cd" * 32,
        visibility="public",
    )
    private = public.model_copy(update={"visibility": "topic_private", "content": "Private result"})
    approve(store, owner, public)
    approve(store, owner, private)
    store.close()
    store = KnowledgeStore(path, owner_public_key=owner.public_key().public_bytes_raw())
    try:
        assert set(
            row.content for row in await client(store, "topic-a").read_verified("topic-a")
        ) == {
            "Verified general technique",
            "Private result",
        }
        assert await client(store, "topic-b").read_verified("topic-b") == [public]
    finally:
        store.close()


async def test_guest_cannot_publish_unapproved_memory_or_invent_evidence(tmp_path):
    owner = Ed25519PrivateKey.from_private_bytes(bytes([13]) * 32)
    store = KnowledgeStore(
        tmp_path / "shared.sqlite3", owner_public_key=owner.public_key().public_bytes_raw()
    )
    guest = client(store, "topic-a")
    observation = Observation(
        topic_id="topic-a", content="Untrusted finding", evidence_digest="cd" * 32
    )
    try:
        assert len(await guest.propose(observation)) == 64
        assert await guest.read_verified("topic-a") == []
        assert store.pending() == [observation]
        with pytest.raises(ToolRejected):
            await guest.propose(observation.model_copy(update={"evidence_digest": "de" * 32}))
        with pytest.raises(ToolRejected):
            await guest.propose(observation.model_copy(update={"visibility": "public"}))
        with pytest.raises(ToolRejected):
            await guest.read_verified("topic-b")
    finally:
        store.close()
