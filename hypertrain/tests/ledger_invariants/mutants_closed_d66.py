"""Mutants that re-open what D66 closed: a reward follows its coldkey through a hotkey rotation
(3d6129d3, ADR 0014 part 5 / F6).

A reward belongs to the coldkey; the hotkey is the key that signed the work. Before the fix a
rotation turned the pending rewards of the admission into dead ones, in three places of
`ChallengeStore`: the epoch answer credited the hotkey recorded at issuance (`_payee_v2` now pays
the hotkey the admission holds), the ledger resolved the owner of stored evidence through the
strict intake gate (`_reward_owner_v2` follows the admission history), and the custody of a shadow
reward was revalidated against the hotkey the admission holds now (`_shadow_trial_v2` takes the
hotkey that signed the original commit). The six pins were strict expected failures until it
landed; each mutant reverts one behaviour in a private copy of src, so that the runner proves a
pin fails without it:

- the three payout pins (test_gaps_escrow, test_gaps_admission_lock and test_gaps_ledger, on the
  real finalize path of four miners) fail when the weight is emitted to the hotkey that signed the
  work; each of the three reverts below is one place where that choice is made (the call site, the
  query, the return) and names one pin first;
- the three rotation cases of test_shadow_2 (SHD-21) fail when the ledger resolves the owner of a
  retired hotkey through the strict gate (the reservation case) and when the revalidation of the
  shadow custody expects the current hotkey of the admission in the commit or in the Finalize it
  replays. The last two fail the three cases together, as does every other use of the signer in
  `_shadow_trial_v2` (checked by hand without -x on the intake of the commit and of the work
  proof, the binding and the probe of the retained commit): the runner stops at the first failure,
  so one case is named per mutant.

What no pin or mutant covers here: the ban of a coldkey that rotated (`_payee_v2` pays nothing to
a banned coldkey whatever hotkey it holds), the payment of an origin that no admission knows to its
own hotkey, and the fall back of `_reward_owner_v2` to the strict gate for a hotkey without an
admission, which tests/challenge/test_v2_weights_pay.py and test_beacon_maturity_v2.py hold.

Run from hypertrain/: .venv/bin/python scripts/check_ledger_invariant_mutants.py --only <ids>
"""

from __future__ import annotations

STORE = "src/hypertrain/challenge/store.py"

PAYOUT_ESCROW = (
    "tests/ledger_invariants/test_gaps_escrow.py::"
    "test_matured_units_of_a_rotated_admission_are_emitted_to_its_current_hotkey"
)
PAYOUT_LOCK = (
    "tests/ledger_invariants/test_gaps_admission_lock.py::"
    "test_matured_reward_units_are_emitted_to_the_hotkey_that_holds_the_admission"
)
PAYOUT_FINALIZE = (
    "tests/ledger_invariants/test_gaps_ledger.py::"
    "test_the_units_of_a_rotated_miner_are_emitted_to_the_hotkey_its_admission_holds_now"
)
SHADOW = "tests/ledger_invariants/test_shadow_2.py::"
RESERVED = SHADOW + "test_a_reservation_made_before_a_hotkey_rotation_can_still_be_credited[True]"
RECOVERED = SHADOW + "test_a_lost_response_is_recovered_after_a_hotkey_rotation[True]"
MATURES = SHADOW + "test_a_credit_still_matures_after_a_hotkey_rotation[True]"

MUTANTS = [
    {
        "id": "closed-d66-the-weight-goes-to-the-hotkey-that-signed-the-work",
        "invariants": ["CRD-12", "WGT-13", "CL-29"],
        "path": STORE,
        "old": '                payee = self._payee_v2(escrow.run_id, row["hotkey"])\n',
        "new": '                payee = row["hotkey"]\n',
        "tests": [PAYOUT_ESCROW],
        "why": (
            "The epoch answer names the hotkey recorded when the origin was issued, so after a "
            "rotation the weight goes to the key the coldkey gave up."
        ),
    },
    {
        "id": "closed-d66-the-payee-query-answers-the-signing-hotkey",
        "invariants": ["CRD-12", "WGT-13", "CL-29"],
        "path": STORE,
        "old": (
            '            "SELECT a.hotkey, EXISTS('
            'SELECT 1 FROM admissions_v2 b WHERE b.run_id=a.run_id "\n'
        ),
        "new": (
            '            "SELECT h.hotkey, EXISTS('
            'SELECT 1 FROM admissions_v2 b WHERE b.run_id=a.run_id "\n'
        ),
        "tests": [PAYOUT_LOCK],
        "why": (
            "The payee lookup finds the admission but reads the hotkey of the history row it "
            "joined, the one that signed, so a rotated admission is paid on its old hotkey."
        ),
    },
    {
        "id": "closed-d66-a-rotated-payee-falls-back-to-the-signing-hotkey",
        "invariants": ["CRD-12", "WGT-13", "CL-29"],
        "path": STORE,
        "old": '        return None if row["banned"] else str(row["hotkey"])\n',
        "new": '        return None if row["banned"] else hotkey\n',
        "tests": [PAYOUT_FINALIZE],
        "why": (
            "The payee of an unbanned coldkey is the hotkey asked about and not the one its "
            "admission holds, so the quarter of the rotated miner reaches the retired key."
        ),
    },
    {
        "id": "closed-d66-the-ledger-resolves-owners-through-the-strict-intake-gate",
        "invariants": ["SHD-21", "SHD-D4"],
        "path": STORE,
        "old": "        if row is None:\n            return self._owner_v2(run_id, hotkey)",
        "new": "        if True:\n            return self._owner_v2(run_id, hotkey)",
        "tests": [RESERVED],
        "why": (
            "The ledger asks the owner of stored evidence from the gate that only knows the "
            "current hotkey, so the evidence signed by a hotkey the admission has moved off is "
            "refused and the reservation cannot be credited."
        ),
    },
    {
        "id": "closed-d66-shadow-commit-must-be-signed-by-the-current-hotkey",
        "invariants": ["SHD-21", "SHD-D4"],
        "path": STORE,
        "old": "                or commit.hotkey != worker\n",
        "new": '                or commit.hotkey != row["hotkey"]\n',
        "tests": [RECOVERED],
        "why": (
            "The revalidation of the original custody expects the commit to name the hotkey the "
            "admission holds now, so the identical retry of a credit accepted before a rotation "
            "is refused instead of answered."
        ),
    },
    {
        "id": "closed-d66-shadow-finalize-must-list-the-current-hotkey",
        "invariants": ["SHD-21", "SHD-D4"],
        "path": STORE,
        "old": "                or final.included != [worker]\n",
        "new": '                or final.included != [row["hotkey"]]\n',
        "tests": [MATURES],
        "why": (
            "The revalidation of the original custody expects the Finalize to list the hotkey the "
            "admission holds now, so a credit accepted before a rotation never matures."
        ),
    },
    {
        "id": "closed-d66-the-reservation-binds-the-current-hotkey",
        "invariants": ["SHD-21", "SHD-D4"],
        "path": STORE,
        "old": '                hotkey=worker,\n                coldkey=row["coldkey"],\n',
        "new": '                hotkey=row["hotkey"],\n                coldkey=row["coldkey"],\n',
        "tests": [RESERVED],
        "why": (
            "The reservation binding names the hotkey the admission holds now instead of the one "
            "that signed the work, so the binding stored before a rotation no longer matches the "
            "custody it is revalidated against."
        ),
    },
]
