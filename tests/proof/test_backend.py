import json

import httpx
import pytest

from cortex.errors import ServiceError
from cortex.proof.backend import VmBackend
from cortex.rlm import AgentRequest, AgentTask, VmContext
from cortex.vm.models import Resources, VmRecord, VmSpec
from cortex.vm.research import ResearchRequest

from .conftest import OFFER, submission


@pytest.mark.parametrize("existing", [True, False])
async def test_topic_vm_resource_mismatch_refuses_attach_and_create(tmp_path, existing):
    requested = Resources(vcpus=1, mem_mib=1024, disk_mib=16384)
    record = VmRecord(
        vm_id="wrong-shape",
        spec=VmSpec(topic_id="small-topic", image_digest="ab" * 32, resources=Resources()),
        state="running",
    )
    calls = []

    def handler(request):
        calls.append(request.method)
        if request.method == "GET" and not existing:
            return httpx.Response(404)
        return httpx.Response(200, json=record.model_dump(mode="json"))

    token = tmp_path / "token"
    token.touch(mode=0o600)
    token.write_text("test-host-token")
    backend = VmBackend(
        url="https://vm.example",
        token_file=token,
        image_digest="sha256:" + "ab" * 32,
        inference_offer_commitment=OFFER.commitment(),
        custom_ids=frozenset(),
        resources=requested,
        transport=httpx.MockTransport(handler),
    )
    try:
        with pytest.raises(ServiceError, match="binding"):
            await backend.topic_vm("small-topic")
    finally:
        await backend.close()
    assert calls == (["GET"] if existing else ["GET", "POST"])


def measured_outcome(topic, body, *, accepted=True):
    context = body["request"]["task"]["context"]
    report = {
        **context,
        "execution_id": "fixture-experiment",
        "sandboxed": True,
        "network_enabled": False,
        "report_digest": "ab" * 32,
        "exit_code": 0,
        "metrics": [{"name": "quality", "value": 0.8}] if accepted else [],
        "rule_checks": [{"rule_id": "integrity-check", "passed": accepted}],
    }
    report.pop("purpose")
    return {
        "run": {
            "result": {
                "topic_id": topic.id,
                "artifact_digest": context["artifact_digest"],
                "rule_revision": topic.revision,
                "outcome": "accepted" if accepted else "rejected",
                "explanation": "Evidence from fixture hypervisor",
                "metric": "quality",
                "value": 0.8 if accepted else None,
                "report_digest": report["report_digest"],
                "rules_checked": ["integrity-check"],
            },
            "transcript": [],
            "calls": 2,
            "tool_calls": 1,
            "tokens": 200,
        },
        "reports": [report],
        "executions": [
            {
                "execution_id": report["execution_id"],
                "vm_id": "actual-sister",
                "report_digest": report["report_digest"],
                "phase": "experiment" if accepted else "preflight",
                "dedicated": True,
                "teardown_confirmed": True,
            }
        ],
        "wall_seconds": 2.0,
    }


def make_backend(
    tmp_path,
    topic,
    *,
    alter=lambda outcome: outcome,
    accepted=True,
    agent_responses=None,
    resources=None,
    resource_caps=...,
):
    if resource_caps is ...:
        resource_caps = Resources().model_dump(mode="json")
    token = tmp_path / "vm-token"
    token.write_text("fixture-vm-token")
    token.chmod(0o600)
    requests = []

    async def handler(request):
        assert request.headers["authorization"] == "Bearer " + token.read_text().strip()
        requests.append(request)
        if request.url.path == "/v1/health":
            return httpx.Response(
                200,
                json={
                    "api_version": 2,
                    "ready": True,
                    "research_ready": True,
                    "image_digests": [topic.eval_image_digest.removeprefix("sha256:")],
                    "inference_offer_commitment": topic.inference_offer_commitment,
                    "custom_ids": [topic.metric.custom_id],
                    "research_limits": OFFER.limits.model_dump(mode="json"),
                    "resource_caps": resource_caps,
                    "inference_offer": OFFER.model_dump(mode="json"),
                },
            )
        if request.url.path.startswith("/v1/vms/by-topic/"):
            return httpx.Response(404)
        body = json.loads(request.content)
        if request.url.path == "/v1/vms":
            return httpx.Response(
                201,
                json={
                    "vm_id": "fixture-vm",
                    "spec": body,
                    "state": "running",
                },
            )
        assert request.url.path == "/v1/vms/fixture-vm/agent"
        if agent_responses:
            response = agent_responses.pop(0)
            if isinstance(response, Exception):
                raise response
            return response
        return httpx.Response(200, json=alter(measured_outcome(topic, body, accepted=accepted)))

    backend = VmBackend(
        url="https://vm.example",
        token_file=token,
        image_digest=topic.eval_image_digest,
        inference_offer_commitment=topic.inference_offer_commitment,
        custom_ids=frozenset({topic.metric.custom_id}),
        resources=resources,
        transport=httpx.MockTransport(handler),
    )
    return backend, token, requests


async def test_http_vm_evidence_is_used_for_actual_intake_and_score(setup, tmp_path):
    service, _, topic, _ = setup
    resources = Resources(vcpus=1, mem_mib=1024, disk_mib=16384)
    backend, _, requests = make_backend(tmp_path, topic, resources=resources)
    service.backend = backend
    body, artifact = submission()
    try:
        result = await service.submit(body, artifact)
        assert result["status"] == "accepted"
        assert service.scores(4) == {body.miner_hotkey: 1000000}
        created = next(request for request in requests if request.url.path == "/v1/vms")
        assert json.loads(created.content)["resources"] == resources.model_dump(mode="json")
        assert json.loads(requests[-1].content)["request"]["task"]["rules"][0]["id"] == (
            "integrity-check"
        )
    finally:
        await backend.close()


@pytest.mark.parametrize(
    "caps",
    [
        None,
        {},
        {"vcpus": 17, "mem_mib": 32768, "disk_mib": 32768},
        {"vcpus": 1, "mem_mib": 1024, "disk_mib": 16384},
    ],
)
async def test_missing_or_incompatible_host_resources_refuse_before_nonce(setup, tmp_path, caps):
    service, _, topic, _ = setup
    backend, _, requests = make_backend(tmp_path, topic, resource_caps=caps)
    service.backend = backend
    body, artifact = submission()
    try:
        with pytest.raises(ServiceError, match="resource"):
            await service.submit(body, artifact)
        with service.store.transaction() as connection:
            assert connection.execute("SELECT count(*) FROM proof_nonces").fetchone()[0] == 0
        assert all(request.method == "GET" for request in requests)
    finally:
        await backend.close()


async def test_rotated_token_is_reread_for_each_request(setup, tmp_path):
    _, _, topic, _ = setup
    backend, token, _ = make_backend(tmp_path, topic)
    try:
        await backend.readiness()
        token.write_text("fixture-rotated-token")
        assert (await backend.readiness()).custom_ids == {"fixture-runner"}
    finally:
        await backend.close()


@pytest.mark.parametrize("tamper", ["topic", "job", "network", "metric", "digest"])
async def test_forged_or_misbound_remote_report_never_scores(setup, tmp_path, tamper):
    service, _, topic, _ = setup

    def alter(outcome):
        report = outcome["reports"][0]
        if tamper == "topic":
            report["topic_id"] = "other-topic"
        elif tamper == "job":
            report["job_id"] = "other-job"
        elif tamper == "network":
            report["network_enabled"] = True
        elif tamper == "metric":
            report["metrics"][0]["value"] = 0.4
        elif tamper == "digest":
            outcome["run"]["result"]["report_digest"] = "cd" * 32
        return outcome

    backend, _, _ = make_backend(tmp_path, topic, alter=alter)
    service.backend = backend
    body, artifact = submission()
    try:
        with pytest.raises(ServiceError) as error:
            await service.submit(body, artifact)
        assert error.value.status == 503
        assert service.store.submissions(4) == []
    finally:
        await backend.close()


async def test_preflight_reject_from_vm_persists_without_paid_metric(setup, tmp_path):
    service, _, topic, _ = setup
    backend, _, _ = make_backend(tmp_path, topic, accepted=False)
    service.backend = backend
    body, artifact = submission()
    try:
        result = await service.submit(body, artifact)
        assert result["status"] == "rejected"
        assert result["report"]["metrics"] == {}
        assert service.scores(4) == {}
    finally:
        await backend.close()


async def test_topic_timeout_above_host_ceiling_never_creates_vm(setup, tmp_path):
    _, _, topic, _ = setup
    backend, _, requests = make_backend(tmp_path, topic)
    submission_body, artifact = submission()
    topic = topic.model_copy(update={"wall_budget_s": 3600})
    try:
        with pytest.raises(ServiceError, match="wall budget exceeds"):
            await backend.evaluate(
                job_id="a" * 64, topic=topic, submission=submission_body, artifact=artifact, env={}
            )
        assert all(request.url.path == "/v1/health" for request in requests)
    finally:
        await backend.close()


@pytest.mark.parametrize("url", ["http://vm.example", "https://secret@vm.example", "https://x/a"])
def test_vm_client_requires_authenticated_https_origin(tmp_path, url):
    with pytest.raises(ValueError, match="HTTPS origin"):
        VmBackend(
            url=url,
            token_file=tmp_path / "unused",
            image_digest="sha256:" + "ab" * 32,
            inference_offer_commitment="bc" * 32,
            custom_ids=frozenset(),
        )


async def test_existing_guest_job_resumes_same_submission_only_on_explicit_marker(setup, tmp_path):
    service, _, topic, _ = setup
    backend, _, requests = make_backend(
        tmp_path,
        topic,
        agent_responses=[httpx.Response(409, json={"error": "research_resume_required"})],
    )
    service.backend = backend
    body, artifact = submission()
    try:
        result = await service.submit(body, artifact)

        assert result["status"] == "accepted"
        assert service.scores(4) == {body.miner_hotkey: 1000000}
        attempts = [json.loads(row.content) for row in requests if row.url.path.endswith("/agent")]
        assert len(attempts) == 2
        assert attempts[0]["request"]["resume"] is False
        assert attempts[1]["request"]["resume"] is True
        attempts[1]["request"]["resume"] = False
        assert attempts[0] == attempts[1]
        assert sum(row.method == "POST" and row.url.path == "/v1/vms" for row in requests) == 1
    finally:
        await backend.close()


@pytest.mark.parametrize(
    "response",
    [
        httpx.Response(409, json={"error": "topic agent busy"}),
        httpx.Response(409, json={"error": "research_resume_required", "extra": "untrusted"}),
        httpx.Response(409, content=b'{"error":"research_resume_required","error":"secret"}'),
        httpx.Response(409, content=b"untrusted-provider-secret"),
        httpx.Response(409, content=b'{"error":"research_resume_required"}' + b" " * 4096),
        httpx.Response(503, json={"error": "research_resume_required"}),
        httpx.ReadTimeout("untrusted-provider-secret"),
    ],
)
async def test_non_recoverable_host_errors_never_retry_or_leak_response(setup, tmp_path, response):
    service, _, topic, _ = setup
    backend, _, requests = make_backend(tmp_path, topic, agent_responses=[response])
    service.backend = backend
    body, artifact = submission()
    try:
        with pytest.raises(ServiceError) as error:
            await service.submit(body, artifact)

        assert error.value.status == 503
        assert "untrusted" not in error.value.reason and "secret" not in error.value.reason
        assert sum(row.url.path.endswith("/agent") for row in requests) == 1
        assert service.store.submissions(4) == []
    finally:
        await backend.close()


async def test_resume_negotiation_is_limited_to_one_identical_retry(setup, tmp_path):
    service, _, topic, _ = setup
    backend, _, requests = make_backend(
        tmp_path,
        topic,
        agent_responses=[
            httpx.Response(409, json={"error": "research_resume_required"}),
            httpx.Response(409, json={"error": "research_resume_required"}),
        ],
    )
    service.backend = backend
    body, artifact = submission()
    try:
        with pytest.raises(ServiceError) as error:
            await service.submit(body, artifact)

        assert error.value.status == 503
        assert sum(row.url.path.endswith("/agent") for row in requests) == 2
        assert service.store.submissions(4) == []
    finally:
        await backend.close()


async def test_already_explicit_resume_never_negotiates_another_attempt(setup, tmp_path):
    _, _, topic, _ = setup
    backend, _, requests = make_backend(
        tmp_path,
        topic,
        agent_responses=[httpx.Response(409, json={"error": "research_resume_required"})],
    )
    envelope = ResearchRequest(
        request=AgentRequest(
            resume=True,
            task=AgentTask(
                context=VmContext(
                    topic_id=topic.id,
                    job_id="known-job",
                    purpose="setup",
                    image_digest=topic.eval_image_digest.removeprefix("sha256:"),
                ),
                objective="Recover the original measured research",
                metric=topic.metric.primary,
                wall_budget_s=topic.wall_budget_s,
            ),
        ),
    )
    try:
        with pytest.raises(ServiceError, match="resume refused"):
            await backend.research(envelope, wall_seconds=30.0)

        attempts = [json.loads(row.content) for row in requests if row.url.path.endswith("/agent")]
        assert len(attempts) == 1
        assert attempts[0]["request"]["resume"] is True
    finally:
        await backend.close()


async def test_resume_marker_on_non_agent_route_is_an_ordinary_sanitized_failure(setup, tmp_path):
    _, _, topic, _ = setup
    token = tmp_path / "route-token"
    token.touch(mode=0o600)
    token.write_text("fixture-token")
    requests = []

    def respond(request):
        requests.append(request)
        return httpx.Response(409, json={"error": "research_resume_required"})

    backend = VmBackend(
        url="https://vm.example",
        token_file=token,
        image_digest=topic.eval_image_digest,
        inference_offer_commitment=topic.inference_offer_commitment,
        custom_ids=frozenset({topic.metric.custom_id}),
        transport=httpx.MockTransport(respond),
    )
    try:
        with pytest.raises(ServiceError, match="VM orchestrator refused request"):
            await backend.readiness()

        assert len(requests) == 1
        assert requests[0].url.path == "/v1/health"
    finally:
        await backend.close()
