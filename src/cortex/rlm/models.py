"""Strict wire contracts for the research agent and its VM boundary.

An agent produces proposals. Only the owner can sign and publish their rules;
only an attested VM report can supply a measured evaluation result.
"""

from __future__ import annotations

import hashlib
import json
import re
from typing import Annotated, Literal, Protocol, runtime_checkable

from pydantic import BaseModel, ConfigDict, Field, field_validator, model_validator

Digest = Annotated[str, Field(pattern=r"^[0-9a-f]{64}$")]
Identifier = Annotated[str, Field(pattern=r"^[a-zA-Z0-9][a-zA-Z0-9_.-]{0,95}$")]
Text = Annotated[str, Field(min_length=1, max_length=16_384)]


class StrictModel(BaseModel):
    model_config = ConfigDict(extra="forbid", strict=True, frozen=True)


def canonical_bytes(value: BaseModel | dict[str, object]) -> bytes:
    data = value.model_dump(mode="json") if isinstance(value, BaseModel) else value
    return json.dumps(data, sort_keys=True, separators=(",", ":"), allow_nan=False).encode()


def digest_of(value: BaseModel | dict[str, object]) -> str:
    return hashlib.sha256(canonical_bytes(value)).hexdigest()


class AgentLimits(StrictModel):
    max_calls: int = Field(default=16, ge=1, le=128)
    max_tool_calls: int = Field(default=48, ge=1, le=512)
    max_tokens: int = Field(default=262_144, ge=256, le=2_000_000)
    max_depth: int = Field(default=3, ge=0, le=8)
    wall_seconds: float = Field(default=300.0, gt=0, le=7200)
    tool_timeout_seconds: float = Field(default=60.0, gt=0, le=7200)
    completion_tokens: int = Field(default=4096, ge=64, le=32_768)
    max_response_bytes: int = Field(default=262_144, ge=1024, le=1_048_576)
    context_bytes: int = Field(default=65_536, ge=8192, le=262_144)
    compact_keep_exchanges: int = Field(default=2, ge=0, le=16)


class VmContext(StrictModel):
    topic_id: Identifier
    job_id: Identifier
    purpose: Literal["setup", "evaluate"]
    image_digest: Digest
    artifact_digest: Digest | None = None

    @model_validator(mode="after")
    def evaluation_has_artifact(self) -> VmContext:
        if self.purpose == "evaluate" and self.artifact_digest is None:
            raise ValueError("evaluation requires an artifact digest")
        return self


class VmAction(StrictModel):
    operation: Literal["run", "read_file"]
    phase: Literal["inspect", "setup", "preflight", "experiment"] = "inspect"
    argv: list[Annotated[str, Field(min_length=1, max_length=4096)]] = Field(
        default_factory=list, max_length=64
    )
    path: str | None = Field(default=None, max_length=1024)
    timeout_seconds: int = Field(default=30, ge=1, le=7200)

    @model_validator(mode="after")
    def validate_operation(self) -> VmAction:
        if self.operation == "run":
            if not self.argv or self.path is not None:
                raise ValueError("run requires argv and forbids path")
        elif self.argv or self.path is None:
            raise ValueError("read_file requires path and forbids argv")
        if self.path is not None:
            if not self.path.startswith("/workspace/") or ".." in self.path.split("/"):
                raise ValueError("read_file is restricted to /workspace")
            if "\x00" in self.path:
                raise ValueError("invalid path")
        return self


class Metric(StrictModel):
    name: Identifier
    value: float = Field(allow_inf_nan=False)


class RuleCheck(StrictModel):
    rule_id: Identifier
    passed: bool


class ProducedArtifact(StrictModel):
    kind: Literal["environment", "private_holdout"]
    digest: Digest


class VmResult(StrictModel):
    """Trusted adapter output, never accepted from a model or a miner.

    The orchestrator adapter verifies its remote attestation and VM/job binding
    before constructing this object. Evaluate runs must use a networkless
    dedicated guest and be destroyed before this result is released.
    """

    topic_id: Identifier
    job_id: Identifier
    image_digest: Digest
    artifact_digest: Digest | None = None
    sandboxed: Literal[True]
    network_enabled: bool
    execution_id: Identifier
    report_digest: Digest
    exit_code: int
    stdout_tail: str = Field(default="", max_length=16_384)
    metrics: list[Metric] = Field(default_factory=list, max_length=64)
    rule_checks: list[RuleCheck] = Field(default_factory=list, max_length=64)
    produced_artifacts: list[ProducedArtifact] = Field(default_factory=list, max_length=16)
    flops_used: int = Field(default=0, ge=0)

    @field_validator("metrics")
    @classmethod
    def unique_metrics(cls, metrics: list[Metric]) -> list[Metric]:
        if len({metric.name for metric in metrics}) != len(metrics):
            raise ValueError("duplicate metric")
        return metrics

    @field_validator("rule_checks")
    @classmethod
    def unique_rule_checks(cls, checks: list[RuleCheck]) -> list[RuleCheck]:
        if len({check.rule_id for check in checks}) != len(checks):
            raise ValueError("duplicate rule check")
        return checks


class VmExecutor(Protocol):
    async def execute(self, context: VmContext, action: VmAction) -> VmResult:
        """Execute only in an authenticated VM bound to context; never on the CP."""
        ...


@runtime_checkable
class ReconciliableVmExecutor(VmExecutor, Protocol):
    async def execute_once(
        self, context: VmContext, action: VmAction, execution_id: str
    ) -> VmResult:
        """Execute using the identity already written to the agent journal."""
        ...

    async def reconcile(self, context: VmContext, action: VmAction, execution_id: str) -> VmResult:
        """Read an exact completed result without starting or retrying a VM job."""
        ...


class Rule(StrictModel):
    id: Identifier
    description: Text
    check: Text
    failure: Literal["reject", "zero"] = "reject"


class Endpoint(StrictModel):
    method: Literal["GET", "POST"]
    purpose: Literal["documentation", "submission", "results"] = "submission"
    suffix: Annotated[str, Field(pattern=r"^/[a-z][a-z0-9/-]{0,127}$")]
    description: Text
    request_schema: dict[str, object]
    response_schema: dict[str, object]

    @field_validator("suffix")
    @classmethod
    def normalized_suffix(cls, value: str) -> str:
        if "//" in value or value.endswith("/"):
            raise ValueError("endpoint suffix must be normalized")
        return value

    @model_validator(mode="after")
    def method_matches_purpose(self) -> Endpoint:
        expected = "POST" if self.purpose == "submission" else "GET"
        if self.method != expected:
            raise ValueError("endpoint method does not match its purpose")
        return self


class SetupProposal(StrictModel):
    """Unsigned setup candidate. This is never itself an open topic."""

    topic_id: Identifier
    title: Annotated[str, Field(min_length=1, max_length=160)]
    instructions: Text
    rules: list[Rule] = Field(min_length=1, max_length=64)
    endpoints: list[Endpoint] = Field(min_length=1, max_length=16)
    metric: Identifier
    direction: Literal["higher", "lower"]
    floor: float = Field(allow_inf_nan=False)
    setup_report_digest: Digest
    baseline_report_digest: Digest
    private_holdout_digest: Digest
    environment_digest: Digest

    @model_validator(mode="after")
    def unique_rules_and_routes(self) -> SetupProposal:
        if len({rule.id for rule in self.rules}) != len(self.rules):
            raise ValueError("duplicate rule")
        routes = {(endpoint.method, endpoint.suffix) for endpoint in self.endpoints}
        if len(routes) != len(self.endpoints):
            raise ValueError("duplicate endpoint")
        return self


class EvaluationVerdict(StrictModel):
    topic_id: Identifier
    artifact_digest: Digest
    rule_revision: int = Field(ge=1)
    outcome: Literal["accepted", "rejected"]
    explanation: Text
    metric: Identifier
    value: float | None = Field(allow_inf_nan=False)
    report_digest: Digest
    rules_checked: list[Identifier] = Field(min_length=1, max_length=64)

    @field_validator("rules_checked")
    @classmethod
    def no_duplicate_rules(cls, value: list[str]) -> list[str]:
        if len(set(value)) != len(value):
            raise ValueError("duplicate rule check")
        return value


class ResearchSummary(StrictModel):
    findings: Text
    evidence_digests: list[Digest] = Field(
        min_length=1,
        max_length=32,
        description=(
            "Exact report_digest values from completed vm_execute results in this job. "
            "Memory archive_digest values identify stored text, not execution evidence, "
            "and must never appear here."
        ),
    )


class AgentTask(StrictModel):
    context: VmContext
    objective: Text
    rule_revision: int = Field(default=1, ge=1)
    rule_ids: list[Identifier] = Field(default_factory=list, max_length=64)
    rules: list[Rule] = Field(default_factory=list, max_length=64)
    metric: Identifier | None = None
    wall_budget_s: int | None = Field(default=None, ge=1, le=7200)
    research_wall_budget_s: int | None = Field(default=None, ge=1, le=7200)

    @model_validator(mode="after")
    def validate_rules(self) -> AgentTask:
        if self.context.purpose == "evaluate":
            if (
                self.metric is None
                or not self.rule_ids
                or {rule.id for rule in self.rules} != set(self.rule_ids)
            ):
                raise ValueError("evaluation requires the complete published rule revision")
        if len(set(self.rule_ids)) != len(self.rule_ids):
            raise ValueError("duplicate rule id")
        return self


class ProvenanceEvent(StrictModel):
    sequence: int = Field(ge=0)
    depth: int = Field(ge=0)
    kind: Literal["model", "tool", "finish"]
    name: str
    input_digest: Digest
    output_digest: Digest
    model: str
    tokens: int = Field(default=0, ge=0)


class AgentRun(StrictModel):
    result: SetupProposal | EvaluationVerdict
    transcript: list[ProvenanceEvent]
    calls: int
    tool_calls: int
    tokens: int


def safe_model_id(value: str) -> str:
    if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.:/-]+", value):
        raise ValueError("invalid model id")
    return value
