"""Versioned Python topic contract; research content comes from signed documents."""

from __future__ import annotations

import hashlib
import json
import re
from typing import Annotated, Literal, Self

from pydantic import BaseModel, ConfigDict, Field, model_validator

Hex64 = Annotated[str, Field(pattern=r"^[0-9a-f]{64}$")]
Slug = Annotated[str, Field(pattern=r"^[a-z0-9][a-z0-9-]{1,62}$")]
Identifier = Annotated[str, Field(pattern=r"^[a-z0-9][a-z0-9_-]{1,63}$")]
Unsigned = Annotated[int, Field(ge=0, le=2**64 - 1, strict=True)]
Finite = Annotated[float, Field(allow_inf_nan=False)]


def canonical_json(value: object) -> bytes:
    """Python topic v2 canonical form (explicitly versioned, never legacy JCS)."""
    return json.dumps(
        value, sort_keys=True, ensure_ascii=False, separators=(",", ":"), allow_nan=False
    ).encode()


def digest(value: object) -> str:
    return hashlib.sha256(canonical_json(value)).hexdigest()


class Document(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, allow_inf_nan=False)


class Rule(Document):
    id: Identifier
    text: Annotated[str, Field(min_length=1, max_length=2048)]
    check: Annotated[str, Field(max_length=16384)] = ""
    failure: Literal["reject", "zero"] = "reject"


class Metric(Document):
    family: Literal["custom", "nll", "throughput"]
    primary: Identifier
    direction: Literal["min", "max"]
    epsilon: Annotated[float, Field(gt=0, allow_inf_nan=False)]
    relative: bool = False
    custom_id: Identifier | None = None
    max_regression: Annotated[float, Field(ge=0, le=0.05)] = 0.05
    quality_floor_nll: Annotated[float, Field(ge=0, le=0.02)] = 0.02

    @model_validator(mode="after")
    def family_contract(self) -> Self:
        if self.family == "custom" and self.custom_id is None:
            raise ValueError("custom_id required")
        if self.family == "nll" and (
            self.primary != "holdout_nll"
            or self.direction != "min"
            or self.relative
            or self.epsilon < 0.02
        ):
            raise ValueError("nll metric loosens global floor")
        if self.family == "throughput" and (
            (self.primary, self.direction)
            not in {("tokens_per_sec", "max"), ("step_latency_ms", "min")}
            or not self.relative
            or self.epsilon < 0.05
        ):
            raise ValueError("throughput metric loosens global floor")
        return self


class Baseline(Document):
    script_sha256: Hex64
    metrics: dict[str, Finite]
    metrics_commitment: Hex64
    evidence_digest: Hex64
    flops_budget: Unsigned
    wall_budget_s: Annotated[int, Field(gt=0, le=7200)]

    @model_validator(mode="after")
    def seal_matches(self) -> Self:
        if not self.metrics or self.metrics_commitment != digest(self.metrics):
            raise ValueError("baseline metrics commitment mismatch")
        return self


class Endpoint(Document):
    """Declarative topic endpoints; no CP code or arbitrary upstream proxies."""

    path: Annotated[str, Field(pattern=r"^/[a-z][a-z0-9/_-]{0,127}$")]
    method: Literal["GET", "POST"]
    purpose: Literal["documentation", "submission", "results"]
    description: Annotated[str, Field(min_length=1, max_length=2048)]
    request_schema: dict[str, object] = Field(default_factory=dict)
    response_schema: dict[str, object] = Field(default_factory=dict)


class TopicEvalExecutor(Document):
    """Signed, tighten-only constraints on the live harvest executor."""

    require_offer_commitment: Hex64 | None = None
    max_proof_deadline_s: Annotated[int, Field(gt=0, le=7200, strict=True)] | None = None


class Topic(Document):
    schema_version: Literal[2] = 2
    id: Slug
    revision: Annotated[int, Field(ge=1, strict=True)] = 1
    statement: Annotated[str, Field(min_length=1, max_length=8192)]
    status: Literal["draft", "open", "closed"] = "draft"
    payout_mode: Literal["wta", "discovery"] = "discovery"
    pass_floor_share_bps: Annotated[int, Field(ge=0, le=10000)] = 3000
    metric: Metric
    flops_budget: Annotated[int, Field(gt=0, le=2_000_000_000_000_000_000)]
    wall_budget_s: Annotated[int, Field(gt=0, le=7200)] = 3600
    params: dict[str, str] = Field(default_factory=dict, max_length=32)
    checklist: list[Rule] = Field(default_factory=list, max_length=64)
    baseline: Baseline | None = None
    holdout_commitment: Hex64 | None = None
    eval_image_digest: Annotated[str, Field(pattern=r"^sha256:[0-9a-f]{64}$")]
    inference_offer_commitment: Hex64
    eval_executor: TopicEvalExecutor = Field(default_factory=TopicEvalExecutor)
    documentation: Annotated[str, Field(max_length=65536)] = ""
    endpoints: list[Endpoint] = Field(default_factory=list, max_length=16)
    valid_from_epoch: Unsigned = 0
    valid_until_epoch: Unsigned | None = None
    signature: str = ""

    @model_validator(mode="after")
    def constraints(self) -> Self:
        if len({r.id for r in self.checklist}) != len(self.checklist):
            raise ValueError("duplicate checklist rule")
        if len({(e.method, e.path) for e in self.endpoints}) != len(self.endpoints):
            raise ValueError("duplicate endpoint")
        for key, value in self.params.items():
            if not re.fullmatch(r"[a-z0-9][a-z0-9_-]{1,63}", key):
                raise ValueError("invalid param name")
            if not value or len(value) > 256 or not value.isprintable():
                raise ValueError("invalid param value")
        for key in ("defer_scoring", "require_training_evidence"):
            if key in self.params and self.params[key] not in {"true", "false"}:
                raise ValueError(f"invalid {key}")
        if self.valid_until_epoch is not None and self.valid_until_epoch < self.valid_from_epoch:
            raise ValueError("invalid epoch window")
        if self.status == "open" and (not self.baseline or not self.holdout_commitment):
            raise ValueError("sealed baseline and holdout commitment required")
        if self.baseline and (
            self.metric.primary not in self.baseline.metrics
            or self.baseline.flops_budget != self.flops_budget
            or self.baseline.wall_budget_s != self.wall_budget_s
        ):
            raise ValueError("baseline comparison is not paired")
        return self

    def signing_payload(self) -> bytes:
        return canonical_json(self.model_dump(exclude={"signature"}))

    def content_digest(self) -> str:
        return hashlib.sha256(self.signing_payload()).hexdigest()

    def active(self, epoch: int) -> bool:
        return (
            self.status == "open"
            and self.valid_from_epoch <= epoch
            and (self.valid_until_epoch is None or epoch <= self.valid_until_epoch)
        )


class Manifest(Document):
    train_content_hashes: list[str] = Field(default_factory=list, max_length=10000)
    train_dataset_ids: list[str] = Field(default_factory=list, max_length=10000)

    def signing_payload(self) -> bytes:
        def ordered(items: list[str]) -> bytes:
            return b"\xff".join([str(len(items)).encode(), *(s.encode() for s in sorted(items))])

        return ordered(self.train_content_hashes) + b"\xff" + ordered(self.train_dataset_ids)


class Submission(Document):
    topic_id: Slug
    miner_hotkey: Hex64
    artifact_digest: Hex64
    artifact_uri: Annotated[str, Field(max_length=2048)] | None = None
    declared_flops: Unsigned = 0
    claim: Annotated[str, Field(min_length=1, max_length=65536)]
    manifest: Manifest = Field(default_factory=Manifest)
    submit_nonce: Hex64
    hotkey_signature: Annotated[str, Field(pattern=r"^[0-9a-f]{128}$")]
    env: dict[str, str] = Field(default_factory=dict, repr=False, exclude=True)

    def signing_payload(self) -> bytes:
        return b"\xff".join(
            [
                self.miner_hotkey.encode(),
                self.topic_id.encode(),
                self.artifact_digest.encode(),
                str(self.declared_flops).encode(),
                self.claim.encode(),
                self.manifest.signing_payload(),
                self.submit_nonce.encode(),
            ]
        )


class SubmissionLookup(Document):
    """Original signed envelope used only to recover a durable submission."""

    topic_id: Slug
    miner_hotkey: Hex64
    artifact_digest: Hex64
    declared_flops: Unsigned = 0
    claim: Annotated[str, Field(min_length=1, max_length=65536)]
    manifest: Manifest = Field(default_factory=Manifest)
    submit_nonce: Hex64
    hotkey_signature: Annotated[str, Field(pattern=r"^[0-9a-f]{128}$")]

    def signing_payload(self) -> bytes:
        return b"\xff".join(
            [
                self.miner_hotkey.encode(),
                self.topic_id.encode(),
                self.artifact_digest.encode(),
                str(self.declared_flops).encode(),
                self.claim.encode(),
                self.manifest.signing_payload(),
                self.submit_nonce.encode(),
            ]
        )


class EvaluationReport(Document):
    topic_id: Slug
    topic_digest: Hex64
    submission_id: Hex64
    artifact_digest: Hex64
    verdict: Literal["clean", "suspicious", "reject"]
    reproduced: bool
    claim_holds: bool
    rule_results: dict[str, bool]
    metrics: dict[str, Finite]
    flops_used: Unsigned
    wall_seconds: Annotated[float, Field(ge=0, allow_inf_nan=False)]
    evidence_digest: Hex64
    vm_id: str
    sandboxed: Literal[True]
    teardown_confirmed: Literal[True]
    executor_offer_id: Identifier | None = None
    executor_offer_commitment: Hex64 | None = None
    executor_config_commitment: Hex64 | None = None
    near_duplicate: bool = False
    rationale: Annotated[str, Field(max_length=4096)] = ""
