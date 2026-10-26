"""Full-round observed work, never physical-device attestation or a payment multiplier."""

from __future__ import annotations

import hashlib
import json
import threading
import time
from pathlib import Path
from typing import Literal, Protocol

from hypertrain.data.store import Put, put_file
from hypertrain.data.trial_assignment import trial_assignment_hash
from hypertrain.miner.island_launch import (
    IslandArtifacts,
    confined,
    launch_island,
    validate_artifacts,
)
from hypertrain.protocol.hashing import MerkleTree
from hypertrain.protocol.messages import LeafPreimage
from hypertrain.protocol.messages_v2 import (
    ArtifactRef,
    IslandJobV1,
    JoinChallenge,
    WorkProof,
    WorkScreenV2,
)


class WorkScreenError(ValueError):
    def __init__(self, code: str) -> None:
        self.code = code
        super().__init__(code)


class IslandLaunch(Protocol):
    """Internal synchronous launch seam; never a wire/public callback."""

    def __call__(
        self,
        job: IslandJobV1,
        directory: Path,
        *,
        backend: Literal["cpu", "cuda"],
        cancel: threading.Event | None = None,
        trace: bool = False,
    ) -> IslandArtifacts: ...


def artifact_ref(path: Path, put: Put | None = None) -> ArtifactRef:
    """put_file key + length of the referenced object (plain bytes or ht-parts-v1 index).

    put=None hashes only (no upload), yielding the same ref the uploader publishes."""
    last = b""

    def record(data: bytes) -> str:
        nonlocal last
        last = data
        return put(data) if put is not None else hashlib.sha256(data).hexdigest()

    sha = put_file(path, record)
    return ArtifactRef(sha256=sha, size=len(last))


def commitments(artifacts: IslandArtifacts) -> tuple[str, str]:
    leaves = [LeafPreimage.model_validate(v) for v in json.loads(artifacts.leaves.read_bytes())]
    with artifacts.delta.open("rb") as f:
        delta = hashlib.file_digest(f, "sha256").hexdigest()
    return MerkleTree([bytes.fromhex(p.digest()) for p in leaves]).root.hex(), delta


def screen_work(
    job: IslandJobV1,
    directory: Path,
    challenge: JoinChallenge,
    *,
    now_beacon: int,
    backend: Literal["cpu", "cuda"] = "cuda",
    launch: IslandLaunch | None = None,
    cancel: threading.Event | None = None,
) -> tuple[WorkScreenV2, WorkProof]:
    """Launch the actual island API and consume its validated, all-rank publication.

    CPU backend is local functional evidence only; admission controls qualification.
    Caller rechecks verified beacon deadline when accepting the resulting proof.
    """
    if cancel is not None and cancel.is_set():
        raise WorkScreenError("CANCELLED")
    if time.time() >= job.deadline:
        raise WorkScreenError("DEADLINE")
    if (
        not challenge.seed_beacon <= now_beacon <= challenge.deadline_beacon
        or challenge.manifest_hash != job.run_id
        or job.manifest.run_id() != job.run_id
        or challenge.layout != job.manifest.training.reference_spec.layout
        or challenge.assignment_hash
        != trial_assignment_hash(job.manifest, job.w, tuple(job.sample_ids))
    ):
        raise WorkScreenError("CHALLENGE_BINDING")
    import hypertrain.trainer  # noqa: F401
    from hypertrain.auditor.replay import unpack_state
    from hypertrain.trainer.compress import state_hash

    theta, _ = unpack_state(confined(directory, job.object_paths["start_state"]).read_bytes())
    if state_hash(theta) != challenge.theta_hash:
        raise WorkScreenError("CHALLENGE_START")
    if cancel is not None and cancel.is_set():
        raise WorkScreenError("CANCELLED")
    if time.time() >= job.deadline:
        raise WorkScreenError("DEADLINE")
    if launch is None and cancel is None:
        artifacts = launch_island(job, directory, backend=backend)
    else:
        artifacts = (launch or launch_island)(job, directory, backend=backend, cancel=cancel)
        artifacts = validate_artifacts(job, artifacts.directory)
        if cancel is not None and cancel.is_set():
            raise WorkScreenError("CANCELLED")
    refs = []
    for path in (artifacts.state, artifacts.ef, artifacts.delta, artifacts.leaves):
        # Ref = stored object (plain or part index); every stored object is <= 1 MiB.
        ref = artifact_ref(path)
        if ref.size > challenge.artifact_limits.max_object_bytes:
            raise WorkScreenError("ARTIFACT_LIMIT")
        refs.append(ref)
    root, delta = commitments(artifacts)
    screen = WorkScreenV2(
        evidence_kind="work-proof-v1",
        admission_id=challenge.admission_id,
        nonce=challenge.nonce,
        policy_hash=challenge.policy_hash,
        image_digest=job.manifest.training.reference_spec.image_digest,
        layout=challenge.layout,
        challenge_hash=challenge.digest(),
        rank_results=list(artifacts.ranks),
        artifact_hashes=[r.sha256 for r in refs],
    )
    proof = WorkProof(
        admission_id=challenge.admission_id,
        challenge_hash=challenge.digest(),
        leaves_root=root,
        delta_hash=delta,
        artifact_refs=refs,
    )
    return screen, proof
