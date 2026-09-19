"""Owner-authorized topic setup, sealed only from host-attested VM evidence."""

from __future__ import annotations

import asyncio
import json
import math
import re
from collections.abc import Callable
from typing import Annotated, Literal, Protocol

from fastapi import APIRouter, Request
from pydantic import Field, ValidationError, model_validator

from cortex.errors import ServiceError
from cortex.http import OperatorAuth, bounded_body, decode_json
from cortex.proof.artifacts import check_env
from cortex.proof.models import (
    Baseline,
    Endpoint,
    Metric,
    Rule,
    Slug,
    Topic,
    TopicEvalExecutor,
    digest,
)
from cortex.proof.service import ProofService, Readiness, sign_topic
from cortex.protocol.crypto import public_key
from cortex.rlm import AgentTask, SetupProposal, VmContext, VmResult
from cortex.rlm.models import StrictModel
from cortex.vm.research import ResearchOutcome
from cortex.vm.setup import GENERATED_RUNNER, SetupEvidence


class SetupPolicy(StrictModel):
    """Operator bounds; generated research content cannot loosen these values."""

    topic_id: Slug
    objective: Annotated[str, Field(min_length=1, max_length=8192)]
    metric: Metric | None = None
    custom_id: str | None = Field(default=None, pattern=r"^[a-z0-9][a-z0-9_-]{1,63}$")
    minimum_improvement: float = Field(default=0.000001, gt=0, allow_inf_nan=False)
    flops_budget: int = Field(default=2_000_000_000_000_000_000, gt=0, le=2_000_000_000_000_000_000)
    wall_budget_s: int = Field(default=600, gt=0, le=7200)
    params: dict[str, str] = Field(default_factory=dict, max_length=30)
    eval_executor: TopicEvalExecutor = Field(default_factory=TopicEvalExecutor)
    payout_mode: Literal["wta", "discovery"] = "discovery"
    pass_floor_share_bps: int = Field(default=3000, ge=0, le=10000)
    valid_from_epoch: int = Field(default=0, ge=0, le=2**64 - 1)
    valid_until_epoch: int | None = Field(default=None, ge=0, le=2**64 - 1)

    @model_validator(mode="after")
    def operator_runner(self) -> SetupPolicy:
        runner = self.params.get("baseline_runner") or self.params.get("in_guest_benchmark_runner")
        if runner is not None and not re.fullmatch(r"[A-Za-z0-9_.-]{1,96}", runner):
            raise ValueError("setup requires an operator-registered runner")
        pack = self.params.get("experiment_pack_digest", "")
        if (runner is not None or pack) and not re.fullmatch(r"(?:sha256:)?[0-9a-f]{64}", pack):
            raise ValueError("setup requires a pinned experiment pack")
        if pack and runner is None:
            raise ValueError("a pinned experiment pack requires a runner")
        if (
            "baseline_runner" in self.params
            and "in_guest_benchmark_runner" in self.params
            and self.params["baseline_runner"] != self.params["in_guest_benchmark_runner"]
        ):
            raise ValueError("runner aliases disagree")
        for key, value in self.params.items():
            if not re.fullmatch(r"[a-z0-9][a-z0-9_-]{1,63}", key):
                raise ValueError("invalid operator param name")
            if not value or len(value) > 256 or not value.isprintable():
                raise ValueError("invalid operator param value")
        if self.valid_until_epoch is not None and self.valid_until_epoch < self.valid_from_epoch:
            raise ValueError("invalid topic epoch window")
        return self


class SetupBackend(Protocol):
    async def readiness(self) -> Readiness: ...

    async def run_agent(
        self,
        task: AgentTask,
        *,
        artifact: bytes | None = None,
        env: dict[str, str],
        params: dict[str, str],
    ) -> tuple[str, ResearchOutcome]: ...


class TopicSetup:
    def __init__(
        self,
        service: ProofService,
        backend: SetupBackend,
        *,
        owner_seed: Callable[[], bytes],
    ) -> None:
        self.service = service
        self.backend = backend
        self.owner_seed = owner_seed
        self._intake_lock = asyncio.Lock()
        self._locks: dict[str, asyncio.Lock] = {}
        self._tasks: dict[str, asyncio.Task[Topic]] = {}
        with self.service.store.transaction() as connection:
            connection.execute(
                """CREATE TABLE IF NOT EXISTS proof_setup_jobs (
                    id TEXT PRIMARY KEY, policy TEXT NOT NULL, env_names TEXT NOT NULL,
                    state TEXT NOT NULL, error TEXT
                )"""
            )

    async def create(self, policy: SetupPolicy, *, env: dict[str, str] | None = None) -> Topic:
        """Set up and publish one signed topic after authenticated operator intake."""
        self._signing_seed()
        policy_value = policy.model_dump(mode="json")
        job_id = digest({"setup_policy": policy_value})
        async with self._intake_lock:
            task, completed = self._accept(job_id, policy, env or {})
        if completed is not None:
            return completed
        if task is None:  # pragma: no cover - _accept always resolves one outcome
            raise ServiceError(503, "setup intake failed")
        return await asyncio.shield(task)

    def _accept(
        self, job_id: str, policy: SetupPolicy, supplied_env: dict[str, str]
    ) -> tuple[asyncio.Task[Topic] | None, Topic | None]:
        with self.service.store.transaction() as connection:
            stored = connection.execute(
                "SELECT state,env_names FROM proof_setup_jobs WHERE id=?", (job_id,)
            ).fetchone()
        if stored is not None and stored["state"] == "complete":
            if supplied_env:
                check_env(policy.params, supplied_env)
            existing = self._existing(policy.topic_id, digest(policy.model_dump(mode="json")))
            if existing is None:
                raise ServiceError(503, "completed setup topic unavailable")
            return None, existing

        environment = check_env(policy.params, supplied_env)
        task = self._tasks.get(job_id)
        if task is not None and task.done():
            self._tasks.pop(job_id, None)
            task = None
        if stored is not None and stored["state"] == "pending":
            previous_env = self.service.vault.get(
                job_id, self.service.store.decode_env_names(stored["env_names"])
            )
            if environment != previous_env:
                raise ServiceError(409, "setup retry must use the original environment")
        elif stored is None:
            with self.service.store.transaction() as connection:
                connection.execute(
                    "INSERT INTO proof_setup_jobs VALUES (?,?,?,'pending',NULL)",
                    (job_id, policy.model_dump_json(), json.dumps(sorted(environment))),
                )
            try:
                # The pending row is durable before private credentials are written.
                self.service.vault.put(job_id, environment)
            except BaseException as caught:
                try:
                    self.service.vault.delete(job_id)
                except ServiceError as cleanup:
                    raise cleanup from caught
                with self.service.store.transaction() as connection:
                    connection.execute(
                        "DELETE FROM proof_setup_jobs WHERE id=? AND state='pending'", (job_id,)
                    )
                raise
        elif stored["state"] == "failed":
            self.service.vault.delete(job_id)
            with self.service.store.transaction() as connection:
                cursor = connection.execute(
                    "UPDATE proof_setup_jobs SET env_names=?,state='pending',error=NULL "
                    "WHERE id=? AND state='failed'",
                    (json.dumps(sorted(environment)), job_id),
                )
                if cursor.rowcount != 1:
                    raise ServiceError(503, "setup retry state changed")
            try:
                self.service.vault.put(job_id, environment)
            except BaseException as caught:
                try:
                    self.service.vault.delete(job_id)
                except ServiceError as cleanup:
                    raise cleanup from caught
                with self.service.store.transaction() as connection:
                    connection.execute(
                        "UPDATE proof_setup_jobs SET state='failed',error=? WHERE id=?",
                        ("setup credential vault unavailable", job_id),
                    )
                raise
        elif stored["state"] != "pending":
            raise ServiceError(503, "stored setup state is invalid")
        if task is None:
            task = self._start(job_id, policy)
        return task, None

    def _start(self, job_id: str, policy: SetupPolicy) -> asyncio.Task[Topic]:
        task = asyncio.create_task(self._work(job_id, policy))
        self._tasks[job_id] = task
        task.add_done_callback(lambda completed: self._finished(job_id, completed))
        return task

    def _finished(self, job_id: str, task: asyncio.Task[Topic]) -> None:
        if self._tasks.get(job_id) is task:
            self._tasks.pop(job_id, None)
        if not task.cancelled():
            task.exception()

    async def _work(self, job_id: str, policy: SetupPolicy) -> Topic:
        with self.service.store.transaction() as connection:
            row = connection.execute(
                "SELECT env_names FROM proof_setup_jobs WHERE id=?", (job_id,)
            ).fetchone()
        if row is None:
            raise ServiceError(503, "setup job disappeared")
        try:
            env = self.service.vault.get(
                job_id, self.service.store.decode_env_names(row["env_names"])
            )
            result = await self._create(policy, env=env)
        except asyncio.CancelledError:
            # The pending row and private vault survive shutdown; resume uses
            # the same backend job id and never silently starts a new baseline.
            raise
        except Exception as caught:
            try:
                self.service.vault.delete(job_id)
            except ServiceError as cleanup:
                raise cleanup from caught
            with self.service.store.transaction() as connection:
                cursor = connection.execute(
                    "UPDATE proof_setup_jobs SET state='failed',error=? "
                    "WHERE id=? AND state='pending'",
                    ("topic setup failed; retry the same policy after diagnosis", job_id),
                )
                if cursor.rowcount != 1:
                    raise ServiceError(503, "setup job state changed") from caught
            raise
        self.service.vault.delete(job_id)
        with self.service.store.transaction() as connection:
            cursor = connection.execute(
                "UPDATE proof_setup_jobs SET state='complete' WHERE id=? AND state='pending'",
                (job_id,),
            )
            if cursor.rowcount != 1:
                raise ServiceError(503, "setup job state changed")
        return result

    async def resume(self) -> None:
        self.service.vault.reconcile(self.service.store.active_vault_entries())
        with self.service.store.transaction() as connection:
            rows = connection.execute(
                "SELECT id,policy FROM proof_setup_jobs WHERE state='pending'"
            ).fetchall()
        for row in rows:
            if row["id"] not in self._tasks:
                self._start(row["id"], SetupPolicy.model_validate_json(row["policy"]))

    async def drain(self) -> None:
        if self._tasks:
            await asyncio.gather(*self._tasks.values(), return_exceptions=True)

    def _signing_seed(self) -> bytes:
        # Validate owner identity and BYOK before renting or calling the RLM.
        try:
            seed = self.owner_seed()
            if public_key(seed) != self.service.topic_public_key:
                raise ValueError("owner key mismatch")
        except (OSError, ValueError, TypeError):
            raise ServiceError(503, "topic signing key unavailable or mismatched") from None
        return seed

    async def _create(self, policy: SetupPolicy, *, env: dict[str, str] | None = None) -> Topic:
        seed = self._signing_seed()
        environment = check_env(policy.params, env or {})
        lock = self._locks.setdefault(policy.topic_id, asyncio.Lock())
        async with lock:
            policy_digest = digest(policy.model_dump(mode="json"))
            existing = self._existing(policy.topic_id, policy_digest)
            if existing is not None:
                return existing
            readiness = await self.backend.readiness()
            self._validate_readiness(policy, readiness)
            task = self._task(policy, readiness, policy_digest)
            vm_id, outcome = await self.backend.run_agent(
                task, env=environment, params=policy.params.copy()
            )
            proposal, evidence, baseline = self._validate_outcome(policy, task, outcome)
            holdout = digest(
                {
                    "content_hashes": sorted(set(evidence.content_hashes)),
                    "dataset_ids": sorted(set(evidence.dataset_ids)),
                }
            )
            if holdout != evidence.private_holdout_digest:
                raise ServiceError(503, "setup holdout commitment mismatch")
            public_document = self._public_fields(proposal, evidence)
            metrics = {metric.name: metric.value for metric in baseline.metrics}
            sealed = {
                "topic_id": policy.topic_id,
                "metrics": metrics,
                "script_sha256": evidence.script_sha256,
                "eval_image_digest": readiness.eval_image_digest,
                "flops_budget": policy.flops_budget,
                "wall_budget_s": policy.wall_budget_s,
                "sandboxed": baseline.sandboxed,
                "teardown_confirmed": evidence.teardown_confirmed,
                "dedicated": True,
                "vm_id": vm_id,
                "setup_job_id": task.context.job_id,
                "baseline_execution_id": baseline.execution_id,
                "baseline_report_digest": baseline.report_digest,
                "environment_digest": evidence.environment_digest,
                "holdout_commitment": holdout,
                "setup_policy_digest": policy_digest,
                "run_digest": digest(outcome.run.model_dump(mode="json")),
                "flops_used": baseline.flops_used,
                "wall_seconds": outcome.wall_seconds,
            }
            # Validate the complete document before writing private evidence.
            try:
                metric = (
                    policy.metric.model_copy(update={"epsilon": proposal.floor})
                    if policy.metric
                    else Metric(
                        family="custom",
                        custom_id=self._custom_id(policy, readiness),
                        primary=proposal.metric,
                        direction="max" if proposal.direction == "higher" else "min",
                        epsilon=proposal.floor,
                    )
                )
                topic = Topic(
                    id=policy.topic_id,
                    statement=policy.objective,
                    status="open",
                    payout_mode=policy.payout_mode,
                    pass_floor_share_bps=policy.pass_floor_share_bps,
                    metric=metric,
                    flops_budget=policy.flops_budget,
                    wall_budget_s=policy.wall_budget_s,
                    params=self._published_params(policy, evidence),
                    checklist=public_document["checklist"],
                    documentation=public_document["documentation"],
                    endpoints=public_document["endpoints"],
                    baseline=Baseline(
                        script_sha256=evidence.script_sha256,
                        metrics=metrics,
                        metrics_commitment=digest(metrics),
                        evidence_digest=digest(sealed),
                        flops_budget=policy.flops_budget,
                        wall_budget_s=policy.wall_budget_s,
                    ),
                    holdout_commitment=holdout,
                    eval_image_digest=readiness.eval_image_digest,
                    inference_offer_commitment=readiness.inference_offer_commitment,
                    eval_executor=policy.eval_executor,
                    valid_from_epoch=policy.valid_from_epoch,
                    valid_until_epoch=policy.valid_until_epoch,
                )
            except ValidationError:
                raise ServiceError(
                    503, "generated topic does not match the publication contract"
                ) from None
            self.service.store.register_holdouts(evidence.content_hashes, evidence.dataset_ids)
            self.service.store.register_evidence(policy.topic_id, sealed)
            signed = sign_topic(topic, seed)
            try:
                return await self.service.publish(signed)
            except ServiceError as error:
                if error.status == 409:
                    existing = self._existing(policy.topic_id, policy_digest)
                    if existing is not None:
                        return existing
                raise

    def _existing(self, topic_id: str, policy_digest: str) -> Topic | None:
        topic = self.service.store.topic(topic_id)
        if topic is None:
            return None
        if topic.baseline is not None:
            evidence = self.service.store.evidence(topic.baseline.evidence_digest, topic.id)
            if evidence.get("setup_policy_digest") == policy_digest and topic.status == "open":
                return topic
        raise ServiceError(409, "topic id already exists with a different setup policy or state")

    @staticmethod
    def _published_params(policy: SetupPolicy, evidence: SetupEvidence) -> dict[str, str]:
        params = policy.params.copy()
        if "baseline_runner" not in params and "in_guest_benchmark_runner" not in params:
            params["baseline_runner"] = GENERATED_RUNNER
            params["experiment_pack_digest"] = "sha256:" + evidence.environment_digest
        return params

    @staticmethod
    def _custom_id(policy: SetupPolicy, readiness: Readiness) -> str:
        if policy.custom_id:
            if policy.custom_id not in readiness.custom_ids:
                raise ServiceError(503, "custom runner unavailable")
            return policy.custom_id
        if len(readiness.custom_ids) != 1:
            raise ServiceError(400, "select custom_id when the host has multiple or no runners")
        return next(iter(readiness.custom_ids))

    @staticmethod
    def _validate_readiness(policy: SetupPolicy, readiness: Readiness) -> None:
        if not re.fullmatch(r"sha256:[0-9a-f]{64}", readiness.eval_image_digest):
            raise ServiceError(503, "setup image is not pinned")
        if not re.fullmatch(r"[0-9a-f]{64}", readiness.inference_offer_commitment):
            raise ServiceError(503, "setup inference offer is not pinned")
        if policy.metric is None:
            TopicSetup._custom_id(policy, readiness)
        elif policy.metric.family == "custom":
            if policy.metric.custom_id not in readiness.custom_ids:
                raise ServiceError(503, "custom runner unavailable")
        elif not readiness.live_harvest_wired:
            raise ServiceError(503, "harvest executor unavailable")

    @staticmethod
    def _task(policy: SetupPolicy, readiness: Readiness, policy_digest: str) -> AgentTask:
        context = VmContext(
            topic_id=policy.topic_id,
            job_id=digest(
                {"purpose": "setup", "policy": policy_digest, "image": readiness.eval_image_digest}
            ),
            purpose="setup",
            image_digest=readiness.eval_image_digest.removeprefix("sha256:"),
        )
        objective = json.dumps(
            {
                "owner_policy": policy.model_dump(mode="json"),
                "contract": (
                    "Prepare the owner objective. Use an explicitly selected operator runner "
                    "and pack, or generate run.py, inspect.py and dependencies in PROOF_SETUP_DIR. "
                    "Run the baseline as a dedicated experiment. Publish a nonempty private "
                    "holdout manifest only through attested setup evidence, never public text. "
                    "Proposal floor is the required baseline improvement epsilon in the owner's "
                    "metric units (or relative fraction); it may only increase. Keep the metric "
                    "name and direction unchanged when provided. If metric is null, define "
                    "a reproducible measured metric and direction suited to the objective; "
                    "floor must be at least minimum_improvement. Generate rules, "
                    "miner documentation and "
                    "declarative endpoint schemas using documentation/submission/results purposes. "
                    "No process health check or inferred metric is a baseline."
                ),
            },
            separators=(",", ":"),
        )
        try:
            return AgentTask(
                context=context,
                objective=objective,
                metric=policy.metric.primary if policy.metric else None,
                wall_budget_s=policy.wall_budget_s,
            )
        except ValidationError:
            raise ServiceError(
                400, "setup objective and policy exceed agent context bounds"
            ) from None

    @staticmethod
    def _validate_outcome(
        policy: SetupPolicy, task: AgentTask, outcome: ResearchOutcome
    ) -> tuple[SetupProposal, SetupEvidence, VmResult]:
        proposal = outcome.run.result
        if not isinstance(proposal, SetupProposal) or proposal.topic_id != policy.topic_id:
            raise ServiceError(503, "setup agent returned a mismatched proposal")
        try:
            evidence = SetupEvidence.model_validate(
                outcome.model_dump(mode="json").get("setup_evidence")
            )
        except ValidationError:
            raise ServiceError(503, "attested private setup evidence required") from None
        if not math.isfinite(outcome.wall_seconds) or outcome.wall_seconds < 0:
            raise ServiceError(503, "invalid setup duration")
        if len({report.report_digest for report in outcome.reports}) != len(outcome.reports):
            raise ServiceError(503, "duplicate setup report digest")
        reports = {report.report_digest: report for report in outcome.reports}
        setup = reports.get(proposal.setup_report_digest)
        baseline = reports.get(proposal.baseline_report_digest)
        if setup is None or baseline is None:
            raise ServiceError(503, "setup proposal references unattested reports")
        for report in outcome.reports:
            if (
                report.topic_id != policy.topic_id
                or report.job_id != task.context.job_id
                or report.image_digest != task.context.image_digest
                or report.artifact_digest is not None
                or report.exit_code != 0
                or report.sandboxed is not True
            ):
                raise ServiceError(503, "setup report binding or execution failed")
        if (
            baseline.network_enabled
            or evidence.setup_report_digest != setup.report_digest
            or evidence.baseline_report_digest != baseline.report_digest
            or evidence.environment_digest != proposal.environment_digest
            or evidence.private_holdout_digest != proposal.private_holdout_digest
            or evidence.flops_budget != policy.flops_budget
            or evidence.wall_budget_s != policy.wall_budget_s
            or baseline.flops_used > policy.flops_budget
            or evidence.teardown_confirmed is not True
        ):
            raise ServiceError(503, "baseline attestation does not match operator policy")
        execution = next(
            (item for item in outcome.executions if item.execution_id == baseline.execution_id),
            None,
        )
        if (
            execution is None
            or execution.report_digest != baseline.report_digest
            or execution.phase != "experiment"
            or not execution.dedicated
            or not execution.teardown_confirmed
            or execution.environment_digest != evidence.environment_digest
        ):
            raise ServiceError(503, "baseline lacks dedicated VM teardown evidence")
        if not evidence.content_hashes and not evidence.dataset_ids:
            raise ServiceError(503, "setup produced no private holdout evidence")
        if policy.metric and (
            proposal.metric != policy.metric.primary
            or proposal.direction != ("higher" if policy.metric.direction == "max" else "lower")
            or proposal.floor < policy.metric.epsilon
        ):
            raise ServiceError(503, "generated metric loosens or changes operator policy")
        if proposal.floor < policy.minimum_improvement:
            raise ServiceError(503, "generated metric loosens minimum improvement")
        if proposal.metric not in {metric.name for metric in baseline.metrics}:
            raise ServiceError(503, "baseline did not measure the operator metric")
        produced = {
            (artifact.kind, artifact.digest)
            for report in (setup, baseline)
            for artifact in report.produced_artifacts
        }
        if ("environment", proposal.environment_digest) not in produced or (
            "private_holdout",
            proposal.private_holdout_digest,
        ) not in produced:
            raise ServiceError(503, "setup artifacts lack execution evidence")
        return proposal, evidence, baseline

    @staticmethod
    def _public_fields(proposal: SetupProposal, evidence: SetupEvidence) -> dict:
        # Generated routes remain topic-local declarations handled by the fixed
        # intake, documentation and results surfaces; never executable CP routes.
        try:
            checklist = [
                Rule.model_validate(
                    {
                        "id": rule.id,
                        "text": rule.description,
                        "check": rule.check,
                        "failure": rule.failure,
                    }
                )
                for rule in proposal.rules
            ]
            endpoints = [
                Endpoint.model_validate(
                    {
                        "path": endpoint.suffix,
                        "method": endpoint.method,
                        "purpose": endpoint.model_dump().get("purpose"),
                        "description": endpoint.description,
                        "request_schema": endpoint.request_schema,
                        "response_schema": endpoint.response_schema,
                    }
                )
                for endpoint in proposal.endpoints
            ]
        except ValidationError:
            raise ServiceError(503, "generated rules or endpoints are not publishable") from None
        if not any(endpoint.purpose == "submission" for endpoint in endpoints):
            raise ServiceError(503, "generated topic has no submission endpoint")
        documentation = f"# {proposal.title}\n\n{proposal.instructions}"
        public = {
            "checklist": [rule.model_dump(mode="json") for rule in checklist],
            "endpoints": [endpoint.model_dump(mode="json") for endpoint in endpoints],
            "documentation": documentation,
        }
        serialized = json.dumps(public, ensure_ascii=False)
        if any(
            secret in serialized for secret in (*evidence.content_hashes, *evidence.dataset_ids)
        ):
            raise ServiceError(503, "generated public content contains private holdout identifiers")
        return {"checklist": checklist, "endpoints": endpoints, "documentation": documentation}


class SetupRequest(StrictModel):
    policy: SetupPolicy
    env: dict[str, str] = Field(default_factory=dict, max_length=8, repr=False)


def create_setup_router(setup: TopicSetup, operator: OperatorAuth) -> APIRouter:
    router = APIRouter()

    @router.post("/v1/admin/proof/setup", status_code=201)
    async def create_topic(request: Request):
        operator.require(request)
        try:
            body = SetupRequest.model_validate(decode_json(await bounded_body(request, 128 * 1024)))
        except ValidationError:
            raise ServiceError(400, "invalid topic setup policy or environment") from None
        return (await setup.create(body.policy, env=body.env)).model_dump(mode="json")

    return router
