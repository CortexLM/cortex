"""Deterministic sample assignment from drand + Feistel PRP, and reconcile of reported exposures.

key_w = sha256("ht-assign" || run_id || u64be(w) || drand_sig[d_assign])
Round block = global exposure positions [base_w, base_w + S*B) (base_w = cumulative count the
coordinator publishes). position p -> epoch e = p // n, pos = p % n.
samples(slot) = { EpochPRP_e(pos(base_w + RoundPRP_w(slot*B + j))) : j < B }
EpochPRP_e is keyed by sha256("ht-epoch" || run_id || u64be(e)) (public shuffle of one data pass);
RoundPRP_w is keyed by key_w (drand-unpredictable slot<->slice mapping). Both are bijections, so
slices are disjoint across slots and across rounds inside one data pass.
"""

from __future__ import annotations

import hashlib
import struct
from collections.abc import Mapping, Sequence
from collections.abc import Set as AbstractSet
from dataclasses import dataclass

ROUNDS = 8


class ReconcileError(ValueError):
    def __init__(self, code: str, detail: str = "") -> None:
        super().__init__(f"{code}: {detail}" if detail else code)
        self.code = code


def assign_key(run_id: bytes, w: int, drand_sig: bytes) -> bytes:
    if len(run_id) != 32 or w < 0 or not drand_sig:
        raise ValueError("run_id must be 32 bytes, w >= 0, drand_sig non-empty")
    return hashlib.sha256(b"ht-assign" + run_id + struct.pack(">Q", w) + drand_sig).digest()


def epoch_key(run_id: bytes, epoch: int) -> bytes:
    return hashlib.sha256(b"ht-epoch" + run_id + struct.pack(">Q", epoch)).digest()


class FeistelPRP:
    """Bijection on [0, n): balanced 8-round Feistel on 2*half bits with SHA-256 round function,
    cycle-walking back into range (domain < 4n, so < 4 expected walks)."""

    def __init__(self, key: bytes, n: int) -> None:
        if n < 1:
            raise ValueError("n must be >= 1")
        self.n = n
        bits = max(2, (n - 1).bit_length())
        bits += bits & 1
        self.half = bits // 2
        self.mask = (1 << self.half) - 1
        self._h = hashlib.sha256(b"ht-feistel-v1" + struct.pack(">Q", n) + key)

    def _f(self, r: int, x: int) -> int:
        h = self._h.copy()
        h.update(struct.pack(">BQ", r, x))
        return int.from_bytes(h.digest()[:8], "big") & self.mask

    def _enc(self, x: int) -> int:
        left, right = x >> self.half, x & self.mask
        for r in range(ROUNDS):
            left, right = right, left ^ self._f(r, right)
        return (left << self.half) | right

    def __call__(self, x: int) -> int:
        if not 0 <= x < self.n:
            raise IndexError(x)
        y = self._enc(x)
        while y >= self.n:
            y = self._enc(y)
        return y


@dataclass(frozen=True)
class RoundAssignment:
    run_id: bytes
    w: int
    generation: int
    epoch: int
    base_w: int
    batch: int
    slices: tuple[tuple[int, ...], ...]  # slot -> sample indices

    @property
    def n_slots(self) -> int:
        return len(self.slices)


def assign_round(
    run_id: bytes,
    w: int,
    drand_sig: bytes,
    *,
    n_samples: int,
    n_slots: int,
    batch: int,
    base_w: int,
    generation: int = 0,
    unit: int = 1,
) -> RoundAssignment:
    if unit < 1 or batch % unit or n_samples % unit or base_w % unit:
        raise ValueError("batch, n_samples and base_w must be multiples of unit")
    size = n_slots * batch
    if n_slots < 1 or batch < 1 or base_w < 0 or size > n_samples:
        raise ValueError("need n_slots, batch >= 1, base_w >= 0, n_slots*batch <= n_samples")
    epoch = base_w // n_samples
    # ponytail: a round may not straddle a data pass; coordinator starts the next pass at the
    # boundary (skips <= S*B tail samples). Upgrade: per-exposure (epoch, idx) ids.
    if (base_w + size - 1) // n_samples != epoch:
        raise ValueError("round block straddles an epoch boundary")
    # Both PRPs act on units of `unit` consecutive samples; unit=1 is the per-sample scheme.
    bu = batch // unit
    rprp = FeistelPRP(assign_key(run_id, w, drand_sig), n_slots * bu)
    eprp = FeistelPRP(epoch_key(run_id, epoch), n_samples // unit)
    off = (base_w % n_samples) // unit
    slices = tuple(
        tuple(eprp(off + rprp(slot * bu + k)) * unit + j for k in range(bu) for j in range(unit))
        for slot in range(n_slots)
    )
    return RoundAssignment(run_id, w, generation, epoch, base_w, batch, slices)


def reconcile(
    a: RoundAssignment,
    reports: Mapping[int, Sequence[int]],
    *,
    generation: int,
    consumed: AbstractSet[tuple[int, int]] = frozenset(),
) -> set[tuple[int, int]]:
    """Validate per-slot reported sample ids; returns new exposure ids {(epoch, idx)}.

    Raises ReconcileError: STALE_GENERATION, SHARD_OVERLAP, DUPLICATE_EXPOSURE,
    ASSIGNMENT_VIOLATION, EXPOSURE_COUNT.
    """
    if generation != a.generation:
        raise ReconcileError("STALE_GENERATION", f"report gen {generation} != {a.generation}")
    owner: dict[int, int] = {}
    for slot, ids in enumerate(a.slices):
        for i in ids:
            if i in owner:
                raise ReconcileError("SHARD_OVERLAP", f"sample {i} in slots {owner[i]},{slot}")
            owner[i] = slot
    seen: set[tuple[int, int]] = set()
    for slot, got in sorted(reports.items()):
        if not 0 <= slot < a.n_slots:
            raise ReconcileError("ASSIGNMENT_VIOLATION", f"unknown slot {slot}")
        for i in got:
            e = (a.epoch, i)
            if e in seen or e in consumed:
                raise ReconcileError("DUPLICATE_EXPOSURE", f"sample {i} (slot {slot})")
            if i not in owner:
                raise ReconcileError(
                    "ASSIGNMENT_VIOLATION", f"sample {i} not assigned (slot {slot})"
                )
            if owner[i] != slot:
                raise ReconcileError(
                    "SHARD_OVERLAP", f"slot {slot} used sample {i} of slot {owner[i]}"
                )
            seen.add(e)
        if len(got) != len(a.slices[slot]):
            raise ReconcileError("EXPOSURE_COUNT", f"slot {slot}: {len(got)} != {a.batch}")
    return seen
