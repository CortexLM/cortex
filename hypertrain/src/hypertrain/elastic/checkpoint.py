"""Content-addressed checkpoint chains for one shard of one round.

A manifest binds the run, the policy digest, the round, the shard, the round's base state, the
number of inner steps completed, the state object, its parent manifest, the member that
produced it and the lease token it held. The first link of a shard-round has no parent and
resumes from the base state. A resumer walks the chain back to the base before trusting the
head, and the coordinator accepts a new head only if its parent is the head it already holds:
the hash link is the proof that a replacement resumed from the right state, and each link is
one replayable audit segment (parent step to step).
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Literal

from pydantic import ValidationError

from hypertrain.data.store import ObjectNotFound
from hypertrain.elastic.store import CheckpointStore
from hypertrain.protocol.envelope import EnvelopeError
from hypertrain.protocol.envelope_v2 import load_json
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages import Hex64
from hypertrain.protocol.messages_v2 import U64, Identifier, Positive, WireModel

MANIFEST_MAGIC = b"ht-elastic-checkpoint-v1\n"


class CheckpointError(ValueError):
    def __init__(self, code: str, detail: str = "") -> None:
        super().__init__(f"{code}: {detail}" if detail else code)
        self.code = code


@dataclass(frozen=True, slots=True)
class WorkUnit:
    """One shard of one round: `steps` inner steps from the round's `base` state object."""

    run_id: str
    policy: str
    round: int
    shard: int
    base: str
    steps: int


class CheckpointManifest(WireModel):
    version: Literal[1] = 1
    run_id: Identifier
    policy: Hex64
    round: U64
    shard: U64
    base: Hex64
    parent: Hex64 | None
    step: Positive
    state: Hex64
    producer: Identifier
    lease: Positive

    def encode(self) -> bytes:
        return MANIFEST_MAGIC + canonicalize(self.body(), allow_float=False)

    def binds(self, unit: WorkUnit) -> bool:
        return (self.run_id, self.policy, self.round, self.shard, self.base) == (
            unit.run_id,
            unit.policy,
            unit.round,
            unit.shard,
            unit.base,
        ) and self.step <= unit.steps


class CheckpointManager:
    def __init__(self, store: CheckpointStore) -> None:
        self.store = store

    def save(
        self,
        unit: WorkUnit,
        parent: str | None,
        step: int,
        state: bytes,
        producer: str,
        lease: int,
    ) -> str:
        """Write the state, then its manifest; returns the manifest key.

        The order matters: a manifest is never visible before the state it names. Saving the
        same link twice writes the same two keys."""
        return self.link(unit, parent, step, self.store.put(state), producer, lease)

    def link(
        self, unit: WorkUnit, parent: str | None, step: int, state: str, producer: str, lease: int
    ) -> str:
        """Manifest for a state already in the store (a large state is uploaded by streaming)."""
        if not 1 <= step <= unit.steps:
            raise CheckpointError("STEP_RANGE", f"step {step} outside 1..{unit.steps}")
        if not self.store.exists(state):
            raise CheckpointError("MISSING_STATE", state)
        manifest = CheckpointManifest(
            run_id=unit.run_id,
            policy=unit.policy,
            round=unit.round,
            shard=unit.shard,
            base=unit.base,
            parent=parent,
            step=step,
            state=state,
            producer=producer,
            lease=lease,
        )
        return self.store.put(manifest.encode())

    def manifest(self, key: str) -> CheckpointManifest:
        try:
            raw = self.store.get(key)
        except ObjectNotFound:
            raise CheckpointError("MISSING_MANIFEST", key) from None
        if not raw.startswith(MANIFEST_MAGIC):
            raise CheckpointError("NOT_A_MANIFEST", key)
        try:
            manifest = CheckpointManifest.model_validate(load_json(raw[len(MANIFEST_MAGIC) :]))
        except (EnvelopeError, ValidationError) as e:
            raise CheckpointError("MALFORMED_MANIFEST", type(e).__name__) from None
        if manifest.encode() != raw:
            raise CheckpointError("NONCANONICAL_MANIFEST", key)
        return manifest

    def check_link(self, unit: WorkUnit, head: str | None, key: str) -> CheckpointManifest:
        """Coordinator-side check of one new link, without downloading the state."""
        manifest = self.manifest(key)
        if not manifest.binds(unit):
            raise CheckpointError("WRONG_UNIT", key)
        if manifest.parent != head:
            raise CheckpointError("NOT_HEAD_CHILD", f"parent {manifest.parent}, head {head}")
        floor = 0 if head is None else self.manifest(head).step
        if manifest.step <= floor:
            raise CheckpointError("STEP_NOT_INCREASING", f"{manifest.step} <= {floor}")
        if not self.store.exists(manifest.state):
            raise CheckpointError("MISSING_STATE", manifest.state)
        return manifest

    def chain(self, unit: WorkUnit, head: str | None) -> list[CheckpointManifest]:
        """Manifests from the first link to `head`, each bound to `unit`, steps increasing.

        Steps strictly decrease towards the base and are at least 1, so the walk ends."""
        out: list[CheckpointManifest] = []
        key = head
        while key is not None:
            manifest = self.manifest(key)
            if not manifest.binds(unit):
                raise CheckpointError("WRONG_UNIT", key)
            if out and manifest.step >= out[-1].step:
                raise CheckpointError("STEP_NOT_INCREASING", key)
            out.append(manifest)
            key = manifest.parent
        out.reverse()
        return out

    def resume(self, unit: WorkUnit, head: str | None) -> tuple[int, bytes]:
        """(steps done, verified state) at `head`; the base state when no link exists yet."""
        chain = self.chain(unit, head)
        key, step = (chain[-1].state, chain[-1].step) if chain else (unit.base, 0)
        try:
            return step, self.store.get(key)
        except ObjectNotFound:
            raise CheckpointError("MISSING_STATE", key) from None
