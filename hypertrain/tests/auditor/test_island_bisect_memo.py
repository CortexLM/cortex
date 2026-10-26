"""IslandParty memoization: one checkpoint load per leaf, unchanged dispute geometry."""

from __future__ import annotations

import time
from collections import Counter
from collections.abc import Callable, Iterable, Sequence
from typing import Any

import pytest
import torch

import hypertrain.trainer  # noqa: F401
from hypertrain.auditor import island_bisect
from hypertrain.auditor.bisect import RefereeError
from hypertrain.auditor.island_bisect import (
    IslandCapture,
    IslandParty,
    RefereeEvidence,
    TraceEntry,
    adjudicate,
)
from hypertrain.auditor.replay import pack_state, tensor_root, unpack_state
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.trainer.optim import OptState

Group = tuple[int, int, int]  # (layer, microbatch, hook count) of one consecutive span
J, H, RANKS = 2, 6, 2


class BaseParty(IslandParty):
    """origin/main 0313989 state_root/window/_shape, verbatim: the unmemoized oracle."""

    def state_root(self, leaf: int) -> str:
        theta, state = unpack_state(self.state_blob(leaf))
        if state is None:
            raise RefereeError("checkpoint omits optimizer")
        return tensor_root(theta, state)

    def window(self, leaf: int) -> tuple[tuple[TraceEntry, ...], tuple[int, ...], list[str]]:
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
        chain.append(self.state_root(leaf))
        ends.append(len(chain) - 1)
        return entries, tuple(ends), chain

    def _shape(self, leaf: int) -> tuple[int, ...]:
        if not 1 <= leaf <= self.H // self.J:
            raise RefereeError("window outside committed trajectory")
        known = (
            leaf
            if leaf in self._lazy
            or any((leaf - 1) * self.J < e.step <= leaf * self.J for e in self.entries)
            else 1
        )
        return self.window(known)[1]


def store_sizing(party: IslandParty) -> tuple[int, int, int]:
    """ChallengeStore.complete_audit_v2's dispute span sizing (store.py), verbatim."""
    layer_span = max(party.span("layer", (window,)) for window in range(1, party.H // party.J + 1))
    op_span = max(
        party.span("op", (window, layer))
        for window in range(1, party.H // party.J + 1)
        for layer in range(party.span("layer", (window,)))
    )
    return party.H // party.J, layer_span, op_span


def small_groups(t: int) -> list[Group]:
    if t == 0:  # opening full-state gathers only
        return [(-1, -1, 2)]
    full = 2 if t % J == 0 else 0
    forward = [(-1, 0, 1), (0, 0, 2), (1, 0, 2), (-1, 1, 1), (0, 1, 2), (1, 1, 2)]
    return [*forward, (-1, -1, 3), (2, -1, 2), (-1, -1, 1 + full)]


def ragged_groups(t: int) -> list[Group]:
    # Committed windows are not skeleton-checked; window 2 carries one extra span.
    return [*small_groups(t), (3, -1, 1)] if t == J + 1 else small_groups(t)


def trace(
    rank: int,
    steps: Iterable[int],
    groups: Callable[[int], Sequence[Group]],
    tamper_from: int | None = None,
) -> list[TraceEntry]:
    out = []
    for t in steps:
        for g, (layer, microbatch, count) in enumerate(groups(t)):
            for i in range(count):
                mark = "x" if tamper_from is not None and t >= tamper_from else ""
                out.append(
                    TraceEntry(
                        rank=rank,
                        step=t,
                        microbatch=microbatch,
                        layer=layer,
                        op=f"hook.{g}.{i}",
                        shape=[1],
                        dtype="torch.float32",
                        sha256=sha256_hex(f"{rank}:{t}:{g}:{i}:{mark}".encode()),
                    )
                )
    return out


def captures(
    entries: Callable[[int], list[TraceEntry]], h: int, j: int, drift_from: int | None = None
) -> list[IslandCapture]:
    out = []
    for rank in range(RANKS):
        capture = IslandCapture()
        capture.entries = entries(rank)
        if rank == 0:  # checkpoints exist under rank-0 only
            for t in range(0, h + 1, j):
                value = float(t) + (0.5 if drift_from is not None and t >= drift_from else 0.0)

                def tensors(value: float = value) -> dict[str, torch.Tensor]:
                    return {"w": torch.full((2,), value)}

                capture.states[t] = pack_state(tensors(), OptState(tensors(), tensors(), t))
        out.append(capture)
    return out


def party(
    kind: str, cls: type[IslandParty], seed: int, retraced: list[int] | None = None
) -> IslandParty:
    """Same-input parties for the memoized class and the oracle; retrace calls are recorded."""
    hotkey = Keypair(bytes([seed]) * 32).ss58
    calls = retraced if retraced is not None else []
    if kind in ("committed", "ragged"):
        groups = ragged_groups if kind == "ragged" else small_groups
        return cls(hotkey, captures(lambda r: trace(r, range(H + 1), groups), H, J), J=J)
    tamper = kind == "fraud"

    def retrace(leaf: int) -> list[list[TraceEntry]]:
        calls.append(leaf)
        lo = (leaf - 1) * J
        ranks = [
            trace(r, range(lo + 1, lo + J + 1), small_groups, J + 2 if tamper else None)
            for r in range(RANKS)
        ]
        return [entries[:-1] for entries in ranks] if kind == "bad-retrace" else ranks

    commit = captures(
        lambda r: trace(r, range(J + 1), small_groups), H, J, 2 * J if tamper else None
    )
    if kind == "bare-checkpoint":  # window 2's end state lacks its optimizer
        commit[0].states[2 * J] = pack_state({"w": torch.full((2,), float(2 * J))})
    return cls(hotkey, commit, J=J, retrace=retrace)


def attempt(query: Callable[..., object], *args: object) -> object:
    try:
        return query(*args)
    except RefereeError as error:  # a refusal is an observable outcome to compare
        return ("RefereeError", str(error))


def observe(p: IslandParty) -> list[object]:
    """Every query a dispute makes, sizing before and after any window re-run."""
    w_max = p.H // p.J
    out = [attempt(store_sizing, p), attempt(p.span, "step", ())]
    for w in (0, w_max + 1):
        out += [attempt(p.window, w), attempt(p.span, "layer", (w,))]
    out.append(attempt(p.hashes, "step", (), list(range(w_max + 1))))
    for w in range(1, w_max + 1):
        out.append(attempt(p.window, w))
        spans = attempt(p.span, "layer", (w,))
        out.append(spans)
        if not isinstance(spans, int):
            continue
        out.append(attempt(p.hashes, "layer", (w,), list(range(spans + 1))))
        out.append(attempt(p.span, "op", (w, spans)))
        for layer in range(spans):
            n = attempt(p.span, "op", (w, layer))
            out.append(n)
            if isinstance(n, int):
                out.append(attempt(p.hashes, "op", (w, layer), list(range(n + 1))))
                out += [attempt(p.op_name, (w, layer), end) for end in range(1, n + 1)]
    out.append(attempt(store_sizing, p))
    return out


@pytest.fixture
def loads(monkeypatch: pytest.MonkeyPatch) -> Counter[bytes]:
    """Checkpoint loads by IslandParty, per blob; a reload fails at once instead of hashing on."""
    seen: Counter[bytes] = Counter()
    real = island_bisect.unpack_state

    def counting(blob: bytes) -> object:
        seen[blob] += 1
        assert seen[blob] == 1, f"checkpoint loaded {seen[blob]} times"
        return real(blob)

    monkeypatch.setattr(island_bisect, "unpack_state", counting)
    return seen


def test_dispute_queries_load_each_checkpoint_and_chain_each_hook_once(
    loads: Counter[bytes], monkeypatch: pytest.MonkeyPatch
) -> None:
    # Given: every window committed, so each window builds its own chain
    p = party("committed", IslandParty, 0x81)
    chained: list[object] = []
    canonicalize = island_bisect.canonicalize

    def counting(value: object) -> bytes:
        chained.append(value)
        return canonicalize(value)

    monkeypatch.setattr(island_bisect, "canonicalize", counting)
    # When: the full (window, layer) grid is queried three times
    for _ in range(3):
        p.hashes("step", (), list(range(H // J + 1)))
        for w in range(1, H // J + 1):
            p.window(w)
            for layer in range(p.span("layer", (w,))):
                n = p.span("op", (w, layer))
                p.hashes("op", (w, layer), list(range(n + 1)))
                p.op_name((w, layer), n)
    # Then
    assert set(loads) == set(p.states.values())
    assert sorted(loads.values()) == [1] * (H // J + 1)
    assert len(chained) == sum(e.step > 0 for e in p.entries)  # step-0 hooks are in no window


OD_H, OD_J, OD_LAYERS, OD_TENSORS = 64, 16, 12, 171  # od-base: 12 blocks, 171 parameters


def od_groups(t: int) -> list[Group]:
    """One od-base step per rank: n_gpus=2, grad_accum=1, all_gather reduction (1+n hooks)."""
    full = 9 * OD_TENSORS if t % OD_J == 0 else 0  # theta/m/v full() gathers at checkpoints
    if t == 0:
        return [(-1, -1, full)]
    gather = 1 + RANKS
    forward = [(-1, 0, 1), (0, 0, 7), *((i, 0, 6) for i in range(1, OD_LAYERS)), (OD_LAYERS, 0, 3)]
    return [
        *forward,
        (-1, -1, gather * OD_TENSORS),  # gradient reduction
        (OD_LAYERS + 1, -1, OD_TENSORS),  # gradient.*
        (-1, -1, gather),  # grad norm
        (OD_LAYERS + 1, -1, 3 * OD_TENSORS),  # optimizer theta/m/v
        (-1, -1, gather + full),  # mean loss, then checkpoint gathers
    ]


def test_store_sizing_at_od_base_geometry_hashes_window_one_once(
    loads: Counter[bytes], monkeypatch: pytest.MonkeyPatch
) -> None:
    # Given: the published commit trace (steps 0..J only) at od-base hook geometry
    p = IslandParty(
        Keypair(b"\x91" * 32).ss58,
        captures(lambda r: trace(r, range(OD_J + 1), od_groups), OD_H, OD_J),
        J=OD_J,
    )
    assert sum(0 < e.step <= OD_J for e in p.entries) == 44_038  # hooks in window 1
    windows, leaves = OD_H // OD_J, OD_H // OD_J + 1
    roots: list[int] = []
    derived: list[int] = []
    state_root, window = IslandParty.state_root, IslandParty.window

    def counted_root(self: IslandParty, leaf: int) -> str:
        roots.append(leaf)
        assert len(roots) <= 2 * leaves, "state_root called per span query"
        return state_root(self, leaf)

    def counted_window(self: IslandParty, leaf: int) -> Any:
        derived.append(leaf)
        assert len(derived) <= windows, "span shape re-derived per span query"
        return window(self, leaf)

    monkeypatch.setattr(IslandParty, "state_root", counted_root)
    monkeypatch.setattr(IslandParty, "window", counted_window)
    # When
    start = time.perf_counter()
    sizes = store_sizing(p)
    elapsed = time.perf_counter() - start
    # Then: 2 ranks x 16 steps x 19 spans + checkpoint; widest span = mean loss + 9 x 171
    assert sizes == (4, 609, 1542)
    assert set(loads) == {p.states[0], p.states[OD_J]} and set(loads.values()) == {1}
    assert len(roots) <= 2 * len(loads)
    assert set(derived) == {1}  # untraced windows take window 1's shape, never a re-run
    assert elapsed < 5, f"sizing took {elapsed:.1f} s"


@pytest.mark.parametrize("kind", ["committed", "lazy", "ragged", "bad-retrace", "bare-checkpoint"])
def test_memoized_party_answers_like_unmemoized_oracle(kind: str) -> None:
    # Given
    calls: list[int] = []
    oracle_calls: list[int] = []
    memo = party(kind, IslandParty, 0xA1, calls)
    oracle = party(kind, BaseParty, 0xA1, oracle_calls)
    # When
    got, expected = observe(memo), observe(oracle)
    # Then
    assert got == expected
    assert calls == oracle_calls
    # Each failure scenario is refused where intended, and only there.
    refusals = {x[1] for x in got if isinstance(x, tuple) and x[0] == "RefereeError"}
    assert ("re-run window trace differs from the probe window shape" in refusals) == (
        kind == "bad-retrace"
    )
    assert ("checkpoint omits optimizer" in refusals) == (kind == "bare-checkpoint")


def test_memos_belong_to_one_party(monkeypatch: pytest.MonkeyPatch) -> None:
    # Given: twin parties over identical artifacts, and one with another shape and checkpoints
    seen: Counter[bytes] = Counter()
    real = island_bisect.unpack_state

    def counting(blob: bytes) -> object:
        seen[blob] += 1
        return real(blob)

    monkeypatch.setattr(island_bisect, "unpack_state", counting)
    twins = [party("committed", IslandParty, seed) for seed in (0xC1, 0xC2)]

    def other(cls: type[IslandParty]) -> IslandParty:
        commit = captures(lambda r: trace(r, range(H + 1), ragged_groups), H, J, 0)
        return cls(Keypair(b"\xc3" * 32).ss58, commit, J=J)

    # When
    for twin in twins:
        observe(twin)
    got = observe(other(IslandParty))
    # Then: no answer leaks between parties, and each party hashes its own checkpoints
    assert got == observe(other(BaseParty))
    assert seen == Counter(
        {
            **dict.fromkeys(twins[0].states.values(), 2),
            **dict.fromkeys(other(IslandParty).states.values(), 1),
        }
    )


@pytest.mark.parametrize("cheat", [True, False])
def test_adjudication_matches_unmemoized_oracle(cheat: bool) -> None:
    def verdict(cls: type[IslandParty]) -> RefereeEvidence:
        honest = party("lazy", cls, 0xB1)
        other = party("fraud" if cheat else "lazy", cls, 0xB2)
        referee = party("lazy", cls, 0xB3)
        return adjudicate(("11" * 32, "22" * 32), (honest, other), referee)

    # When
    evidence = verdict(IslandParty)
    # Then
    assert evidence == verdict(BaseParty)
    assert evidence.reason == ("FRAUD" if cheat else "MATCH")
    if cheat:
        assert evidence.loser == Keypair(b"\xb2" * 32).ss58
