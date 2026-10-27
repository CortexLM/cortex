"""Mutants of the escrow gap tests: one small regression of src/hypertrain per pinned defence.

A strict xfail is killed through its control: the mutant breaks what the control asserts, which is
a plain AssertionError the xfail does not expect, so the test fails instead of failing as marked.

Run from hypertrain/: .venv/bin/python scripts/check_ledger_invariant_mutants.py --only <ids>
"""

from __future__ import annotations

LEDGER = "src/hypertrain/ledger/escrow_v2.py"
STORE = "src/hypertrain/challenge/store.py"
ADMISSION = "src/hypertrain/challenge/admission.py"
APP = "src/hypertrain/challenge/app.py"
MESSAGES = "src/hypertrain/protocol/messages_v2.py"

SERVICE = "tests/ledger_invariants/test_gaps_escrow.py::"
LEDGER_TESTS = "tests/ledger_invariants/test_gaps_escrow_2.py::"

WRITERS = SERVICE + "test_the_credit_tables_are_written_only_by_the_statements_the_ledger_relies_on"
KINDS = SERVICE + "test_each_finalization_record_kind_is_stored_only_by_finalize"
RESTART_CONTROL = (
    SERVICE + "test_a_due_origin_matures_on_the_first_beacon_when_a_request_built_the_services"
)
ROTATION_CONTROL = (
    SERVICE
    + "test_matured_units_are_emitted_to_the_hotkey_that_earned_them_while_it_is_the_current_one"
)
SLASH_ID = SERVICE + "test_a_client_event_never_blocks_the_dispute_slash_id"
BEACON_CLOSE = SERVICE + "test_a_client_event_never_stops_the_beacon_from_closing_a_blocked_dispute"
CONVICTED = (
    SERVICE + "test_a_client_event_never_blocks_the_settlement_of_a_convicted_miner_over_http"
)
HOLD_SCOPE = (
    SERVICE + "test_an_open_dispute_holds_the_origin_of_its_own_miner_only_and_a_match_releases_it"
)
HOLD_FRAUD = (
    SERVICE + "test_a_miner_convicted_of_fraud_on_a_round_is_emitted_nothing_for_that_rounds_origin"
)
ACCEPT_FUNDING = (
    SERVICE + "test_an_accept_is_refused_when_the_miner_lost_its_stake_after_the_opening"
)
FINALIZE_FUNDING = (
    SERVICE + "test_a_finalize_input_is_refused_for_a_member_who_lost_its_stake_after_the_opening"
)
UNCOMMITTED = SERVICE + "test_a_route_never_decides_on_the_uncommitted_state_of_another_thread"

SELF_REFEREE = (
    LEDGER_TESTS + "test_a_slash_naming_an_empty_or_self_referee_is_refused_and_changes_nothing"
)
NAMED_ORIGINS = LEDGER_TESTS + "test_a_release_naming_an_origin_the_lock_does_not_hold_is_refused"
NO_FINALIZE_ROW = (
    LEDGER_TESTS + "test_a_reward_whose_finalized_round_is_not_on_record_does_not_mature"
)
REPLAY_NO_ASK = (
    LEDGER_TESTS
    + "test_a_replayed_release_or_slash_is_answered_from_the_ledger_not_from_the_settlement"
)
SLASH_TERMS = (
    LEDGER_TESTS + "test_a_slash_operation_id_names_one_request_and_one_burn_even_after_a_restart"
)
TRANSFER_RECIPIENT = LEDGER_TESTS + "test_a_transfer_operation_id_names_one_recipient"
LATE_FAILURE = (
    LEDGER_TESTS + "test_a_release_and_a_slash_with_a_referee_cost_that_fail_late_change_nothing"
)
CONTEST_LOCK = (
    LEDGER_TESTS + "test_the_admission_service_takes_no_contest_lock_and_writes_nothing_for_it"
)
SHARED_STAKE = (
    LEDGER_TESTS
    + "test_the_stake_of_one_admission_of_a_coldkey_never_funds_another_one_of_the_same_coldkey"
)

MUTANTS = [
    {
        "id": "gaps-escrow-release-deletes-a-snapshot",
        "invariants": ["GAP-06"],
        "path": LEDGER,
        "old": (
            '            return self._event(op.operation_id, "RELEASE",'
            " op.model_dump_json(), op.owner)\n"
        ),
        "new": (
            '            self.db.execute("DELETE FROM escrow_snapshots WHERE operation_id=?",'
            " (op.lock_id,))\n"
            '            return self._event(op.operation_id, "RELEASE", op.model_dump_json('
            "), op.owner)\n"
        ),
        "tests": [WRITERS],
        "why": (
            "a release erases the snapshot of its lock's event, a second "
            "writer of an append-only table."
        ),
    },
    {
        "id": "gaps-escrow-maturity-rewrites-an-origin",
        "invariants": ["GAP-06"],
        "path": LEDGER,
        "old": (
            "                if not rows:\n                    raise "
            'EscrowError("ALREADY_MATURED")\n'
        ),
        "new": (
            '                self.db.execute("UPDATE escrow_origins SET issued=issued '
            'WHERE origin=?",'
            " (origin,))\n"
            "                if not rows:\n"
            '                    raise EscrowError("ALREADY_MATURED")\n'
        ),
        "tests": [WRITERS],
        "why": "maturity updates an issued origin, which no operation but the issuance may write.",
    },
    {
        "id": "gaps-escrow-epoch-answer-replaced",
        "invariants": ["GAP-06"],
        "path": STORE,
        "old": '            db.execute("INSERT INTO weights_runs VALUES(?, ?)", (epoch, merged))\n',
        "new": (
            '            db.execute("INSERT OR REPLACE INTO weights_runs '
            'VALUES(?, ?)", (epoch, merged))\n'
        ),
        "tests": [WRITERS],
        "why": (
            "a stored epoch answer can be replaced, so the answer the master received is not final."
        ),
    },
    {
        "id": "gaps-escrow-settlement-record-has-a-second-writer",
        "invariants": ["GAP-06"],
        "path": STORE,
        "old": '        self._put_record_v2(run_id, "finality", str(w), status)\n',
        "new": (
            '        self._put_record_v2(run_id, "finality", str(w), status)\n'
            '        self._put_record_v2(run_id, "settlement", str(w), status)\n'
        ),
        "tests": [KINDS],
        "why": (
            "the recount of a round's disputes also writes settlement "
            "records, which only finalize may."
        ),
    },
    {
        "id": "gaps-escrow-training-action-opens-a-finalization-kind",
        "invariants": ["GAP-06"],
        "path": STORE,
        "old": (
            '        expected = {"accept": "AcceptV2", "commit": "CommitV2",'
            ' "delta": "DeltaManifestV2"}[action]\n'
        ),
        "new": (
            '        expected = {"accept": "AcceptV2", "commit": "CommitV2",'
            ' "delta": "DeltaManifestV2", "finalize": "Finalize"}[action]\n'
        ),
        "tests": [KINDS],
        "why": (
            "the training route can store a record under the finalize kind, chosen by the caller."
        ),
    },
    {
        "id": "gaps-escrow-maturity-writes-a-finalization-record",
        "invariants": ["GAP-06"],
        "path": LEDGER,
        "old": '            return self._event(operation_id, "MATURE", request, owner)\n',
        "new": (
            "            self.db.execute("
            "\"UPDATE records_v2 SET data=data WHERE kind='finalize-result'\")\n"
            '            return self._event(operation_id, "MATURE", request, owner)\n'
        ),
        "tests": [KINDS],
        "why": (
            "maturity writes the record table directly, outside the one function that finalizes."
        ),
    },
    {
        "id": "gaps-escrow-beacon-matures-nothing",
        "invariants": ["GAP-07", "LCK-09"],
        "path": STORE,
        "old": (
            "                for origin in origins:\n"
            "                    try:\n"
            "                        escrow.mature(\n"
        ),
        "new": (
            "                for origin in ():\n"
            "                    try:\n"
            "                        escrow.mature(\n"
        ),
        "tests": [RESTART_CONTROL, HOLD_SCOPE, HOLD_FRAUD],
        "why": (
            "the beacon route vests no reward origin, so an undisputed miner is not paid either."
        ),
    },
    {
        "id": "gaps-escrow-weights-keyed-by-the-owner",
        "invariants": ["CRD-12", "WGT-13"],
        "path": STORE,
        "old": '                weights[row["hotkey"]] = weights.get(row["hotkey"], 0.0) + take\n',
        "new": '                weights[row["owner"]] = weights.get(row["owner"], 0.0) + take\n',
        "tests": [ROTATION_CONTROL],
        "why": (
            "the epoch answer lists the coldkey that owns the units instead of the earning hotkey."
        ),
    },
    {
        "id": "gaps-escrow-slash-returns-the-bond",
        "invariants": ["ESC-04", "ESC-14"],
        "path": LEDGER,
        "old": '                self._move(burn, source, "burned", source_ref=op.lock_id)\n',
        "new": '                self._move(burn, source, "available", source_ref=op.lock_id)\n',
        "tests": [
            SLASH_ID + "[transfer]",
            SLASH_ID + "[lock]",
            CONVICTED + "[resolution]",
        ],
        "why": "a proven fraud gives the bond back to the owner instead of burning it.",
    },
    {
        "id": "gaps-escrow-beacon-exit-keeps-the-bond",
        "invariants": ["ESC-04", "DIA-14"],
        "path": STORE,
        "old": "                        disputes.settle_lock(turn.dispute_id, now=now)\n",
        "new": "                        pass\n",
        "tests": [BEACON_CLOSE + "[transfer]", BEACON_CLOSE + "[lock]"],
        "why": "the beacon closes a blocked dispute but leaves its contest bond locked.",
    },
    {
        "id": "gaps-escrow-timeout-route-keeps-the-bond",
        "invariants": ["ESC-04", "DIA-14"],
        "path": STORE,
        "old": "                disputes.settle_lock(dispute_id, now=self._now(self._db))\n",
        "new": "                pass\n",
        "tests": [CONVICTED + "[timeout]"],
        "why": "the timeout route convicts the party but leaves the contest bond locked.",
    },
    {
        "id": "gaps-escrow-slash-refunds-the-referee-share-to-the-owner",
        "invariants": ["ESC-15"],
        "path": LEDGER,
        "old": (
            '                self._move(cost, source, "available", source_ref=op.lock_id,'
            " recipient=referee)\n"
        ),
        "new": (
            '                self._move(cost, source, "available", source_ref=op.lock_id,'
            " recipient=None)\n"
        ),
        "tests": [
            SELF_REFEREE + "[an_empty_referee]",
            SELF_REFEREE + "[the_slashed_owner_as_referee]",
        ],
        "why": (
            "the referee's share of a slash goes back to the slashed owner, whoever the referee is."
        ),
    },
    {
        "id": "gaps-escrow-move-ignores-the-named-origin-set",
        "invariants": ["ESC-09", "ESC-11"],
        "path": LEDGER,
        "old": (
            '        if {r["origin"] for r in selected} != set(op.origin_ids) or sum(\n'
            '            r["units"] for r in selected\n'
            "        ) < op.units:\n"
        ),
        "new": '        if sum(\n            r["units"] for r in selected\n        ) < op.units:\n',
        "tests": [NAMED_ORIGINS + "[own_origin_and_another_owners]"],
        "why": "a move accepts a request that names more origins than the units it draws on.",
    },
    {
        "id": "gaps-escrow-mature-without-the-finalize-row",
        "invariants": ["ESC-08", "ESC-12"],
        "path": LEDGER,
        "old": "                        or finality is None\n",
        "new": "",
        "tests": [NO_FINALIZE_ROW],
        "why": "maturity trusts a settlement without the finalized round it is bound to.",
    },
    {
        "id": "gaps-escrow-release-asks-the-settlement-before-replaying",
        "invariants": ["ESC-03", "ESC-04"],
        "path": LEDGER,
        "old": (
            "            old = self._owner_operation(op, signer)\n"
            "            if old is not None:\n"
            "                return old\n"
            "            status = self.settlement(op.lock_id)\n"
            "            if (\n"
            "                status.unresolved\n"
            '                or status.outcome == "FRAUD"\n'
        ),
        "new": (
            "            status = self.settlement(op.lock_id)\n"
            "            old = self._owner_operation(op, signer)\n"
            "            if old is not None:\n"
            "                return old\n"
            "            if (\n"
            "                status.unresolved\n"
            '                or status.outcome == "FRAUD"\n'
        ),
        "tests": [REPLAY_NO_ASK],
        "why": (
            "a replayed release asks the settlement first, so a settlement "
            "that went away refuses it."
        ),
    },
    {
        "id": "gaps-escrow-slash-asks-the-settlement-before-replaying",
        "invariants": ["ESC-03", "ESC-14"],
        "path": LEDGER,
        "old": (
            "            old = self._existing(op.operation_id, request)\n"
            "            if old is not None:\n"
            "                return old\n"
            "            status = self.settlement(op.lock_id)\n"
            "            if (\n"
            "                status.unresolved\n"
            '                or status.outcome != "FRAUD"\n'
        ),
        "new": (
            "            status = self.settlement(op.lock_id)\n"
            "            old = self._existing(op.operation_id, request)\n"
            "            if old is not None:\n"
            "                return old\n"
            "            if (\n"
            "                status.unresolved\n"
            '                or status.outcome != "FRAUD"\n'
        ),
        "tests": [REPLAY_NO_ASK],
        "why": (
            "a replayed slash asks the settlement first, so a settlement that went away refuses it."
        ),
    },
    {
        "id": "gaps-escrow-slash-request-forgets-the-referee-terms",
        "invariants": ["ESC-03", "ESC-14"],
        "path": LEDGER,
        "old": (
            '                [op.model_dump(mode="json"), referee, referee_cost,'
            " max_referee_cost]\n"
        ),
        "new": '                [op.model_dump(mode="json")]\n',
        "tests": [
            SLASH_TERMS + "[another_referee]",
            SLASH_TERMS + "[another_referee_cost]",
            SLASH_TERMS + "[another_cost_cap]",
        ],
        "why": (
            "the stored slash request leaves out the referee terms, "
            "so another set of terms replays it."
        ),
    },
    {
        "id": "gaps-escrow-transfer-request-forgets-the-recipient",
        "invariants": ["ESC-03"],
        "path": MESSAGES,
        "old": "class EscrowTransfer(EscrowOperation):\n    recipient: SS58\n",
        "new": (
            "class EscrowTransfer(EscrowOperation):\n"
            "    recipient: Annotated[SS58, Field(exclude=True)]\n"
        ),
        "tests": [TRANSFER_RECIPIENT],
        "why": (
            "the stored transfer request leaves out the recipient, so another recipient replays it."
        ),
    },
    {
        "id": "gaps-escrow-release-is-not-atomic",
        "invariants": ["ESC-18"],
        "path": LEDGER,
        "old": (
            "        with self.tx():\n"
            "            old = self._owner_operation(op, signer)\n"
            "            if old is not None:\n"
            "                return old\n"
            "            status = self.settlement(op.lock_id)\n"
            "            if (\n"
            "                status.unresolved\n"
            '                or status.outcome == "FRAUD"\n'
        ),
        "new": (
            "        if True:\n"
            "            old = self._owner_operation(op, signer)\n"
            "            if old is not None:\n"
            "                return old\n"
            "            status = self.settlement(op.lock_id)\n"
            "            if (\n"
            "                status.unresolved\n"
            '                or status.outcome == "FRAUD"\n'
        ),
        "tests": [LATE_FAILURE],
        "why": (
            "a release runs outside a transaction, so a failure after "
            "its movement keeps the movement."
        ),
    },
    {
        "id": "gaps-escrow-slash-is-not-atomic",
        "invariants": ["ESC-18"],
        "path": LEDGER,
        "old": "        with self.tx():\n            request = canonicalize(\n",
        "new": "        if True:\n            request = canonicalize(\n",
        "tests": [LATE_FAILURE],
        "why": (
            "a slash runs outside a transaction, so a failure after "
            "its referee share keeps the share."
        ),
    },
    {
        "id": "gaps-escrow-admission-route-takes-a-contest-lock",
        "invariants": ["ESC-26"],
        "path": ADMISSION,
        "old": (
            '            r = self.store.by_id(op.admission_id or "")\n'
            '            Intake(self.escrow.run_id, {"EscrowLock": lambda s: s == r.coldkey})'
            ".accept(raw, now)\n"
            '            if op.owner != r.coldkey or op.kind != "LOCK_ADMISSION":\n'
        ),
        "new": (
            "            r = self.store.by_id(\n"
            "                op.admission_id\n"
            "                or self.store.db.execute(\n"
            '                    "SELECT admission_id FROM admissions_v2 WHERE coldkey=?",'
            " (op.owner,)\n"
            "                ).fetchone()[0]\n"
            "            )\n"
            '            Intake(self.escrow.run_id, {"EscrowLock": lambda s: s == r.coldkey})'
            ".accept(raw, now)\n"
            "            if op.owner != r.coldkey:\n"
        ),
        "tests": [CONTEST_LOCK],
        "why": (
            "the admission route stakes a contest lock for the signer's "
            "admission instead of refusing it."
        ),
    },
    {
        "id": "gaps-escrow-stake-shared-across-a-coldkeys-admissions",
        "invariants": ["ESC-26"],
        "path": ADMISSION,
        "old": "        units, receipt = self.escrow.locked(r.admission_id, r.coldkey)\n",
        "new": (
            "        units, receipt = self.escrow.locked(r.admission_id, r.coldkey)\n"
            "        units += sum(\n"
            '            self.escrow.locked(other["admission_id"], r.coldkey)[0]\n'
            "            for other in self.store.db.execute(\n"
            '                "SELECT admission_id FROM admissions_v2 WHERE coldkey=? '
            'AND admission_id!=?",\n'
            "                (r.coldkey, r.admission_id),\n"
            "            ).fetchall()\n"
            "        )\n"
        ),
        "tests": [SHARED_STAKE],
        "why": "the stake of one admission also funds every other admission of the same coldkey.",
    },
    {
        "id": "gaps-escrow-accept-ignores-the-funding",
        "invariants": ["LCK-02", "ESC-28"],
        "path": STORE,
        "old": (
            "                if (\n"
            "                    not status.eligible\n"
            "                    or model.work_screen_hash"
        ),
        "new": (
            "                if (\n"
            "                    False\n"
            "                    or model.work_screen_hash"
        ),
        "tests": [ACCEPT_FUNDING],
        "why": "an accept checks only the work screen, so a miner without stake joins the round.",
    },
    {
        "id": "gaps-escrow-finalize-input-ignores-the-funding",
        "invariants": ["LCK-01", "ESC-29"],
        "path": STORE,
        "old": (
            "            if not status.eligible or"
            " status.record.admission_id != entry.admission_id:\n"
        ),
        "new": "            if status.record.admission_id != entry.admission_id:\n",
        "tests": [FINALIZE_FUNDING],
        "why": "a reward input is built for a member whose stake is gone, if its identity matches.",
    },
    {
        "id": "gaps-escrow-challenge-route-refuses-the-hotkey",
        "invariants": ["GAP-08"],
        "path": APP,
        "old": (
            "        await subnet_member((await run(admission.store.by_id, admission_id)).hotkey)\n"
        ),
        "new": "        await subnet_member(None)\n",
        "tests": [UNCOMMITTED],
        "why": "the challenge route refuses every applicant, so its control never answers 200.",
    },
]
