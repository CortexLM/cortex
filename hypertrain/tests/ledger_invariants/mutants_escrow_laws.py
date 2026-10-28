"""Regressions of src/hypertrain that the escrow-laws tests must catch (package escrow_laws, D39).

Each entry is one small realistic change: a check deleted, a source bucket widened, a comparison
dropped. `old` occurs exactly once in `path`; `tests` are node ids that must fail under the change.
"""

from __future__ import annotations

L = "tests/ledger_invariants/test_escrow_laws.py::"
L2 = "tests/ledger_invariants/test_escrow_laws_2.py::"
ESCROW = "src/hypertrain/ledger/escrow_v2.py"
DISPUTES = "src/hypertrain/challenge/disputes_v2.py"
MESSAGES = "src/hypertrain/protocol/messages_v2.py"
STORE = "src/hypertrain/challenge/store.py"

OWNER_MATRIX = L + "test_units_move_only_under_the_signature_of_their_owner"
OWNER_HTTP = L2 + "test_escrow_routes_refuse_a_foreign_signature_and_change_nothing"
LOCK_FUNDING = L + "test_a_lock_is_funded_only_by_the_owners_available_units_of_the_listed_origins"
CLOSED_EXIT = L + "test_no_operation_draws_on_a_locked_pending_paid_or_burned_bucket"
RELEASE_LAWS = L + "test_a_release_is_the_owners_for_the_owners_own_lock_with_matching_references"
RELEASE_BOUND = L + "test_a_lock_releases_at_most_what_it_locked_and_only_through_its_own_units"
SLASH_AUTHORITY = L + "test_a_slash_needs_the_reward_authority_and_a_bounded_referee_cost"
SLASH_COST_BOUND = L + "test_a_referee_cost_never_exceeds_the_units_slashed"
SLASH_SETTLEMENT = L + "test_a_slash_needs_a_resolved_vested_fraud_settlement_bound_to_the_request"
SLASH_REFERENCES = L + "test_a_slash_burns_only_the_lock_it_names_with_matching_references"
RELEASE_GATE = (
    L + "test_a_release_waits_for_a_resolved_vested_non_fraud_settlement_bound_to_its_hashes"
)
SPEND_FUNDING = L + "test_transfer_and_pay_spend_only_the_signers_own_available_units"
SLASH_ONCE = L + "test_a_proven_fraud_burns_the_bond_once_and_closes_the_release_path"
SLASH_SPLIT = L + "test_a_slash_pays_the_referee_cost_then_burns_exactly_the_rest"
SLASH_REFEREE = L + "test_a_referee_cost_needs_a_funded_referee_and_a_contest_bond"
GENESIS_AUTHORITY = (
    L + "test_units_are_issued_only_under_the_pinned_authority_signature_bound_to_the_run"
)
REWARD_AUTHORITY = (
    L2 + "test_a_reward_is_issued_once_per_round_and_only_under_the_authority_signature"
)
ROUND_ORDER = L2 + "test_rounds_are_issued_in_order_and_never_beyond_the_pinned_total"
PAY_ONCE = L2 + "test_a_pay_operation_id_is_spent_once_and_never_for_another_amount"
REOPEN = L + "test_a_ledger_file_edited_or_cut_in_place_is_refused_at_reopen"
REOPEN_BARE = L + "test_units_that_issuance_does_not_explain_are_refused_when_no_event_exists"
STRAY_UNIT = L + "test_a_unit_row_that_issuance_does_not_explain_stops_the_next_event"
STRICT_UNITS = L + "test_escrow_amounts_are_strict_positive_integers_within_the_wire_ceiling"
STAKE_COUNT = L + "test_stake_counts_only_locks_that_name_its_admission_and_owner"
ONE_BACKING = L + "test_the_same_units_cannot_back_two_locks_or_two_admissions"
MODEL = L2 + "test_random_operation_sequences_conserve_units_and_follow_a_bucket_model"
BOND = L2 + "test_a_dispute_opens_only_on_a_funded_owner_bound_adequately_sized_contest_bond"
ONE_OPEN = L2 + "test_a_miner_has_at_most_one_open_dispute"
SETTLE_ONCE = L2 + "test_a_bond_is_settled_once_whole_and_with_the_production_default_cost"
MATURITY = L2 + "test_a_reward_matures_only_after_vesting_and_a_resolved_hash_bound_settlement"
STAKE_HTTP = L2 + "test_an_admission_stake_is_accepted_only_from_the_coldkey_recorded_at_join"
ROUTES = L2 + "test_clients_reach_only_lock_transfer_and_release_of_the_escrow"
SLASH_CALLERS = L2 + "test_the_only_code_that_slashes_passes_the_coordinator_key_as_authority"
COLLATERAL = L + "test_collateral_issued_at_genesis_is_never_payable_as_weight"
HELD = "tests/challenge/test_v2_weights_pay.py::test_held_units_never_paid"
LOCKED_NEVER = (
    "tests/challenge/test_v2_weights_pay.py::test_dispute_locked_and_burned_units_never_paid"
)
SHARED_MASS = "tests/challenge/test_v2_weights_pay.py::test_v1_and_v2_share_one_full_share_mass"
PAY_ROLLS_BACK = (
    "tests/challenge/test_v2_weights_pay.py::test_pay_rolls_back_when_answer_not_persisted"
)

CONVICTION = '            if reason == "TRAINING_FRAUD" and r.state != "BANNED":\n'
SEIZURE_AUTHORITY = (
    "                    authority=self.coord.ss58,\n"
    "                    now_beacon=now,\n"
    '                    burn_pending=reason == "TRAINING_FRAUD",\n'
)
CONTEST_LOCK_GATE = (
    '                            self._authenticated_v2(run_id, raw, "EscrowLock", {})'
)
ADMISSION_LOCK_GATE = 'Intake(self.escrow.run_id, {{"EscrowLock": lambda s: {}}}).accept(raw, now)'

MUTANTS = [
    {
        "id": "el-owner-check-dropped",
        "invariants": ["ESC-02", "LCK-05", "ES-01"],
        "path": ESCROW,
        "old": '        if signer != op.owner:\n            raise EscrowError("OWNER_AUTHORITY")',
        "new": '        if False:\n            raise EscrowError("OWNER_AUTHORITY")',
        "tests": [OWNER_MATRIX, OWNER_HTTP],
        "why": "no operation checks that the signer owns the units any more.",
    },
    {
        "id": "el-transfer-skips-owner-gate",
        "invariants": ["ESC-02", "ES-01"],
        "path": ESCROW,
        "old": (
            "    def transfer(self, op: EscrowTransfer, *, signer: str) -> EscrowReceipt:\n"
            "        with self.tx():\n"
            "            old = self._owner_operation(op, signer)"
        ),
        "new": (
            "    def transfer(self, op: EscrowTransfer, *, signer: str) -> EscrowReceipt:\n"
            "        with self.tx():\n"
            "            old = self._existing(op.operation_id, op.model_dump_json())"
        ),
        "tests": [OWNER_MATRIX, OWNER_HTTP],
        "why": "transfer looks up its replay without asking who signed: any key moves any units.",
    },
    {
        "id": "el-owner-check-after-replay",
        "invariants": ["ESC-02", "LCK-05"],
        "path": ESCROW,
        "old": (
            '        if signer != op.owner:\n            raise EscrowError("OWNER_AUTHORITY")\n'
            "        return self._existing(op.operation_id, op.model_dump_json())"
        ),
        "new": (
            "        old = self._existing(op.operation_id, op.model_dump_json())\n"
            "        if old is None and signer != op.owner:\n"
            '            raise EscrowError("OWNER_AUTHORITY")\n'
            "        return old"
        ),
        "tests": [OWNER_MATRIX],
        "why": "the ownership check moves after the replay lookup, so a stranger gets the receipt.",
    },
    {
        "id": "el-lock-ignores-unfunded-origin",
        "invariants": ["ESC-01", "ES-02"],
        "path": ESCROW,
        "old": '        if {r["origin"] for r in selected} != set(op.origin_ids) or sum(',
        "new": "        if sum(",
        "tests": [LOCK_FUNDING],
        "why": "an origin the owner holds nothing of may be listed next to one it does hold.",
    },
    {
        "id": "el-overdraw-allowed",
        "invariants": ["ESC-01", "ESC-11", "ES-02"],
        "path": ESCROW,
        "old": '        ) < op.units:\n            raise EscrowError("UNFUNDED_OR_UNKNOWN_ORIGIN")',
        "new": '        ) < 0:\n            raise EscrowError("UNFUNDED_OR_UNKNOWN_ORIGIN")',
        "tests": [LOCK_FUNDING, SPEND_FUNDING, RELEASE_BOUND],
        "why": "a movement larger than the units held is accepted and silently moves less.",
    },
    {
        "id": "el-transfer-draws-reward-pending",
        "invariants": ["ESC-07"],
        "path": ESCROW,
        "old": 'self._move(op, "available", "available", recipient=op.recipient)',
        "new": 'self._move(op, "reward_pending", "available", recipient=op.recipient)',
        "tests": [CLOSED_EXIT],
        "why": "a transfer spends units that are still pending, skipping the maturity gate.",
    },
    {
        "id": "el-move-ignores-bucket",
        "invariants": ["ESC-07"],
        "path": ESCROW,
        "old": (
            '            "SELECT * FROM escrow_units WHERE owner=? AND bucket=? AND ref=?",\n'
            "            (op.owner, source, source_ref),"
        ),
        "new": (
            '            "SELECT * FROM escrow_units WHERE owner=? AND ref=?",\n'
            "            (op.owner, source_ref),"
        ),
        "tests": [CLOSED_EXIT],
        "why": "the source bucket is no longer part of the selection of the units to spend.",
    },
    {
        "id": "el-move-ignores-lock-reference",
        "invariants": ["ESC-09", "ESC-11", "LCK-11"],
        "path": ESCROW,
        "old": (
            '            "SELECT * FROM escrow_units WHERE owner=? AND bucket=? AND ref=?",\n'
            "            (op.owner, source, source_ref),"
        ),
        "new": (
            '            "SELECT * FROM escrow_units WHERE owner=? AND bucket=?",\n'
            "            (op.owner, source),"
        ),
        "tests": [RELEASE_BOUND],
        "why": "a release draws on every lock of the owner, not on the lock it names.",
    },
    {
        "id": "el-release-reference-unchecked",
        "invariants": ["ESC-09"],
        "path": ESCROW,
        "old": (
            '            original = EscrowLock.model_validate_json(lock["request"])\n'
            "            if (op.admission_id, op.dispute_id, op.owner) != ("
        ),
        "new": (
            '            original = EscrowLock.model_validate_json(lock["request"])\n'
            "            if False and (op.admission_id, op.dispute_id, op.owner) != ("
        ),
        "tests": [RELEASE_LAWS],
        "why": "a release may name any admission, dispute or owner next to a real lock id.",
    },
    {
        "id": "el-release-lock-kind-unchecked",
        "invariants": ["ESC-09"],
        "path": ESCROW,
        "old": (
            '            if lock is None or lock["kind"] not in ("LOCK_ADMISSION", "LOCK_CONTEST"):'
        ),
        "new": "            if lock is None:",
        "tests": [RELEASE_LAWS],
        "why": "any recorded event, a transfer for one, is accepted as the lock to release.",
    },
    {
        "id": "el-release-dispute-reference-ignored",
        "invariants": ["ESC-09"],
        "path": ESCROW,
        "old": (
            '            original = EscrowLock.model_validate_json(lock["request"])\n'
            "            if (op.admission_id, op.dispute_id, op.owner) != (\n"
            "                original.admission_id,\n"
            "                original.dispute_id,\n"
            "                original.owner,\n"
            "            ):"
        ),
        "new": (
            '            original = EscrowLock.model_validate_json(lock["request"])\n'
            "            if (op.admission_id, op.owner) != (\n"
            "                original.admission_id,\n"
            "                original.owner,\n"
            "            ):"
        ),
        "tests": [RELEASE_LAWS],
        "why": "a release may name another dispute than the one its contest bond was locked for.",
    },
    {
        "id": "el-slash-reference-unchecked",
        "invariants": ["ESC-14"],
        "path": ESCROW,
        "old": (
            '            original = EscrowLock.model_validate_json(row["request"])\n'
            "            if (op.admission_id, op.dispute_id, op.owner) != ("
        ),
        "new": (
            '            original = EscrowLock.model_validate_json(row["request"])\n'
            "            if False and (op.admission_id, op.dispute_id, op.owner) != ("
        ),
        "tests": [SLASH_REFERENCES],
        "why": "a slash may name any admission, dispute or owner next to a real lock id.",
    },
    {
        "id": "el-slash-lock-kind-unchecked",
        "invariants": ["ESC-14"],
        "path": ESCROW,
        "old": (
            '            if row is None or row["kind"] not in ("LOCK_ADMISSION", "LOCK_CONTEST"):'
        ),
        "new": "            if row is None:",
        "tests": [SLASH_REFERENCES],
        "why": "any recorded event, a transfer for one, is accepted as the lock to slash.",
    },
    {
        "id": "el-slash-authority-unchecked",
        "invariants": ["ESC-13"],
        "path": ESCROW,
        "old": (
            "            authority != self.policy.reward_authority\n"
            "            or min(referee_cost, max_referee_cost) < 0"
        ),
        "new": "            False\n            or min(referee_cost, max_referee_cost) < 0",
        "tests": [SLASH_AUTHORITY],
        "why": "slash accepts whichever authority string its caller passes.",
    },
    {
        "id": "el-slash-cost-cap-unchecked",
        "invariants": ["ESC-13", "ESC-15"],
        "path": ESCROW,
        "old": "            or referee_cost > min(max_referee_cost, op.units)",
        "new": "            or False",
        "tests": [SLASH_AUTHORITY, SLASH_COST_BOUND],
        "why": "the referee cost may exceed the cap and the units slashed.",
    },
    {
        "id": "el-slash-accepts-any-outcome",
        "invariants": ["ESC-14"],
        "path": ESCROW,
        "old": '                or status.outcome != "FRAUD"',
        "new": "                or False",
        "tests": [SLASH_SETTLEMENT],
        "why": "a MATCH or INFRASTRUCTURE settlement burns the bond as if it were FRAUD.",
    },
    {
        "id": "el-slash-ignores-release-beacon",
        "invariants": ["ESC-14"],
        "path": ESCROW,
        "old": (
            '                or status.outcome != "FRAUD"\n'
            "                or now_beacon < status.release_beacon\n"
        ),
        "new": '                or status.outcome != "FRAUD"\n',
        "tests": [SLASH_SETTLEMENT],
        "why": "a FRAUD settlement burns the bond before its release horizon.",
    },
    {
        "id": "el-slash-hash-binding-dropped",
        "invariants": ["ESC-14"],
        "path": ESCROW,
        "old": (
            "                or (op.finality_hash, op.closed_dispute_root)\n"
            "                != (status.finality_hash, status.closed_dispute_root)\n"
            "            ):\n"
            '                raise EscrowError("UNSETTLED_SLASH")'
        ),
        "new": '            ):\n                raise EscrowError("UNSETTLED_SLASH")',
        "tests": [SLASH_SETTLEMENT],
        "why": "a slash is no longer bound to the finality and closed-dispute hashes it names.",
    },
    {
        "id": "el-release-after-fraud-allowed",
        "invariants": ["ESC-14", "ESC-15"],
        "path": ESCROW,
        "old": (
            '                or status.outcome == "FRAUD"\n'
            "                or now_beacon < status.release_beacon\n"
        ),
        "new": "                or False\n                or now_beacon < status.release_beacon\n",
        "tests": [SLASH_ONCE, MODEL],
        "why": "a bond settled FRAUD can still be released to its owner.",
    },
    {
        "id": "el-release-hash-binding-dropped",
        "invariants": ["ESC-09", "ESC-14"],
        "path": ESCROW,
        "old": (
            "                or (op.finality_hash, op.closed_dispute_root)\n"
            "                != (status.finality_hash, status.closed_dispute_root)\n"
            "            ):\n"
            '                raise EscrowError("DISPUTED_OR_UNVESTED")'
        ),
        "new": '            ):\n                raise EscrowError("DISPUTED_OR_UNVESTED")',
        "tests": [RELEASE_GATE],
        "why": "a release is no longer bound to the finality and closed-dispute hashes it names.",
    },
    {
        "id": "el-release-ignores-release-beacon",
        "invariants": ["ESC-09", "ESC-14"],
        "path": ESCROW,
        "old": (
            '                or status.outcome == "FRAUD"\n'
            "                or now_beacon < status.release_beacon\n"
        ),
        "new": '                or status.outcome == "FRAUD"\n',
        "tests": [RELEASE_GATE],
        "why": "a bond is released before the release horizon of its settlement.",
    },
    {
        "id": "el-slash-burn-destination",
        "invariants": ["ESC-15"],
        "path": ESCROW,
        "old": 'self._move(burn, source, "burned", source_ref=op.lock_id)',
        "new": 'self._move(burn, source, "available", source_ref=op.lock_id)',
        "tests": [SLASH_ONCE, SLASH_SPLIT],
        "why": "a slash returns the remainder to the owner's available bucket, it burns nothing.",
    },
    {
        "id": "el-slash-referee-unfunded-accepted",
        "invariants": ["ESC-15"],
        "path": ESCROW,
        "old": "                    or not self.balances(referee).issued\n",
        "new": "",
        "tests": [SLASH_REFEREE],
        "why": "the referee cost is paid to a key that holds no unit of the ledger.",
    },
    {
        "id": "el-authority-signature-unchecked",
        "invariants": ["ESC-20"],
        "path": ESCROW,
        "old": "        if model.run_id != self.run_id or not verify(",
        "new": "        if model.run_id != self.run_id or (lambda *_: False)(",
        "tests": [GENESIS_AUTHORITY, REWARD_AUTHORITY],
        "why": "issuance no longer verifies the reward authority's signature.",
    },
    {
        "id": "el-authority-run-unbound",
        "invariants": ["ESC-20"],
        "path": ESCROW,
        "old": "        if model.run_id != self.run_id or not verify(",
        "new": "        if not verify(",
        "tests": [GENESIS_AUTHORITY],
        "why": "an issuance signed for another run is accepted by this one.",
    },
    {
        "id": "el-replay-lookup-disabled",
        "invariants": ["ESC-17", "ESC-20"],
        "path": ESCROW,
        "old": (
            "        if row is None:\n            return None\n"
            '        if row["request"] != request:'
        ),
        "new": ('        if True:\n            return None\n        if row["request"] != request:'),
        "tests": [PAY_ONCE, GENESIS_AUTHORITY, REWARD_AUTHORITY],
        "why": "an operation id is never found again, so every replay applies a second time.",
    },
    {
        "id": "el-operation-conflict-ignored",
        "invariants": ["ESC-17", "ESC-20"],
        "path": ESCROW,
        "old": (
            '        if row["request"] != request:\n'
            '            raise EscrowError("OPERATION_CONFLICT")'
        ),
        "new": '        if False:\n            raise EscrowError("OPERATION_CONFLICT")',
        "tests": [PAY_ONCE, GENESIS_AUTHORITY, REWARD_AUTHORITY],
        "why": "a different body under a known operation id gets the stored receipt of the first.",
    },
    {
        "id": "el-finalize-order-unchecked",
        "invariants": ["ESC-20"],
        "path": ESCROW,
        "old": "            if number != (last + 1 if last is not None else 0):",
        "new": "            if False:",
        "tests": [ROUND_ORDER],
        "why": "a reward round can be issued before the rounds that precede it.",
    },
    {
        "id": "el-issuance-ceiling-unchecked",
        "invariants": ["ESC-20"],
        "path": ESCROW,
        "old": (
            "                self.balances().issued + pending_units + reward.budget_units\n"
            "                > self.policy.max_total_issuance"
        ),
        "new": (
            "                self.balances().issued + pending_units + reward.budget_units\n"
            "                > 10**30"
        ),
        "tests": [ROUND_ORDER],
        "why": "reward issuance is no longer bounded by the pinned total.",
    },
    {
        "id": "el-verify-state-compare-dropped",
        "invariants": ["ESC-22"],
        "path": ESCROW,
        "old": "        if last_state is not None and last_state != self._snapshot():",
        "new": "        if False:",
        "tests": [REOPEN],
        "why": "reopening no longer compares the unit rows with the last recorded state.",
    },
    {
        "id": "el-verify-row-hash-unchecked",
        "invariants": ["ESC-22"],
        "path": ESCROW,
        "old": '                or row["event_hash"] != actual\n',
        "new": "",
        "tests": [REOPEN],
        "why": "the stored hash of an event is no longer recomputed at reopen.",
    },
    {
        "id": "el-verify-sequence-unchecked",
        "invariants": ["ESC-22"],
        "path": ESCROW,
        "old": '                row["seq"] != i\n                or row["prev_hash"] != prev\n',
        "new": '                row["prev_hash"] != prev\n',
        "tests": [REOPEN],
        "why": "reopening no longer checks that the event sequence numbers are contiguous.",
    },
    {
        "id": "el-verify-receipt-operation-unchecked",
        "invariants": ["ESC-22"],
        "path": ESCROW,
        "old": '                or receipt.operation_id != row["operation_id"]\n',
        "new": "",
        "tests": [REOPEN],
        "why": "reopening no longer checks that a stored receipt belongs to its event.",
    },
    {
        "id": "el-verify-receipt-mode-unchecked",
        "invariants": ["ESC-22"],
        "path": ESCROW,
        "old": "                or receipt.ledger_mode != self.policy.ledger_mode\n",
        "new": "",
        "tests": [REOPEN],
        "why": "reopening no longer checks that a stored receipt was issued in this ledger mode.",
    },
    {
        "id": "el-verify-final-conservation-dropped",
        "invariants": ["ESC-22", "ESC-06"],
        "path": ESCROW,
        "old": (
            '            raise EscrowError("PERSISTED_STATE_MISMATCH")\n'
            "        self._conservation()"
        ),
        "new": '            raise EscrowError("PERSISTED_STATE_MISMATCH")',
        "tests": [REOPEN_BARE],
        "why": "reopening a ledger with no event no longer re-checks per-origin conservation.",
    },
    {
        "id": "el-event-conservation-skipped",
        "invariants": ["ESC-06"],
        "path": ESCROW,
        "old": (
            "    def _event(self, operation_id: str, kind: str, request: str, owner: str)"
            " -> EscrowReceipt:\n"
            "        self._conservation()\n"
        ),
        "new": (
            "    def _event(self, operation_id: str, kind: str, request: str, owner: str)"
            " -> EscrowReceipt:\n"
        ),
        "tests": [STRAY_UNIT],
        "why": "an event no longer re-checks conservation before it is recorded.",
    },
    {
        "id": "el-amount-not-strict",
        "invariants": ["ESC-06"],
        "path": MESSAGES,
        "old": "Positive = Annotated[StrictInt, Field(ge=1, le=2**53 - 1)]",
        "new": "Positive = Annotated[int, Field(ge=1, le=2**53 - 1)]",
        "tests": [STRICT_UNITS],
        "why": "escrow amounts accept strings and booleans that coerce to integers.",
    },
    {
        "id": "el-stake-ignores-admission",
        "invariants": ["LCK-10", "LCK-11"],
        "path": ESCROW,
        "old": "            if op.admission_id == admission_id and op.owner == owner:",
        "new": "            if op.owner == owner:",
        "tests": [STAKE_COUNT, ONE_BACKING],
        "why": "the stake of one admission counts the locks of every admission of the coldkey.",
    },
    {
        "id": "el-stake-ignores-owner",
        "invariants": ["LCK-10"],
        "path": ESCROW,
        "old": (
            "            if op.admission_id == admission_id and op.owner == owner:\n"
            "                amount = self.db.execute(\n"
            '                    "SELECT COALESCE(SUM(units),0) FROM escrow_units WHERE "\n'
            "                    \"bucket='admission_locked' AND ref=? AND owner=?\",\n"
            "                    (op.operation_id, owner),\n"
        ),
        "new": (
            "            if op.admission_id == admission_id:\n"
            "                amount = self.db.execute(\n"
            '                    "SELECT COALESCE(SUM(units),0) FROM escrow_units WHERE "\n'
            "                    \"bucket='admission_locked' AND ref=?\",\n"
            "                    (op.operation_id,),\n"
        ),
        "tests": [STAKE_COUNT],
        "why": "the stake of an admission counts locks that another owner placed on it.",
    },
    {
        "id": "el-bond-kind-unchecked",
        "invariants": ["ESC-24", "DIA-01"],
        "path": DISPUTES,
        "old": '            if lockrow is None or lockrow[0] != "LOCK_CONTEST":',
        "new": "            if lockrow is None:",
        "tests": [BOND],
        "why": "an admission stake can be named as the bond of a dispute.",
    },
    {
        "id": "el-bond-owner-unbound",
        "invariants": ["ESC-24", "DIA-01"],
        "path": DISPUTES,
        "old": (
            "            if (lock.owner, lock.dispute_id) != "
            "(self.owner_of(contest.miner), dispute_id) or ("
        ),
        "new": "            if (lock.dispute_id,) != (dispute_id,) or (",
        "tests": [BOND],
        "why": "a bond owned by another coldkey opens a dispute for this miner.",
    },
    {
        "id": "el-bond-dispute-unbound",
        "invariants": ["ESC-24", "DIA-01"],
        "path": DISPUTES,
        "old": (
            "            if (lock.owner, lock.dispute_id) != "
            "(self.owner_of(contest.miner), dispute_id) or ("
        ),
        "new": "            if (lock.owner,) != (self.owner_of(contest.miner),) or (",
        "tests": [BOND],
        "why": "a bond locked for another dispute opens this one.",
    },
    {
        "id": "el-bond-floor-dropped",
        "invariants": ["ESC-24", "DIA-01"],
        "path": DISPUTES,
        "old": (
            "                amount != lock.units or amount < self.policy.max_referee_cost_units"
        ),
        "new": "                amount != lock.units",
        "tests": [BOND],
        "why": "a bond smaller than the maximum referee cost opens a dispute.",
    },
    {
        "id": "el-one-open-dispute-per-miner-dropped",
        "invariants": ["ESC-24"],
        "path": DISPUTES,
        "old": (
            "            if sum(t.resolution is None for t in active) >= self.policy.max_open "
            "or any(\n"
            "                t.resolution is None and t.contest.miner == contest.miner "
            "for t in active\n"
            "            ):"
        ),
        "new": "            if sum(t.resolution is None for t in active) >= self.policy.max_open:",
        "tests": [ONE_OPEN],
        "why": "a miner may keep several disputes open at once.",
    },
    {
        "id": "el-settle-id-varies-per-call",
        "invariants": ["DIA-10"],
        "path": DISPUTES,
        "old": (
            '                operation_id=sha256_hex(canonicalize([turn.dispute_id, "settle"])),'
        ),
        "new": (
            "                operation_id=sha256_hex("
            'canonicalize([turn.dispute_id, "settle", now])),'
        ),
        "tests": [SETTLE_ONCE],
        "why": "every settle call carries a new operation id, so a bond can be settled twice.",
    },
    {
        "id": "el-maturity-ignores-fraud",
        "invariants": ["ESC-08"],
        "path": ESCROW,
        "old": (
            "                        status.unresolved\n"
            '                        or status.outcome == "FRAUD"\n'
        ),
        "new": "                        status.unresolved\n                        or False\n",
        "tests": [MATURITY],
        "why": "a reward origin whose settlement is FRAUD still vests.",
    },
    {
        "id": "el-maturity-ignores-vesting-round",
        "invariants": ["ESC-08"],
        "path": ESCROW,
        "old": '                if record is None or now_beacon < record["mature_at"]:',
        "new": "                if record is None:",
        "tests": [MATURITY],
        "why": "a reward origin vests before its maturity round.",
    },
    {
        "id": "el-maturity-ignores-settlement-hashes",
        "invariants": ["ESC-08"],
        "path": ESCROW,
        "old": (
            "                        or finality is None\n"
            '                        or status.finality_hash != finality["finalize_hash"]\n'
        ),
        "new": "                        or finality is None\n",
        "tests": [MATURITY],
        "why": "a reward origin vests on a settlement that answers another finalize.",
    },
    {
        "id": "el-maturity-ignores-release-beacon",
        "invariants": ["ESC-08"],
        "path": ESCROW,
        "old": "                        or now_beacon < status.release_beacon\n",
        "new": "",
        "tests": [MATURITY],
        "why": "a reward origin vests before the release horizon of its settlement.",
    },
    {
        "id": "el-maturity-ignores-open-dispute",
        "invariants": ["ESC-08"],
        "path": ESCROW,
        "old": (
            "                        status.unresolved\n"
            '                        or status.outcome == "FRAUD"\n'
        ),
        "new": (
            '                        False\n                        or status.outcome == "FRAUD"\n'
        ),
        "tests": [MATURITY],
        "why": "a reward origin whose dispute is still open vests.",
    },
    {
        "id": "el-http-pay-route-added",
        "invariants": ["ESC-02"],
        "path": "src/hypertrain/challenge/app.py",
        "old": '        ("escrow/release", "release"),\n',
        "new": '        ("escrow/release", "release"),\n        ("escrow/pay", "pay"),\n',
        "tests": [ROUTES],
        "why": "a client route for pay is registered next to lock, transfer and release.",
    },
    {
        "id": "el-fraud-settlement-slashes-with-caller-authority",
        "invariants": ["ESC-13"],
        "path": "src/hypertrain/challenge/admission.py",
        "old": CONVICTION,
        "new": (
            "            self.escrow.slash(\n"
            "                EscrowRelease.model_validate({}), authority=reason, now_beacon=now\n"
            "            )\n" + CONVICTION
        ),
        "tests": [SLASH_CALLERS],
        "why": "a third slash call site takes its authority from the caller's arguments.",
    },
    {
        "id": "el-bond-seizure-slashes-with-the-owners-key",
        "invariants": ["ESC-13"],
        "path": "src/hypertrain/challenge/admission.py",
        "old": SEIZURE_AUTHORITY,
        "new": SEIZURE_AUTHORITY.replace("self.coord.ss58", "lock.owner"),
        "tests": [SLASH_CALLERS],
        "why": "the seizure of an admission bond passes the bond owner's key as slash authority.",
    },
    {
        "id": "el-payable-ignores-holder",
        "invariants": ["ESC-16", "LCK-24"],
        "path": ESCROW,
        "old": (
            '            "AND EXISTS(SELECT 1 FROM escrow_units p WHERE p.origin=u.origin "\n'
            "            \"AND p.owner=u.owner AND p.bucket='reward_pending') \"\n"
        ),
        "new": "",
        "tests": [HELD, MODEL],
        "why": "units transferred to another owner stay payable to the origin's hotkey.",
    },
    {
        "id": "el-payable-includes-locked",
        "invariants": ["ESC-16", "LCK-24"],
        "path": ESCROW,
        "old": "JOIN escrow_origins o ON o.origin=u.origin WHERE u.bucket='available' ",
        "new": (
            "JOIN escrow_origins o ON o.origin=u.origin WHERE u.bucket "
            "IN ('available','admission_locked','dispute_locked') "
        ),
        "tests": [HELD, LOCKED_NEVER, MODEL],
        "why": "stake and bond units become payable weight while they are locked.",
    },
    {
        "id": "el-payable-includes-collateral",
        "invariants": ["ESC-16"],
        "path": ESCROW,
        "old": "AND u.units>0 AND o.hotkey IS NOT NULL AND o.mature_at<=? ",
        "new": "AND u.units>0 AND o.mature_at<=? ",
        "tests": [COLLATERAL],
        "why": "genesis and bootstrap collateral, which has no hotkey, becomes payable weight.",
    },
    {
        "id": "el-pay-id-without-epoch",
        "invariants": ["ESC-17"],
        "path": STORE,
        "old": "f\"ht-pay/2|{escrow.run_id}|{epoch}|{row['origin']}|{row['owner']}\".encode()",
        "new": "f\"ht-pay/2|{escrow.run_id}|{row['origin']}|{row['owner']}\".encode()",
        "tests": [SHARED_MASS],
        "why": "the pay id loses its epoch, so the second epoch's payment replays the first one.",
    },
    {
        "id": "el-pay-committed-before-the-answer",
        "invariants": ["ESC-17"],
        "path": STORE,
        "old": "                answer = self._pay_v2(escrows, epoch, epoch_at, answer)\n",
        "new": (
            "                answer = self._pay_v2(escrows, epoch, epoch_at, answer)\n"
            '                db.execute("COMMIT")\n'
            '                db.execute("BEGIN IMMEDIATE")\n'
        ),
        "tests": [PAY_ROLLS_BACK],
        "why": "units are paid in a transaction of their own, before the epoch answer is stored.",
    },
    {
        "id": "el-pay-ignores-epoch-budget",
        "invariants": ["ESC-17"],
        "path": STORE,
        "old": '                take = min(budget, row["units"])',
        "new": '                take = row["units"]',
        "tests": [SHARED_MASS],
        "why": "an epoch pays every unit of the origin, whatever mass is left.",
    },
    {
        "id": "el-contest-lock-route-trusts-envelope-signer",
        "invariants": ["ESC-01", "ES-01"],
        "path": STORE,
        "old": CONTEST_LOCK_GATE.format("op.owner"),
        "new": CONTEST_LOCK_GATE.format("env.signer"),
        "tests": [OWNER_HTTP],
        "why": "the contest-lock route accepts any signer for the owner it names.",
    },
    {
        "id": "el-admission-lock-signer-unchecked",
        "invariants": ["ES-03", "LCK-10"],
        "path": "src/hypertrain/challenge/admission.py",
        "old": ADMISSION_LOCK_GATE.format("s == r.coldkey"),
        "new": ADMISSION_LOCK_GATE.format("True"),
        "tests": [STAKE_HTTP],
        "why": "an admission lock is accepted from any signer, not the coldkey recorded at join.",
    },
    {
        "id": "el-admission-lock-owner-unchecked",
        "invariants": ["ES-03", "LCK-10"],
        "path": "src/hypertrain/challenge/admission.py",
        "old": '            if op.owner != r.coldkey or op.kind != "LOCK_ADMISSION":',
        "new": '            if op.kind != "LOCK_ADMISSION":',
        "tests": [STAKE_HTTP],
        "why": "a lock for an admission may name an owner other than the coldkey recorded at join.",
    },
]
