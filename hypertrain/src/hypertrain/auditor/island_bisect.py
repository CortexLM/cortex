"""Layout-preserving disputes over hooks emitted by the real island trainer.

STEP indexes committed windows, not fabricated intermediate full states. LAYER
indexes contiguous context spans; OP indexes rank-ordered real hook boundaries.
Every window starts with its full-state root and ends with its next state root.
SIZE_OK: all-level point indexing and its referee share one trace coordinate
system; the exact seven-path scope precludes a second indexing module.
"""

from __future__ import annotations

from collections.abc import Callable, Sequence
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


TRACE_BYTES = 64 * 1024 * 1024  # per rank and per J-window trace file
Retrace = Callable[[int], Sequence[Sequence[TraceEntry]]]


def _skeleton(entries: Sequence[TraceEntry], base: int) -> list[tuple[object, ...]]:
    # Shapes excluded: MoE dispatch rows are data-dependent; the event sequence is not.
    return [(e.rank, e.step - base, e.microbatch, e.layer, e.op, e.dtype) for e in entries]


class IslandParty:
    """Read-only, rank-major transcript from authenticated execution artifacts.

    Commit-path traces cover window 1 only (the determinism probe). Any other window's hooks
    come from ``retrace(leaf)``: a re-run of that window from checkpoint (leaf-1)*J. Hook events
    per step do not depend on the step, so every window has window 1's skeleton (checked).
    """

    def __init__(
        self,
        hotkey: str,
        captures: Sequence[IslandCapture],
        *,
        J: int,
        retrace: Retrace | None = None,
    ) -> None:
        self.hotkey, self.J, self.retrace = hotkey, J, retrace
        self._lazy: dict[int, tuple[TraceEntry, ...]] = {}
        # Per-leaf memos: one checkpoint load+hash and one hook chain per leaf, not per query.
        self._roots: dict[int, str] = {}
        self._windows: dict[int, tuple[tuple[TraceEntry, ...], tuple[int, ...], list[str]]] = {}
        self._traced: dict[int, bool] = {}
        self._shapes: dict[int, tuple[int, ...]] = {}
        if not captures or J < 1:
            raise RefereeError("missing island captures or invalid checkpoint stride")
        self.states = dict(captures[0].states)
        # Rank 0 alone carries checkpoints (ranks are bitwise identical by validate_artifacts).
        if not self.states or any(c.states and c.states != self.states for c in captures):
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

    @staticmethod
    def read_trace(directory: Path, relative: str) -> list[TraceEntry]:
        raw = confined(directory, relative).read_bytes()
        if len(raw) > TRACE_BYTES:
            raise RefereeError("local trace exceeds artifact byte budget")
        # Local hooks contain -1 collective contexts, unlike unsigned wire envelopes.
        return TypeAdapter(list[TraceEntry]).validate_json(raw)

    @classmethod
    def published(
        cls,
        hotkey: str,
        job: IslandJobV1,
        directory: Path,
        retrace: Retrace | None = None,
    ) -> IslandParty:
        """``retrace`` defaults to launch_window on the publication's own backend."""
        validate_artifacts(job, directory)
        inner = job.manifest.training.inner
        captures: list[IslandCapture] = []
        n_gpus = job.manifest.training.reference_spec.layout.n_gpus
        for rank in range(n_gpus):
            capture = IslandCapture()
            capture.entries = cls.read_trace(directory, f"rank-{rank}/trace.json")
            captures.append(capture)
        for t in range(0, inner.H + 1, inner.J):
            captures[0].states[t] = confined(
                directory, f"rank-0/checkpoints/{t}.safetensors"
            ).read_bytes()
        if retrace is None:

            def retrace(leaf: int) -> list[list[TraceEntry]]:
                from hypertrain.miner.island_launch import launch_window

                launch_window(job, directory, leaf)
                return [
                    cls.read_trace(directory, f"rank-{rank}/windows/{leaf}.json")
                    for rank in range(n_gpus)
                ]

        return cls(hotkey, captures, J=inner.J, retrace=retrace)

    def _entries(self, leaf: int) -> tuple[TraceEntry, ...]:
        lo, hi = (leaf - 1) * self.J, leaf * self.J
        got = tuple(e for e in self.entries if lo < e.step <= hi)
        if got or self.retrace is None or leaf == 1:
            return got
        if leaf not in self._lazy:
            ranks = self.retrace(leaf)
            lazy = tuple(
                e
                for rank, entries in enumerate(ranks)
                for e in entries
                if self._rank(e, rank) and lo < e.step <= hi
            )
            if _skeleton(lazy, lo) != _skeleton(self._entries(1), 0):
                raise RefereeError("re-run window trace differs from the probe window shape")
            self._lazy[leaf] = lazy
        return self._lazy[leaf]

    def state_blob(self, leaf: int) -> bytes:
        try:
            return self.states[leaf * self.J]
        except KeyError as error:
            raise RefereeError("unknown checkpoint") from error

    def state_root(self, leaf: int) -> str:
        if leaf not in self._roots:
            theta, state = unpack_state(self.state_blob(leaf))
            if state is None:
                raise RefereeError("checkpoint omits optimizer")
            self._roots[leaf] = tensor_root(theta, state)
        return self._roots[leaf]

    def window(self, leaf: int) -> tuple[tuple[TraceEntry, ...], tuple[int, ...], list[str]]:
        if leaf not in self._windows:
            self._windows[leaf] = self._window(leaf)
        entries, ends, chain = self._windows[leaf]
        return entries, ends, list(chain)

    def _window(self, leaf: int) -> tuple[tuple[TraceEntry, ...], tuple[int, ...], list[str]]:
        if not 1 <= leaf <= self.H // self.J:
            raise RefereeError("window outside committed trajectory")
        entries = self._entries(leaf)
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
                return len(self._shape(ctx[0]))
            case "op":
                if len(ctx) != 2:
                    raise RefereeError("OP context requires window/span")
                ends = self._shape(ctx[0])
                if not 0 <= ctx[1] < len(ends):
                    raise RefereeError("unknown layer span")
                return ends[ctx[1]] - (ends[ctx[1] - 1] if ctx[1] else 0)
            case other:
                assert_never(other)

    def _shape(self, leaf: int) -> tuple[int, ...]:
        """Span ends of a window, from an already traced one (all windows share the skeleton),
        so sizing a dispute never forces a re-run."""
        if not 1 <= leaf <= self.H // self.J:
            raise RefereeError("window outside committed trajectory")
        if leaf not in self._traced:  # self.entries never changes after __init__
            self._traced[leaf] = any(
                (leaf - 1) * self.J < e.step <= leaf * self.J for e in self.entries
            )
        known = leaf if leaf in self._lazy or self._traced[leaf] else 1
        if known not in self._shapes:
            self._shapes[known] = self.window(known)[1]
        return self._shapes[known]

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
