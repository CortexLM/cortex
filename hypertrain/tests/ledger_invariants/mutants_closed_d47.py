"""Mutants that re-open the hole D47 closed (the sanction of a confirmed audit mismatch is applied,
ADR 0011 part 8, decisions 15-18).

GAP-01 was a strict expected failure while the store journaled the sanction as WOULD_APPLY and
never called the hook. The worlds of the lot now pin `enforce_audit_mismatch` themselves
(`support.enforce_mismatch_sanction`), so the pin holds whatever the default of the store is; these
mutants prove it on the merged tree: reverting one behaviour of the fix in a private copy of src
must fail it. A default put back to False is not among them on purpose: the pin does not read it.

Run from hypertrain/: .venv/bin/python scripts/check_ledger_invariant_mutants.py --only <ids>
"""

from __future__ import annotations

STORE = "src/hypertrain/challenge/store.py"

UNCONTESTED_CHEAT = (
    "tests/ledger_invariants/test_gaps_finality_2.py::"
    "test_a_detected_cheat_that_is_not_contested_has_a_consequence_beyond_the_round"
)

MUTANTS = [
    {
        "id": "closed-d47-mismatch-sanction-only-recorded",
        "invariants": ["GAP-01"],
        "path": STORE,
        "old": 'return None if self.enforce_audit_mismatch else "WOULD_APPLY"',
        "new": 'return "WOULD_APPLY"',
        "tests": [UNCONTESTED_CHEAT],
        "why": (
            "A due sanction of a confirmed mismatch is journaled as the one that would apply and "
            "the hook is never called, so the forger keeps an ACTIVE admission, no strike and its "
            "whole bond."
        ),
    },
    {
        "id": "closed-d47-mismatch-never-booked",
        "invariants": ["GAP-01"],
        "path": STORE,
        "old": "            self._book_mismatches_v2(run_id, open_rounds, now)\n",
        "new": "",
        "tests": [UNCONTESTED_CHEAT],
        "why": (
            "The exit reaper no longer books the sanction row of an audit verdict that excluded a "
            "member as UNVERIFIED, so there is nothing for the sweep to apply."
        ),
    },
]
