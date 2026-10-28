"""The pure rules of a mismatch sanction: what a verdict proves, and when the bond burns.

ADR 0011 Part 8, decisions 14 to 17. Everything is counted in verified beacon rounds. The sweeper
of the store feeds these functions from its records and calls the hook; there is no database and
no clock here. The hook is `Admission.seize_bond` (ADR 0010 Part 6), bound to a fake in tests.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Literal, Protocol

SanctionReason = Literal["NO_SHOW", "AUDIT_MISMATCH"]
Grade = Literal["LOCATED", "COMMIT_CONTRADICTS_LEAVES"]
Action = Literal["WAIT", "APPLY", "CANCEL"]

# A referee finding that clears the miner: the independent referee ruled against the auditor, or
# two healthy witnesses showed that the auditor let its turn lapse (DisputesV2.timeout()). A
# closure INFRASTRUCTURE of any cause is not one: with no auditor-side responder every contest
# would end that way, and a contest would become an escape.
EXONERATING = frozenset({"FRAUD", "AUDITOR_FAULT"})


class SanctionHook(Protocol):
    """What the audit side may ask of the admission side. `Admission.seize_bond` satisfies it."""

    def seize_bond(
        self, admission_id: str, *, reason: SanctionReason, evidence_hash: str, now: int
    ) -> int: ...


@dataclass(frozen=True, slots=True)
class DisputeView:
    """What the sanction needs to know about the contest of one verdict."""

    resolved: bool
    reason: str | None = None
    loser: str | None = None


def grade(
    result: str,
    first_bad_leaf: int | None,
    recomputed_leaves_root: str,
    committed_leaves_root: str,
) -> Grade | None:
    """What the verdict proves about the miner, or None when it proves nothing sanctionable.

    Only a MISMATCH is graded: WITHHELD, BAD_PROOF and ASSIGNMENT_VIOLATION carry no leaf and a
    root that says nothing about the commit, and a MATCH proves the opposite. LOCATED: the replay
    names the first published leaf that differs. COMMIT_CONTRADICTS_LEAVES: every published leaf
    equals the replay (so the leaves root does too) yet a signed commit hash does not: the miner's
    artifacts contradict its own leaves, which no environment produces. Anything else (no leaf
    named and the roots differ) is an anomaly of the audit, never the miner's fault.
    """
    if result != "MISMATCH":
        return None
    if first_bad_leaf is not None:
        return "LOCATED"
    if recomputed_leaves_root == committed_leaves_root:
        return "COMMIT_CONTRADICTS_LEAVES"
    return None


def decide(*, now: int, due: int, dispute: DisputeView | None, auditor: str) -> Action:
    """WAIT inside the contest window and while a contest is open; CANCEL as soon as a referee
    finding clears the miner (`auditor` is the lease's); otherwise APPLY. An undecided contest
    postpones the sanction and clears nobody."""
    if dispute is not None:
        if not dispute.resolved:
            return "WAIT"
        if dispute.reason in EXONERATING and dispute.loser == auditor:
            return "CANCEL"
    return "APPLY" if now >= due else "WAIT"


def due_beacon(first_seen: int, dispute_per_level: int) -> int:
    """The contest window is one dispute turn (`verify.T.dispute_per_level`): the time the
    manifest already gives a party to make one move. No constant is added."""
    return first_seen + dispute_per_level
