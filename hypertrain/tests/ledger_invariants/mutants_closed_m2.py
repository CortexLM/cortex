"""Mutants that re-open F3-M2: reward evidence judged at the finalize beacon, not at acceptance.

ADR 0014 part 3 (5bc7524d): EscrowV2 judges the stored commit, verdicts and audit challenge at the
beacon where the service accepted them, so an expiry that lapsed after acceptance cannot block the
reward. The pins below were strict xfails until that fix landed; each mutant puts back, in one
place, the old judgement at the finalize beacon (ledger legs) or the store answer that is not the
acceptance beacon (store legs), so the flipped pins are proven on the merged tree.

Run from hypertrain/: .venv/bin/python scripts/check_ledger_invariant_mutants.py --only <ids>
"""

from __future__ import annotations

ESCROW = "src/hypertrain/ledger/escrow_v2.py"
STORE = "src/hypertrain/challenge/store.py"

LAPSED = "tests/ledger_invariants/test_expiry.py::"
CHALLENGE = "tests/ledger_invariants/test_gaps_finality.py::"
LATE = "tests/ledger_invariants/test_gaps_ledger.py::"

COMMIT_LAPSED = (
    LAPSED + "test_an_expiry_that_lapsed_after_acceptance_cannot_block_the_reward[commit]"
)
VERDICT_LAPSED = (
    LAPSED + "test_an_expiry_that_lapsed_after_acceptance_cannot_block_the_reward[verdict]"
)
CHALLENGE_LAPSED = (
    CHALLENGE + "test_a_challenge_that_lapsed_after_the_audit_cannot_block_the_reward"
)
SHORT_COMMIT_FINALIZES = (
    LAPSED + "test_a_round_whose_only_fault_is_one_short_lived_commit_still_finalizes"
)
LATE_FINALIZE = (
    LATE + "test_a_finalize_after_d_final_settles_the_round_or_retires_it_but_does_not_freeze_it"
)

MUTANTS = [
    {
        "id": "closed-m2-ledger-commit-judged-at-finalize",
        "invariants": ["CRD-15", "CRD-D02"],
        "path": ESCROW,
        "old": 'item.commit.model_dump(mode="json"), self._accepted(item.commit, evidence)',
        "new": 'item.commit.model_dump(mode="json"), evidence.finalized_beacon',
        "tests": [COMMIT_LAPSED],
        "why": (
            "the ledger judges the stored commit at the finalize beacon again, so a commit whose "
            "expiry lapsed after its acceptance is refused as expired and the reward is not issued."
        ),
    },
    {
        "id": "closed-m2-ledger-verdict-judged-at-finalize",
        "invariants": ["A2W-17", "A2W-D10"],
        "path": ESCROW,
        "old": 'e.model_dump(mode="json"), self._accepted(e, evidence)',
        "new": 'e.model_dump(mode="json"), evidence.finalized_beacon',
        "tests": [VERDICT_LAPSED],
        "why": (
            "the ledger judges each stored verdict at the finalize beacon again, so an auditor "
            "verdict whose expiry lapsed after its acceptance freezes the round it audited."
        ),
    },
    {
        "id": "closed-m2-ledger-challenge-judged-at-finalize",
        "invariants": ["A2W-17", "CRD-15", "CL-10"],
        "path": ESCROW,
        "old": ('item.challenge.model_dump(mode="json"), self._accepted(item.challenge, evidence)'),
        "new": 'item.challenge.model_dump(mode="json"), evidence.finalized_beacon',
        "tests": [CHALLENGE_LAPSED, LATE_FINALIZE],
        "why": (
            "the ledger judges the coordinator's audit challenge at the finalize beacon again, so "
            "a challenge that lapsed after the audit it opened blocks the reward of the round."
        ),
    },
    {
        "id": "closed-m2-store-commit-answers-the-finalize-beacon",
        "invariants": ["CRD-15", "CRD-D02"],
        "path": STORE,
        "old": 'return int(row["accepted_beacon"])',
        "new": "return self._now(self._db)",
        "tests": [SHORT_COMMIT_FINALIZES],
        "why": (
            "the store answers the current beacon instead of the accepted_v2 beacon of a stored "
            "commit, so a delayed finalize finds the short-lived commit expired and answers 409."
        ),
    },
    {
        "id": "closed-m2-store-challenge-answers-the-finalize-beacon",
        "invariants": ["CRD-15", "CL-10"],
        "path": STORE,
        "old": 'return int(row["created"])',
        "new": "return self._now(self._db)",
        "tests": [LATE_FINALIZE],
        "why": (
            "the store answers the current beacon instead of the lease beacon of the coordinator's "
            "challenge, so a Finalize one beacon after d_final is refused as expired and the "
            "round stays PENDING."
        ),
    },
]
