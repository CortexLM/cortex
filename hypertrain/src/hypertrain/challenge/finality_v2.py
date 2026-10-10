"""Transaction-local predicates for the two-round speculative window.

L0 persists these records with its opening/apply/settlement CAS. No timers or
elapsed-time assumptions settle an audit. Late finalized fraud is money-only.
"""

from __future__ import annotations

from collections.abc import Sequence
from dataclasses import dataclass, replace
from typing import Literal

from hypertrain.aggregator.core import FinalityError
from hypertrain.protocol.messages import Finalize, Rollback


@dataclass(frozen=True, slots=True)
class RoundStatus:
    run_id: str
    w: int
    applied: bool
    disposition: Literal["PENDING", "SETTLED", "EXCLUDED"]
    unresolved_disputes: int
    unresolved_audits: int
    rollback_complete: bool
    state_hash: str
    resolution_hash: str | None

    @property
    def closed(self) -> bool:
        return (
            self.unresolved_disputes == 0
            and self.unresolved_audits == 0
            and (
                self.disposition == "SETTLED"
                or (self.disposition == "EXCLUDED" and self.rollback_complete)
            )
        )


def require_open(run_id: str, w: int, history: Sequence[RoundStatus]) -> None:
    """Block w+2 opening while w is unresolved, even if it was never applied."""
    if (
        type(w) is not int
        or w < 0
        or any(
            r.run_id != run_id or r.w < 0 or min(r.unresolved_audits, r.unresolved_disputes) < 0
            for r in history
        )
        or len({r.w for r in history}) != len(history)
    ):
        raise FinalityError("invalid finality history")
    ordered = sorted(history, key=lambda r: r.w)
    if [r.w for r in ordered] != list(range(w)):
        raise FinalityError("opening requires complete sequential history")
    if any(not r.closed for r in ordered if r.w <= w - 2):
        raise FinalityError("w+2 blocked until settlement or exclusion plus rollback")


def require_apply(run_id: str, w: int, history: Sequence[RoundStatus]) -> None:
    require_open(run_id, w, history)
    if w and not next(r for r in history if r.w == w - 1).applied:
        raise FinalityError("predecessor not applied")
    if sum(r.applied and not r.closed for r in history) >= 2:
        raise FinalityError("two speculative applied rounds already exist")


def settle(status: RoundStatus, final: Finalize, finality_hash: str) -> RoundStatus:
    """L0 calls only after authorized finality and all audit/dispute records close."""
    if (status.w, status.state_hash) != (final.w, final.final_theta_hash_w1) or (
        not status.applied
        or status.unresolved_audits
        or status.unresolved_disputes
        or status.disposition == "EXCLUDED"
    ):
        raise FinalityError("round cannot settle")
    if status.resolution_hash is not None and status.resolution_hash != finality_hash:
        raise FinalityError("conflicting finality")
    return replace(status, disposition="SETTLED", resolution_hash=finality_hash)


def exclude(status: RoundStatus, evidence_hash: str) -> RoundStatus:
    if status.disposition == "SETTLED":
        raise FinalityError("finalized late fraud cannot rewrite model history")
    if status.resolution_hash is not None and status.resolution_hash != evidence_hash:
        raise FinalityError("conflicting exclusion")
    return replace(
        status,
        disposition="EXCLUDED",
        resolution_hash=evidence_hash,
        rollback_complete=not status.applied,
    )


def finish_rollback(
    status: RoundStatus,
    rollback: Rollback,
    replayed_theta_hash: str,
    replayed_outer_hash: str,
) -> RoundStatus:
    """Require replayed two-round repair, not merely an exclusion flag."""
    if status.disposition != "EXCLUDED" or status.w != rollback.w or not status.applied:
        raise FinalityError("rollback has no pending applied exclusion")
    if (
        rollback.recomputed != ["agg_w", "step_w", "agg_w1", "step_w1"]
        or (rollback.new_theta_hash_w2, rollback.new_outer_state_hash)
        != (replayed_theta_hash, replayed_outer_hash)
        or (status.resolution_hash not in rollback.cause_hashes)
    ):
        raise FinalityError("rollback lacks complete authenticated replay")
    return replace(status, rollback_complete=True)
