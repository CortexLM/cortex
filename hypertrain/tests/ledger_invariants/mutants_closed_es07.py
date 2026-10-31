"""Mutants that re-open the hole ES-07 closed (a transfer goes only to a key the ledger already
knows, `UNKNOWN_RECIPIENT`, ADR 0014 part 5.2, lot v3-ledger-small-fixes, D85): each one brings
back, in one place, a transfer to a key nobody can sign for or that no issuance named.

The pin `test_a_transfer_to_a_recipient_that_is_no_decodable_key_is_refused` was a strict expected
failure until the rule landed; it and the two pins written with the rule hold it now. The bucket
model (`test_random_operation_sequences_conserve_units_and_follow_a_bucket_model`), the property
test of tests/audit_cheaters/test_ledger_defences.py and the empty-ledger refusal of
test_escrow_laws.py also fail without the rule; they are not named here because the runner stops
at the first failing test of a mutant.

Run from hypertrain/: .venv/bin/python scripts/check_ledger_invariant_mutants.py --only <ids>
"""

from __future__ import annotations

LEDGER = "src/hypertrain/ledger/escrow_v2.py"

LAWS = "tests/ledger_invariants/test_escrow_laws_2.py::"
UNDECODABLE = LAWS + "test_a_transfer_to_a_recipient_that_is_no_decodable_key_is_refused"
FRESH_KEY = LAWS + "test_a_transfer_goes_only_to_a_key_the_ledger_already_holds"
ISSUED_STRING = (
    LAWS + "test_a_row_held_by_a_string_nobody_can_sign_for_does_not_make_it_a_recipient"
)

MUTANTS = [
    {
        "id": "closed-es07-any-recipient",
        "invariants": ["ES-07"],
        "path": LEDGER,
        "old": (
            "            if not self._known_owner(op.recipient):\n"
            '                raise EscrowError("UNKNOWN_RECIPIENT")\n'
        ),
        "new": "",
        "tests": [UNDECODABLE, FRESH_KEY],
        "why": (
            "Without the rule a transfer credits any SS58-shaped string: units go to an owner "
            "nobody can sign for, or to a fresh key per hop that bypasses a per-key quota."
        ),
    },
    {
        "id": "closed-es07-a-decodable-key-is-enough",
        "invariants": ["ES-07"],
        "path": LEDGER,
        "old": (
            '        row = self.db.execute("SELECT 1 FROM escrow_units WHERE owner=? LIMIT 1", '
            "(key,))\n"
            "        return row.fetchone() is not None\n"
        ),
        "new": "        return True\n",
        "tests": [FRESH_KEY],
        "why": (
            "A recipient that only has to decode lets every hop go to a key no issuance named, "
            "which is what the rule exists to refuse (79 signers accepted once each)."
        ),
    },
    {
        "id": "closed-es07-a-held-row-is-enough",
        "invariants": ["ES-07"],
        "path": LEDGER,
        "old": (
            "        try:\n"
            "            decode_hotkey(key)\n"
            "        except KeyError_:\n"
            "            return False\n"
        ),
        "new": "",
        "tests": [ISSUED_STRING],
        "why": (
            "A recipient that only has to hold a row takes units sent to a string an issuance "
            "named by mistake, where no key will ever sign them out again."
        ),
    },
    {
        "id": "closed-es07-only-units-make-a-key-known",
        "invariants": ["ES-07"],
        "path": LEDGER,
        "old": '"SELECT 1 FROM escrow_units WHERE owner=? LIMIT 1"',
        "new": '"SELECT 1 FROM escrow_units WHERE owner=? AND units>0 LIMIT 1"',
        "tests": [FRESH_KEY],
        "why": (
            "An owner that spent or gave away every unit would stop being a recipient, so an "
            "issued key could no longer be paid back by the owner it paid."
        ),
    },
    {
        "id": "closed-es07-the-rule-before-the-replay",
        "invariants": ["ES-07"],
        "path": LEDGER,
        "old": (
            "            old = self._owner_operation(op, signer, server=False)\n"
            "            if old is not None:\n"
            "                return old\n"
            "            if op.recipient == op.owner:\n"
        ),
        "new": (
            "            if not self._known_owner(op.recipient):\n"
            '                raise EscrowError("UNKNOWN_RECIPIENT")\n'
            "            old = self._owner_operation(op, signer, server=False)\n"
            "            if old is not None:\n"
            "                return old\n"
            "            if op.recipient == op.owner:\n"
        ),
        "tests": [UNDECODABLE],
        "why": (
            "Checked before the replay lookup, the rule refuses the retry of a transfer a chain "
            "recorded before it, so the owner can no longer read its own stored receipt."
        ),
    },
]
