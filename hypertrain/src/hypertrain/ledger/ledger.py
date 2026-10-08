"""Reward ledger: integer units, escrow vesting, burns, clawback debt and get_weights.

All amounts are integers. Mass that is not paid burns (Cortex algorithm 3 burns
`full_share_mass - W`); it is never redistributed. State is a pure replay of the journal, so
every persisted get_weights answer can be recomputed and compared byte for byte.

Event timestamps `at` are chain/ingestion Unix seconds, non-decreasing, and strictly after the
last answered `epoch_at` (a late event must not change what an earlier answer could see). Events
are applied lazily: an answer for `epoch_at` T sees exactly the events with `at <= T`, so one
in-order pass over the journal reproduces every answer (open is O(journal)).
"""

from __future__ import annotations

import math
from collections import deque
from dataclasses import asdict, dataclass
from fractions import Fraction
from pathlib import Path
from typing import Any

from .journal import ChainBreak, Journal, canonical

UNIT = 10**6
FULL_SHARE_MASS = UNIT
MAX_ENTRIES = 65_536
F_MIN_PPM = 800_000
VERDICTS = frozenset({"MATCH", "UNSAMPLED", "FAULT", "TRANSIENT", "NO_UPLOAD"})
EVENT_KINDS = frozenset({"commit", "verdict", "finalize", "clawback", "vindicate", "fault"})


class LedgerError(ValueError):
    """Rejected input (maps to HTTP 4xx at the challenge boundary)."""


class RebuildMismatch(ChainBreak):
    """A persisted answer is not reproduced by replaying the journal."""


def vest_rounds_for_q(q: float | str) -> int:
    """E = ceil(1/q), computed exactly from the decimal text of q."""
    exact = Fraction(str(q))
    if not 0 < exact <= 1:
        raise LedgerError("q must be in (0, 1]")
    return math.ceil(1 / exact)


@dataclass(frozen=True)
class Params:
    challenge_slug: str
    genesis_unix: int
    epoch_seconds: int
    epochs_per_round: int
    vest_rounds: int
    forgive_per_epoch: int = 1

    def __post_init__(self) -> None:
        ints = (self.genesis_unix, self.epoch_seconds, self.epochs_per_round, self.vest_rounds)
        if any(type(v) is not int for v in (*ints, self.forgive_per_epoch)):
            raise LedgerError("params must be integers")
        if self.genesis_unix < 0 or min(ints[1:]) < 1 or self.forgive_per_epoch < 0:
            raise LedgerError("params out of range")
        if not self.challenge_slug:
            raise LedgerError("challenge_slug required")

    @property
    def round_budget(self) -> int:
        return self.epochs_per_round * UNIT

    def epoch_of(self, t: int) -> int:
        """Chain epoch index containing Unix time t; -1 before genesis."""
        return -1 if t < self.genesis_unix else (t - self.genesis_unix) // self.epoch_seconds

    def round_of(self, t: int) -> int:
        """Training round containing Unix time t; -1 before genesis."""
        epoch = self.epoch_of(t)
        return -1 if epoch < 0 else epoch // self.epochs_per_round


@dataclass
class Entitlement:
    w: int
    final_round: int
    hotkey: str
    amount: int
    paid: int = 0
    burned: int = 0

    @property
    def outstanding(self) -> int:
        return self.amount - self.paid - self.burned


@dataclass
class State:
    minted: int = 0
    burned: int = 0
    paid: int = 0
    last_finalized: int = -1

    def __post_init__(self) -> None:
        self.verdicts: dict[int, dict[str, tuple[str, int, int]]] = {}
        self.entitlements: list[Entitlement] = []
        self.by_hotkey: dict[str, list[int]] = {}
        self.cursor = 0
        self.blacklist: set[str] = set()
        self.debt: dict[str, int] = {}
        self.transients: dict[tuple[str, int], int] = {}
        self.forgone: dict[tuple[int, str], int] = {}
        # units actually burned by the fault of (w, hotkey): round slice and escrow
        self.fault_slice: dict[tuple[int, str], int] = {}
        self.fault_escrow: dict[tuple[int, str], int] = {}

    @property
    def pending(self) -> int:
        return sum(e.outstanding for e in self.entitlements)

    def burn_outstanding(self, hotkey: str, limit: int | None = None) -> int:
        taken = 0
        for index in self.by_hotkey.get(hotkey, []):
            ent = self.entitlements[index]
            take = ent.outstanding if limit is None else min(ent.outstanding, limit - taken)
            ent.burned += take
            taken += take
        self.burned += taken
        return taken


def _check_int(name: str, value: Any, lo: int = 0) -> int:
    if type(value) is not int or not lo <= value < 2**63:
        raise LedgerError(f"{name} must be an integer in [{lo}, 2^63)")
    return value


def _check_hotkey(value: Any) -> str:
    if not isinstance(value, str) or not 1 <= len(value) <= 128 or not value.isprintable():
        raise LedgerError("hotkey must be a printable string of 1..128 chars")
    return value


def _apply(state: State, params: Params, rec: dict[str, Any]) -> None:
    kind = rec["kind"]
    if kind == "verdict":
        verdict, hotkey = rec["verdict"], rec["hotkey"]
        if verdict == "TRANSIENT":
            key = (hotkey, params.epoch_of(rec["at"]))
            state.transients[key] = state.transients.get(key, 0) + 1
            if state.transients[key] > params.forgive_per_epoch:
                verdict = "FAULT"
        if verdict == "FAULT" and hotkey not in state.blacklist:
            state.blacklist.add(hotkey)
            state.fault_escrow[(rec["w"], hotkey)] = state.burn_outstanding(hotkey)
        score = rec["soft_score_ppm"]
        state.verdicts.setdefault(rec["w"], {})[hotkey] = (verdict, rec["tokens"], score)
    elif kind == "finalize":
        _finalize(state, params, rec["w"], rec["final_round"])
    elif kind == "vindicate":
        _vindicate(state, rec["w"], rec["hotkey"], rec["final_round"])
    elif kind == "fault":
        _fault(state, rec["w"], rec["hotkey"])
    elif kind == "clawback":
        hotkey = rec["hotkey"]
        owed = rec["units"] - state.burn_outstanding(hotkey, rec["units"])
        state.debt[hotkey] = state.debt.get(hotkey, 0) + owed


def _finalize(state: State, params: Params, w: int, final_round: int) -> None:
    budget = params.round_budget
    state.minted += budget
    state.last_finalized = w
    verdicts = state.verdicts.pop(w, {})
    contrib = {
        h: tokens * (F_MIN_PPM + min(max(score, 0), UNIT) // 5)
        for h, (_, tokens, score) in verdicts.items()
    }
    total = sum(contrib.values())
    granted = 0
    for hotkey in sorted(verdicts):
        verdict = verdicts[hotkey][0]
        if total == 0:
            continue
        if verdict == "FAULT":
            state.fault_slice[(w, hotkey)] = budget * contrib[hotkey] // total
        if hotkey in state.blacklist:
            continue
        if verdict == "NO_UPLOAD":
            state.forgone[(w, hotkey)] = budget * contrib[hotkey] // total
        if verdict in ("MATCH", "UNSAMPLED"):
            granted += _grant(state, w, final_round, hotkey, budget * contrib[hotkey] // total)
    state.burned += budget - granted


def _grant(state: State, w: int, final_round: int, hotkey: str, amount: int) -> int:
    """Repay debt first, then add an entitlement; returns the units granted (repaid units burn)."""
    repay = min(amount, state.debt.get(hotkey, 0))
    if repay:
        state.debt[hotkey] -= repay
        amount -= repay
    if amount:
        state.by_hotkey.setdefault(hotkey, []).append(len(state.entitlements))
        state.entitlements.append(Entitlement(w, final_round, hotkey, amount))
    return amount


def _vindicate(state: State, w: int, hotkey: str, final_round: int) -> None:
    """Pay on vindication: the round-w slice that burned as NO_UPLOAD becomes an entitlement
    vesting from the credit's round. Units move from burned to pending (conservation holds)."""
    amount = state.forgone.pop((w, hotkey), 0)
    if hotkey in state.blacklist:
        return
    state.burned -= _grant(state, w, final_round, hotkey, amount)


def _fault(state: State, w: int, hotkey: str) -> None:
    """Post-finality FAULT (dispute lost after a NO_UPLOAD finalize): the forgone round slice
    stays burned, every outstanding entitlement burns and the hotkey is blacklisted, so no
    future entitlement is granted (ultrabrain section 4 collateral and clawback)."""
    state.fault_slice[(w, hotkey)] = state.forgone.pop((w, hotkey), 0)
    if hotkey not in state.blacklist:
        state.blacklist.add(hotkey)
        state.fault_escrow[(w, hotkey)] = state.burn_outstanding(hotkey)


class _Engine:
    """Ledger state machine shared by the live ledger and the rebuild-from-journal check."""

    def __init__(self, params: Params) -> None:
        self.params = params
        self.state = State()
        self.queue: deque[dict[str, Any]] = deque()
        self.commits: dict[int, set[str]] = {}
        self.verdicts: dict[int, set[str]] = {}
        self.head_finalized = -1
        self.last_event_at = -1
        self.last_answer_at = -1
        self.last_epoch = -1
        self.answers: dict[int, dict[str, Any]] = {}
        self.no_upload: dict[int, set[str]] = {}
        self.vindicable: set[tuple[int, str]] = set()
        self.vindicated: set[tuple[str, int, str]] = set()
        self.faulted: set[tuple[str, int, str]] = set()
        # hotkeys with a journaled post-finality fault: unpaid from now on, even for epochs
        # timed before the event applies
        self.frozen: set[str] = set()

    def check_event(self, rec: dict[str, Any]) -> None:
        kind, at = rec["kind"], rec["at"]
        if at < self.last_event_at or at <= self.last_answer_at:
            raise LedgerError("event time precedes journal history")
        w: int = rec.get("w", -1)
        if kind == "commit":
            if w <= self.head_finalized:
                raise LedgerError("round already finalized")
            if rec["hotkey"] in self.commits.get(w, set()):
                raise LedgerError("duplicate commit")
        elif kind == "verdict":
            if rec["hotkey"] not in self.commits.get(w, set()) or w <= self.head_finalized:
                raise LedgerError("verdict without an open commit")
            if rec["hotkey"] in self.verdicts.get(w, set()):
                raise LedgerError("duplicate verdict")
        elif kind == "finalize":
            if w != self.head_finalized + 1:
                raise LedgerError("rounds finalize in order")
            if self.commits.get(w, set()) != self.verdicts.get(w, set()):
                raise LedgerError("every commit needs a verdict before finalize")
            if rec["final_round"] != self.params.round_of(at):
                raise LedgerError("final_round must be round_of(at)")
        elif kind in ("vindicate", "fault"):
            if (w, rec["hotkey"]) not in self.vindicable:
                raise LedgerError(f"{kind}: needs a finalized NO_UPLOAD not yet settled")
            if kind == "vindicate" and rec["final_round"] != self.params.round_of(at):
                raise LedgerError("final_round must be round_of(at)")

    def event(self, rec: dict[str, Any]) -> None:
        kind = rec["kind"]
        w: int = rec.get("w", -1)
        if kind == "commit":
            self.commits.setdefault(w, set()).add(rec["hotkey"])
        elif kind == "verdict":
            self.verdicts.setdefault(w, set()).add(rec["hotkey"])
            if rec["verdict"] == "NO_UPLOAD":
                self.no_upload.setdefault(w, set()).add(rec["hotkey"])
        elif kind == "vindicate":
            self.vindicable.discard((w, rec["hotkey"]))
            self.vindicated.add((rec["run_id"], w, rec["hotkey"]))
        elif kind == "fault":
            self.vindicable.discard((w, rec["hotkey"]))
            self.faulted.add((rec["run_id"], w, rec["hotkey"]))
            self.frozen.add(rec["hotkey"])
        elif kind == "finalize":
            self.vindicable |= {(w, hk) for hk in self.no_upload.pop(w, set())}
            self.head_finalized = w
            self.commits.pop(w, None)
            self.verdicts.pop(w, None)
        self.last_event_at = rec["at"]
        self.queue.append(rec)

    def flush(self, t: int) -> None:
        while self.queue and self.queue[0]["at"] <= t:
            _apply(self.state, self.params, self.queue.popleft())

    def compute(
        self, epoch: int, epoch_at: int, computed_at: int
    ) -> tuple[dict[str, Any], list[list[int]]]:
        self.flush(epoch_at)
        state, params = self.state, self.params
        round_at = params.round_of(epoch_at)
        payments: list[list[int]] = []
        reason = "ok"
        capped = 0
        if epoch < self.last_epoch or epoch_at < self.last_answer_at:
            reason = "out_of_order"
        else:
            ents = state.entitlements
            while state.cursor < len(ents) and ents[state.cursor].outstanding == 0:
                state.cursor += 1
            budget = FULL_SHARE_MASS
            index = state.cursor
            while index < len(ents) and budget > 0:
                ent = ents[index]
                if ent.final_round + params.vest_rounds > round_at:
                    break
                units = 0 if ent.hotkey in self.frozen else min(ent.outstanding, budget)
                if units:
                    payments.append([index, units])
                    budget -= units
                index += 1
            per_hotkey: dict[str, int] = {}
            for index, units in payments:
                hk = ents[index].hotkey
                per_hotkey[hk] = per_hotkey.get(hk, 0) + units
            if len(per_hotkey) > MAX_ENTRIES:
                ranked = sorted(per_hotkey, key=lambda h: (-per_hotkey[h], h))
                keep = set(ranked[:MAX_ENTRIES])
                capped = len(per_hotkey) - MAX_ENTRIES
                payments = [p for p in payments if ents[p[0]].hotkey in keep]
        weights: dict[str, int] = {}
        for index, units in payments:
            hk = state.entitlements[index].hotkey
            weights[hk] = weights.get(hk, 0) + units
        paid_now = sum(weights.values())
        answer = {
            "challenge_slug": params.challenge_slug,
            "epoch": epoch,
            "weights": {h: float(weights[h]) for h in sorted(weights)},
            "full_share_mass": FULL_SHARE_MASS,
            "metadata": {
                "epoch_at": epoch_at,
                "round_at": round_at,
                "reason": reason,
                "units_paid": paid_now,
                "units_burned_this_epoch": FULL_SHARE_MASS - paid_now,
                "hotkeys_capped": capped,
                "ledger_minted": state.minted,
                "ledger_burned": state.burned,
                "ledger_paid": state.paid + paid_now,
                "ledger_pending": state.minted - state.burned - state.paid - paid_now,
            },
            "computed_at": computed_at,
        }
        return answer, payments

    def record_answer(self, rec: dict[str, Any]) -> None:
        for index, units in rec["payments"]:
            self.state.entitlements[index].paid += units
            self.state.paid += units
        meta = rec["answer"]["metadata"]
        self.last_answer_at = max(self.last_answer_at, meta["epoch_at"])
        self.last_epoch = max(self.last_epoch, rec["answer"]["epoch"])
        self.answers[rec["answer"]["epoch"]] = rec

    def replay(self, rec: dict[str, Any]) -> None:
        """Feed one verified journal record; raise RebuildMismatch if it is not reproduced."""
        kind = rec["kind"]
        if kind in EVENT_KINDS:
            try:
                self.check_event(rec)
            except LedgerError as exc:
                raise RebuildMismatch(f"record {rec['seq']}: {exc}") from exc
            self.event(rec)
        elif kind == "weights":
            ans = rec["answer"]
            if ans["epoch"] in self.answers:
                raise RebuildMismatch(f"record {rec['seq']}: second answer for an epoch")
            meta = ans["metadata"]
            answer, payments = self.compute(ans["epoch"], meta["epoch_at"], ans["computed_at"])
            if canonical(answer) != canonical(ans) or payments != rec["payments"]:
                raise RebuildMismatch(f"answer for epoch {ans['epoch']} not reproduced")
            self.record_answer(rec)


class Ledger:
    def __init__(self, directory: Path, params: Params) -> None:
        self.params = params
        self.journal = Journal(directory)
        records = self.journal.records
        stored = next((r for r in records if r["kind"] == "init"), None)
        if stored is None:
            if any(r["kind"] != "journal_repaired" for r in records):
                raise ChainBreak("journal has no init record")
            self.journal.append("init", {"params": asdict(params)})
        elif stored["params"] != asdict(params):
            raise LedgerError("params differ from the journal's init record")
        self._engine = self._rebuild()

    def _rebuild(self) -> _Engine:
        engine = _Engine(self.params)
        for rec in self.journal.records:
            engine.replay(rec)
        return engine

    def verify(self) -> None:
        """Recompute every persisted answer from the journal; raise on any byte drift."""
        self._rebuild()

    def state(self) -> State:
        """Full ledger state (every event applied) replayed from the journal."""
        engine = self._rebuild()
        engine.flush(2**63)
        return engine.state

    def _event(self, kind: str, value: dict[str, Any]) -> None:
        self._engine.check_event({"kind": kind, **value})
        self._engine.event(self.journal.append(kind, value))

    def commit(self, w: int, hotkey: str, at: int) -> None:
        _check_int("w", w)
        _check_hotkey(hotkey)
        _check_int("at", at)
        self._event("commit", {"w": w, "hotkey": hotkey, "at": at})

    def verdict(
        self, w: int, hotkey: str, verdict: str, tokens: int, soft_score_ppm: int, at: int
    ) -> None:
        _check_int("w", w)
        _check_hotkey(hotkey)
        _check_int("tokens", tokens)
        _check_int("soft_score_ppm", soft_score_ppm, lo=-(2**62))
        _check_int("at", at)
        if verdict not in VERDICTS:
            raise LedgerError("unknown verdict")
        rec = {"w": w, "hotkey": hotkey, "verdict": verdict, "tokens": tokens}
        self._event("verdict", {**rec, "soft_score_ppm": soft_score_ppm, "at": at})

    def finalize(self, w: int, at: int) -> None:
        _check_int("w", w)
        _check_int("at", at)
        self._event("finalize", {"w": w, "at": at, "final_round": self.params.round_of(at)})

    def vindicate(self, run_id: str, w: int, hotkey: str, at: int) -> bool:
        """Credit a finalized NO_UPLOAD round after a won dispute; False if already credited."""
        if not isinstance(run_id, str) or not 1 <= len(run_id) <= 128:
            raise LedgerError("run_id must be a string of 1..128 chars")
        _check_int("w", w)
        _check_hotkey(hotkey)
        _check_int("at", at)
        if (run_id, w, hotkey) in self._engine.vindicated:
            return False
        final_round = self.params.round_of(at)
        rec = {"run_id": run_id, "w": w, "hotkey": hotkey, "at": at, "final_round": final_round}
        self._event("vindicate", rec)
        return True

    def fault(self, run_id: str, w: int, hotkey: str, at: int) -> bool:
        """Post-finality FAULT of a NO_UPLOAD round (dispute lost); False if already applied."""
        if not isinstance(run_id, str) or not 1 <= len(run_id) <= 128:
            raise LedgerError("run_id must be a string of 1..128 chars")
        _check_int("w", w)
        _check_hotkey(hotkey)
        _check_int("at", at)
        if (run_id, w, hotkey) in self._engine.faulted:
            return False
        self._event("fault", {"run_id": run_id, "w": w, "hotkey": hotkey, "at": at})
        return True

    def burned_for_fault(self, w: int, hotkey: str) -> tuple[int, int]:
        """(round slice, escrow) units the ledger burns for the fault of (w, hotkey)."""
        state = self.state()
        key = (w, hotkey)
        return state.fault_slice.get(key, 0), state.fault_escrow.get(key, 0)

    def clawback(self, hotkey: str, units: int, at: int) -> None:
        _check_hotkey(hotkey)
        _check_int("units", units, lo=1)
        _check_int("at", at)
        self._event("clawback", {"hotkey": hotkey, "units": units, "at": at})

    def get_weights(self, epoch: int, epoch_at: int, computed_at: int) -> bytes:
        """First answer per epoch is final: later calls return the stored bytes."""
        _check_int("epoch", epoch)
        _check_int("epoch_at", epoch_at)
        _check_int("computed_at", computed_at)
        if epoch in self._engine.answers:
            return canonical(self._engine.answers[epoch]["answer"])
        answer, payments = self._engine.compute(epoch, epoch_at, computed_at)
        rec = self.journal.append("weights", {"answer": answer, "payments": payments})
        self._engine.record_answer(rec)
        return canonical(answer)
