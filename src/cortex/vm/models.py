"""Python VM protocol v2: every command and result carries its topic/job binding."""

from __future__ import annotations

import base64
import hashlib
from typing import Annotated, Literal

from pydantic import Field, model_validator

from cortex.rlm.models import (
    Digest,
    Identifier,
    Metric,
    ProducedArtifact,
    RuleCheck,
    StrictModel,
    VmAction,
    VmContext,
    canonical_bytes,
)

API_VERSION = 2
MAX_ARTIFACT = 64 * 1024 * 1024
MAX_FRAME = 96 * 1024 * 1024
TAIL_BYTES = 16_384


class VmError(Exception):
    def __init__(self, reason: str, status: int = 503):
        self.reason, self.status = reason, status
        super().__init__(reason)


class Resources(StrictModel):
    vcpus: int = Field(default=16, ge=1, le=16)
    mem_mib: int = Field(default=32768, ge=128, le=32768)
    disk_mib: int = Field(default=32768, ge=16384, le=1048576)


class VmSpec(StrictModel):
    topic_id: Identifier
    image_digest: Digest
    kind: Literal["topic", "experiment"] = "topic"
    resources: Resources = Field(default_factory=Resources)


class VmRecord(StrictModel):
    vm_id: Identifier
    spec: VmSpec
    state: Literal["booting", "running", "destroyed", "retained", "uncertain"]
    pid: int | None = None


class ExecuteRequest(StrictModel):
    execution_id: Identifier
    context: VmContext
    action: VmAction
    artifact_b64: str = Field(default="", max_length=(MAX_ARTIFACT * 4 // 3) + 4, repr=False)
    pack_b64: str = Field(default="", max_length=(MAX_ARTIFACT * 4 // 3) + 4, repr=False)
    env: dict[str, str] = Field(default_factory=dict, max_length=8, repr=False)
    params: dict[str, str] = Field(default_factory=dict, max_length=32)

    @model_validator(mode="after")
    def bound_artifact(self) -> ExecuteRequest:
        if self.context.purpose == "evaluate" and not self.artifact_b64:
            raise ValueError("artifact required")
        return self

    def artifact(self) -> bytes:
        return self._decode(self.artifact_b64)

    def pack(self) -> bytes:
        return self._decode(self.pack_b64)

    @staticmethod
    def _decode(encoded: str) -> bytes:
        try:
            raw = base64.b64decode(encoded, validate=True)
        except ValueError:
            raise VmError("invalid artifact encoding", 400) from None
        if len(raw) > MAX_ARTIFACT:
            raise VmError("artifact too large", 400)
        return raw

    def commitment(self) -> str:
        # Only the digest enters the durable job ledger; never credential values.
        return hashlib.sha256(canonical_bytes(self)).hexdigest()

    @property
    def dedicated(self) -> bool:
        return self.context.purpose == "evaluate" or self.action.phase == "experiment"


class GuestMeasurement(StrictModel):
    """Written by the operator adaptor; no model prose can supply these fields."""

    metrics: list[Metric] = Field(default_factory=list, max_length=64)
    rule_checks: list[RuleCheck] = Field(default_factory=list, max_length=64)
    produced_artifacts: list[ProducedArtifact] = Field(default_factory=list, max_length=16)
    flops_used: Annotated[int, Field(ge=0)]


class GuestOutput(StrictModel):
    context: VmContext
    execution_id: Identifier
    exit_code: int
    stdout_tail: str = Field(max_length=TAIL_BYTES)
    measurement: GuestMeasurement | None = None
    report_digest: Digest
    setup_export: dict[str, object] | None = Field(default=None, repr=False)
