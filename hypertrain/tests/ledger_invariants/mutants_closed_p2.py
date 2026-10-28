"""Mutants that re-open the hole F4(iii) closed (a lock id cannot borrow another object's
settlement, ADR 0014 part 2, fixes 42851f0e and 999ac061): each one brings back, in one place, the
lookup that took a lock, a dispute and a reward origin for the same string.

The pins were strict expected failures until the fix landed and now hold the fix. These mutants
prove it on the merged tree: reverting one behaviour of the fix in a private copy of src must fail
them. The fix is a defence in depth, which limits what one mutant can show.

POST /dispute refuses a bond named like a dispute or a reward origin (CONTEST_LOCK_ID_COLLISION),
and behind that refusal the typed lookups settle a bond only through the dispute that took it. The
pins of test_opids.py hold when either layer holds: a dispute refused at creation must be refused
by that collision, and a dispute that opens must still not take the settlement of the id it
borrowed. Reverting the check alone therefore leaves the two closed-dispute pins green, as does
reverting one lookup alone.

Measured on the merged tree, each mutant of this table applied alone against the two closed-dispute
pins, [origin-first] and [bond-first] of the origin pin, all run without -x:
- the collision check (closed-p2-open-accepts-a-bond-named-like-a-dispute-or-an-origin) fails
  [origin-first] only, because the dispute then opens and the lookups behind the check hold, so
  closed-p2-open-accepts-a-bond-named-like-a-reward-origin, which reverts its origin half and
  fails that variant only, is the mutant of that pin;
- the store lookup fails [bond-first] only, which opens the dispute while the name is free;
- the store lookup and the check together fail both origin variants and neither closed-dispute pin;
- every other mutant leaves all four green, except the one below.

`closed-p2-contest-bond-settled-by-its-name` fails the two closed-dispute pins, and does so through
the control step of their helper (settle the first dispute's own bond: "its own life works"), not
through the borrowed id: asked by its own name, the bond of an ordinary dispute finds no settlement.
It shows that those pins exercise the typed lookup of contest bonds; it does not show a re-opened
borrow. They reach the borrowed id only when five snippets of three files are reverted together,
which the runner (one snippet per mutant) cannot express: the store lookup, the by-name settlement
of contest bonds, the dispute-side lookup, the collision check and the reference check of
lock_status. With the first four, only the settlement one of the two pins fails; with the fifth
too, both do, at the release of the borrowed bond and at the settlement of the second dispute. The
borrow itself is shown by `closed-p2-contest-bond-not-bound-to-the-dispute-that-took-it`, killed by
the typing test of a contest lock that no dispute took and that a dispute named like it must not
release.

The other mutants are killed by the tests of the fix (tests/challenge/test_settlement_typing_v2.py).
The escrow, dispute and lock test files of the lot (test_escrow_laws*, test_gaps_disputes,
test_gaps_escrow, test_locks, test_disputes, test_exit_disputes) survive the mutants of the guard,
the contest and admission resolvers, the unknown lock and the dispute-side lookup, so the lot holds
no pin on those behaviours; they are here so that each behaviour of the fix has a mutant on the
merged tree.

Run from hypertrain/: .venv/bin/python scripts/check_ledger_invariant_mutants.py --only <ids>
"""

from __future__ import annotations

LEDGER = "src/hypertrain/ledger/escrow_v2.py"
DISPUTES = "src/hypertrain/challenge/disputes_v2.py"
STORE = "src/hypertrain/challenge/store.py"

OPIDS = "tests/ledger_invariants/test_opids.py::"
TYPING = "tests/challenge/test_settlement_typing_v2.py::"

CLOSED_RELEASE = OPIDS + (
    "test_a_bond_borrowing_a_closed_disputes_id_cannot_be_released_while_its_dispute_is_open"
)
CLOSED_SETTLE = OPIDS + (
    "test_a_dispute_whose_bond_borrows_a_closed_disputes_id_can_still_be_settled"
)
ORIGIN_VESTS = OPIDS + (
    "test_a_bond_borrowing_a_reward_origins_id_cannot_keep_that_origin_from_vesting"
)

NAMED_AFTER_AN_ORIGIN = (
    TYPING + "test_a_bond_named_after_a_later_reward_origin_does_not_stop_the_origin_maturing"
)
NAMED_AFTER_A_DISPUTE = (
    TYPING + "test_a_bond_named_after_a_later_dispute_does_not_answer_for_that_dispute"
)
NO_DISPUTE_TOOK_IT = (
    TYPING + "test_a_contest_lock_that_no_dispute_took_is_not_released_by_a_dispute_named_like_it"
)
OTHER_DISPUTE = TYPING + "test_contest_lock_is_not_settled_by_the_settlement_of_another_dispute"
NO_CONTEST_RESOLVER = TYPING + "test_contest_lock_has_no_settlement_without_a_contest_resolver"
NO_ADMISSION_RESOLVER = (
    TYPING + "test_an_admission_lock_has_no_settlement_without_an_admission_resolver"
)
UNKNOWN_LOCK = TYPING + "test_an_unknown_lock_is_refused_before_any_settlement_is_asked"
OPEN_REFUSES = TYPING + "test_open_refuses_a_bond_whose_name_is_already_a_dispute_or_an_origin"

MUTANTS = [
    {
        "id": "closed-p2-origin-settlement-follows-the-bond-name",
        "invariants": ["LCK-08", "DIB-17", "DIA-11"],
        "path": STORE,
        "old": (
            "        if (\n"
            "            disputes is not None\n"
            '            and self._db.execute("SELECT 1 FROM disputes_v2 WHERE id=?", (reference,))'
            ".fetchone()\n"
            "        ):\n"
            "            return disputes[2].settlement(reference)\n"
        ),
        "new": (
            "        found = (\n"
            "            None\n"
            "            if disputes is None\n"
            "            else self._db.execute(\n"
            '                "SELECT id FROM disputes_v2 WHERE id=? OR json_extract(turn,'
            "'$.lock_id')=?\",\n"
            "                (reference, reference),\n"
            "            ).fetchone()\n"
            "        )\n"
            "        if found:\n"
            "            return disputes[2].settlement(found[0])\n"
        ),
        "tests": [ORIGIN_VESTS + "[bond-first]", NAMED_AFTER_AN_ORIGIN],
        "why": (
            "the settlement of a reward origin is the one of the open dispute whose bond is named"
            " like the origin, so the origin is held as disputed and cannot vest."
        ),
    },
    {
        "id": "closed-p2-contest-bond-settled-by-its-name",
        "invariants": ["LCK-08", "DIB-17", "DIA-11"],
        "path": LEDGER,
        "old": "        status = self.contest_settlement(lock)\n",
        "new": "        status = self.settlement(lock_id)\n",
        "tests": [CLOSED_RELEASE, CLOSED_SETTLE],
        "why": (
            "a contest bond is settled by asking the callback for a dispute or an origin under the"
            " bond's own name, so the bond of an ordinary dispute finds no settlement, and one"
            " named like a closed dispute would take that dispute's."
        ),
    },
    {
        "id": "closed-p2-contest-bond-not-bound-to-the-dispute-that-took-it",
        "invariants": ["LCK-08", "DIB-17", "DIA-11"],
        "path": DISPUTES,
        "old": (
            "        if row is None or Turn.model_validate_json(row[0]).lock_id"
            " != lock.operation_id:\n"
        ),
        "new": "        if row is None:\n",
        "tests": [NO_DISPUTE_TOOK_IT],
        "why": (
            "the dispute named by a lock's own dispute id settles the lock although the dispute"
            " never took that lock as its bond."
        ),
    },
    {
        "id": "closed-p2-contest-bond-reference-not-compared",
        "invariants": ["LCK-08", "DIA-11"],
        "path": LEDGER,
        "old": (
            "        if status.dispute_id != lock.dispute_id:\n"
            '            raise EscrowError("LOCK_REFERENCE")\n'
        ),
        "new": "",
        "tests": [OTHER_DISPUTE + "[MATCH]"],
        "why": (
            "the escrow accepts the settlement of a dispute other than the one its contest lock"
            " names, so a bond is released or slashed on the verdict of another dispute."
        ),
    },
    {
        "id": "closed-p2-contest-bond-without-a-resolver-uses-its-name",
        "invariants": ["LCK-08", "DIB-17"],
        "path": LEDGER,
        "old": (
            "        if self.contest_settlement is None:\n"
            '            raise EscrowError("UNACCEPTED_SETTLEMENT")\n'
        ),
        "new": (
            "        if self.contest_settlement is None:\n"
            "            return self.settlement(lock_id)\n"
        ),
        "tests": [NO_CONTEST_RESOLVER],
        "why": (
            "a ledger without a resolver of contest bonds asks the untyped settlement callback by"
            " the name of the lock instead of refusing, so whatever it answers for that name"
            " settles the bond."
        ),
    },
    {
        "id": "closed-p2-dispute-settlement-follows-the-bond-name",
        "invariants": ["LCK-08", "DIB-17", "DIA-11"],
        "path": DISPUTES,
        "old": (
            '        """Settlement of one dispute, found by its id and nothing else."""\n'
            "        turn = self.get(dispute_id)\n"
        ),
        "new": (
            '        """Settlement of one dispute, found by its id and nothing else."""\n'
            "        found = self.db.execute(\n"
            '            "SELECT turn FROM disputes_v2 WHERE id=? OR json_extract(turn,'
            "'$.lock_id')=?\",\n"
            "            (dispute_id, dispute_id),\n"
            "        ).fetchone()\n"
            "        if found is None:\n"
            '            raise DisputeError("UNKNOWN_DISPUTE")\n'
            "        turn = Turn.model_validate_json(found[0])\n"
        ),
        "tests": [NAMED_AFTER_A_DISPUTE],
        "why": (
            "the settlement of a dispute is found by its id or by the bond named like it, so a"
            " dispute whose id is the name of an older dispute's bond answers with the older row."
        ),
    },
    {
        "id": "closed-p2-open-accepts-a-bond-named-like-a-dispute-or-an-origin",
        "invariants": ["LCK-08", "DIB-17", "DIA-11"],
        "path": DISPUTES,
        "old": (
            "            if self.db.execute(\n"
            '                "SELECT 1 FROM disputes_v2 WHERE id=? UNION ALL "\n'
            '                "SELECT 1 FROM escrow_origins WHERE origin=?",\n'
            "                (model.bond_lock, model.bond_lock),\n"
            "            ).fetchone():\n"
            '                raise DisputeError("CONTEST_LOCK_ID_COLLISION")\n'
        ),
        "new": "",
        "tests": [OPEN_REFUSES + "[dispute]", OPEN_REFUSES + "[origin]"],
        "why": (
            "a dispute opens with a bond whose name is already a dispute or a reward origin, so"
            " one string again carries two meanings."
        ),
    },
    {
        "id": "closed-p2-open-accepts-a-bond-named-like-a-reward-origin",
        "invariants": ["LCK-08", "DIB-17", "DIA-11"],
        "path": DISPUTES,
        "old": (
            '                "SELECT 1 FROM disputes_v2 WHERE id=? UNION ALL "\n'
            '                "SELECT 1 FROM escrow_origins WHERE origin=?",\n'
            "                (model.bond_lock, model.bond_lock),\n"
        ),
        "new": (
            '                "SELECT 1 FROM disputes_v2 WHERE id=?",\n'
            "                (model.bond_lock,),\n"
        ),
        "tests": [ORIGIN_VESTS + "[origin-first]"],
        "why": (
            "the collision check looks only at disputes, so a bond named like a reward origin that"
            " already exists is accepted and the dispute opens with it."
        ),
    },
    {
        "id": "closed-p2-admission-lock-without-a-resolver-uses-its-name",
        "invariants": ["LCK-08", "DIB-17"],
        "path": LEDGER,
        "old": (
            "            if self.admission_settlement is None:\n"
            '                raise EscrowError("UNACCEPTED_SETTLEMENT")\n'
        ),
        "new": (
            "            if self.admission_settlement is None:\n"
            "                return self.settlement(lock_id)\n"
        ),
        "tests": [NO_ADMISSION_RESOLVER],
        "why": (
            "a ledger without a resolver of admission locks falls back to the settlement callback"
            " by the name of the lock, which finds a closed dispute or an origin of that name."
        ),
    },
    {
        "id": "closed-p2-unknown-lock-asks-the-settlement",
        "invariants": ["LCK-08", "DIB-17"],
        "path": LEDGER,
        "old": (
            "        if row is None:\n"
            '            raise EscrowError("UNKNOWN_LOCK")\n'
            '        if row["kind"] == "LOCK_ADMISSION":\n'
        ),
        "new": (
            "        if row is None:\n"
            "            return self.settlement(lock_id)\n"
            '        if row["kind"] == "LOCK_ADMISSION":\n'
        ),
        "tests": [UNKNOWN_LOCK],
        "why": (
            "a release or a slash that names no lock asks the settlement callback for that name,"
            " which answers for a dispute or an origin that is not a lock."
        ),
    },
]
