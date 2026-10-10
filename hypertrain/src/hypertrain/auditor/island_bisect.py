"""Layout-preserving disputes over hooks emitted by the real island trainer.

STEP indexes committed windows, not fabricated intermediate full states. LAYER
indexes contiguous context spans; OP indexes rank-ordered real hook boundaries.
Every window starts with its full-state root and ends with its next state root.
SIZE_OK: all-level point indexing and its referee share one trace coordinate
system; the exact seven-path scope precludes a second indexing module.
"""

from __future__ import annotations

from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Literal, assert_never

import torch
from pydantic import Field, StrictInt, TypeAdapter
from torch import Tensor

import hypertrain.trainer  # noqa: F401
from hypertrain.auditor.bisect import Level, RefereeError, points
from hypertrain.auditor.replay import pack_state, tensor_root, unpack_state
from hypertrain.miner.island_launch import confined, validate_artifacts
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages import SS58, Hex64, Layout
from hypertrain.protocol.messages_v2 import IslandJobV1, WireModel
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.island import CancelHook, Comm, TraceContext, TraceHook, train_island
from hypertrain.trainer.loop import Assignment, Params, RoundResult, SampleFn
from hypertrain.trainer.optim import OptState


class TraceEntry(WireModel):
    rank: StrictInt = Field(ge=0)
    step: StrictInt = Field(ge=0)
    microbatch: StrictInt = Field(ge=-1)
    layer: StrictInt = Field(ge=-1)
    op: str = Field(min_length=1, max_length=256)
    shape: list[StrictInt]
    dtype: str
    sha256: Hex64


class IslandCapture:
    """Mutable rank-local collector; identity return preserves actual training."""

    def __init__(self, fault: TraceHook | None = None) -> None:
        self.entries: list[TraceEntry] = []
        self.states: dict[int, bytes] = {}
        self.fault = fault

    def hook(self, ctx: TraceContext, tensor: Tensor) -> Tensor:
        out = tensor if self.fault is None else self.fault(ctx, tensor)
        raw = out.detach().cpu().contiguous().reshape(-1).view(torch.uint8)
        self.entries.append(
            TraceEntry(
                rank=ctx.rank,
                step=ctx.step,
                microbatch=ctx.microbatch,
                layer=ctx.layer,
                op=ctx.op,
                shape=list(out.shape),
                dtype=str(out.dtype),
                sha256=sha256_hex(raw.numpy().tobytes()),
            )
        )
        return out

    def checkpoint(self, t: int, theta: Params, state: OptState) -> None:
        self.states[t] = pack_state(theta, state)


@dataclass(frozen=True, slots=True)
class IslandExecution:
    cfg: TrainConfig
    layout: Layout
    assignment: Assignment
    theta: Params
    get_sample: SampleFn
    carry: OptState | None = None
    ef_in: Params | None = None
    v0: Params | None = None

    def trace(
        self,
        comm: Comm,
        *,
        fault: TraceHook | None = None,
        cancel: CancelHook | None = None,
    ) -> tuple[RoundResult, IslandCapture]:
        capture = IslandCapture(fault)
        result = train_island(
            self.cfg,
            self.layout,
            comm,
            self.theta,
            self.assignment,
            self.get_sample,
            carry=self.carry,
            ef_in=self.ef_in,
            v0=self.v0,
            hook=capture.hook,
            checkpoint=capture.checkpoint,
            cancel=cancel,
        )
        return result, capture


class IslandParty:
    """Read-only, rank-major transcript from authenticated execution artifacts."""

    def __init__(
        self,
        hotkey: str,
        captures: Sequence[IslandCapture],
        *,
        J: int,
    ) -> None:
        self.hotkey, self.J = hotkey, J
        if not captures or J < 1:
            raise RefereeError("missing island captures or invalid checkpoint stride")
        self.states = dict(captures[0].states)
        if not self.states or any(c.states != self.states for c in captures):
            raise RefereeError("rank checkpoint disagreement")
        self.entries = tuple(
            entry
            for rank, capture in enumerate(captures)
            for entry in capture.entries
            if self._rank(entry, rank)
        )
        self.H = max(self.states)
        if set(self.states) != set(range(0, self.H + 1, J)):
            raise RefereeError("incomplete committed window checkpoints")

    @staticmethod
    def _rank(entry: TraceEntry, rank: int) -> bool:
        if entry.rank != rank:
            raise RefereeError("trace rank binding mismatch")
        return True

    @classmethod
    def published(cls, hotkey: str, job: IslandJobV1, directory: Path) -> IslandParty:
        validate_artifacts(job, directory)
        captures: list[IslandCapture] = []
        for rank in range(job.manifest.training.reference_spec.layout.n_gpus):
            capture = IslandCapture()
            raw = confined(directory, f"rank-{rank}/trace.json").read_bytes()
            if len(raw) > 64 * 1024 * 1024:
                raise RefereeError("local trace exceeds artifact byte budget")
            # Local hooks contain -1 collective contexts, unlike unsigned wire envelopes.
            capture.entries = TypeAdapter(list[TraceEntry]).validate_json(raw)
            for t in range(0, job.manifest.training.inner.H + 1, job.manifest.training.inner.J):
                capture.states[t] = confined(
                    directory,
                    f"rank-{rank}/checkpoints/{t}.safetensors",
                ).read_bytes()
            captures.append(capture)
        return cls(hotkey, captures, J=job.manifest.training.inner.J)

    def state_blob(self, leaf: int) -> bytes:
        try:
            return self.states[leaf * self.J]
        except KeyError as error:
            raise RefereeError("unknown checkpoint") from error

    def state_root(self, leaf: int) -> str:
        theta, state = unpack_state(self.state_blob(leaf))
        if state is None:
            raise RefereeError("checkpoint omits optimizer")
        return tensor_root(theta, state)

    def window(self, leaf: int) -> tuple[tuple[TraceEntry, ...], tuple[int, ...], list[str]]:
        if not 1 <= leaf <= self.H // self.J:
            raise RefereeError("window outside committed trajectory")
        entries = tuple(e for e in self.entries if (leaf - 1) * self.J < e.step <= leaf * self.J)
        if not entries:
            raise RefereeError("missing actual island hooks")
        chain = [self.state_root(leaf - 1)]
        ends = []
        for i, entry in enumerate(entries):
            chain.append(sha256_hex(bytes.fromhex(chain[-1]) + canonicalize(entry.body())))
            if i + 1 == len(entries) or (entry.rank, entry.step, entry.layer, entry.microbatch) != (
                entries[i + 1].rank,
                entries[i + 1].step,
                entries[i + 1].layer,
                entries[i + 1].microbatch,
            ):
                ends.append(i + 1)
        # The full-state boundary is a real authenticated checkpoint, not a hook surrogate.
        chain.append(self.state_root(leaf))
        ends.append(len(chain) - 1)
        return entries, tuple(ends), chain

    def span(self, level: Level, ctx: tuple[int, ...]) -> int:
        match level:
            case "step":
                if ctx:
                    raise RefereeError("STEP context must be empty")
                return self.H // self.J
            case "layer":
                if len(ctx) != 1:
                    raise RefereeError("LAYER context requires window")
                return len(self.window(ctx[0])[1])
            case "op":
                if len(ctx) != 2:
                    raise RefereeError("OP context requires window/span")
                _, ends, _ = self.window(ctx[0])
                if not 0 <= ctx[1] < len(ends):
                    raise RefereeError("unknown layer span")
                return ends[ctx[1]] - (ends[ctx[1] - 1] if ctx[1] else 0)
            case other:
                assert_never(other)

    def hashes(self, level: Level, ctx: tuple[int, ...], at: Sequence[int]) -> list[str]:
        span = self.span(level, ctx)
        if any(not 0 <= i <= span for i in at):
            raise RefereeError("point outside dispute span")
        match level:
            case "step":
                return [self.state_root(i) for i in at]
            case "layer":
                _, ends, chain = self.window(ctx[0])
                return [chain[(0, *ends)[i]] for i in at]
            case "op":
                _, ends, chain = self.window(ctx[0])
                start = ends[ctx[1] - 1] if ctx[1] else 0
                return [chain[start + i] for i in at]
            case other:
                assert_never(other)

    def op_name(self, ctx: tuple[int, ...], end: int) -> str:
        entries, ends, _ = self.window(ctx[0])
        start = ends[ctx[1] - 1] if ctx[1] else 0
        index = start + end - 1
        if index == len(entries):
            return "full_state.checkpoint"
        e = entries[index]
        return f"rank={e.rank};step={e.step};microbatch={e.microbatch};layer={e.layer};op={e.op}"


class RefereeEvidence(WireModel):
    dispute_id: Hex64
    transcript_hash: Hex64
    reason: Literal["MATCH", "FRAUD", "INFRASTRUCTURE"]
    loser: SS58 | None
    predecessor_hash: Hex64
    inputs_hash: Hex64
    output_hash: Hex64
    op_spec: str


def adjudicate(
    record_id: tuple[str, str],
    parties: tuple[IslandParty, IslandParty],
    referee: IslandParty,
) -> RefereeEvidence:
    """Referee must originate in independently anchored same-layout execution.

    Comparing only a miner-supplied checkpoint is insufficient. All agreed starts
    below must also match the independent referee's anchored trajectory.
    """
    if referee.hotkey in {p.hotkey for p in parties}:
        raise RefereeError("referee must be independent")
    level: Level = "step"
    ctx: tuple[int, ...] = ()
    a, b = 0, referee.span(level, ctx)
    predecessor = referee.state_root(0)
    while True:
        at = points(a, b, 2) if b - a > 1 else [a, b]
        left, right, truth = [p.hashes(level, ctx, at) for p in (*parties, referee)]
        if left[0] != right[0] or left[0] != truth[0]:
            raise RefereeError("independent replay cannot reproduce agreed input")
        different = next((i for i in range(1, len(at)) if left[i] != right[i]), None)
        if different is None:
            if left != truth:
                raise RefereeError("both claimants disagree with independent replay")
            return RefereeEvidence(
                dispute_id=record_id[0],
                transcript_hash=record_id[1],
                reason="MATCH",
                loser=None,
                predecessor_hash=predecessor,
                inputs_hash=truth[0],
                output_hash=truth[-1],
                op_spec="anchored-full-match",
            )
        a, b = at[different - 1], at[different]
        if left[different - 1] != truth[different - 1]:
            raise RefereeError("independent replay cannot reproduce agreed op input")
        if b - a > 1:
            continue
        match level:
            case "step":
                predecessor = referee.state_root(a)
                ctx, level = (b,), "layer"
            case "layer":
                ctx, level = (ctx[0], a), "op"
            case "op":
                claims = [p.hashes(level, ctx, [b])[0] for p in parties]
                output = referee.hashes(level, ctx, [b])[0]
                wrong = [p.hotkey for p, h in zip(parties, claims, strict=True) if h != output]
                if len(wrong) != 1:
                    raise RefereeError("independent output matches neither claimant")
                return RefereeEvidence(
                    dispute_id=record_id[0],
                    transcript_hash=record_id[1],
                    reason="FRAUD",
                    loser=wrong[0],
                    predecessor_hash=predecessor,
                    inputs_hash=referee.hashes(level, ctx, [a])[0],
                    output_hash=output,
                    op_spec=referee.op_name(ctx, b),
                )
            case other:
                assert_never(other)
        a, b = 0, referee.span(level, ctx)
