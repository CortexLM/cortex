"""The general exit: the bound of every wait on a peer, and the planner of one round.

ADR 0011, Part 8. Everything is counted in verified beacon rounds. The planner is pure: it reads
a snapshot of one round and answers whom to exclude, whose fault it is, and whether the rest can
still carry the round. The reaper on `push_beacon` applies the answer; this module has no
database and no clock.
"""

from __future__ import annotations

from collections.abc import Callable, Collection, Iterable, Mapping
from dataclasses import dataclass, field, replace
from enum import StrEnum
from typing import Literal

from hypertrain.challenge.sanction_v2 import SanctionReason
from hypertrain.protocol.hashing import sha256_hex

Blame = Literal["MINER", "INFRASTRUCTURE"]
Stage = Literal["commit", "leaves", "delta"]
Retire = Literal["INFEASIBLE", "AUDIT_WINDOW_MISSED"]

# The master refuses its callers once the wall clock is more than this many rounds ahead of the
# last round it holds: the beacon is paused, and a pause is never a miner's silence.
MAX_LAG_ROUNDS = 2


def pause_span(previous: int, emitted: int) -> tuple[int, int] | None:
    """The views the master refused its callers in, shown by one push, or None.

    `previous` is the last round the master held before the push and `emitted` the round the wall
    clock is at. A view more than MAX_LAG_ROUNDS behind the clock is refused for as long as it is
    held, and the clock only moves on: every view from `previous` to `emitted - MAX_LAG_ROUNDS - 1`
    was refused, however fast the pusher then catches up. The views stand for the wall-clock
    moments of the pause, which can only waive a blame, never add one.
    """
    if previous < 1 or emitted - previous <= MAX_LAG_ROUNDS:
        return None
    return previous, emitted - MAX_LAG_ROUNDS - 1


def served(spans: Iterable[tuple[int, int]], low: int, high: int) -> bool:
    """Whether the master served the window [low, high): none of its refused views is in it."""
    return not any(first < high and low <= last for first, last in spans)


def no_show_evidence(run_id: str, w: int, hotkey: str, stage: str) -> str:
    """The evidence hash of an exclusion for silence: the preimage `_put_no_show_v2` signs."""
    return sha256_hex(f"ht-no-show/1|{run_id}|{w}|{hotkey}|{stage}".encode())


class Wait(StrEnum):
    DISPUTE = "dispute"
    COMMIT = "commit"
    LEAVES = "leaves"
    DELTA = "delta"
    AUDIT_LEASE = "audit-lease"
    AUDIT_WINDOW = "audit-window"
    INFEASIBLE = "infeasible"
    ROTATION = "rotation"
    OWN_CONTEST = "own-contest"
    MEMBER_STATE = "member-state"
    FINALIZE_ORDER = "finalize-order"


@dataclass(frozen=True)
class Schedule:
    now: int
    d_commit: int
    d_audit: int
    d_upload: int
    d_final: int
    lease_rounds: int

    @property
    def absolute(self) -> int:
        return min(self.d_final, self.d_audit + 2 * self.lease_rounds)

    @property
    def audits_end(self) -> int:
        """Where the second audit window ends at the latest: past it no job waits on anybody."""
        return self.d_audit + 4 * self.lease_rounds


# The first push at or after the returned round has the exit due. "At the door" is refused when
# asked, so its bound is the round of the question.
BOUNDS: Mapping[Wait, Callable[[Schedule], int]] = {
    Wait.DISPUTE: lambda s: s.audits_end + 1,
    Wait.COMMIT: lambda s: s.d_commit,
    Wait.LEAVES: lambda s: s.absolute,
    Wait.DELTA: lambda s: s.d_upload,
    Wait.AUDIT_LEASE: lambda s: s.audits_end,
    Wait.AUDIT_WINDOW: lambda s: s.absolute - 1,
    Wait.INFEASIBLE: lambda s: s.now,
    Wait.ROTATION: lambda s: s.now,
    Wait.OWN_CONTEST: lambda s: s.now,
    Wait.MEMBER_STATE: lambda s: s.d_final,
    Wait.FINALIZE_ORDER: lambda s: s.now,
}

# (reason, stage) of an exclusion -> the seizure reason of `Admission.seize_bond`.
SANCTION: Mapping[tuple[str, str], SanctionReason] = {
    ("NO_SHOW", "commit"): "NO_SHOW",
    ("NO_SHOW", "leaves"): "NO_SHOW",
    ("NO_SHOW", "delta"): "NO_SHOW",
    ("AUDIT_MISMATCH", "audit"): "AUDIT_MISMATCH",
}


@dataclass(frozen=True)
class Member:
    hotkey: str
    admission_id: str


@dataclass(frozen=True)
class Exclusion:
    hotkey: str
    admission_id: str
    reason: str
    stage: Stage
    blame: Blame
    record: bool = True


@dataclass(frozen=True)
class RoundPlan:
    exclusions: tuple[Exclusion, ...]
    retire: Retire | None
    # The members the round can still count on (`survivors_of`): what a retirement is given.
    survivors: tuple[str, ...] = field(default=(), compare=False)


@dataclass(frozen=True)
class RoundSnapshot:
    w: int
    schedule: Schedule
    roster: tuple[Member, ...]
    committed: frozenset[str]
    excluded: frozenset[str]
    audited: frozenset[str]
    delivered: frozenset[str]
    uploaded: frozenset[str]  # members the master holds a relay receipt or an acceptance for
    applied: bool
    served: bool
    served_upload: bool
    has_leases: bool
    carries: Callable[[Collection[str]], bool]


def classify(
    stage: Stage,
    *,
    served: bool,
    committed_any: bool,
    delivered_any: bool,
    uploaded: bool = False,
) -> Blame:
    """Whose fault a NO_SHOW is. Only a witnessed silence is the miner's; `served` is the pause
    witness of the window the stage waits on (assignment window, upload window). A delta the
    master never received is not the miner's when the master holds a relay receipt or an
    acceptance of the member's upload (`uploaded`): the relay or the storage lost it."""
    if stage == "commit":
        return "MINER" if served and committed_any else "INFRASTRUCTURE"
    if stage == "delta":
        return "MINER" if served and delivered_any and not uploaded else "INFRASTRUCTURE"
    # leaves: only the master lacks them, and it cannot prove that the miner never posted them
    # (a post that failed on the master or storage side leaves no record)
    return "INFRASTRUCTURE"


def survivors_of(
    roster: Collection[str],
    *,
    now: int,
    d_commit: int,
    d_upload: int,
    committed: Collection[str],
    excluded: Collection[str],
    delivered: Collection[str],
) -> list[str]:
    """The members a round can still count on, in roster order: not excluded, a commit from
    `d_commit` on, a delta from `d_upload` on. Every term only shrinks: once delta intake closed, a
    member that committed and delivered nothing is gone for this round whether or not its audit
    has finished. The one definition: the planner decides with it and the retirement checks the
    survivors it is given against it, so that a caller cannot void a round that can still carry."""
    gone = set(excluded)
    if now >= d_commit:
        gone |= {hotkey for hotkey in roster if hotkey not in committed}
    if now >= d_upload:
        gone |= set(committed) - set(delivered)
    return [hotkey for hotkey in roster if hotkey not in gone]


def plan_round(s: RoundSnapshot) -> RoundPlan:
    if s.applied:
        return RoundPlan((), None)
    now, roster = s.schedule.now, s.roster
    out: list[Exclusion] = []
    if now >= s.schedule.d_commit:
        blame = classify(
            "commit",
            served=s.served,
            committed_any=bool(s.committed),
            delivered_any=bool(s.delivered),
        )
        out += [
            Exclusion(m.hotkey, m.admission_id, "NO_SHOW", "commit", blame)
            for m in roster
            if m.hotkey not in s.committed and m.hotkey not in s.excluded
        ]
    if now >= s.schedule.d_upload:
        out += [
            Exclusion(
                m.hotkey,
                m.admission_id,
                "NO_SHOW",
                "delta",
                classify(
                    "delta",
                    served=s.served_upload,
                    committed_any=bool(s.committed),
                    delivered_any=bool(s.delivered),
                    uploaded=m.hotkey in s.uploaded,
                ),
            )
            for m in roster
            if m.hotkey in s.audited and m.hotkey not in s.delivered and m.hotkey not in s.excluded
        ]
    survivors = survivors_of(
        [m.hotkey for m in roster],
        now=now,
        d_commit=s.schedule.d_commit,
        d_upload=s.schedule.d_upload,
        committed=s.committed,
        excluded=s.excluded,
        delivered=s.delivered,
    )
    retire: Retire | None = None
    if now >= s.schedule.d_commit and len(survivors) < len(roster) and not s.carries(survivors):
        retire = "INFEASIBLE"
    elif not s.has_leases and now >= s.schedule.absolute - 1:
        retire = "AUDIT_WINDOW_MISSED"  # schedule_audits_v2 answers 503 from here on
    if retire:  # a round that cannot go on keeps its membership facts: only the blame is booked
        out = [replace(e, record=False) for e in out]
    return RoundPlan(tuple(out), retire, tuple(survivors))
