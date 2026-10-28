"""Mutants that re-open the hole F7a closed (admission-lock reads are indexed and bounded, ADR 0014
part 4, fix 8320f71e): each one brings back, in one place, the unbounded read of EscrowV2.locked().

The pin was a strict expected failure until the fix landed and now holds the fix. These mutants
prove it on the merged tree: reverting one behaviour of the fix in a private copy of src must fail
it.

The table covers the indexed read only. The caps and the minimum that 8320f71e added to new
admission locks (LOCK_BELOW_MINIMUM, OPEN_LOCK_CAP and LEDGER_LOCK_CAP, all raised by
require_lock_capacity) are held by tests/ledger/test_escrow_lock_cost_v2.py only: that file fails
when any of the three is removed, and the lock files of the lot stay green when all three are
(measured on test_opids_2, test_locks, test_locks_2, test_escrow_laws, test_escrow_laws_2,
test_admission_http, test_admission_http_2, test_gaps_admission_lock, test_gaps_admission_lock_2,
test_gaps_escrow, test_gaps_escrow_2, test_gaps_http, test_credit and test_credit_2).

Run from hypertrain/: .venv/bin/python scripts/check_ledger_invariant_mutants.py --only <ids>
"""

from __future__ import annotations

LEDGER = "src/hypertrain/ledger/escrow_v2.py"

# The lookup counts SQLite VM instructions (ticks of ten), not seconds: 2 against 80 locks of
# another owner must cost the same within a factor two.
LOOKUP_COST = (
    "tests/ledger_invariants/test_escrow_laws_2.py::"
    "test_the_database_work_of_a_stake_lookup_does_not_grow_with_the_locks_of_other_owners"
)

MUTANTS = [
    {
        "id": "closed-f7a-no-open-lock-index",
        "invariants": ["ESC-23", "F7a"],
        "path": LEDGER,
        "old": (
            "          CREATE INDEX IF NOT EXISTS escrow_open_admission_locks\n"
            "            ON escrow_units(owner, ref) WHERE bucket='admission_locked' AND units>0;\n"
        ),
        "new": "",
        "tests": [LOOKUP_COST],
        "why": (
            "Without the partial index on the open admission locks the lookup walks every unit row "
            "of the ledger, so its database work grows with the locks of other owners."
        ),
    },
    {
        "id": "closed-f7a-open-lock-index-not-keyed-by-owner",
        "invariants": ["ESC-23", "F7a"],
        "path": LEDGER,
        "old": "ON escrow_units(owner, ref) WHERE bucket='admission_locked' AND units>0;",
        "new": "ON escrow_units(ref) WHERE bucket='admission_locked' AND units>0;",
        "tests": [LOOKUP_COST],
        "why": (
            "An index keyed by lock reference alone cannot seek the owner, so the lookup reads "
            "every open lock of every owner and filters afterwards."
        ),
    },
    {
        "id": "closed-f7a-owner-read-from-the-event-json",
        "invariants": ["ESC-23", "F7a"],
        "path": LEDGER,
        "old": "\"WHERE u.bucket='admission_locked' AND u.units>0 AND u.owner=? \"\n",
        "new": (
            "\"WHERE u.bucket='admission_locked' AND u.units>0 \"\n"
            "            \"AND json_extract(e.request,'$.owner')=? \"\n"
        ),
        "tests": [LOOKUP_COST],
        "why": (
            "Comparing the owner of the request instead of the indexed owner column leaves no "
            "constant to seek, so every open lock of the ledger is joined and parsed per lookup."
        ),
    },
    {
        "id": "closed-f7a-lookup-walks-every-event",
        "invariants": ["ESC-23", "F7a"],
        "path": LEDGER,
        "old": '"FROM escrow_units u JOIN escrow_events e "\n',
        "new": '"FROM escrow_events e CROSS JOIN escrow_units u "\n',
        "tests": [LOOKUP_COST],
        "why": (
            "Driving the join from the event log brings back the pre-fix scan of every event of "
            "the ledger, whoever owns the lock, before it looks at one unit row."
        ),
    },
]
