"""Run the RLM loop inside its topic VM; broker only inference and isolated tools."""

from __future__ import annotations

import asyncio
import hashlib
import os
import time
from collections.abc import Callable
from pathlib import Path
from typing import Literal

from pydantic import Field

from cortex.rlm import (
    AgentLimits,
    AgentRequest,
    AgentRun,
    EvaluationVerdict,
    InferenceBroker,
    InferenceRequest,
    SetupProposal,
)
from cortex.rlm.knowledge import KnowledgeStore
from cortex.rlm.memory_proxy import SharedKnowledgeBroker
from cortex.rlm.models import Digest, StrictModel, VmAction, VmContext, VmResult, canonical_bytes
from cortex.rlm.provider import ModelProvider

from .models import API_VERSION, ExecuteRequest, Identifier, VmError
from .runtime import Orchestrator
from .setup import GENERATED_RUNNER, SetupEvidence
from .transport import VsockTransport, read_frame, write_frame


class ResearchRequest(StrictModel):
    request: AgentRequest
    artifact_b64: str = Field(default="", repr=False, max_length=90_000_000)
    artifact_uri: str | None = Field(default=None, max_length=2048)
    env: dict[str, str] = Field(default_factory=dict, repr=False, max_length=8)
    params: dict[str, str] = Field(default_factory=dict, max_length=32)


class ToolCallback(StrictModel):
    execution_id: Identifier
    context: VmContext
    action: VmAction


class ExecutionEvidence(StrictModel):
    execution_id: Identifier
    vm_id: Identifier
    report_digest: Digest
    phase: Literal["inspect", "setup", "preflight", "experiment"]
    dedicated: bool
    teardown_confirmed: bool
    environment_digest: Digest | None = None


class ResearchOutcome(StrictModel):
    run: AgentRun
    reports: list[VmResult]
    wall_seconds: float
    executions: list[ExecutionEvidence] = Field(default_factory=list)
    setup_evidence: SetupEvidence | None = Field(default=None, repr=False)


class ResearchOperation(StrictModel):
    action: VmAction
    params: dict[str, str]


class ResearchProgress(StrictModel):
    started_at: float = Field(gt=0, allow_inf_nan=False)
    deadline: float = Field(gt=0, allow_inf_nan=False)
    calls: int = Field(default=0, ge=0)
    tokens: int = Field(default=0, ge=0)
    remaining_tools: int = Field(ge=0)
    checks: dict[str, bool] = Field(default_factory=dict)
    reports: dict[str, tuple[str, VmResult]] = Field(default_factory=dict)
    executions: list[ExecutionEvidence] = Field(default_factory=list)
    setup_evidence: SetupEvidence | None = Field(default=None, repr=False)
    pending_execution: Identifier | None = None
    operations: dict[str, ResearchOperation] = Field(default_factory=dict)


class CheckpointProvider:
    """Persist the broker's reservation before a provider request can spend it."""

    def __init__(self, provider: ModelProvider, checkpoint: Callable[[], None]):
        self.provider = provider
        self.model = provider.model
        self.checkpoint = checkpoint

    async def complete(self, **kwargs):
        self.checkpoint()
        return await self.provider.complete(**kwargs)


def _callback_socket_exists(path: Path) -> bool:
    return os.path.lexists(path)


class ResearchHost:
    def __init__(
        self,
        orchestrator: Orchestrator,
        provider: ModelProvider,
        socket_for,
        *,
        limits: AgentLimits | None = None,
        transport: VsockTransport | None = None,
        knowledge: KnowledgeStore | None = None,
    ):
        self.orchestrator, self.provider, self.socket_for = orchestrator, provider, socket_for
        self.limits = limits or AgentLimits()
        self.transport = transport or VsockTransport()
        self.knowledge = knowledge
        self._tasks: dict[str, asyncio.Task] = {}
        self._busy: set[str] = set()
        self.orchestrator.db.execute("""CREATE TABLE IF NOT EXISTS research_jobs (
            job_id TEXT PRIMARY KEY, commitment TEXT NOT NULL, vm_id TEXT NOT NULL,
            state TEXT NOT NULL, result TEXT
        )""")
        self.orchestrator.db.execute("""CREATE TABLE IF NOT EXISTS research_progress (
            job_id TEXT PRIMARY KEY, progress TEXT NOT NULL
        )""")

    async def run(self, vm_id: str, request: ResearchRequest) -> ResearchOutcome:
        self._validate_limits(request.request.task.wall_budget_s)
        limits = self._job_limits(request)
        record = self.orchestrator.get(vm_id)
        context = request.request.task.context
        if (
            record.spec.topic_id != context.topic_id
            or record.spec.image_digest != context.image_digest
            or record.spec.kind != "topic"
            or record.state != "running"
        ):
            raise VmError("topic_mismatch", 409)
        if context.purpose == "evaluate" and not request.artifact_b64 and request.artifact_uri:
            fetched = await self.transport.exchange(
                self.socket_for(vm_id),
                {
                    "api_version": API_VERSION,
                    "type": "artifact_fetch",
                    "context": context.model_dump(mode="json"),
                    "uri": request.artifact_uri,
                },
                75.0,
            )
            encoded = fetched.get("artifact_b64")
            if not isinstance(encoded, str):
                raise VmError("guest artifact fetch failed")
            request = ResearchRequest.model_validate(
                {
                    **request.model_dump(),
                    "artifact_b64": encoded,
                    "artifact_uri": None,
                }
            )
        # Validate the eventual experiment envelope; this request is never dispatched.
        validation = ExecuteRequest(
            execution_id="validate-request",
            context=context,
            action=VmAction(operation="run", argv=["validate-request"], phase="experiment"),
            artifact_b64=request.artifact_b64,
            env=request.env,
            params=request.params,
        )
        self.orchestrator._validate_job(validation)
        job_id = context.job_id
        commitment = hashlib.sha256(
            canonical_bytes(
                {
                    "request": request.model_dump(mode="json", exclude={"request": {"resume"}}),
                    "model": self.provider.model,
                    "limits": limits.model_dump(mode="json"),
                }
            )
        ).hexdigest()
        row = self.orchestrator.db.execute(
            "SELECT * FROM research_jobs WHERE job_id=?", (job_id,)
        ).fetchone()
        if row:
            if row["commitment"] != commitment or row["vm_id"] != vm_id:
                raise VmError("research job id reused", 409)
            if row["state"] == "succeeded":
                return ResearchOutcome.model_validate_json(row["result"])
            if job_id in self._tasks:
                return await asyncio.shield(self._tasks[job_id])
            if not request.request.resume:
                raise VmError("research_resume_required", 409)
            saved = self.orchestrator.db.execute(
                "SELECT progress FROM research_progress WHERE job_id=?", (job_id,)
            ).fetchone()
            if saved is None:
                raise VmError("research progress missing; explicit host recovery required")
            try:
                progress = ResearchProgress.model_validate_json(saved["progress"])
            except ValueError:
                raise VmError(
                    "research progress invalid; explicit host recovery required"
                ) from None
            if (
                progress.pending_execution is not None
                and progress.pending_execution not in progress.operations
            ):
                raise VmError("interrupted VM operation requires orchestrator reconciliation")
            if progress.deadline <= time.time():
                raise VmError("research wall-clock budget exhausted")
        else:
            if request.request.resume:
                raise VmError("research resume requires an existing job", 409)
            started_at = time.time()
            progress = ResearchProgress(
                started_at=started_at,
                deadline=started_at + limits.wall_seconds,
                remaining_tools=limits.max_tool_calls,
            )
        if vm_id in self._busy:
            raise VmError("topic agent busy", 409)
        if row is None:
            self.orchestrator.db.execute(
                "INSERT INTO research_jobs VALUES (?,?,?,'accepted',NULL)",
                (job_id, commitment, vm_id),
            )
            self._save_progress(job_id, progress)
        self._busy.add(vm_id)
        task = asyncio.create_task(self._run(vm_id, request, progress))
        task.add_done_callback(self.orchestrator._harvest)
        task.add_done_callback(lambda completed: self._tasks.pop(job_id, None))
        self._tasks[job_id] = task
        return await asyncio.shield(self._tasks[job_id])

    def _save_progress(self, job_id: str, progress: ResearchProgress) -> None:
        self.orchestrator.db.execute(
            "INSERT INTO research_progress VALUES (?,?) "
            "ON CONFLICT(job_id) DO UPDATE SET progress=excluded.progress",
            (job_id, progress.model_dump_json()),
        )

    async def _run(
        self, vm_id: str, request: ResearchRequest, progress: ResearchProgress
    ) -> ResearchOutcome:
        context = request.request.task.context
        limits = self._job_limits(request)
        remaining = min(limits.wall_seconds, progress.deadline - time.time())
        started = time.monotonic() - (limits.wall_seconds - remaining)
        memory = SharedKnowledgeBroker(self.knowledge, context) if self.knowledge else None
        socket_path = Path(str(self.socket_for(vm_id)) + "_5001")
        # The jail root itself is uid-owned and 0700; only that guest can connect here.
        connections: set[asyncio.Task] = set()
        remaining_tools = progress.remaining_tools
        checks = progress.checks
        reports = progress.reports
        executions = progress.executions
        setup_evidence = progress.setup_evidence
        pending_execution = progress.pending_execution
        operations = progress.operations

        def checkpoint():
            self._save_progress(
                context.job_id,
                progress.model_copy(
                    update={
                        "calls": broker.calls,
                        "tokens": broker.tokens,
                        "remaining_tools": remaining_tools,
                        "checks": checks,
                        "reports": reports,
                        "executions": executions,
                        "setup_evidence": setup_evidence,
                        "pending_execution": pending_execution,
                        "operations": operations,
                    }
                ),
            )

        broker = InferenceBroker(CheckpointProvider(self.provider, checkpoint), limits)
        broker.calls, broker.tokens = progress.calls, progress.tokens
        broker.deadline = started + limits.wall_seconds
        if any(not passed for passed in checks.values()):
            broker.block()
        callback_lock = asyncio.Lock()
        callback_open = True

        async def callback(reader, writer):
            nonlocal remaining_tools, setup_evidence, pending_execution
            task = asyncio.current_task()
            if task is not None:
                connections.add(task)
            try:
                async with (
                    asyncio.timeout(limits.wall_seconds - (time.monotonic() - started)),
                    callback_lock,
                ):
                    message = await read_frame(reader)
                    if not callback_open:
                        raise VmError("research callback session closed")
                    if message.get("type") == "inference":
                        if pending_execution is not None:
                            raise VmError("pending VM operation requires reconciliation")
                        if context.purpose == "evaluate" and any(
                            checks.get(rule) is not True for rule in request.request.task.rule_ids
                        ):
                            raise VmError("all published checks must pass before inference")
                        completion = await broker.complete(
                            InferenceRequest.model_validate(message["request"])
                        )
                        checkpoint()
                        response = {"completion": completion.model_dump(mode="json")}
                    elif message.get("type") in {"execute", "reconcile"}:
                        tool = ToolCallback.model_validate(message["request"])
                        if tool.context != context:
                            raise VmError("guest callback binding mismatch")
                        reconcile = message["type"] == "reconcile"
                        if reconcile:
                            recorded = operations.get(tool.execution_id)
                            if (
                                recorded is None
                                or recorded.action != tool.action
                                or pending_execution not in {None, tool.execution_id}
                            ):
                                raise VmError("unknown or changed VM operation")
                            params = recorded.params
                        else:
                            if (
                                pending_execution is not None
                                or tool.execution_id in operations
                                or remaining_tools <= 0
                            ):
                                raise VmError("pending operation or exhausted tool budget")
                            self._validate_action_limits(request, tool.action, started)
                            if context.purpose == "evaluate" and tool.action.phase == "experiment":
                                if any(
                                    checks.get(rule) is not True
                                    for rule in request.request.task.rule_ids
                                ):
                                    raise VmError("preflight rules must pass before experiment")
                            params = request.params
                            if context.purpose == "setup" and setup_evidence is not None:
                                params = {
                                    **params,
                                    "baseline_runner": GENERATED_RUNNER,
                                    "experiment_pack_digest": setup_evidence.environment_digest,
                                }
                            if context.purpose == "setup" and tool.action.phase == "experiment":
                                if setup_evidence is None:
                                    raise VmError("baseline requires exported setup evidence")
                                self._validate_setup_limits(request, setup_evidence)
                        job = ExecuteRequest(
                            **tool.model_dump(),
                            artifact_b64=request.artifact_b64,
                            env=request.env if tool.action.phase == "experiment" else {},
                            params=params,
                        )
                        if reconcile:
                            result = await self.orchestrator.reconcile(vm_id, job)
                        else:
                            remaining_tools -= 1
                            operations[job.execution_id] = ResearchOperation(
                                action=job.action, params=job.params
                            )
                            pending_execution = job.execution_id
                            checkpoint()
                            result = await self.orchestrator.execute(vm_id, job)
                        exported = self.orchestrator.setup_evidence(job.execution_id)
                        if exported is not None:
                            self._validate_setup_limits(request, exported)
                            setup_evidence = exported
                        row = self.orchestrator.db.execute(
                            "SELECT vm_id FROM jobs WHERE execution_id=?", (job.execution_id,)
                        ).fetchone()
                        executed_vm = self.orchestrator.get(row[0])
                        evidence = ExecutionEvidence(
                            execution_id=job.execution_id,
                            vm_id=executed_vm.vm_id,
                            report_digest=result.report_digest,
                            phase=job.action.phase,
                            dedicated=job.dedicated,
                            teardown_confirmed=executed_vm.state == "destroyed",
                            environment_digest=job.params.get(
                                "experiment_pack_digest", ""
                            ).removeprefix("sha256:")
                            or None,
                        )
                        existing = next(
                            (item for item in executions if item.execution_id == job.execution_id),
                            None,
                        )
                        if existing is not None and existing != evidence:
                            raise VmError("reconciled execution evidence changed")
                        if existing is None:
                            executions.append(evidence)
                        previous = reports.get(result.report_digest)
                        if previous is not None and previous != (tool.action.phase, result):
                            raise VmError("VM report digest reused for different evidence")
                        reports[result.report_digest] = (tool.action.phase, result)
                        if tool.action.phase == "preflight":
                            for check in result.rule_checks:
                                checks[check.rule_id] = (
                                    checks.get(check.rule_id, True) and check.passed
                                )
                            if any(not check.passed for check in result.rule_checks):
                                broker.block()
                        pending_execution = None
                        checkpoint()
                        response = {"result": result.model_dump(mode="json")}
                    elif message.get("type") in {"knowledge_read", "knowledge_propose"}:
                        if memory is None:
                            raise VmError("shared knowledge is unavailable")
                        response = memory.handle(message, set(reports))
                    else:
                        raise VmError("unknown guest callback")
                    await write_frame(writer, response)
            except Exception:
                await write_frame(writer, {"error": "host callback refused"})
            finally:
                writer.close()
                await writer.wait_closed()
                if task is not None:
                    connections.discard(task)

        server = None
        try:
            if remaining <= 0:
                raise VmError("research wall-clock budget exhausted")
            if _callback_socket_exists(socket_path):
                raise VmError("stale guest callback socket requires recovery")
            server = await asyncio.start_unix_server(callback, path=str(socket_path))
            os.chmod(socket_path, 0o666)
            response = await self.transport.exchange(
                self.socket_for(vm_id),
                {
                    "api_version": API_VERSION,
                    "type": "agent",
                    "request": request.request.model_dump(mode="json"),
                    "model": self.provider.model,
                    "limits": limits.model_dump(mode="json"),
                    "shared_knowledge": self.knowledge is not None,
                },
                max(0, remaining) + 30,
            )
            callback_open = False
            result = AgentRun.model_validate(response["output"])
            if pending_execution is not None:
                raise VmError("pending VM operation requires reconciliation before completion")
            self._verify_result(request, result, reports, checks)
            if isinstance(result.result, SetupProposal):
                proposal = result.result
                if (
                    setup_evidence is None
                    or proposal.environment_digest != setup_evidence.environment_digest
                    or proposal.private_holdout_digest != setup_evidence.private_holdout_digest
                    or proposal.setup_report_digest != setup_evidence.setup_report_digest
                ):
                    raise VmError("setup proposal does not bind its exported evidence")
                baseline = reports[proposal.baseline_report_digest][1]
                baseline_execution = next(
                    (item for item in executions if item.report_digest == baseline.report_digest),
                    None,
                )
                if (
                    baseline_execution is None
                    or not baseline_execution.dedicated
                    or not baseline_execution.teardown_confirmed
                    or baseline_execution.environment_digest != setup_evidence.environment_digest
                    or baseline.flops_used > setup_evidence.flops_budget
                    or proposal.metric not in {item.name for item in baseline.metrics}
                ):
                    raise VmError("baseline lacks completed experiment evidence within budget")
                setup_evidence = setup_evidence.model_copy(
                    update={
                        "baseline_report_digest": baseline.report_digest,
                        "teardown_confirmed": True,
                    }
                )
            outcome = ResearchOutcome(
                run=result,
                reports=[report for phase, report in reports.values()],
                wall_seconds=max(0, time.time() - progress.started_at),
                executions=executions,
                setup_evidence=setup_evidence,
            )
            self.orchestrator.db.execute(
                "UPDATE research_jobs SET state='succeeded',result=? WHERE job_id=?",
                (outcome.model_dump_json(), context.job_id),
            )
            return outcome
        except Exception as exc:
            self.orchestrator.db.execute(
                "UPDATE research_jobs SET state='failed' WHERE job_id=?", (context.job_id,)
            )
            reason = exc.reason if isinstance(exc, VmError) else "guest protocol failure"
            if pending_execution is not None:
                reason = "interrupted VM operation requires orchestrator reconciliation"
            raise VmError(f"topic RLM execution failed: {reason}") from None
        finally:
            callback_open = False
            broker.block()
            if server is not None:
                server.close()
            closing = tuple(connections)
            for task in closing:
                task.cancel()
            if closing:
                await asyncio.gather(*closing, return_exceptions=True)
            if server is not None:
                await server.wait_closed()
                try:
                    os.unlink(socket_path)
                except FileNotFoundError:
                    pass
            self._busy.discard(vm_id)

    @staticmethod
    def _verify_result(request, run, reports, checks):
        task = request.request.task
        result = run.result
        if result.topic_id != task.context.topic_id:
            raise VmError("agent output topic mismatch")
        if isinstance(result, EvaluationVerdict):
            if (
                result.artifact_digest != task.context.artifact_digest
                or result.rule_revision != task.rule_revision
                or result.metric != task.metric
                or set(result.rules_checked) != set(task.rule_ids)
            ):
                raise VmError("agent verdict binding mismatch")
            phase, report = reports.get(result.report_digest, (None, None))
            if report is None:
                raise VmError("agent report was not produced by this host job")
            if result.outcome == "accepted":
                values = {metric.name: metric.value for metric in report.metrics}
                if (
                    phase != "experiment"
                    or values.get(result.metric) != result.value
                    or any(checks.get(rule) is not True for rule in task.rule_ids)
                ):
                    raise VmError("agent accepted without measured evidence")
            elif result.value is not None:
                raise VmError("rejected verdict must not claim a measured value")
        elif isinstance(result, SetupProposal):
            for digest in (result.setup_report_digest, result.baseline_report_digest):
                if digest not in reports:
                    raise VmError("setup refers to unattested report")
            if reports[result.baseline_report_digest][0] != "experiment":
                raise VmError("baseline requires a dedicated experiment")

    async def close(self) -> None:
        await asyncio.gather(*self._tasks.values(), return_exceptions=True)

    def is_busy(self, vm_id: str) -> bool:
        return vm_id in self._busy

    def _validate_limits(self, wall_budget_s: int | None) -> None:
        if wall_budget_s is not None and (
            wall_budget_s > self.limits.tool_timeout_seconds
            or wall_budget_s > self.limits.wall_seconds
        ):
            raise VmError("topic wall budget exceeds configured research or tool ceiling")

    def _validate_action_limits(
        self, request: ResearchRequest, action: VmAction, started: float
    ) -> None:
        if action.timeout_seconds > self.limits.tool_timeout_seconds:
            raise VmError("requested VM timeout exceeds configured tool ceiling")
        topic_wall = request.request.task.wall_budget_s
        if topic_wall is not None and action.timeout_seconds > topic_wall:
            raise VmError("requested VM timeout exceeds topic wall budget")
        limits = self._job_limits(request)
        if action.timeout_seconds > limits.wall_seconds - (time.monotonic() - started):
            raise VmError("requested VM timeout cannot fit remaining research budget")

    def _job_limits(self, request: ResearchRequest) -> AgentLimits:
        wall = request.request.task.research_wall_budget_s
        if wall is None:
            return self.limits
        if wall > self.limits.wall_seconds:
            raise VmError("requested research wall budget exceeds host ceiling")
        return AgentLimits.model_validate({**self.limits.model_dump(), "wall_seconds": float(wall)})

    def _validate_setup_limits(self, request: ResearchRequest, evidence: SetupEvidence) -> None:
        self._validate_limits(evidence.wall_budget_s)
        declared = request.request.task.wall_budget_s
        if declared is not None and evidence.wall_budget_s != declared:
            raise VmError("setup manifest wall budget does not match owner policy")
