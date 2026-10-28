"""Mutants that re-open what D59 closed: a convicted coldkey is paid nothing for the round it
cheated in, and a round of convicted inputs settles without a reward (f664f076, 6d935814; ADR 0011
part 8 decision 23, D64-1).

A conviction of training fraud that lands between the aggregation and the finalize used to leave
the convicted coldkey paid for that round. The fix is three guards: `ChallengeStore._unconvicted_v2`
filters what a Finalize may list through `_banned_v2` (which follows the coldkey and the hotkey
history, so the other admissions of a convicted owner are unpaid and not banned); a Finalize that
lists nobody settles the round as CLOSED_UNPAID; and the ledger refuses the work of a convicted
hotkey (BANNED_WORK) as a wall behind the store. The four pins were strict expected failures until
audit-integrity-2 landed. Each mutant reverts one behaviour in a private copy of src, so that the
runner proves a pin fails without it:

- ROS-03 (the four others of a five-member round are paid and the convicted one is not) fails when
  the Finalize pays an input convicted since;
- the FIN-18+D59 coldkey test (a Finalize that lists the convicted miner or its sibling is refused,
  the unconvicted share the whole budget) fails when the ban follows the admission and not the
  coldkey;
- the FIN-18+D59 all-convicted test (a Finalize that lists nobody answers CLOSED_UNPAID and its
  replay answers the same) fails when that Finalize is not a settlement and when its answer is
  not kept;
- the FIN-18+D59 BANNED_WORK test fails when the ledger no longer asks whether the work is
  convicted.

What no pin or mutant covers here: the search of the hotkey history by `_banned_v2` (a conviction
followed by a rotation), the comparison of the coordinator-signed Finalize of the service profile
with the roster less the convicted coldkeys (m3 of 6d935814: the lot has no positive finalize on
that profile), the derivation of the withdrawal of an unpaid round by `_round_closure_v2` (held by
tests/challenge/test_round_withdrawal_v2.py), and the wiring of `banned=self._banned_v2` in the
store, which the filter in front of the ledger makes unreachable from a pin.

Run from hypertrain/: .venv/bin/python scripts/check_ledger_invariant_mutants.py --only <ids>
"""

from __future__ import annotations

STORE = "src/hypertrain/challenge/store.py"
ESCROW = "src/hypertrain/ledger/escrow_v2.py"

F2 = "tests/ledger_invariants/test_finality_2.py::"
COLDKEY = F2 + "test_a_miner_convicted_after_its_round_was_audited_is_never_credited_by_a_finalize"
ALL_CONVICTED = F2 + "test_a_round_whose_inputs_are_all_convicted_settles_without_a_reward"
LEDGER_WALL = F2 + "test_the_ledger_refuses_the_round_of_a_convicted_hotkey_and_mints_nothing"
CONVICTED_NOT_PAID = (
    "tests/ledger_invariants/test_exit_roster_2.py::"
    "test_a_banned_admission_is_not_paid_and_the_others_are"
)

TAPE_INCLUDED = (
    "            included = self._unconvicted_v2(\n"
    '                item["roster"]["hotkey"]\n'
    '                for item in self._record_v2(run_id, "tape-inputs", str(w))["inputs"]\n'
    "            )\n"
)
BANNED_BY_COLDKEY = (
    "                \"SELECT 1 FROM admissions_v2 b WHERE b.state='BANNED' "
    'AND b.coldkey IN "\n'
    '                "(SELECT a.coldkey FROM admissions_v2 a WHERE a.hotkey=? '
    'OR a.admission_id IN "\n'
    '                "(SELECT admission_id FROM admission_history WHERE hotkey=?)) LIMIT 1",\n'
)
BANNED_BY_ADMISSION = (
    "                \"SELECT 1 FROM admissions_v2 WHERE state='BANNED' "
    'AND (hotkey=? OR admission_id IN "\n'
    '                "(SELECT admission_id FROM admission_history WHERE hotkey=?)) LIMIT 1",\n'
)
NOBODY_LISTED = "            if not included:\n"
CLOSED_ANSWER = '                self._put_record_v2(run_id, "finalize-result", str(w), closed)\n'
LEDGER_ASKS = "            if self.banned is not None and self.banned(c.hotkey):\n"

MUTANTS = [
    {
        "id": "closed-d59-finalize-pays-an-input-convicted-since",
        "invariants": ["ROS-03", "FIN-18", "D59"],
        "path": STORE,
        "old": TAPE_INCLUDED,
        "new": (
            "            included = sorted(\n"
            '                item["roster"]["hotkey"]\n'
            '                for item in self._record_v2(run_id, "tape-inputs", str(w))["inputs"]\n'
            "            )\n"
        ),
        "tests": [CONVICTED_NOT_PAID],
        "why": (
            "The Finalize is held to every input of the applied tape again, so the four others of "
            "a round whose fifth member was convicted since cannot be paid unless the convicted "
            "one is listed and paid with them."
        ),
    },
    {
        "id": "closed-d59-the-ban-does-not-follow-the-coldkey",
        "invariants": ["FIN-18", "D59"],
        "path": STORE,
        "old": BANNED_BY_COLDKEY,
        "new": BANNED_BY_ADMISSION,
        "tests": [COLDKEY],
        "why": (
            "A conviction is read per admission, so another admission run by the convicted owner "
            "is listed and paid for the round while the owner's bond and unvested rewards burn."
        ),
    },
    {
        "id": "closed-d59-a-finalize-that-lists-nobody-does-not-settle",
        "invariants": ["FIN-18", "D59"],
        "path": STORE,
        "old": NOBODY_LISTED,
        "new": "            if False:\n",
        "tests": [ALL_CONVICTED],
        "why": (
            "A round whose inputs are all convicted goes on to the ledger with an empty work set, "
            "which the ledger refuses, so the round cannot be finalized and blocks the rounds "
            "after it."
        ),
    },
    {
        "id": "closed-d59-the-answer-of-an-unpaid-round-is-not-kept",
        "invariants": ["FIN-18", "D59"],
        "path": STORE,
        "old": CLOSED_ANSWER,
        "new": "",
        "tests": [ALL_CONVICTED],
        "why": (
            "The CLOSED_UNPAID answer is returned but not recorded, so the identical Finalize "
            "that replays it finds a settled round with no answer to give."
        ),
    },
    {
        "id": "closed-d59-the-ledger-does-not-ask-whether-the-work-is-convicted",
        "invariants": ["FIN-18", "D59"],
        "path": ESCROW,
        "old": LEDGER_ASKS,
        "new": "            if False:\n",
        "tests": [LEDGER_WALL],
        "why": (
            "The ledger mints the work of a convicted hotkey when a Finalize that lists it gets "
            "past the store, so nothing stops the payment of the round it cheated in."
        ),
    },
]
