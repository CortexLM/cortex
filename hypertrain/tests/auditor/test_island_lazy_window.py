"""Lazy per-window traces: synthetic captures only, no training."""

from __future__ import annotations

import pytest
import torch

import hypertrain.trainer  # noqa: F401
from hypertrain.auditor.bisect import RefereeError
from hypertrain.auditor.island_bisect import IslandCapture, IslandParty, TraceEntry
from hypertrain.auditor.replay import pack_state
from hypertrain.protocol.keys import Keypair
from hypertrain.trainer.optim import OptState

J, H, RANKS = 2, 6, 2


def entries(rank: int, lo: int, salt: str = "") -> list[TraceEntry]:
    return [
        TraceEntry(
            rank=rank,
            step=t,
            microbatch=-1,
            layer=-1,
            op=op,
            shape=[t],  # data-dependent shapes (MoE rows) must not break the skeleton check
            dtype="torch.float32",
            sha256=(f"{rank}{t}{op}{salt}".encode().hex() * 64)[:64],
        )
        for t in range(lo + 1, lo + J + 1)
        for op in ("all_gather.input", "optimizer.theta.w")
    ]


def captures() -> list[IslandCapture]:
    out = []
    for rank in range(RANKS):
        c = IslandCapture()
        c.entries = entries(rank, 0)  # commit path: window 1 probe only
        if rank == 0:  # checkpoints exist under rank-0 only
            for t in range(0, H + 1, J):

                def w(t: int = t) -> dict[str, torch.Tensor]:
                    return {"w": torch.full((2,), float(t))}

                c.states[t] = pack_state(w(), OptState(w(), w(), t))
        out.append(c)
    return out


def test_lazy_window_reruns_once_and_matches_continuous_trace() -> None:
    calls: list[int] = []

    def retrace(leaf: int) -> list[list[TraceEntry]]:
        calls.append(leaf)
        return [entries(r, (leaf - 1) * J) for r in range(RANKS)]

    party = IslandParty(Keypair(b"\x71" * 32).ss58, captures(), J=J, retrace=retrace)
    continuous = captures()
    for rank, c in enumerate(continuous):
        c.entries = [e for lo in range(0, H, J) for e in entries(rank, lo)]
    full = IslandParty(Keypair(b"\x72" * 32).ss58, continuous, J=J)
    # Sizing a dispute never re-runs; every window has window 1's shape.
    assert [party.span("layer", (w,)) for w in (1, 2, 3)] == [
        full.span("layer", (w,)) for w in (1, 2, 3)
    ]
    assert calls == []
    at = list(range(party.span("layer", (3,)) + 1))
    assert party.hashes("layer", (3,), at) == full.hashes("layer", (3,), at)
    party.hashes("op", (3, 0), [0, 1])
    assert calls == [3]


def test_lazy_window_rejects_rerun_with_a_different_event_shape() -> None:
    def retrace(leaf: int) -> list[list[TraceEntry]]:
        return [entries(r, (leaf - 1) * J)[:-1] for r in range(RANKS)]

    party = IslandParty(Keypair(b"\x73" * 32).ss58, captures(), J=J, retrace=retrace)
    with pytest.raises(RefereeError, match="probe window shape"):
        party.window(2)
