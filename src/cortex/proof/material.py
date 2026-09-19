"""Private setup exports verified against an owner-signed topic before rent."""

from __future__ import annotations

import os
import stat
from pathlib import Path
from typing import Literal, Protocol

from pydantic import Field

from cortex.errors import ServiceError
from cortex.protocol.crypto import verify_raw
from cortex.vm.setup import SetupEvidence, SetupExport

from .models import Document, Hex64, Topic
from .service import TOPIC_DOMAIN

MAX_MATERIAL_BYTES = 128 * 1024 * 1024
_FAILURE = "harvest topic or private material invalid"


def _verified_export(
    export: SetupExport, topic: Topic, topic_public_key: bytes
) -> tuple[Topic, SetupExport, bytes, SetupEvidence]:
    # Rebuild nested models: frozen Pydantic objects can still contain mutable data.
    topic = Topic.model_validate(topic.model_dump(warnings=False))
    if not verify_raw(
        topic_public_key, TOPIC_DOMAIN, topic.signing_payload(), bytes.fromhex(topic.signature)
    ):
        raise ValueError("untrusted topic")
    if (
        topic.status != "open"
        or topic.metric.family == "custom"
        or topic.baseline is None
        or topic.holdout_commitment is None
    ):
        raise ValueError("incompatible topic")
    export = SetupExport.model_validate(export.model_dump(warnings=False))
    raw, evidence = export.verified()
    if (
        topic.params.get("experiment_pack_digest") != "sha256:" + evidence.environment_digest
        or topic.baseline.script_sha256 != evidence.script_sha256
        or topic.holdout_commitment != evidence.private_holdout_digest
        or topic.flops_budget != evidence.flops_budget
        or topic.wall_budget_s != evidence.wall_budget_s
        or topic.baseline.flops_budget != evidence.flops_budget
        or topic.baseline.wall_budget_s != evidence.wall_budget_s
    ):
        raise ValueError("setup evidence mismatch")
    return topic, export, raw, evidence


def _bindings(topic: Topic, evidence: SetupEvidence) -> dict[str, object]:
    return {
        "schema_version": 1,
        "topic_digest": topic.content_digest(),
        "environment_digest": evidence.environment_digest,
        "private_holdout_digest": evidence.private_holdout_digest,
        "script_sha256": evidence.script_sha256,
        "flops_budget": evidence.flops_budget,
        "wall_budget_s": evidence.wall_budget_s,
    }


class HarvestMaterial(Document):
    schema_version: Literal[1] = 1
    topic_digest: Hex64
    environment_digest: Hex64
    private_holdout_digest: Hex64
    script_sha256: Hex64
    flops_budget: int = Field(gt=0, strict=True)
    wall_budget_s: int = Field(gt=0, le=7200, strict=True)
    setup_export: SetupExport = Field(repr=False, exclude=True)

    @classmethod
    def from_export(
        cls, export: SetupExport, topic: Topic, topic_public_key: bytes
    ) -> HarvestMaterial:
        try:
            topic, snapshot, _, evidence = _verified_export(export, topic, topic_public_key)
            return cls.model_validate({**_bindings(topic, evidence), "setup_export": snapshot})
        except Exception:
            raise ServiceError(503, _FAILURE) from None

    def verified(self, topic: Topic, topic_public_key: bytes) -> tuple[bytes, SetupEvidence]:
        try:
            topic, _, raw, evidence = _verified_export(self.setup_export, topic, topic_public_key)
            public = self.model_dump(warnings=False)
            type(self).model_validate({**public, "setup_export": self.setup_export}, strict=True)
            if public != _bindings(topic, evidence):
                raise ValueError("material commitment mismatch")
            return raw, evidence
        except Exception:
            raise ServiceError(503, _FAILURE) from None


class HarvestMaterialSource(Protocol):
    def load(self, topic: Topic) -> HarvestMaterial: ...


class PrivateFileMaterialSource:
    def __init__(self, root: Path, *, topic_public_key: bytes):
        self.root = Path(root)
        self.topic_public_key = topic_public_key

    def load(self, topic: Topic) -> HarvestMaterial:
        try:
            topic = Topic.model_validate(topic.model_dump(warnings=False))
            directory = os.open(
                self.root, os.O_RDONLY | os.O_CLOEXEC | os.O_DIRECTORY | os.O_NOFOLLOW
            )
            try:
                metadata = os.fstat(directory)
                if metadata.st_uid != os.geteuid() or stat.S_IMODE(metadata.st_mode) != 0o700:
                    raise ValueError("unsafe material directory")
                descriptor = os.open(
                    f"{topic.content_digest()}.json",
                    os.O_RDONLY | os.O_CLOEXEC | os.O_NOFOLLOW | os.O_NONBLOCK,
                    dir_fd=directory,
                )
                with os.fdopen(descriptor, "rb") as source:
                    metadata = os.fstat(source.fileno())
                    if (
                        not stat.S_ISREG(metadata.st_mode)
                        or metadata.st_uid != os.geteuid()
                        or stat.S_IMODE(metadata.st_mode) != 0o600
                        or metadata.st_nlink != 1
                        or not 0 < metadata.st_size <= MAX_MATERIAL_BYTES
                    ):
                        raise ValueError("unsafe material file")
                    raw = source.read(MAX_MATERIAL_BYTES + 1)
                    if len(raw) > MAX_MATERIAL_BYTES:
                        raise ValueError("oversized material file")
            finally:
                os.close(directory)
            export = SetupExport.model_validate_json(raw)
            return HarvestMaterial.from_export(export, topic, self.topic_public_key)
        except Exception:
            raise ServiceError(503, _FAILURE) from None
