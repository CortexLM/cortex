"""Regressions of the weights path that test_weights.py / test_weights_2.py must kill (D39)."""

STORE = "src/hypertrain/challenge/store.py"
ESCROW = "src/hypertrain/ledger/escrow_v2.py"
LEDGER = "src/hypertrain/ledger/ledger.py"
APP = "src/hypertrain/challenge/app.py"

W1 = "tests/ledger_invariants/test_weights.py::"
W2 = "tests/ledger_invariants/test_weights_2.py::"
PAY = "tests/challenge/test_v2_weights_pay.py::"
CONTRACT = "tests/challenge/test_contract.py::"
LEDGER_TESTS = "tests/ledger/test_ledger.py::"
BOOTSTRAP = "tests/ledger/test_production_bootstrap_v2.py::"

GATE = W1 + "test_a_reward_whose_settlement_is_not_closed_stays_pending_and_pays_nothing"
PROPERTY = W1 + "test_every_stored_answer_conserves_the_mass_and_pays_what_the_documentation_says"
EPOCH_AT_PROPERTY = (
    W1 + "test_no_unit_pays_before_the_pushed_beacon_matured_it_whatever_the_epoch_at"
)
FINAL = W1 + "test_an_epoch_answer_is_final_for_any_later_epoch_at_computed_at_and_restart"
BEYOND = W1 + "test_an_origin_is_never_paid_beyond_what_it_issued_across_epochs"
ORDER = W1 + "test_payment_order_is_oldest_maturity_then_origin_id_whatever_the_insertion_order"
LOWER = W1 + "test_an_epoch_below_the_newest_answered_one_is_empty_final_and_pays_later"
EARLIER = W1 + "test_a_higher_epoch_with_an_earlier_epoch_at_is_out_of_order"
MERGED = W2 + "test_a_merged_answer_sums_the_runs_and_each_run_pays_only_its_own_origins"
MERGE_PROPERTY = (
    W2 + "test_merging_runs_is_sum_preserving_and_independent_of_the_order_of_the_children"
)
CHILD_PAID = (
    W2 + "test_units_a_child_paid_while_the_root_failed_to_persist_reach_a_delivered_answer"
)
REPLAY = (
    W2 + "test_a_stored_answer_replays_without_the_v2_services_and_not_while_the_metagraph_is_down"
)

MUTANTS = [
    {
        "id": "wgt-genesis-collateral-is-payable",
        "invariants": ["WGT-01"],
        "path": ESCROW,
        "old": "AND u.units>0 AND o.hotkey IS NOT NULL AND o.mature_at<=? ",
        "new": "AND u.units>0 AND o.mature_at<=? ",
        "tests": [
            W1 + "test_genesis_collateral_matured_next_to_a_live_origin_is_never_emitted",
            BOOTSTRAP + "test_bootstrap_units_never_payable",
        ],
        "why": "payable() lists origins without a hotkey: free genesis and bootstrap units emit.",
    },
    {
        "id": "wgt-shadow-reward-is-payable",
        "invariants": ["WGT-01"],
        "path": ESCROW,
        "old": '"AND o.origin NOT IN (SELECT origin FROM escrow_shadow_finalized) "',
        "new": '"AND 1=1 "',
        "tests": [PAY + "test_shadow_bootstrap_reward_not_paid"],
        "why": "payable() stops excluding shadow-finalized origins, so trial collateral emits.",
    },
    {
        "id": "wgt-unaccepted-settlement-is-accepted",
        "invariants": ["WGT-02"],
        "path": STORE,
        "old": '        if row is None:\n            raise EscrowError("UNACCEPTED_SETTLEMENT")\n',
        "new": (
            "        if row is None:\n"
            "            return Settlement(\n"
            '                finality_hash=str(self._db.execute("SELECT finalize_hash FROM '
            'escrow_finalized").fetchone()[0]),\n'
            '                closed_dispute_root="00" * 32,\n'
            "                release_beacon=0,\n"
            "                unresolved=False,\n"
            '                outcome="MATCH",\n'
            "            )\n"
        ),
        "tests": [GATE + "[unaccepted-UNACCEPTED_SETTLEMENT]"],
        "why": "a reward with no accepted settlement record vests as if it were closed.",
    },
    {
        "id": "wgt-open-dispute-does-not-hold-the-reward",
        "invariants": ["WGT-02"],
        "path": ESCROW,
        "old": "                        status.unresolved\n                        or status.out",
        "new": "                        False\n                        or status.out",
        "tests": [GATE + "[unresolved-DISPUTED_OR_UNVESTED]"],
        "why": "maturity ignores an unresolved dispute on the reward origin.",
    },
    {
        "id": "wgt-fraud-does-not-hold-the-reward",
        "invariants": ["WGT-02"],
        "path": ESCROW,
        "old": '                        or status.outcome == "FRAUD"\n',
        "new": "",
        "tests": [GATE + "[fraud-DISPUTED_OR_UNVESTED]"],
        "why": "maturity ignores a FRAUD settlement of the reward origin.",
    },
    {
        "id": "wgt-release-window-is-ignored",
        "invariants": ["WGT-02", "WGT-03"],
        "path": ESCROW,
        "old": "                        or now_beacon < status.release_beacon\n",
        "new": "",
        "tests": [GATE + "[inside_window-DISPUTED_OR_UNVESTED]"],
        "why": "maturity ignores the settlement's release beacon, so units vest inside the window.",
    },
    {
        "id": "wgt-foreign-finality-vests-the-reward",
        "invariants": ["WGT-02"],
        "path": ESCROW,
        "old": '                        or status.finality_hash != finality["finalize_hash"]\n',
        "new": "",
        "tests": [GATE + "[other_finality-DISPUTED_OR_UNVESTED]"],
        "why": "maturity accepts a settlement of another finality than the one that issued it.",
    },
    {
        "id": "wgt-payable-reads-pending-units",
        "invariants": ["WGT-03"],
        "path": ESCROW,
        "old": "WHERE u.bucket='available' ",
        "new": "WHERE u.bucket IN ('available','reward_pending') ",
        "tests": [
            EPOCH_AT_PROPERTY,
            W1 + "test_a_unit_vests_at_its_maturity_round_not_before",
            W1 + "test_a_round_without_payable_units_answers_a_defined_full_burn[pending]",
        ],
        "why": "payable() also lists reward_pending units, so epoch_at alone decides vesting.",
    },
    {
        "id": "wgt-payable-ignores-the-epoch-round",
        "invariants": ["WGT-03"],
        "path": ESCROW,
        "old": "AND o.mature_at<=? ",
        "new": "AND ?>=0 ",
        "tests": [
            EPOCH_AT_PROPERTY,
            W1 + "test_a_round_without_payable_units_answers_a_defined_full_burn[early]",
            W1 + "test_a_partly_vested_epoch_pays_the_vested_part_and_burns_the_rest",
            PAY + "test_immature_round_not_paid_until_epoch_reaches_mature_at",
        ],
        "why": "payable() drops the maturity round of the epoch, so a vested unit pays early.",
    },
    {
        "id": "wgt-vesting-ignores-the-maturity-round",
        "invariants": ["WGT-03"],
        "path": ESCROW,
        "old": '                if record is None or now_beacon < record["mature_at"]:',
        "new": "                if record is None:",
        "tests": [W1 + "test_a_unit_vests_at_its_maturity_round_not_before"],
        "why": "mature() vests a unit before the beacon reached its maturity round.",
    },
    {
        "id": "wgt-payable-lists-transferred-units",
        "invariants": ["WGT-04"],
        "path": ESCROW,
        "old": (
            '            "AND EXISTS(SELECT 1 FROM escrow_units p WHERE p.origin=u.origin "\n'
            "            \"AND p.owner=u.owner AND p.bucket='reward_pending') \"\n"
        ),
        "new": '            "AND 1=1 "\n',
        "tests": [
            PAY + "test_held_units_never_paid",
            W1 + "test_a_client_event_carrying_the_pay_id_never_blocks_the_payout[False]",
        ],
        "why": "payable() drops the issued-owner check, so units moved to another owner emit.",
    },
    {
        "id": "wgt-payable-reads-locked-units",
        "invariants": ["WGT-04"],
        "path": ESCROW,
        "old": "WHERE u.bucket='available' ",
        "new": "WHERE u.bucket IN ('available','admission_locked','dispute_locked') ",
        "tests": [
            PAY + "test_held_units_never_paid",
            PAY + "test_dispute_locked_and_burned_units_never_paid",
        ],
        "why": "payable() lists locked units: a bond behind an admission or a contest emits.",
    },
    {
        "id": "wgt-answer-is-stored-under-epoch-at",
        "invariants": ["WGT-06", "WGT-15", "WGT-17"],
        "path": STORE,
        "old": 'db.execute("INSERT INTO weights_runs VALUES(?, ?)", (epoch, merged))',
        "new": 'db.execute("INSERT INTO weights_runs VALUES(?, ?)", (epoch_at, merged))',
        "tests": [
            FINAL,
            W1 + "test_a_stored_answer_does_not_move_when_units_are_locked_or_burned_later",
            CHILD_PAID + "[True]",
            CONTRACT + "test_get_weights_first_answer_is_immutable",
        ],
        "why": "the final answer is keyed on epoch_at, so a re-ask with another time recomputes.",
    },
    {
        "id": "wgt-stored-answer-is-recomputed-with-services",
        "invariants": ["WGT-20"],
        "path": STORE,
        "old": (
            "            if stored is not None:  # final: a replay needs no v2 services\n"
            "                return bytes(stored[0])\n"
        ),
        "new": "            if False:\n                return bytes(stored[0])\n",
        "tests": [REPLAY],
        "why": "a stored epoch is no longer returned before the v2 services are built.",
    },
    {
        "id": "wgt-pay-and-persist-commit-apart",
        "invariants": ["WGT-07"],
        "path": STORE,
        "old": (
            "            if escrows:\n"
            "                answer = self._pay_v2(escrows, epoch, epoch_at, answer)\n"
        ),
        "new": (
            "            if escrows:\n"
            "                answer = self._pay_v2(escrows, epoch, epoch_at, answer)\n"
            '                db.execute("COMMIT")\n'
            '                db.execute("BEGIN IMMEDIATE")\n'
        ),
        "tests": [
            W1 + "test_a_failed_persist_pays_nothing_and_the_retry_of_the_same_epoch_pays_once"
        ],
        "why": "the pay commits before the answer is persisted: a crash keeps the pay only.",
    },
    {
        "id": "wgt-order-gate-ignores-the-epoch",
        "invariants": ["WGT-08"],
        "path": LEDGER,
        "old": "        if epoch < self.last_epoch or epoch_at < self.last_answer_at:",
        "new": "        if epoch_at < self.last_answer_at:",
        "tests": [LOWER],
        "why": "an older epoch asked at a later time is answered in order and pays.",
    },
    {
        "id": "wgt-order-gate-ignores-the-epoch-at",
        "invariants": ["WGT-08"],
        "path": LEDGER,
        "old": "        if epoch < self.last_epoch or epoch_at < self.last_answer_at:",
        "new": "        if epoch < self.last_epoch:",
        "tests": [EARLIER],
        "why": "a higher epoch asked at an earlier time is answered in order and pays.",
    },
    {
        "id": "wgt-out-of-order-epoch-is-completed-with-v2",
        "invariants": ["WGT-08"],
        "path": STORE,
        "old": '        if meta["reason"] != "ok":\n            return text\n',
        "new": "        if False:\n            return text\n",
        "tests": [LOWER, EARLIER],
        "why": "v2 units fill the mass of an out_of_order answer, which would reorder emission.",
    },
    {
        "id": "wgt-service-build-failure-is-swallowed",
        "invariants": ["WGT-10"],
        "path": STORE,
        "old": (
            "        escrows = [\n"
            "            self._services(r[0])[0]\n"
            "            for r in self._db.execute("
            '"SELECT run_id, manifest FROM runs ORDER BY run_id")\n'
            '            if live and json.loads(r[1]).get("manifest_version") == 2\n'
            "        ]\n"
        ),
        "new": (
            "        escrows = []\n"
            "        for r in self._db.execute("
            '"SELECT run_id, manifest FROM runs ORDER BY run_id"):\n'
            '            if live and json.loads(r[1]).get("manifest_version") == 2:\n'
            "                try:\n"
            "                    escrows.append(self._services(r[0])[0])\n"
            "                except ChallengeError:\n"
            "                    continue\n"
        ),
        "tests": [
            W1 + "test_a_failing_service_build_refuses_the_new_epoch_before_anything_is_journaled"
        ],
        "why": "a service that cannot be built is skipped: a final plain burn is stored.",
    },
    {
        "id": "wgt-pay-ignores-the-epoch-budget",
        "invariants": ["WGT-11", "WGT-12"],
        "path": STORE,
        "old": '                take = min(budget, row["units"])',
        "new": '                take = row["units"]',
        "tests": [PROPERTY, BEYOND],
        "why": "an origin is paid whole whatever is left of the epoch's mass.",
    },
    {
        "id": "wgt-budget-is-not-reduced-by-a-payout",
        "invariants": ["WGT-11", "WGT-12"],
        "path": STORE,
        "old": "                budget -= take\n",
        "new": "                budget -= 0\n",
        "tests": [PROPERTY, BEYOND],
        "why": "the epoch keeps its whole mass after paying, so one answer can exceed it.",
    },
    {
        "id": "wgt-units-paid-is-not-updated",
        "invariants": ["WGT-11"],
        "path": STORE,
        "old": '        meta["units_paid"] += paid\n',
        "new": '        meta["units_paid"] += 0\n',
        "tests": [
            PROPERTY,
            W1 + "test_origins_of_one_hotkey_are_summed_into_one_entry_whatever_their_owner",
        ],
        "why": "units_paid no longer counts the v2 payouts, so paid plus burned is off the mass.",
    },
    {
        "id": "wgt-origins-of-one-hotkey-overwrite-each-other",
        "invariants": ["WGT-14"],
        "path": STORE,
        "old": '                weights[row["hotkey"]] = weights.get(row["hotkey"], 0.0) + take\n',
        "new": '                weights[row["hotkey"]] = take\n',
        "tests": [
            W1 + "test_origins_of_one_hotkey_are_summed_into_one_entry_whatever_their_owner",
            PAY + "test_oldest_maturity_paid_first_and_same_hotkey_accumulates",
        ],
        "why": "the second origin of a hotkey replaces the first instead of adding to it.",
    },
    {
        "id": "wgt-pay-id-includes-the-epoch-at",
        "invariants": ["WGT-13", "WGT-19"],
        "path": STORE,
        "old": "f\"ht-pay/2|{escrow.run_id}|{epoch}|{row['origin']}|{row['owner']}\".encode()",
        "new": (
            "f\"ht-pay/2|{escrow.run_id}|{epoch}|{row['origin']}|{row['owner']}|{epoch_at}\""
            ".encode()"
        ),
        "tests": [
            W1 + "test_pay_ids_depend_only_on_run_epoch_origin_and_owner",
            W1 + "test_each_stored_answer_is_the_sum_of_the_pay_events_that_carry_its_epoch",
        ],
        "why": "the pay id depends on the caller's epoch_at, so a retry would pay a second time.",
    },
    {
        "id": "wgt-pay-id-drops-the-epoch",
        "invariants": ["WGT-12", "WGT-13", "WGT-19"],
        "path": STORE,
        "old": "f\"ht-pay/2|{escrow.run_id}|{epoch}|{row['origin']}|{row['owner']}\".encode()",
        "new": "f\"ht-pay/2|{escrow.run_id}|{row['origin']}|{row['owner']}\".encode()",
        "tests": [
            BEYOND,
            W1 + "test_each_stored_answer_is_the_sum_of_the_pay_events_that_carry_its_epoch",
        ],
        "why": "two epochs share one pay id, so the second partial payout of an origin conflicts.",
    },
    {
        "id": "wgt-newest-maturity-is-paid-first",
        "invariants": ["WGT-22"],
        "path": ESCROW,
        "old": '"ORDER BY o.mature_at,u.origin,u.owner",',
        "new": '"ORDER BY o.mature_at DESC,u.origin,u.owner",',
        "tests": [
            ORDER,
            PAY + "test_oldest_maturity_paid_first_and_same_hotkey_accumulates",
        ],
        "why": "the newest vested units are paid first and the oldest wait.",
    },
    {
        "id": "wgt-tie-break-follows-the-reverse-origin-id",
        "invariants": ["WGT-22"],
        "path": ESCROW,
        "old": '"ORDER BY o.mature_at,u.origin,u.owner",',
        "new": '"ORDER BY o.mature_at,u.origin DESC,u.owner",',
        "tests": [ORDER],
        "why": "ties of one maturity round are paid in the reverse of the documented order.",
    },
    {
        "id": "wgt-entry-cap-is-ignored",
        "invariants": ["WGT-22"],
        "path": STORE,
        "old": '                if row["hotkey"] not in weights and len(weights) >= MAX_ENTRIES:',
        "new": "                if False:",
        "tests": [PAY + "test_hotkey_past_entry_cap_keeps_units_for_later"],
        "why": "a new hotkey past the entry cap is paid anyway.",
    },
    {
        "id": "wgt-merge-overwrites-a-shared-hotkey",
        "invariants": ["WGT-15", "WGT-14"],
        "path": STORE,
        "old": "            weights[hotkey] = weights.get(hotkey, 0.0) + units",
        "new": "            weights[hotkey] = units",
        "tests": [MERGED, MERGE_PROPERTY],
        "why": "the merge keeps the last store's weight of a hotkey instead of the sum.",
    },
    {
        "id": "wgt-merge-keeps-one-run-s-mass",
        "invariants": ["WGT-15"],
        "path": STORE,
        "old": '            "full_share_mass": sum(a["full_share_mass"] for a in parsed),',
        "new": '            "full_share_mass": first["full_share_mass"],',
        "tests": [MERGED, MERGE_PROPERTY],
        "why": "the merged answer reports the mass of one run while it lists the weight of all.",
    },
    {
        "id": "wgt-merge-keeps-one-run-s-counters",
        "invariants": ["WGT-15"],
        "path": STORE,
        "old": '        meta[key] = sum(a["metadata"][key] for a in parsed)',
        "new": '        meta[key] = first["metadata"][key]',
        "tests": [MERGED, MERGE_PROPERTY],
        "why": "the merged counters are the root's alone, so paid plus burned is off the mass.",
    },
    {
        "id": "wgt-second-run-shares-the-escrow-database",
        "invariants": ["WGT-23"],
        "path": ESCROW,
        "old": '                or rows[0]["run_id"] != self.run_id\n',
        "new": "",
        "tests": [
            W2 + "test_a_second_escrow_cannot_be_opened_for_another_run_on_the_same_database"
        ],
        "why": "an escrow of another run opens on a database that holds one, mixing their origins.",
    },
    {
        "id": "wgt-default-epoch-at-is-the-epoch-start",
        "invariants": ["WGT-09"],
        "path": STORE,
        "old": "genesis_unix + (epoch + 1) * self.params.epoch_seconds",
        "new": "genesis_unix + epoch * self.params.epoch_seconds",
        "tests": [W2 + "test_the_default_epoch_at_is_the_end_of_the_epoch"],
        "why": "an epoch asked without epoch_at is answered at its start, a whole epoch early.",
    },
    {
        "id": "wgt-replay-skips-the-metagraph",
        "invariants": ["WGT-20", "AU-06"],
        "path": APP,
        "old": '            await metagraph.hotkeys() if json.loads(text)["weights"] else {}',
        "new": "            {}",
        "tests": [
            REPLAY,
            W2 + "test_the_registered_filtered_answer_is_final_even_after_the_hotkey_leaves",
        ],
        "why": "the route never asks the master: every hotkey looks deregistered and is dropped.",
    },
    {
        "id": "wgt-registered-answer-is-filtered-again",
        "invariants": ["AU-06", "WGT-06"],
        "path": APP,
        "old": (
            "            if row is not None:\n"
            "                return bytes(row[0])\n"
            "            answer = json.loads(text)\n"
        ),
        "new": "            answer = json.loads(text)\n",
        "tests": [W2 + "test_the_registered_filtered_answer_is_final_even_after_the_hotkey_leaves"],
        "why": "the master gets a different filtered answer for an epoch once a hotkey leaves.",
    },
    {
        "id": "wgt-get-weights-needs-no-internal-bearer",
        "invariants": ["AU-06"],
        "path": APP,
        "old": "        _require(config.internal_token_file, authorization)\n        if x_platform",
        "new": "        if x_platform",
        "tests": [
            CONTRACT + "test_get_weights_auth",
            "tests/ledger_invariants/test_infra_guards.py::"
            "test_get_weights_needs_the_internal_bearer_and_a_refusal_changes_no_state",
        ],
        "why": "get_weights answers without the internal bearer.",
    },
    {
        "id": "wgt-get-weights-ignores-the-challenge-slug",
        "invariants": ["AU-06"],
        "path": APP,
        "old": "        if x_platform_challenge_slug != config.slug:",
        "new": "        if False:",
        "tests": [
            CONTRACT + "test_get_weights_auth",
            "tests/ledger_invariants/test_infra_guards.py::"
            "test_get_weights_needs_the_internal_bearer_and_a_refusal_changes_no_state",
        ],
        "why": "get_weights answers a request meant for another challenge.",
    },
    {
        "id": "wgt-rebuild-accepts-a-changed-answer",
        "invariants": ["WGT-18"],
        "path": LEDGER,
        "old": '            if canonical(answer) != canonical(ans) or payments != rec["payments"]:',
        "new": "            if False:",
        "tests": [LEDGER_TESTS + "test_rechained_tampered_answer_raises_rebuild_mismatch"],
        "why": "a journal answer that the ledger does not reproduce is accepted at open.",
    },
    {
        "id": "wgt-rebuild-accepts-a-second-answer-for-an-epoch",
        "invariants": ["WGT-18"],
        "path": LEDGER,
        "old": '            if ans["epoch"] in self.answers:\n',
        "new": "            if False:\n",
        "tests": [W1 + "test_a_journal_that_answers_one_epoch_twice_is_refused_at_open"],
        "why": "a second journal answer for one epoch is replayed instead of refused.",
    },
    {
        "id": "wgt-empty-epoch-burns-nothing",
        "invariants": ["WGT-10", "WGT-11"],
        "path": LEDGER,
        "old": '                "units_burned_this_epoch": FULL_SHARE_MASS - paid_now,',
        "new": '                "units_burned_this_epoch": 0,',
        "tests": [
            W2 + "test_an_epoch_of_a_run_without_rewards_is_a_defined_full_burn_and_final[0]",
        ],
        "why": "an epoch with nothing payable reports no burned mass, so the share is unaccounted.",
    },
    {
        "id": "wgt-answer-envelope-loses-a-field",
        "invariants": ["WGT-11", "WGT-18"],
        "path": LEDGER,
        "old": '                "round_at": round_at,\n',
        "new": "",
        "tests": [W1 + "test_the_answer_envelope_has_exactly_the_documented_fields"],
        "why": "the stored answer drops a documented metadata field the master may read.",
    },
    {
        "id": "wgt-unfinalized-run-owes-weight",
        "invariants": ["WGT-02", "WGT-10"],
        "path": STORE,
        "old": (
            '            and self._db.execute("SELECT 1 FROM escrow_finalized LIMIT 1")'
            ".fetchone()\n"
        ),
        "new": "            and True\n",
        "tests": [
            PAY + "test_no_v2_pay_before_any_live_reward_finalized",
            W1 + "test_a_store_that_finalized_no_reward_answers_a_defined_full_burn[True]",
        ],
        "why": "a run that never finalized a live reward is asked for v2 weight anyway.",
    },
    {
        "id": "wgt-equal-epoch-at-is-out-of-order",
        "invariants": ["WGT-08"],
        "path": LEDGER,
        "old": "        if epoch < self.last_epoch or epoch_at < self.last_answer_at:",
        "new": "        if epoch < self.last_epoch or epoch_at <= self.last_answer_at:",
        "tests": [W1 + "test_epochs_asked_at_the_same_chain_time_are_both_in_order"],
        "why": "an epoch asked at the chain time of the previous answer is wrongly out_of_order.",
    },
    {
        "id": "wgt-journal-answer-is-recomputed-on-retry",
        "invariants": ["WGT-07"],
        "path": LEDGER,
        "old": (
            "        if epoch in self._engine.answers:\n"
            '            return canonical(self._engine.answers[epoch]["answer"])\n'
        ),
        "new": (
            "        if False:\n"
            '            return canonical(self._engine.answers[epoch]["answer"])\n'
        ),
        "tests": [
            W1 + "test_a_failed_persist_pays_nothing_and_the_retry_of_the_same_epoch_pays_once"
        ],
        "why": "a retry of a journaled epoch recomputes the v1 answer instead of reading it.",
    },
    {
        "id": "wgt-merge-forgets-an-additive-counter",
        "invariants": ["WGT-15"],
        "path": STORE,
        "old": (
            '        "units_burned_this_epoch",\n'
            '        "hotkeys_capped",\n'
            '        "ledger_minted",\n'
        ),
        "new": '        "units_burned_this_epoch",\n        "ledger_minted",\n',
        "tests": [MERGE_PROPERTY],
        "why": "the merged answer reports the capped count of the first run alone.",
    },
]
