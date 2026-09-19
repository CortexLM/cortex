"""Bounded setup exports: model-created code becomes a pinned owner-reviewable pack."""

from __future__ import annotations

import base64
import hashlib
import io
import os
import stat
import tarfile
from pathlib import Path
from typing import Literal

from pydantic import Field, model_validator

from cortex.proof.artifacts import verify_artifact
from cortex.proof.models import digest
from cortex.rlm.models import Digest, StrictModel

from .models import MAX_ARTIFACT, VmError

GENERATED_RUNNER = "generated-python"


class SetupManifest(StrictModel):
    version: Literal[1] = 1
    content_hashes: list[Digest] = Field(default_factory=list, max_length=100_000)
    dataset_ids: list[str] = Field(default_factory=list, max_length=10_000)
    flops_budget: int = Field(gt=0)
    wall_budget_s: int = Field(ge=1, le=7200)

    @model_validator(mode="after")
    def private_holdouts_required(self) -> SetupManifest:
        if not self.content_hashes and not self.dataset_ids:
            raise ValueError("private holdout evidence required")
        if any(not value or len(value) > 512 for value in self.dataset_ids):
            raise ValueError("invalid private dataset identifier")
        return self

    def holdout_digest(self) -> str:
        return digest(
            {
                "content_hashes": sorted(set(self.content_hashes)),
                "dataset_ids": sorted(set(self.dataset_ids)),
            }
        )


class SetupEvidence(SetupManifest):
    """Private operator-channel evidence. Never included in the model's tool result."""

    script_sha256: Digest
    environment_digest: Digest
    private_holdout_digest: Digest
    setup_report_digest: Digest
    baseline_report_digest: Digest | None = None
    teardown_confirmed: bool = False


class SetupExport(StrictModel):
    manifest: SetupManifest
    pack_b64: str = Field(max_length=(MAX_ARTIFACT * 4 // 3) + 4, repr=False)

    def verified(self) -> tuple[bytes, SetupEvidence]:
        try:
            raw = base64.b64decode(self.pack_b64, validate=True)
        except ValueError:
            raise VmError("invalid setup pack encoding") from None
        environment_digest = hashlib.sha256(raw).hexdigest()
        verify_artifact(raw, environment_digest, limit=MAX_ARTIFACT)
        with tarfile.open(fileobj=io.BytesIO(raw), mode="r:") as archive:
            names = {member.name for member in archive}
            if not {"run.py", "inspect.py"}.issubset(names):
                raise VmError("setup pack requires run.py and inspect.py")
            script = archive.extractfile("run.py")
            inspector = archive.extractfile("inspect.py")
            if script is None or inspector is None:
                raise VmError("setup runner must be a regular file")
            script_bytes = script.read(MAX_ARTIFACT + 1)
            if not script_bytes.strip() or not inspector.read(MAX_ARTIFACT + 1).strip():
                raise VmError("setup runner may not be empty")
            member_hashes = set()
            for member in archive:
                if member.isfile():
                    content = archive.extractfile(member)
                    if content is not None:
                        member_hashes.add(
                            hashlib.sha256(content.read(MAX_ARTIFACT + 1)).hexdigest()
                        )
            if not self.manifest.content_hashes or not set(self.manifest.content_hashes).issubset(
                member_hashes
            ):
                raise VmError("private holdout hashes must identify files in the setup pack")
        evidence = {
            **self.manifest.model_dump(),
            "script_sha256": hashlib.sha256(script_bytes).hexdigest(),
            "environment_digest": environment_digest,
            "private_holdout_digest": self.manifest.holdout_digest(),
        }
        return raw, SetupEvidence(**evidence, setup_report_digest=digest(evidence))


def export_setup(directory: Path, manifest: SetupManifest) -> SetupExport:
    if directory.is_symlink() or not directory.is_dir():
        raise VmError("setup pack directory unavailable")
    total = 0
    stream = io.BytesIO()
    with tarfile.open(fileobj=stream, mode="w", format=tarfile.USTAR_FORMAT) as archive:
        for path in sorted(directory.rglob("*")):
            info = path.lstat()
            if stat.S_ISDIR(info.st_mode):
                continue
            if not stat.S_ISREG(info.st_mode) or path.is_symlink():
                raise VmError("setup pack contains a special file")
            total += info.st_size
            if total > MAX_ARTIFACT:
                raise VmError("setup pack too large")
            member = tarfile.TarInfo(path.relative_to(directory).as_posix())
            member.size, member.mode = info.st_size, 0o600
            fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
            with os.fdopen(fd, "rb") as content:
                current = os.fstat(content.fileno())
                if not stat.S_ISREG(current.st_mode) or current.st_size != info.st_size:
                    raise VmError("setup file changed while exporting")
                archive.addfile(member, content)
    export = SetupExport(manifest=manifest, pack_b64=base64.b64encode(stream.getvalue()).decode())
    export.verified()
    return export
