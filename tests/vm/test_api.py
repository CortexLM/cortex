"""HTTPS and reread file credentials protect every host route."""

from types import SimpleNamespace

import httpx
import pytest
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

from cortex.protocol.crypto import public_key
from cortex.rlm import KnowledgeApproval, KnowledgeStore, Observation
from cortex.rlm.models import digest_of
from cortex.rlm.offer import InferenceOffer, sign_offer
from cortex.vm.api import create_app
from cortex.vm.models import Resources, VmError
from cortex.vm.runtime import Orchestrator

from .test_research_recovery import recovery as recovery
from .test_runtime import FakeHypervisor


async def test_health_advertises_exact_host_resource_ceilings(tmp_path):
    token = tmp_path / "token"
    token.touch(mode=0o600)
    token.write_text("host-token")
    caps = Resources(vcpus=1, mem_mib=1024, disk_mib=16384)
    host = Orchestrator(tmp_path / "jobs.sqlite3", FakeHypervisor(), caps=caps)
    try:
        async with httpx.AsyncClient(
            transport=httpx.ASGITransport(app=create_app(host, token)),
            base_url="https://host.invalid",
            headers={"Authorization": "Bearer host-token"},
        ) as client:
            response = await client.get("/v1/health")

        assert response.status_code == 200
        assert response.json()["resource_caps"] == caps.model_dump(mode="json")
    finally:
        await host.close()


async def test_token_rotation_takes_effect_on_the_next_request(tmp_path):
    token = tmp_path / "token"
    token.write_text("first-token")
    token.chmod(0o600)
    host = Orchestrator(tmp_path / "jobs.sqlite3", FakeHypervisor())
    app = create_app(host, token)
    try:
        async with httpx.AsyncClient(
            transport=httpx.ASGITransport(app=app), base_url="https://host.invalid"
        ) as client:
            assert (await client.get("/v1/health")).status_code == 401
            assert (
                await client.get("/v1/health", headers={"Authorization": "Bearer first-token"})
            ).status_code == 200
            token.write_text("second-token")
            assert (
                await client.get("/v1/health", headers={"Authorization": "Bearer first-token"})
            ).status_code == 401
            assert (
                await client.get("/v1/health", headers={"Authorization": "Bearer second-token"})
            ).status_code == 200
            token.write_text("")
            assert (
                await client.get("/v1/health", headers={"Authorization": "Bearer second-token"})
            ).status_code == 503
    finally:
        await host.close()


async def test_plain_http_is_refused_even_with_valid_bearer(tmp_path):
    token = tmp_path / "token"
    token.write_text("token")
    token.chmod(0o600)
    host = Orchestrator(tmp_path / "jobs.sqlite3", FakeHypervisor())
    try:
        async with httpx.AsyncClient(
            transport=httpx.ASGITransport(app=create_app(host, token)),
            base_url="http://host.invalid",
        ) as client:
            response = await client.get("/v1/health", headers={"Authorization": "Bearer token"})
        assert response.status_code == 400
        assert response.json()["error"] == "HTTPS required"
    finally:
        await host.close()


async def test_forged_direct_knowledge_approval_leaves_no_pending_claim(tmp_path):
    token = tmp_path / "token"
    token.write_text("token")
    token.chmod(0o600)
    owner = Ed25519PrivateKey.generate()
    knowledge = KnowledgeStore(
        tmp_path / "knowledge.sqlite3", owner_public_key=owner.public_key().public_bytes_raw()
    )
    observation = Observation(
        topic_id="topic-a", content="unverified claim", evidence_digest="a" * 64
    )
    approval = KnowledgeApproval(
        observation_digest=digest_of(observation),
        verification_report_digest="b" * 64,
        owner_signature="0" * 128,
    )
    attacker = Ed25519PrivateKey.generate()
    approval = approval.model_copy(
        update={"owner_signature": attacker.sign(approval.signing_bytes()).hex()}
    )
    host = Orchestrator(tmp_path / "jobs.sqlite3", FakeHypervisor())
    research = SimpleNamespace(knowledge=knowledge)
    try:
        async with httpx.AsyncClient(
            transport=httpx.ASGITransport(app=create_app(host, token, research)),
            base_url="https://host.invalid",
            headers={"Authorization": "Bearer token"},
        ) as client:
            response = await client.post(
                "/v1/knowledge/approve",
                json={
                    "observation": observation.model_dump(mode="json"),
                    "approval": approval.model_dump(mode="json"),
                },
            )

        assert response.status_code == 400
        assert knowledge.pending() == []
        assert knowledge.read_verified("topic-a") == []
    finally:
        knowledge.close()
        await host.close()


async def test_existing_research_job_exposes_exact_authenticated_resume_signal(recovery, tmp_path):
    host, orchestrator, topic, _, provider, envelope = recovery
    initial = host()
    with pytest.raises(VmError):
        await initial.run(topic.vm_id, envelope)
    await initial.close()
    recovering = host()
    seed = bytes([61]) * 32
    offer = sign_offer(
        InferenceOffer(
            model=provider.model,
            limits=recovering.limits,
            issuer_public_key=public_key(seed).hex(),
            status="open",
            valid_from_unix=0,
            valid_until_unix=4102444800,
            signature="0" * 128,
        ),
        seed,
    )
    token = tmp_path / "api-token"
    token.touch(mode=0o600)
    token.write_text("operator-token")
    app = create_app(
        orchestrator,
        token,
        recovering,
        inference_offer=offer,
        inference_offer_commitment=offer.commitment(),
    )

    async with httpx.AsyncClient(
        transport=httpx.ASGITransport(app=app), base_url="https://host.invalid"
    ) as client:
        response = await client.post(
            f"/v1/vms/{topic.vm_id}/agent",
            json=envelope.model_dump(mode="json"),
            headers={"Authorization": "Bearer operator-token"},
        )

    assert response.status_code == 409, response.text
    assert response.json() == {"error": "research_resume_required"}
    assert len(provider.requests) == 2
