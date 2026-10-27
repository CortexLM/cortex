"""Mutants of the opids package: one small regression of src/hypertrain per pinned defence.

Run from hypertrain/: .venv/bin/python scripts/check_ledger_invariant_mutants.py --only <ids>
"""

from __future__ import annotations

LEDGER = "src/hypertrain/ledger/escrow_v2.py"
STORE = "src/hypertrain/challenge/store.py"
OUTBOX = "src/hypertrain/challenge/admission_store.py"
DISPUTES = "src/hypertrain/challenge/disputes_v2.py"

ENGINE = "tests/ledger_invariants/test_opids.py::"
HTTP = "tests/ledger_invariants/test_opids_2.py::"

PROPERTY = ENGINE + "test_an_operation_id_names_one_body_and_one_movement"
REPLAYS = ENGINE + "test_a_release_and_a_maturity_replay_with_their_first_receipt_after_a_restart"
FAILS = ENGINE + "test_an_operation_that_fails_before_its_event_commits_changes_nothing"
NESTED = ENGINE + "test_a_failed_operation_inside_a_callers_transaction_is_undone_alone"
GUARD = ENGINE + "test_the_engine_refuses_to_be_built_inside_an_open_transaction"
TERMINAL = ENGINE + "test_exactly_one_terminal_transition_applies_to_a_dispute"
CONCURRENT_LOCKS = "tests/ledger/test_escrow_v2.py::test_concurrent_locks_when_same_funded_origin"
SIGNED_TWICE = (
    HTTP + "test_a_signed_operation_sent_twice_is_applied_once_and_another_body_is_refused"
)
REFUSED_LEAVES_NOTHING = (
    HTTP + "test_a_refused_escrow_request_leaves_no_reservation_no_event_and_no_burnt_id"
)
LOCK_ONCE = HTTP + "test_an_admission_lock_is_applied_once_and_a_refused_one_leaves_no_outbox_row"
OUTBOX_ROW = (
    HTTP
    + "test_an_unacknowledged_outbox_row_survives_a_refusal_only_outside_the_services_transaction"
)
SHARED_LOCK = HTTP + "test_the_ledger_waits_for_a_store_transaction_only_while_they_share_the_lock"

MUTANTS = [
    {
        "id": "opids-existing-ignores-the-body",
        "invariants": ["ESC-03"],
        "path": LEDGER,
        "old": '        if row["request"] != request:\n',
        "new": "        if False:\n",
        "tests": [PROPERTY, REPLAYS],
        "why": (
            "a used operation id answers its first receipt to any other body instead of a conflict."
        ),
    },
    {
        "id": "opids-owner-operation-forgets-replays",
        "invariants": ["ESC-03"],
        "path": LEDGER,
        "old": "        return self._existing(op.operation_id, op.model_dump_json())\n",
        "new": "        return None\n",
        "tests": [PROPERTY],
        "why": "an owner operation never looks for its own earlier event, so a replay moves again.",
    },
    {
        "id": "opids-event-without-snapshot",
        "invariants": ["ESC-03"],
        "path": LEDGER,
        "old": (
            '        self.db.execute("INSERT INTO'
            ' escrow_snapshots VALUES(?,?)", (operation_id, state))\n'
        ),
        "new": "        pass\n",
        "tests": [PROPERTY, FAILS],
        "why": (
            "an event commits without its snapshot, so"
            " the chain no longer verifies after a restart."
        ),
    },
    {
        "id": "opids-failed-operation-commits",
        "invariants": ["ESC-18"],
        "path": LEDGER,
        "old": '                self.db.execute("ROLLBACK" if failed else "COMMIT")\n',
        "new": '                self.db.execute("COMMIT")\n',
        "tests": [FAILS],
        "why": "an operation that raised after its movement commits the partial movement.",
    },
    {
        "id": "opids-failed-savepoint-kept",
        "invariants": ["ESC-18"],
        "path": LEDGER,
        "old": (
            "                    if failed:\n"
            '                        self.db.execute("ROLLBACK TO escrow")\n'
        ),
        "new": "",
        "tests": [NESTED],
        "why": (
            "a failed operation inside a caller's"
            " transaction leaves its writes in that transaction."
        ),
    },
    {
        "id": "opids-engine-built-inside-a-transaction",
        "invariants": ["ESC-18"],
        "path": LEDGER,
        "old": '        self.db.execute("PRAGMA synchronous=FULL")\n',
        "new": '        self.db.execute("SELECT 1")\n',
        "tests": [GUARD],
        "why": (
            "without the pragma nothing refuses a construction"
            " inside a transaction and the schema script commits it."
        ),
    },
    {
        "id": "opids-ledger-lock-not-shared-with-the-store",
        "invariants": ["ESC-18"],
        "path": STORE,
        "old": (
            "            escrow.mutex = self._lock  # One"
            " SQLite authority lock, including event acknowledgments.\n"
        ),
        "new": "            pass\n",
        "tests": [SHARED_LOCK],
        "why": (
            "the ledger no longer waits for the store's"
            " transaction and joins it on the shared connection."
        ),
    },
    {
        "id": "opids-outbox-accepts-another-body",
        "invariants": ["LCK-12"],
        "path": OUTBOX,
        "old": (
            '            if row and (row["request"] !='
            ' operation.model_dump_json() or row["signer"] != signer):\n'
        ),
        "new": '            if row and row["signer"] != signer:\n',
        "tests": [LOCK_ONCE],
        "why": (
            "a second lock body under a queued operation"
            " id is answered with the first lock's receipt."
        ),
    },
    {
        "id": "opids-admission-requests-leave-the-transaction",
        "invariants": ["LCK-12", "ES-08"],
        "path": STORE,
        "old": (
            "        with self._tx():\n"
            "            manifest = self._run_v2(run_id)\n"
            "            try:\n"
            "                now ="
            ' self._fresh_v2(manifest, f"admission:{action}:{self._now(self._db)}")\n'
        ),
        "new": (
            "        with self._lock:\n"
            "            manifest = self._run_v2(run_id)\n"
            "            try:\n"
            "                now ="
            ' self._fresh_v2(manifest, f"admission:{action}:{self._now(self._db)}")\n'
        ),
        "tests": [LOCK_ONCE, REFUSED_LEAVES_NOTHING],
        "why": (
            "admission and escrow requests run without the"
            " store transaction, so a refusal keeps its first writes."
        ),
    },
    {
        "id": "opids-pending-outbox-ignored",
        "invariants": ["LCK-12"],
        "path": OUTBOX,
        "old": '"SELECT request FROM admission_outbox WHERE receipt IS NULL"',
        "new": '"SELECT request FROM admission_outbox WHERE receipt IS NULL AND 0"',
        "tests": [OUTBOX_ROW],
        "why": "an unacknowledged lock no longer makes its admission ineligible.",
    },
    {
        "id": "opids-signed-nonce-body-not-compared",
        "invariants": ["SG-09", "ES-08"],
        "path": STORE,
        "old": "        if known is not None and known[0] != digest:\n",
        "new": "        if False:\n",
        "tests": [SIGNED_TWICE],
        "why": (
            "another body under a signed operation id is"
            " no longer refused by the signed-nonce record."
        ),
    },
    {
        "id": "opids-signed-replay-refused",
        "invariants": ["SG-09"],
        "path": STORE,
        "old": "        if known is not None and known[0] != digest:\n",
        "new": "        if known is not None:\n",
        "tests": [SIGNED_TWICE],
        "why": (
            "the identical signed operation sent again is"
            " refused instead of answered with its receipt."
        ),
    },
    {
        "id": "opids-store-commits-a-failed-request",
        "invariants": ["ES-08"],
        "path": STORE,
        "old": (
            "                yield self._db\n"
            "            except BaseException:\n"
            '                self._db.execute("ROLLBACK")\n'
            "                raise\n"
        ),
        "new": (
            "                yield self._db\n"
            "            except BaseException:\n"
            '                self._db.execute("COMMIT")\n'
            "                raise\n"
        ),
        "tests": [REFUSED_LEAVES_NOTHING],
        "why": "the store commits what a refused request wrote, including its signed-nonce record.",
    },
    {
        "id": "opids-second-resolution-accepted",
        "invariants": ["DIA-08"],
        "path": DISPUTES,
        "old": (
            "                if model != turn.resolution:\n"
            '                    raise DisputeError("RESOLUTION_CONFLICT")\n'
        ),
        "new": "",
        "tests": [TERMINAL],
        "why": "a different resolution of a resolved dispute is answered like the stored one.",
    },
    {
        "id": "opids-exclusion-overwrites-a-resolution",
        "invariants": ["DIA-08"],
        "path": DISPUTES,
        "old": (
            "            if turn.resolution is not None:\n"
            "                return turn\n"
            "            if not turn.paused and now < turn.absolute_deadline:\n"
        ),
        "new": "            if not turn.paused and now < turn.absolute_deadline:\n",
        "tests": [TERMINAL],
        "why": "an exclusion replaces the resolution a dispute already has.",
    },
    {
        "id": "opids-resolved-dispute-can-be-paused",
        "invariants": ["DIA-08"],
        "path": DISPUTES,
        "old": (
            "            if turn.resolution is not None:\n"
            '                raise DisputeError("CLOSED_DISPUTE")\n'
        ),
        "new": "",
        "tests": [TERMINAL],
        "why": "a resolved dispute is paused and rewritten.",
    },
    {
        "id": "opids-timeout-after-resolution",
        "invariants": ["DIA-08"],
        "path": DISPUTES,
        "old": "            if turn.resolution is not None or now <= min(\n",
        "new": "            if now <= min(\n",
        "tests": [TERMINAL],
        "why": "a timeout is judged on a dispute that already has a resolution.",
    },
    {
        "id": "opids-bisection-after-resolution",
        "invariants": ["DIA-08"],
        "path": DISPUTES,
        "old": ("                turn.resolution is not None\n                or turn.paused\n"),
        "new": "                turn.paused\n",
        "tests": [TERMINAL],
        "why": (
            "a bisection step is accepted on a resolved dispute that is neither paused nor expired."
        ),
    },
    {
        "id": "opids-reassignment-after-resolution",
        "invariants": ["DIA-08"],
        "path": DISPUTES,
        "old": (
            "                not turn.paused\n                or turn.resolution is not None\n"
        ),
        "new": "                not turn.paused\n",
        "tests": [TERMINAL],
        "why": "a resolved but paused dispute can be given another referee.",
    },
    {
        "id": "opids-settlement-id-depends-on-time",
        "invariants": ["DIA-08", "ESC-03"],
        "path": DISPUTES,
        "old": 'canonicalize([turn.dispute_id, "settle"])',
        "new": 'canonicalize([turn.dispute_id, "settle", now])',
        "tests": [TERMINAL],
        "why": "settling a resolved dispute again at another beacon is a new release, no replay.",
    },
    {
        "id": "opids-deferred-begin",
        "invariants": ["ESC-18"],
        "path": LEDGER,
        "old": '            self.db.execute("SAVEPOINT escrow" if nested else "BEGIN IMMEDIATE")\n',
        "new": '            self.db.execute("SAVEPOINT escrow" if nested else "BEGIN")\n',
        "tests": [CONCURRENT_LOCKS],
        "why": "an operation opens a deferred transaction, so two connections stop serializing.",
    },
    {
        "id": "opids-resolution-conflict-after-the-evidence-lookup",
        "invariants": ["DIA-08"],
        "path": DISPUTES,
        "old": (
            "            if turn.resolution is not None:\n"
            "                if model != turn.resolution:\n"
            '                    raise DisputeError("RESOLUTION_CONFLICT")\n'
            "                return turn\n"
            "            if now > turn.absolute_deadline:\n"
            '                raise DisputeError("RESOLUTION_ABSOLUTE_DEADLINE")\n'
            "            row = self.db.execute(\n"
            '                "SELECT body FROM dispute_evidence_v2 WHERE hash=?",'
            " (model.evidence_hash,)\n"
            "            ).fetchone()\n"
            "            if row is None:\n"
            '                raise DisputeError("UNVERIFIED_REFEREE_EVIDENCE")\n'
        ),
        "new": (
            "            row = self.db.execute(\n"
            '                "SELECT body FROM dispute_evidence_v2 WHERE hash=?",'
            " (model.evidence_hash,)\n"
            "            ).fetchone()\n"
            "            if row is None:\n"
            '                raise DisputeError("UNVERIFIED_REFEREE_EVIDENCE")\n'
            "            if turn.resolution is not None:\n"
            "                if model != turn.resolution:\n"
            '                    raise DisputeError("RESOLUTION_CONFLICT")\n'
            "                return turn\n"
            "            if now > turn.absolute_deadline:\n"
            '                raise DisputeError("RESOLUTION_ABSOLUTE_DEADLINE")\n'
        ),
        "tests": [TERMINAL],
        "why": "a second resolution is judged on its evidence first, so its conflict is hidden.",
    },
]
