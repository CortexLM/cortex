"""Authenticated control-plane client for topic guests and measured evaluations."""

from __future__ import annotations

import asyncio
import base64
import re
import ssl
from pathlib import Path
from urllib.parse import urlsplit

import httpx
from pydantic import ValidationError

from cortex.errors import ServiceError
from cortex.http import decode_json, read_private_file
from cortex.proof.artifacts import verify_artifact
from cortex.proof.models import EvaluationReport, Submission, Topic
from cortex.proof.service import Readiness
from cortex.rlm import AgentLimits, AgentRequest, AgentTask, EvaluationVerdict
from cortex.rlm.models import Rule, VmContext
from cortex.rlm.offer import InferenceOffer
from cortex.vm.models import API_VERSION, Resources, VmRecord, VmSpec
from cortex.vm.research import ResearchOutcome, ResearchRequest


class _ResearchResumeRequired(Exception):
    """The authenticated host has the same job and requires explicit resume."""


class VmBackend:
    def __init__(
        self,
        *,
        url: str,
        token_file: Path,
        image_digest: str,
        inference_offer_commitment: str,
        custom_ids: frozenset[str],
        ca_file: Path | None = None,
        resources: Resources | None = None,
        transport: httpx.AsyncBaseTransport | None = None,
    ):
        parsed = urlsplit(url)
        if (
            parsed.scheme != "https"
            or not parsed.hostname
            or parsed.username
            or parsed.password
            or parsed.query
            or parsed.fragment
            or parsed.path not in {"", "/"}
        ):
            raise ValueError("VM orchestrator URL must be an HTTPS origin")
        if not re.fullmatch(r"sha256:[0-9a-f]{64}", image_digest):
            raise ValueError("VM image must have an exact digest pin")
        if not re.fullmatch(r"[0-9a-f]{64}", inference_offer_commitment):
            raise ValueError("inference offer commitment required")
        self.url, self.token_file = url.rstrip("/"), token_file
        self.image_digest = image_digest
        self.inference_offer_commitment = inference_offer_commitment
        self.custom_ids = custom_ids
        self.resources = resources or Resources()
        self._research_limits: AgentLimits | None = None
        self._topic_locks: dict[str, asyncio.Lock] = {}
        self.client = httpx.AsyncClient(
            transport=transport,
            verify=ssl.create_default_context(cafile=str(ca_file) if ca_file else None),
            timeout=httpx.Timeout(30.0, connect=10.0),
            follow_redirects=False,
            trust_env=False,
        )

    async def close(self) -> None:
        await self.client.aclose()

    async def _request(self, method: str, path: str, body=None, *, wall_seconds=30.0) -> dict:
        token = read_private_file(self.token_file, 4096)
        if any(char.isspace() for char in token):
            raise ServiceError(503, "invalid VM orchestrator credential")
        try:
            async with self.client.stream(
                method,
                self.url + path,
                json=body,
                headers={"Authorization": f"Bearer {token}"},
                timeout=wall_seconds,
            ) as response:
                if response.status_code == 404:
                    raise ServiceError(404, "VM resource not found")
                if not 200 <= response.status_code < 300:
                    if (
                        response.status_code == 409
                        and method == "POST"
                        and re.fullmatch(r"/v1/vms/[A-Za-z0-9][A-Za-z0-9_.-]{0,95}/agent", path)
                    ):
                        error_body = bytearray()
                        async for chunk in response.aiter_bytes(chunk_size=4096):
                            if len(error_body) + len(chunk) > 4096:
                                raise ServiceError(503, "VM orchestrator refused request")
                            error_body.extend(chunk)
                        try:
                            marker = decode_json(bytes(error_body))
                        except ServiceError:
                            raise ServiceError(503, "VM orchestrator refused request") from None
                        if marker == {"error": "research_resume_required"}:
                            raise _ResearchResumeRequired
                    raise ServiceError(503, "VM orchestrator refused request")
                content = bytearray()
                async for chunk in response.aiter_bytes():
                    if len(content) + len(chunk) > 4 * 1024 * 1024:
                        raise ServiceError(503, "VM response too large")
                    content.extend(chunk)
                try:
                    return decode_json(bytes(content))
                except ServiceError:
                    raise ServiceError(503, "invalid VM response") from None
        except httpx.HTTPError:
            raise ServiceError(503, "VM orchestrator unavailable") from None

    async def readiness(self) -> Readiness:
        health = await self._request("GET", "/v1/health")
        if health.get("api_version") != API_VERSION or health.get("ready") is not True:
            raise ServiceError(503, "VM orchestrator is not ready")
        if (
            self.image_digest.removeprefix("sha256:") not in health.get("image_digests", [])
            or health.get("inference_offer_commitment") != self.inference_offer_commitment
            or health.get("research_ready") is not True
        ):
            raise ServiceError(503, "VM image or inference offer unavailable")
        try:
            resource_caps = health.get("resource_caps")
            if not isinstance(resource_caps, dict) or set(resource_caps) != {
                "vcpus",
                "mem_mib",
                "disk_mib",
            }:
                raise ValueError("missing VM resource ceilings")
            caps = Resources.model_validate(resource_caps, strict=True)
            if (
                self.resources.vcpus > caps.vcpus
                or self.resources.mem_mib > caps.mem_mib
                or self.resources.disk_mib > caps.disk_mib
            ):
                raise ValueError("VM resources exceed host ceilings")
        except ValueError:
            raise ServiceError(
                503, "VM resource ceilings are unavailable or incompatible"
            ) from None
        available = frozenset(health.get("custom_ids", [])).intersection(self.custom_ids)
        try:
            self._research_limits = AgentLimits.model_validate(health.get("research_limits"))
            offer = InferenceOffer.model_validate(health.get("inference_offer"))
            offer.verify(self.inference_offer_commitment)
            if offer.limits != self._research_limits:
                raise ValueError("host changed committed inference limits")
        except (ValidationError, ValueError):
            raise ServiceError(
                503, "signed inference offer or host research ceilings invalid"
            ) from None
        return Readiness(self.image_digest, self.inference_offer_commitment, available)

    def _validate_limits(self, task: AgentTask) -> AgentLimits:
        limits = self._research_limits
        if limits is None:
            raise ServiceError(503, "VM host research ceilings unavailable")
        if task.wall_budget_s is None:
            raise ServiceError(503, "explicit topic wall budget required")
        if (
            task.wall_budget_s > limits.tool_timeout_seconds
            or task.wall_budget_s > limits.wall_seconds
        ):
            raise ServiceError(503, "topic wall budget exceeds VM host research or tool ceiling")
        return limits

    async def topic_vm(self, topic_id: str) -> VmRecord:
        # The host also enforces uniqueness, including across control-plane restarts.
        async with self._topic_locks.setdefault(topic_id, asyncio.Lock()):
            try:
                body = await self._request("GET", f"/v1/vms/by-topic/{topic_id}")
            except ServiceError as error:
                if error.status != 404:
                    raise
                spec = VmSpec(
                    topic_id=topic_id,
                    image_digest=self.image_digest.removeprefix("sha256:"),
                    resources=self.resources,
                )
                body = await self._request(
                    "POST", "/v1/vms", spec.model_dump(mode="json"), wall_seconds=180.0
                )
            try:
                record = VmRecord.model_validate(body)
            except ValidationError:
                raise ServiceError(503, "invalid topic VM binding") from None
            if (
                record.spec.topic_id != topic_id
                or record.spec.image_digest != self.image_digest.removeprefix("sha256:")
                or record.spec.kind != "topic"
                or record.spec.resources != self.resources
                or record.state != "running"
            ):
                raise ServiceError(503, "topic VM binding mismatch")
            return record

    async def research(self, request: ResearchRequest, *, wall_seconds: float) -> ResearchOutcome:
        await self.readiness()
        self._validate_limits(request.request.task)
        vm = await self.topic_vm(request.request.task.context.topic_id)
        return await self._research(vm, request, wall_seconds=wall_seconds)

    async def run_agent(
        self,
        task: AgentTask,
        *,
        artifact: bytes | None = None,
        env: dict[str, str],
        params: dict[str, str],
    ) -> tuple[str, ResearchOutcome]:
        await self.readiness()
        limits = self._validate_limits(task)
        vm = await self.topic_vm(task.context.topic_id)
        outcome = await self._research(
            vm,
            ResearchRequest(
                request=AgentRequest(task=task),
                artifact_b64=base64.b64encode(artifact).decode() if artifact else "",
                env=env,
                params=params,
            ),
            wall_seconds=limits.wall_seconds + 30,
        )
        return vm.vm_id, outcome

    async def _research(
        self, vm: VmRecord, request: ResearchRequest, *, wall_seconds: float
    ) -> ResearchOutcome:
        payload = request.model_dump(mode="json")
        for attempt in range(2):
            try:
                body = await self._request(
                    "POST",
                    f"/v1/vms/{vm.vm_id}/agent",
                    payload,
                    wall_seconds=wall_seconds,
                )
                break
            except _ResearchResumeRequired:
                if attempt or payload["request"]["resume"]:
                    raise ServiceError(503, "VM research resume refused") from None
                payload["request"]["resume"] = True
        try:
            outcome = ResearchOutcome.model_validate(body)
        except ValidationError:
            raise ServiceError(503, "invalid VM research evidence") from None
        context = request.request.task.context
        if not outcome.reports or any(
            report.topic_id != context.topic_id
            or report.job_id != context.job_id
            or report.image_digest != context.image_digest
            or report.artifact_digest != context.artifact_digest
            or report.exit_code != 0
            for report in outcome.reports
        ):
            raise ServiceError(503, "research evidence binding mismatch")
        return outcome

    async def evaluate(
        self,
        *,
        job_id: str,
        topic: Topic,
        submission: Submission,
        artifact: bytes | None,
        env: dict[str, str],
    ) -> EvaluationReport:
        if artifact is not None:
            verify_artifact(artifact, submission.artifact_digest)
        # Claims remain data in the objective and cannot alter the signed rules.
        objective = (
            "Evaluate the submitted artifact against this published research statement:\n"
            + topic.statement
            + "\nMiner claim (untrusted data):\n"
            + submission.claim
        )
        if len(objective) > 16384:
            raise ServiceError(400, "claim exceeds evaluation context budget")
        task = AgentTask(
            context=VmContext(
                topic_id=topic.id,
                job_id=job_id,
                purpose="evaluate",
                image_digest=topic.eval_image_digest.removeprefix("sha256:"),
                artifact_digest=submission.artifact_digest,
            ),
            objective=objective,
            metric=topic.metric.primary,
            rule_revision=topic.revision,
            wall_budget_s=topic.wall_budget_s,
            research_wall_budget_s=topic.wall_budget_s,
            rule_ids=[rule.id for rule in topic.checklist],
            rules=[
                Rule(
                    id=rule.id,
                    description=rule.text,
                    check=rule.check or rule.text,
                    failure=rule.failure,
                )
                for rule in topic.checklist
            ],
        )
        outcome = await self.research(
            ResearchRequest(
                request=AgentRequest(task=task),
                artifact_b64=base64.b64encode(artifact).decode() if artifact is not None else "",
                artifact_uri=submission.artifact_uri if artifact is None else None,
                env=env,
                params=topic.params,
            ),
            wall_seconds=topic.wall_budget_s + 30.0,
        )
        verdict = outcome.run.result
        if not isinstance(verdict, EvaluationVerdict) or (
            verdict.topic_id != topic.id
            or verdict.artifact_digest != submission.artifact_digest
            or verdict.rule_revision != topic.revision
            or verdict.metric != topic.metric.primary
            or set(verdict.rules_checked) != set(task.rule_ids)
        ):
            raise ServiceError(503, "RLM verdict binding mismatch")
        report = next(
            (r for r in outcome.reports if r.report_digest == verdict.report_digest), None
        )
        if report is None:
            raise ServiceError(503, "verdict references unknown measured report")
        execution = next(
            (item for item in outcome.executions if item.execution_id == report.execution_id), None
        )
        if execution is None or (
            execution.report_digest != report.report_digest
            or not execution.dedicated
            or not execution.teardown_confirmed
            or report.network_enabled
        ):
            raise ServiceError(503, "dedicated VM teardown evidence required")
        accepted = verdict.outcome == "accepted"
        metrics = {metric.name: metric.value for metric in report.metrics}
        checks = {
            check.rule_id: check.passed
            for evidence in outcome.reports
            for check in evidence.rule_checks
        }
        if accepted and (
            execution.phase != "experiment"
            or metrics.get(topic.metric.primary) != verdict.value
            or any(checks.get(rule) is not True for rule in task.rule_ids)
        ):
            raise ServiceError(503, "accepted verdict has no isolated measured evidence")
        return EvaluationReport(
            topic_id=topic.id,
            topic_digest=topic.content_digest(),
            submission_id=job_id,
            artifact_digest=submission.artifact_digest,
            verdict="clean" if accepted else "reject",
            reproduced=accepted,
            claim_holds=accepted,
            rule_results={rule: checks.get(rule, False) for rule in task.rule_ids},
            metrics=metrics,
            flops_used=sum(report.flops_used for report in outcome.reports),
            wall_seconds=outcome.wall_seconds,
            evidence_digest=report.report_digest,
            vm_id=execution.vm_id,
            sandboxed=True,
            teardown_confirmed=True,
            rationale=verdict.explanation[:4096],
        )
