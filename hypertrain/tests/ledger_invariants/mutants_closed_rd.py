"""Mutants that re-open the holes the R-D and ledger-refusals fixes closed.

The pins listed here were strict xfails until the fixes landed (78154aaa and f8d91302: a round
finality closed without a reward is recorded and never paid; 81a1903c: a refused round answers
409). Each mutant reverts one behaviour of a fix in a private copy of src/hypertrain, so that a
pin is proven to fail on the code the fix replaced and not only to pass on the merged tree.

The InsufficientEligibleWeight to 409 ROUND_INFEASIBLE mapping of 81a1903c (a roster of three at
the opening of a round) has no pin in the lot and no mutant here: the refused opening of the
finality pin is the FinalityError path (round w+2 blocked). Leaving the class out of the handler of
app.py survives test_gaps_finality, test_gaps_finality_2, test_gaps_http, test_audit_2,
test_exit_roster and test_exit_roster_2 (measured); tests/challenge/test_round_refusals_v2.py fails
on it.

Run from hypertrain/: .venv/bin/python scripts/check_ledger_invariant_mutants.py --only <ids>
"""

from __future__ import annotations

LEDGER = "src/hypertrain/ledger/escrow_v2.py"
STORE = "src/hypertrain/challenge/store.py"
APP = "src/hypertrain/challenge/app.py"

DISPUTES = "tests/ledger_invariants/test_gaps_disputes.py::"
LEDGER_TESTS = "tests/ledger_invariants/test_gaps_ledger.py::"
FINALITY = "tests/ledger_invariants/test_gaps_finality_2.py::"
HTTP = "tests/ledger_invariants/test_gaps_http.py::"

CREDITED_AFTER_ROLLBACK = DISPUTES + "test_a_round_opened_after_a_two_round_rollback_is_credited"
CREDITED_THROUGH_ROUTE = (
    LEDGER_TESTS + "test_the_first_round_after_a_rollback_is_credited_through_the_finalize_route"
)
MINTED_AFTER_REPAIRS = (
    FINALITY + "test_the_round_after_two_repaired_rounds_is_minted_and_the_repaired_ones_never_are"
)
REFUSAL_REASON = (
    FINALITY + "test_a_refused_finalize_and_a_refused_opening_answer_409_with_their_reason"
)
REFUSED_AGGREGATION = (
    HTTP + "test_an_aggregation_refused_by_tape_or_finality_rules_is_a_4xx_with_its_reason"
)
# A pin that the base tests already hold (PIN_EXISTING): no lot test reaches the ledger's own
# refusal to pay a closed round, because finalize_v2 turns such a round away first.
BASE_WITHDRAWAL = "tests/challenge/test_round_withdrawal_v2.py::"
CLOSED_ROUND_UNPAID = BASE_WITHDRAWAL + "test_the_evidence_of_a_closed_round_is_never_paid"

MUTANTS = [
    {
        "id": "closed-rd-order-is-max-plus-one-again",
        "invariants": ["CRD-04", "DIB-21"],
        "path": LEDGER,
        "old": "expected = self._first_open_round(below=reward.w)",
        "new": (
            'last = self.db.execute("SELECT MAX(w) FROM escrow_finalized").fetchone()[0]\n'
            "                    expected = last + 1 if last is not None else 0"
        ),
        "tests": [CREDITED_AFTER_ROLLBACK],
        "why": (
            "the next round must follow the highest rewarded one again, so rounds 0 and 1 that a"
            " rollback closed without a reward are never accounted for and round 2 is refused"
            " FINALIZE_ORDER for ever."
        ),
    },
    {
        "id": "closed-rd-closure-callback-is-not-wired",
        "invariants": ["CRD-04", "CL-04"],
        "path": STORE,
        "old": "round_closure=lambda w: self._round_closure_v2(run_id, w),",
        "new": "round_closure=None,",
        "tests": [CREDITED_THROUGH_ROUTE],
        "why": (
            "the run's ledger is built without finality's answer, so no closed round is ever"
            " recorded as withdrawn and the first round after a rollback is refused FINALIZE_ORDER"
            " through the Finalize route."
        ),
    },
    {
        "id": "closed-rd-closure-ignores-a-completed-rollback",
        "invariants": ["FIN-08", "DIB-21"],
        "path": STORE,
        "old": 'if status.closed and status.disposition in ("EXCLUDED", "RETIRED"):',
        "new": 'if status.closed and status.disposition == "RETIRED":',
        "tests": [MINTED_AFTER_REPAIRS],
        "why": (
            "a round that a completed rollback left EXCLUDED is no longer reported as closed"
            " without a reward, so the round after two repaired rounds is refused FINALIZE_ORDER."
        ),
    },
    {
        "id": "closed-rd-closed-round-is-paid-by-the-ledger",
        "invariants": ["CRD-04"],
        "path": LEDGER,
        "old": "if self.round_closure is not None and self.round_closure(reward.w) is not None:",
        "new": "if False:",
        "tests": [CLOSED_ROUND_UNPAID],
        "why": (
            "the ledger no longer refuses the evidence of a round finality closed without a"
            " reward, so a caller that forgets the upstream check mints that round."
        ),
    },
    {
        "id": "closed-rd-finality-refusal-is-a-bare-500",
        "invariants": ["FIN-26", "CL-48"],
        "path": APP,
        "old": "for refusal in (InsufficientEligibleWeight, FinalityError, TapeError):",
        "new": "for refusal in (InsufficientEligibleWeight, TapeError):",
        "tests": [REFUSED_AGGREGATION + "[finality]"],
        "why": (
            "a FinalityError raised by a round route is no longer mapped, so the operator who"
            " triggers it gets an unhandled 500 with no reason."
        ),
    },
    {
        "id": "closed-rd-tape-refusal-is-a-bare-500",
        "invariants": ["CL-48"],
        "path": APP,
        "old": "for refusal in (InsufficientEligibleWeight, FinalityError, TapeError):",
        "new": "for refusal in (InsufficientEligibleWeight, FinalityError):",
        "tests": [REFUSED_AGGREGATION + "[tape]"],
        "why": (
            "a TapeError raised by the aggregate route is no longer mapped, so a refused"
            " aggregation is an unhandled 500 with no reason."
        ),
    },
    {
        "id": "closed-rd-refusal-drops-its-reason",
        "invariants": ["FIN-26", "D-FIN-10"],
        "path": APP,
        "old": 'return JSONResponse({"detail": f"{code}: {error}"}, status_code=409)',
        "new": 'return JSONResponse({"detail": code}, status_code=409)',
        "tests": [REFUSAL_REASON],
        "why": (
            "a refused round still answers 409 but only with its code, so the operator never"
            " learns that the round cannot settle or that w+2 is blocked."
        ),
    },
]
