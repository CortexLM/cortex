"""Mutants of the lockless burn of a convicted coldkey's unvested rewards (D82, ADR 0010 Part 6).

One small regression of src/hypertrain per defence of `EscrowV2.burn_pending` and of its call in
`Admission.seize_bond`. The tests are the two lot pins this change flipped (DIB-07 / DIB-17 and
CRD-22) and the tests written with the operation, which live outside this directory.

One mutant is not here on purpose: dropping `and not burned` from the call makes the burn
unconditional. The balances are the same, since the slash of a lock has burned the rewards before
the call looks for any, so no test can tell them apart. The guard is there for the
`burn_pending=False` mutants of the other tables, which keep the slash's own burn tested and could
not be killed if the new call burned the rewards behind them.

Run from hypertrain/: .venv/bin/python scripts/check_ledger_invariant_mutants.py --only <ids>
"""

from __future__ import annotations

LEDGER = "src/hypertrain/ledger/escrow_v2.py"
ADMISSION = "src/hypertrain/challenge/admission.py"

LEDGER_TESTS = "tests/ledger/test_admission_bond_v2.py::"
BOND_TESTS = "tests/challenge/test_admission_bond_v2.py::"
HARDENING = "tests/challenge/test_admission_hardening_v2.py::"

BURNS_AND_REPLAYS = (
    LEDGER_TESTS + "test_burn_pending_burns_the_named_origin_of_its_owner_and_nothing_else"
)
BURNS_ONLY_THE_OWNERS_PENDING = (
    LEDGER_TESTS + "test_burn_pending_burns_nothing_that_is_not_the_named_owners_pending_units"
)
AUTHORITY_AND_KEY = LEDGER_TESTS + "test_burn_pending_needs_the_reward_authority_and_the_id_key"
CONFLICT = LEDGER_TESTS + "test_a_burn_of_an_origin_for_another_owner_is_a_conflict_not_a_replay"
NO_BOND_LEFT = (
    BOND_TESTS
    + "test_a_verdict_against_the_miner_burns_its_coldkeys_unvested_rewards_with_no_bond_to_slash"
)
ROUTES = (
    HARDENING
    + "test_a_verdict_after_a_bond_only_seizure_burns_the_unvested_reward_once_through_the_routes"
)
AUDIT_MISMATCH = (
    BOND_TESTS
    + "test_seize_bond_for_a_confirmed_audit_mismatch_suspends_strikes_and_burns_the_bond"
)
NO_SHOW = (
    BOND_TESTS
    + "test_seize_bond_puts_a_first_no_show_on_probation_with_a_strike_and_burns_the_bond"
)
REPOSTED = (
    BOND_TESTS + "test_a_verdict_posted_again_burns_the_origin_the_coldkey_gained_unvested_since"
)
LOT_DISPUTES = (
    "tests/ledger_invariants/test_gaps_disputes.py::"
    "test_only_the_origin_of_the_disputed_miner_is_held_and_nothing_is_paid_for_it"
)
LOT_LOCKS = (
    "tests/ledger_invariants/test_locks.py::"
    "test_fraud_proven_after_the_bond_was_seized_still_burns_the_unvested_reward"
)

CALL = '            if reason == "TRAINING_FRAUD" and not burned:\n'
AUTHORITY = (
    "        if authority != self.policy.reward_authority:\n"
    '            raise EscrowError("BURN_AUTHORITY")\n'
)
SELECT = (
    '                "SELECT units FROM escrow_units WHERE origin=? AND owner=? "\n'
    "                \"AND bucket='reward_pending' AND units>0\",\n"
)
EMPTIED_ROW = (
    "            # The emptied row stays: `payable` reads it as the mark of a live reward origin.\n"
    "            self.db.execute(\n"
    '                "UPDATE escrow_units SET units=0 WHERE origin=? AND owner=? "\n'
    "                \"AND bucket='reward_pending'\",\n"
    "                (origin, owner),\n"
    "            )\n"
)
PENDING_ORIGINS = '                "AND units>0 ORDER BY origin",\n'
HOTKEYLESS_SKIPPED = (
    '                "AND units>0 AND origin IN (SELECT origin FROM escrow_origins "\n'
    '                "WHERE hotkey IS NOT NULL) ORDER BY origin",\n'
)
REPLAY = (
    "            old = self._existing(operation, request, server=True)\n"
    "            if old is not None:\n"
    "                return old\n"
    "            row = self.db.execute(\n"
    '                "SELECT units FROM escrow_units WHERE origin=? AND owner=? "\n'
)
CREDIT = (
    '            self._credit(origin, owner, "burned", "", row["units"])\n'
    '            return self._event(operation, "BURN_PENDING"'
)

MUTANTS = [
    {
        "id": "burn-pending-never-called",
        "invariants": ["DIB-07", "DIB-17", "CRD-22"],
        "path": ADMISSION,
        "old": CALL,
        "new": "            if False:\n",
        "tests": [LOT_DISPUTES, LOT_LOCKS, NO_BOND_LEFT, ROUTES],
        "why": "a conviction after a bond-only seizure bans the miner but its origin still vests.",
    },
    {
        "id": "burn-pending-on-any-sanction",
        "invariants": ["DIB-07", "DIB-17"],
        "path": ADMISSION,
        "old": CALL,
        "new": "            if not burned:\n",
        "tests": [AUDIT_MISMATCH, NO_SHOW],
        "why": "an audit mismatch or a no-show on an emptied bond burns the unvested rewards too.",
    },
    {
        "id": "burn-pending-authority-unchecked",
        "invariants": ["DIB-07", "DIB-17"],
        "path": LEDGER,
        "old": AUTHORITY,
        "new": "",
        "tests": [AUTHORITY_AND_KEY],
        "why": "any caller of the ledger burns a coldkey's unvested rewards.",
    },
    {
        "id": "burn-pending-replay-not-recognised",
        "invariants": ["DIB-07", "DIB-17"],
        "path": LEDGER,
        "old": REPLAY,
        "new": (
            "            old = None\n"
            "            row = self.db.execute(\n"
            '                "SELECT units FROM escrow_units WHERE origin=? AND owner=? "\n'
        ),
        "tests": [BURNS_AND_REPLAYS, CONFLICT],
        "why": "a retried conviction finds no event for its origin and answers None.",
    },
    {
        "id": "burn-pending-owner-not-in-the-request",
        "invariants": ["DIB-07", "DIB-17"],
        "path": LEDGER,
        "old": 'canonicalize({"origin": origin, "owner": owner})',
        "new": 'canonicalize({"origin": origin})',
        "tests": [CONFLICT],
        "why": "the origin burned again for another owner is answered with the first receipt.",
    },
    {
        "id": "burn-pending-credits-available",
        "invariants": ["DIB-07", "DIB-17", "CRD-22"],
        "path": LEDGER,
        "old": CREDIT,
        "new": CREDIT.replace('"burned"', '"available"'),
        "tests": [BURNS_AND_REPLAYS, NO_BOND_LEFT, ROUTES, LOT_DISPUTES],
        "why": "the unvested units of a convicted miner are vested instead of burned.",
    },
    {
        "id": "burn-pending-client-key-space",
        "invariants": ["DIB-07", "DIB-17"],
        "path": LEDGER,
        "old": 'return self._event(operation, "BURN_PENDING", request, owner, server=True)',
        "new": 'return self._event(operation, "BURN_PENDING", request, owner, server=False)',
        "tests": [BURNS_AND_REPLAYS],
        "why": "the event is stored under the bare id a client can squat (ADR 0014 Part 1).",
    },
    {
        "id": "burn-pending-event-for-nothing",
        "invariants": ["DIB-07", "DIB-17"],
        "path": LEDGER,
        "old": SELECT,
        "new": SELECT.replace(" AND units>0", ""),
        "tests": [BURNS_ONLY_THE_OWNERS_PENDING],
        "why": "a vested origin, whose pending row is empty, still gets a BURN_PENDING event.",
    },
    {
        "id": "burn-pending-takes-vested-units",
        "invariants": ["DIB-07", "DIB-17"],
        "path": LEDGER,
        "old": SELECT,
        "new": SELECT.replace(
            "bucket='reward_pending'", "bucket IN ('reward_pending','available')"
        ),
        "tests": [BURNS_ONLY_THE_OWNERS_PENDING],
        "why": "the burn reaches the units a miner already vested.",
    },
    {
        "id": "burn-pending-takes-another-owners-units",
        "invariants": ["DIB-07", "DIB-17"],
        "path": LEDGER,
        "old": SELECT,
        "new": SELECT.replace("AND owner=? ", "AND (owner=? OR 1) "),
        "tests": [BURNS_ONLY_THE_OWNERS_PENDING],
        "why": "the burn reaches the pending units of an owner that is not the convicted one.",
    },
    {
        "id": "burn-pending-not-after-the-ban",
        "invariants": ["DIB-07", "DIB-17"],
        "path": ADMISSION,
        "old": CALL,
        "new": (
            '            if reason == "TRAINING_FRAUD" and not burned and r.state != "BANNED":\n'
        ),
        "tests": [REPOSTED],
        "why": "a resolution posted again leaves the reward the coldkey gained since the ban.",
    },
    {
        "id": "burn-pending-skips-hotkeyless-origins",
        "invariants": ["DIB-07", "DIB-17"],
        "path": LEDGER,
        "old": PENDING_ORIGINS,
        "new": HOTKEYLESS_SKIPPED,
        "tests": [NO_BOND_LEFT],
        "why": "the genesis or bootstrap collateral of a convicted coldkey vests, unburned.",
    },
    {
        "id": "burn-pending-deletes-the-emptied-row",
        "invariants": ["DIB-07", "DIB-17"],
        "path": LEDGER,
        "old": EMPTIED_ROW,
        "new": EMPTIED_ROW.replace("UPDATE escrow_units SET units=0", "DELETE FROM escrow_units"),
        "tests": [BURNS_AND_REPLAYS, NO_BOND_LEFT],
        "why": "the origin loses the pending row that marks it a live reward origin for payable().",
    },
]
