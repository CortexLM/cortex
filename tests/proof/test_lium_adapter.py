from __future__ import annotations

import asyncio
import hashlib
import io
import json
import os
import tarfile
from collections.abc import Callable, Coroutine
from pathlib import Path

import httpx
import pytest

from cortex.proof.executor import (
    ExecutorPlan,
    HarvestExecution,
    HarvestFailure,
    HarvestRequest,
    LiumLease,
)
from cortex.proof.lium import (
    LiumAdapterConfig,
    LiumGuestWire,
    LiumRestSshAdapter,
    SshResult,
    SshTarget,
    SshTransport,
)
from cortex.proof.models import SubmissionLookup

from .harvest_fixtures import FixtureMaterialSource
from .test_executor import harvest_topic, signed_submission

IMAGE_HEX = "ab" * 32
IMAGE = "sha256:" + IMAGE_HEX
JOB_ID = "34" * 32
TEMPLATE_NAME = "proof-eval-" + IMAGE_HEX[:12]
TEMPLATE_ID = "template-provider-id"
PUBLIC_KEY = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFixture cortex-test"
PLAN_COMMITMENT = "bc" * 32
POD_IDENTITY = hashlib.sha256(bytes.fromhex(JOB_ID + PLAN_COMMITMENT)).hexdigest()
POD_NAME = "cortex-proof-" + POD_IDENTITY[:50]

SyncHandler = Callable[[httpx.Request], httpx.Response]
AsyncHandler = Callable[[httpx.Request], Coroutine[None, None, httpx.Response]]


def artifact_bytes() -> bytes:
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode="w") as archive:
        body = b"print('candidate')\n"
        info = tarfile.TarInfo("candidate.py")
        info.size = len(body)
        archive.addfile(info, io.BytesIO(body))
    return output.getvalue()


def harvest_request(*, env: dict[str, str] | None = None) -> HarvestRequest:
    topic = harvest_topic()
    submission, artifact = signed_submission()
    return HarvestRequest(
        job_id=JOB_ID,
        topic=topic,
        submission=SubmissionLookup.model_validate(submission.model_dump(exclude={"artifact_uri"})),
        material=FixtureMaterialSource().load(topic),
        topic_id=topic.id,
        topic_digest=topic.content_digest(),
        artifact_digest=hashlib.sha256(artifact).hexdigest(),
        artifact=artifact,
        claim=submission.claim,
        metric="holdout_nll",
        checklist=("integrity",),
        params=topic.params,
        env=env or {},
        eval_image_digest=IMAGE,
        inference_offer_commitment=topic.inference_offer_commitment,
        plan=ExecutorPlan(
            offer_id="lium-1x-v1",
            topic_id="harvest-topic",
            template_id=TEMPLATE_NAME,
            gpu_count=1,
            deadline_s=30,
            offer_commitment="9a" * 32,
            config_commitment=PLAN_COMMITMENT,
            overridden=False,
        ),
    )


def execution(request: HarvestRequest) -> HarvestExecution:
    return HarvestExecution(
        request_commitment=request.commitment(),
        topic_digest=request.topic_digest,
        environment_digest=request.material.environment_digest,
        private_holdout_digest=request.material.private_holdout_digest,
        inference_offer_commitment=request.inference_offer_commitment,
        experiment_vm_id="experiment-fixture-vm",
        teardown_confirmed=True,
        topic_id=request.topic_id,
        job_id=request.job_id,
        artifact_digest=request.artifact_digest,
        eval_image_digest=request.eval_image_digest,
        executor_config_commitment=request.plan.config_commitment,
        gpu_count=1,
        exit_code=0,
        verdict="clean",
        reproduced=True,
        claim_holds=True,
        metrics={"holdout_nll": 0.8},
        rule_results={"integrity": True},
        flops_used=100,
        wall_seconds=2.0,
        evidence_digest="de" * 32,
        stdout_tail="image completed",
        sandboxed=True,
    )


@pytest.fixture
def lium_files(tmp_path: Path) -> dict[str, Path]:
    paths = {
        "api": tmp_path / "lium-api-key",
        "private": tmp_path / "lium-ssh",
        "public": tmp_path / "lium-ssh.pub",
        "known_hosts": tmp_path / "lium-known-hosts",
    }
    paths["api"].write_text("provider-secret")
    paths["private"].write_text("private-key-fixture")
    paths["public"].write_text(PUBLIC_KEY)
    paths["known_hosts"].write_text(f"[203.0.113.10]:22022 {PUBLIC_KEY}\n")
    os.chmod(paths["api"], 0o600)
    os.chmod(paths["private"], 0o600)
    return paths


def config(files: dict[str, Path]) -> LiumAdapterConfig:
    return LiumAdapterConfig(
        api_base_url="https://lium.test/api",
        api_key_file=files["api"],
        ssh_private_key_file=files["private"],
        ssh_public_key_file=files["public"],
        ssh_known_hosts_file=files["known_hosts"],
        image_repository="ghcr.io/cortexlm/proof-eval",
        gpu_name="B200",
        max_price_per_hour=8.0,
        max_lifetime_hours=1.0,
        poll_interval_seconds=0.25,
        running_timeout_seconds=1,
        request_timeout_seconds=1,
        teardown_attempts=2,
    )


def template_row(*, digest: str = IMAGE) -> dict[str, object]:
    return {
        "id": TEMPLATE_ID,
        "name": TEMPLATE_NAME,
        "docker_image": "ghcr.io/cortexlm/proof-eval",
        "docker_image_digest": digest,
    }


def offer_row() -> dict[str, object]:
    return {
        "id": "executor-b200",
        "gpu_type": "NVIDIA B200",
        "gpu_count": 1,
        "price_per_hour": 5.5,
    }


def pod_row(*, pod_id: str = "pod-1", status: str = "RUNNING") -> dict[str, object]:
    return {
        "id": pod_id,
        "pod_name": POD_NAME,
        "status": status,
        "gpu_type": "NVIDIA B200",
        "template_id": TEMPLATE_ID,
        "ssh_connect_cmd": "ssh root@203.0.113.10 -p 22022",
    }


def bump(state: dict[str, object], key: str) -> None:
    current = state.get(key, 0)
    assert isinstance(current, int)
    state[key] = current + 1


class FakeSsh:
    def __init__(self) -> None:
        self.calls: list[tuple[SshTarget, str, bytes | None, bool]] = []

    async def run(
        self,
        target: SshTarget,
        command: str,
        *,
        stdin: bytes | None = None,
        timeout_seconds: float,
        allow_failure: bool = False,
    ) -> SshResult:
        del timeout_seconds
        self.calls.append((target, command, stdin, allow_failure))
        return SshResult(0, "ok", "")


class FakeGuestWire:
    def __init__(self, report: HarvestExecution) -> None:
        self.report = report
        self.calls: list[tuple[SshTransport, SshTarget, HarvestRequest]] = []
        self.timeout = False

    async def execute(
        self,
        ssh: SshTransport,
        target: SshTarget,
        request: HarvestRequest,
    ) -> HarvestExecution:
        self.calls.append((ssh, target, request))
        if self.timeout:
            raise TimeoutError
        return self.report


def standard_handler(
    state: dict[str, object],
) -> SyncHandler:
    def handler(request: httpx.Request) -> httpx.Response:
        path = request.url.path
        if path == "/api/users/me":
            return httpx.Response(200, json={"id": "operator"})
        if path == "/api/ssh-keys" and request.method == "GET":
            return httpx.Response(200, json=[{"id": "key-1", "public_key": PUBLIC_KEY}])
        if path == "/api/templates" and request.method == "GET":
            return httpx.Response(200, json=[state.get("template", template_row())])
        if path == "/api/executors":
            return httpx.Response(200, json=[offer_row()])
        if path == "/api/pods" and request.method == "GET":
            return httpx.Response(200, json=state.get("pods", []))
        if path == "/api/executors/executor-b200/rent":
            bump(state, "rent_posts")
            state["rent_body"] = json.loads(request.content)
            state["pods"] = [pod_row()]
            return httpx.Response(200, json={"id": "pod-1"})
        if path.startswith("/api/pods/"):
            pod_id = path.rsplit("/", 1)[-1]
            pods = state.get("pods", [])
            assert isinstance(pods, list)
            row = next(
                (item for item in pods if isinstance(item, dict) and item.get("id") == pod_id),
                None,
            )
            if request.method == "GET":
                return httpx.Response(200, json=row) if row else httpx.Response(404)
            if request.method != "DELETE":
                raise AssertionError(f"unexpected provider request: {request.method} {path}")
            state["deleted"] = True
            deleted_ids = state.setdefault("deleted_ids", [])
            assert isinstance(deleted_ids, list)
            deleted_ids.append(pod_id)
            state["pods"] = [item for item in pods if item is not row]
            return httpx.Response(204)
        raise AssertionError(f"unexpected provider request: {request.method} {path}")

    return handler


async def no_sleep(_: float) -> None:
    return None


async def adapter_for(
    files: dict[str, Path],
    state: dict[str, object],
    ssh: FakeSsh,
    *,
    guest_wire: LiumGuestWire | None = None,
    handler: SyncHandler | AsyncHandler | None = None,
) -> tuple[LiumRestSshAdapter, httpx.AsyncClient]:
    transport_handler = handler if handler is not None else standard_handler(state)
    client = httpx.AsyncClient(transport=httpx.MockTransport(transport_handler))
    adapter = LiumRestSshAdapter(
        config(files),
        http=client,
        ssh=ssh,
        guest_wire=guest_wire,
        sleep=no_sleep,
    )
    return adapter, client


async def test_rent_is_exactly_one_gpu_and_retry_reuses_the_job_pod(lium_files):
    state: dict[str, object] = {}
    request = harvest_request()
    ssh = FakeSsh()
    adapter, client = await adapter_for(lium_files, state, ssh)
    try:
        first = await adapter.rent(request)
        second = await adapter.rent(request)
    finally:
        await client.aclose()

    assert first == second
    assert first.gpu_count == 1
    assert state["rent_posts"] == 1
    assert state["rent_body"] == {
        "pod_name": POD_NAME,
        "user_public_key": [PUBLIC_KEY],
        "termination_hours": 1,
        "gpu_count": 1,
        "template_id": TEMPLATE_ID,
    }
    assert len(POD_NAME) == 63


async def test_parallel_rent_calls_for_one_job_emit_one_paid_post(lium_files):
    state: dict[str, object] = {}
    request = harvest_request()
    ssh = FakeSsh()
    adapter, client = await adapter_for(lium_files, state, ssh)
    try:
        first, second = await asyncio.gather(adapter.rent(request), adapter.rent(request))
    finally:
        await client.aclose()

    assert first == second
    assert state["rent_posts"] == 1


async def test_ambiguous_rent_response_is_reconciled_without_second_post(lium_files):
    state: dict[str, object] = {"pods": []}
    request = harvest_request()
    ssh = FakeSsh()
    rent_attempted = False
    post_rent_lists = 0

    def handler(provider_request: httpx.Request) -> httpx.Response:
        nonlocal rent_attempted, post_rent_lists
        if provider_request.url.path == "/api/executors/executor-b200/rent":
            bump(state, "rent_posts")
            state["pods"] = [pod_row(status="PENDING")]
            rent_attempted = True
            raise httpx.ReadTimeout("uncertain rent response", request=provider_request)
        if provider_request.url.path == "/api/pods" and rent_attempted:
            post_rent_lists += 1
            if post_rent_lists == 1:
                return httpx.Response(200, json=[])
        if provider_request.url.path == "/api/pods/pod-1":
            state["pods"] = [pod_row()]
            return httpx.Response(200, json=pod_row())
        return standard_handler(state)(provider_request)

    adapter, client = await adapter_for(lium_files, state, ssh, handler=handler)
    try:
        lease = await adapter.rent(request)
        retried = await adapter.rent(request)
    finally:
        await client.aclose()

    assert lease == retried
    assert lease.instance_id == "pod-1"
    assert state["rent_posts"] == 1
    assert post_rent_lists >= 2


async def test_cancellation_during_rent_reconciles_and_deletes_created_pod(lium_files):
    state: dict[str, object] = {"pods": []}
    request = harvest_request()
    ssh = FakeSsh()
    rent_entered = asyncio.Event()

    async def handler(provider_request: httpx.Request) -> httpx.Response:
        if provider_request.url.path == "/api/executors/executor-b200/rent":
            bump(state, "rent_posts")
            state["pods"] = [pod_row(status="PENDING")]
            rent_entered.set()
            await asyncio.Future()
        response = standard_handler(state)(provider_request)
        assert isinstance(response, httpx.Response)
        return response

    adapter, client = await adapter_for(lium_files, state, ssh, handler=handler)
    task = asyncio.create_task(adapter.rent(request))
    await rent_entered.wait()
    task.cancel()
    try:
        with pytest.raises(asyncio.CancelledError):
            await task
    finally:
        await client.aclose()

    assert state["rent_posts"] == 1
    assert state["deleted"] is True
    assert state["pods"] == []


async def test_template_name_bound_to_another_digest_refuses_before_rent(lium_files):
    state: dict[str, object] = {"template": template_row(digest="sha256:" + "cd" * 32)}
    request = harvest_request()
    ssh = FakeSsh()
    adapter, client = await adapter_for(lium_files, state, ssh)
    try:
        with pytest.raises(HarvestFailure, match="template digest binding mismatch"):
            await adapter.rent(request)
    finally:
        await client.aclose()

    assert state.get("rent_posts", 0) == 0


async def test_conflicting_template_digest_fields_refuse_before_rent(lium_files):
    template = template_row()
    template["docker_image"] = "ghcr.io/cortexlm/proof-eval@sha256:" + "cd" * 32
    state: dict[str, object] = {"template": template}
    request = harvest_request()
    ssh = FakeSsh()
    adapter, client = await adapter_for(lium_files, state, ssh)
    try:
        with pytest.raises(HarvestFailure, match="template digest binding mismatch"):
            await adapter.rent(request)
    finally:
        await client.aclose()

    assert state.get("rent_posts", 0) == 0


async def test_malformed_pod_collection_refuses_before_paid_rent(lium_files):
    state: dict[str, object] = {}
    request = harvest_request()
    ssh = FakeSsh()

    def handler(provider_request: httpx.Request) -> httpx.Response:
        if provider_request.url.path == "/api/pods":
            return httpx.Response(200, json={})
        return standard_handler(state)(provider_request)

    adapter, client = await adapter_for(lium_files, state, ssh, handler=handler)
    try:
        with pytest.raises(HarvestFailure, match="malformed collection"):
            await adapter.rent(request)
    finally:
        await client.aclose()

    assert state.get("rent_posts", 0) == 0


async def test_cancellation_while_reconciling_a_rent_cleans_the_late_pod(lium_files):
    state: dict[str, object] = {"pods": []}
    request = harvest_request()
    ssh = FakeSsh()
    reconcile_entered = asyncio.Event()
    rent_returned = False

    async def handler(provider_request: httpx.Request) -> httpx.Response:
        nonlocal rent_returned
        path = provider_request.url.path
        if path == "/api/executors/executor-b200/rent":
            bump(state, "rent_posts")
            state["pods"] = [pod_row(status="PENDING")]
            rent_returned = True
            return httpx.Response(200, json={})
        if path == "/api/pods" and rent_returned and not reconcile_entered.is_set():
            reconcile_entered.set()
            await asyncio.Future()
        return standard_handler(state)(provider_request)

    adapter, client = await adapter_for(lium_files, state, ssh, handler=handler)
    task = asyncio.create_task(adapter.rent(request))
    await reconcile_entered.wait()
    task.cancel()
    try:
        with pytest.raises(asyncio.CancelledError):
            await task
    finally:
        await client.aclose()

    assert state["rent_posts"] == 1
    assert state["deleted_ids"] == ["pod-1"]
    assert state["pods"] == []


async def test_duplicate_job_pods_are_all_deleted_before_rent_refuses(lium_files):
    state: dict[str, object] = {"pods": [pod_row(pod_id="pod-1"), pod_row(pod_id="pod-2")]}
    request = harvest_request()
    ssh = FakeSsh()
    adapter, client = await adapter_for(lium_files, state, ssh)
    try:
        with pytest.raises(HarvestFailure, match="multiple Lium pods"):
            await adapter.rent(request)
    finally:
        await client.aclose()

    assert state["deleted_ids"] == ["pod-1", "pod-2"]
    assert state["pods"] == []
    assert state.get("rent_posts", 0) == 0


async def test_nested_wrong_gpu_metadata_is_rejected_and_cleaned_up(lium_files):
    state: dict[str, object] = {"pods": []}
    request = harvest_request()
    ssh = FakeSsh()

    def handler(provider_request: httpx.Request) -> httpx.Response:
        if (
            provider_request.url.path == "/api/pods/pod-1"
            and provider_request.method == "GET"
            and state.get("pods")
        ):
            row = pod_row()
            del row["gpu_type"]
            row["executor"] = {"gpu_type": "NVIDIA A100"}
            return httpx.Response(200, json=row)
        return standard_handler(state)(provider_request)

    adapter, client = await adapter_for(lium_files, state, ssh, handler=handler)
    try:
        with pytest.raises(HarvestFailure, match="GPU does not match"):
            await adapter.rent(request)
    finally:
        await client.aclose()

    assert state["deleted_ids"] == ["pod-1"]
    assert state["pods"] == []


async def test_existing_job_pod_with_wrong_template_is_deleted_and_refused(lium_files):
    row = pod_row()
    row["template_id"] = "different-template"
    state: dict[str, object] = {"pods": [row]}
    request = harvest_request()
    ssh = FakeSsh()
    adapter, client = await adapter_for(lium_files, state, ssh)
    try:
        with pytest.raises(HarvestFailure, match="pod template binding mismatch"):
            await adapter.rent(request)
    finally:
        await client.aclose()

    assert state["deleted_ids"] == ["pod-1"]
    assert state["pods"] == []
    assert state.get("rent_posts", 0) == 0


async def test_new_pod_without_template_proof_is_deleted_and_refused(lium_files):
    state: dict[str, object] = {"pods": []}
    request = harvest_request()
    ssh = FakeSsh()

    def handler(provider_request: httpx.Request) -> httpx.Response:
        if (
            provider_request.url.path == "/api/pods/pod-1"
            and provider_request.method == "GET"
            and state.get("pods")
        ):
            row = pod_row()
            del row["template_id"]
            return httpx.Response(200, json=row)
        return standard_handler(state)(provider_request)

    adapter, client = await adapter_for(lium_files, state, ssh, handler=handler)
    try:
        with pytest.raises(HarvestFailure, match="pod template binding mismatch"):
            await adapter.rent(request)
    finally:
        await client.aclose()

    assert state["deleted_ids"] == ["pod-1"]
    assert state["pods"] == []


async def test_invalid_rent_response_id_reconciles_and_deletes_named_pod(lium_files):
    state: dict[str, object] = {"pods": []}
    request = harvest_request()
    ssh = FakeSsh()

    def handler(provider_request: httpx.Request) -> httpx.Response:
        if provider_request.url.path == "/api/executors/executor-b200/rent":
            bump(state, "rent_posts")
            state["pods"] = [pod_row(status="PENDING")]
            return httpx.Response(200, json={"id": "invalid/pod-id"})
        return standard_handler(state)(provider_request)

    adapter, client = await adapter_for(lium_files, state, ssh, handler=handler)
    try:
        with pytest.raises(HarvestFailure, match="pod has an invalid id"):
            await adapter.rent(request)
    finally:
        await client.aclose()

    assert state["rent_posts"] == 1
    assert state["deleted_ids"] == ["pod-1"]
    assert state["pods"] == []


async def test_rent_response_cannot_substitute_a_different_pod_id(lium_files):
    state: dict[str, object] = {"pods": []}
    request = harvest_request()
    ssh = FakeSsh()

    def handler(provider_request: httpx.Request) -> httpx.Response:
        path = provider_request.url.path
        if path == "/api/executors/executor-b200/rent":
            bump(state, "rent_posts")
            state["pods"] = [pod_row()]
            return httpx.Response(200, json={"id": "pod-2"})
        if path == "/api/pods/pod-2" and provider_request.method == "GET":
            if state.get("pod2_deleted"):
                return httpx.Response(404)
            return httpx.Response(200, json=pod_row(pod_id="pod-2"))
        if path == "/api/pods/pod-2" and provider_request.method == "DELETE":
            deleted_ids = state.setdefault("deleted_ids", [])
            assert isinstance(deleted_ids, list)
            deleted_ids.append("pod-2")
            state["pod2_deleted"] = True
            return httpx.Response(204)
        return standard_handler(state)(provider_request)

    adapter, client = await adapter_for(lium_files, state, ssh, handler=handler)
    try:
        with pytest.raises(HarvestFailure, match="pod identity binding mismatch"):
            await adapter.rent(request)
    finally:
        await client.aclose()

    assert state["rent_posts"] == 1
    deleted_ids = state["deleted_ids"]
    assert isinstance(deleted_ids, list)
    assert set(deleted_ids) == {"pod-1", "pod-2"}
    assert state["pods"] == []


async def test_missing_template_is_created_then_reloaded_with_exact_digest_binding(
    lium_files,
):
    state: dict[str, object] = {"templates": [], "pods": []}
    request = harvest_request()
    ssh = FakeSsh()

    def handler(provider_request: httpx.Request) -> httpx.Response:
        path = provider_request.url.path
        if path == "/api/templates" and provider_request.method == "GET":
            return httpx.Response(200, json=state["templates"])
        if path == "/api/templates" and provider_request.method == "POST":
            state["template_body"] = json.loads(provider_request.content)
            state["templates"] = [template_row()]
            return httpx.Response(201, json={"id": TEMPLATE_ID})
        return standard_handler(state)(provider_request)

    adapter, client = await adapter_for(lium_files, state, ssh, handler=handler)
    try:
        lease = await adapter.rent(request)
    finally:
        await client.aclose()

    assert lease.template_id == TEMPLATE_NAME
    assert state["template_body"] == {
        "name": TEMPLATE_NAME,
        "docker_image": f"ghcr.io/cortexlm/proof-eval@{IMAGE}",
        "internal_ports": [22],
        "is_private": True,
        "container_start_immediately": True,
    }


async def test_provider_error_never_reflects_api_key(lium_files):
    state: dict[str, object] = {}
    request = harvest_request()
    ssh = FakeSsh()

    def handler(provider_request: httpx.Request) -> httpx.Response:
        if provider_request.url.path == "/api/templates":
            return httpx.Response(500, text="provider-secret failed provider request")
        return standard_handler(state)(provider_request)

    adapter, client = await adapter_for(lium_files, state, ssh, handler=handler)
    try:
        with pytest.raises(HarvestFailure) as caught:
            await adapter.rent(request)
    finally:
        await client.aclose()

    assert "provider-secret" not in str(caught.value)
    assert "<redacted>" in str(caught.value)
    assert state.get("rent_posts", 0) == 0


async def test_probe_is_false_without_a_versioned_guest_wire(lium_files):
    state: dict[str, object] = {}
    ssh = FakeSsh()
    adapter, client = await adapter_for(lium_files, state, ssh)
    try:
        ready = await adapter.probe()
    finally:
        await client.aclose()

    assert ready is False
    assert state == {}


async def test_probe_is_true_when_credentials_provider_and_guest_wire_are_ready(lium_files):
    state: dict[str, object] = {}
    request = harvest_request()
    ssh = FakeSsh()
    guest_wire = FakeGuestWire(execution(request))
    adapter, client = await adapter_for(lium_files, state, ssh, guest_wire=guest_wire)
    try:
        ready = await adapter.probe()
    finally:
        await client.aclose()

    assert ready is True


async def test_execute_refuses_without_a_guest_wire_before_provider_or_ssh_io(lium_files):
    state: dict[str, object] = {}
    request = harvest_request()
    ssh = FakeSsh()
    adapter, client = await adapter_for(lium_files, state, ssh)
    lease = LiumLease(
        instance_id="pod-1",
        template_id=TEMPLATE_NAME,
        gpu_count=1,
        image_digest=IMAGE,
    )
    try:
        with pytest.raises(HarvestFailure, match="guest request/result contract unavailable"):
            await adapter.execute(lease, request)
    finally:
        await client.aclose()

    assert state == {}
    assert ssh.calls == []


async def test_execute_delegates_the_versioned_guest_contract_and_verifies_binding(lium_files):
    state: dict[str, object] = {"pods": [pod_row()]}
    request = harvest_request(env={"OPENROUTER_API_KEY": "miner-private-secret"})
    ssh = FakeSsh()
    guest_wire = FakeGuestWire(execution(request))
    adapter, client = await adapter_for(lium_files, state, ssh, guest_wire=guest_wire)
    try:
        lease = await adapter.rent(request)
        result = await adapter.execute(lease, request)
    finally:
        await client.aclose()

    assert result == execution(request)
    assert guest_wire.calls == [
        (ssh, SshTarget(user="root", host="203.0.113.10", port=22022), request)
    ]


async def test_guest_wire_timeout_is_reported_as_a_proof_deadline(lium_files):
    state: dict[str, object] = {"pods": [pod_row()]}
    request = harvest_request()
    ssh = FakeSsh()
    guest_wire = FakeGuestWire(execution(request))
    guest_wire.timeout = True
    adapter, client = await adapter_for(lium_files, state, ssh, guest_wire=guest_wire)
    try:
        lease = await adapter.rent(request)
        with pytest.raises(HarvestFailure, match="proof deadline exceeded"):
            await adapter.execute(lease, request)
    finally:
        await client.aclose()


async def test_execute_rejects_a_guest_report_bound_to_another_job(lium_files):
    state: dict[str, object] = {"pods": [pod_row()]}
    request = harvest_request()
    ssh = FakeSsh()
    wrong_report = execution(request).model_copy(update={"job_id": "12" * 32})
    guest_wire = FakeGuestWire(wrong_report)
    adapter, client = await adapter_for(lium_files, state, ssh, guest_wire=guest_wire)
    try:
        lease = await adapter.rent(request)
        with pytest.raises(HarvestFailure, match="report binding mismatch"):
            await adapter.execute(lease, request)
    finally:
        await client.aclose()


async def test_adapter_revalidates_environment_names_before_building_shell_file(
    lium_files,
):
    state: dict[str, object] = {"pods": [pod_row()]}
    request = harvest_request().model_copy(update={"env": {"BAD; touch /tmp/escaped": "private"}})
    ssh = FakeSsh()
    guest_wire = FakeGuestWire(execution(request))
    adapter, client = await adapter_for(lium_files, state, ssh, guest_wire=guest_wire)
    try:
        lease = await adapter.rent(request)
        with pytest.raises(HarvestFailure, match="environment name"):
            await adapter.execute(lease, request)
    finally:
        await client.aclose()

    assert guest_wire.calls == []
    assert ssh.calls == []


async def test_adapter_revalidates_environment_count_before_provider_io(lium_files):
    state: dict[str, object] = {}
    request = harvest_request().model_copy(
        update={"env": {f"MINER_KEY_{index}": "private" for index in range(9)}}
    )
    ssh = FakeSsh()
    guest_wire = FakeGuestWire(execution(request))
    adapter, client = await adapter_for(lium_files, state, ssh, guest_wire=guest_wire)
    lease = LiumLease(
        instance_id="pod-1",
        template_id=TEMPLATE_NAME,
        gpu_count=1,
        image_digest=IMAGE,
    )
    try:
        with pytest.raises(HarvestFailure, match="too many guest environment variables"):
            await adapter.execute(lease, request)
    finally:
        await client.aclose()

    assert state == {}
    assert guest_wire.calls == []


async def test_malformed_pod_detail_never_confirms_teardown(lium_files):
    state: dict[str, object] = {}
    ssh = FakeSsh()

    def handler(provider_request: httpx.Request) -> httpx.Response:
        if provider_request.url.path in {"/api/pods", "/api/pods/pod-1"}:
            return httpx.Response(200, json={})
        return standard_handler(state)(provider_request)

    adapter, client = await adapter_for(lium_files, state, ssh, handler=handler)
    lease = LiumLease(
        instance_id="pod-1",
        template_id=TEMPLATE_NAME,
        gpu_count=1,
        image_digest=IMAGE,
    )
    try:
        gone = await adapter.verify_terminated(lease)
    finally:
        await client.aclose()

    assert gone is False


async def test_terminate_requires_provider_absence_confirmation(lium_files):
    state: dict[str, object] = {"pods": [pod_row()]}
    request = harvest_request()
    ssh = FakeSsh()
    adapter, client = await adapter_for(lium_files, state, ssh)
    try:
        lease = await adapter.rent(request)
        await adapter.terminate(lease)
        gone = await adapter.verify_terminated(lease)
    finally:
        await client.aclose()

    assert state["deleted"] is True
    assert gone is True
