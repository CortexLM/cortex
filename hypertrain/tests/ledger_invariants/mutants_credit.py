"""Mutants of the credit package: each is a small regression of the credit paths that one of the
named tests must catch. Read by scripts/check_ledger_invariant_mutants.py; the tree is not edited.
"""

from __future__ import annotations

from typing import Any

ESCROW = "src/hypertrain/ledger/escrow_v2.py"
STORE = "src/hypertrain/challenge/store.py"
FINALITY = "src/hypertrain/challenge/finality_v2.py"
ADMISSION = "src/hypertrain/challenge/admission.py"

C1 = "tests/ledger_invariants/test_credit.py::"
C2 = "tests/ledger_invariants/test_credit_2.py::"
MATRIX = C1 + "test_no_credit_without_audited_match_work_bound_to_this_round_miner_and_owner"
REWARD_DISAGREES = C1 + "test_a_signed_reward_that_disagrees_with_the_evidence_credits_nothing"
BOOTSTRAP_FAULT = C2 + "test_a_bootstrap_that_is_not_the_pinned_one_is_refused_with_its_code"
RESTART = C1 + (
    "test_an_identical_finalize_after_a_restart_returns_the_stored_receipt_and_moves_nothing"
)
QUORUM = C1 + (
    "test_the_service_credits_on_one_verdict_per_miner_and_a_quorum_ledger_would_refuse_it"
)
SHADOW_SETTLEMENT_ARGUMENT = (
    "                shadow_settlement=lambda origin: self._shadow_settlement_v2(run_id, origin),\n"
)
SETTLE = C2 + "test_a_reward_origin_matures_only_on_an_accepted_resolved_clean_vested_settlement"
FINALIZE_CALL = (
    "        return await run(store.for_run(run_id).finalize_v2, run_id, w, await v2_raw(request))"
)
MODEL = (
    C1 + "test_finalize_attempts_in_any_order_credit_each_round_once_and_follow_the_ledger_model"
)
PINNED_ALLOCATION = C2 + "test_origins_that_do_not_hash_to_the_pinned_allocation_are_refused"


def mutant(
    id: str,
    invariants: list[str],
    path: str,
    old: str,
    new: str,
    tests: list[str],
    why: str,
) -> dict[str, Any]:
    return {
        "id": id,
        "invariants": invariants,
        "path": path,
        "old": old,
        "new": new,
        "tests": tests,
        "why": why,
    }


MUTANTS = [
    mutant(
        "credit-op-id-includes-the-finalize-hash",
        ["CRD-01", "CRD-02"],
        ESCROW,
        'f"ht-reward-finalize-v2|{self.run_id}|{reward.w}".encode()',
        'f"ht-reward-finalize-v2|{self.run_id}|{reward.w}|{reward.finalize_hash}".encode()',
        [C1 + "test_a_changed_second_finalize_of_a_round_is_refused_and_credits_nothing"],
        "a second Finalize of an applied round is no longer the same operation, so it is no "
        "longer OPERATION_CONFLICT and only the order test stands between it and a second origin.",
    ),
    mutant(
        "credit-finalize-order-allows-gaps",
        ["CRD-03"],
        ESCROW,
        "if number != (last + 1 if last is not None else 0):",
        "if number < (last + 1 if last is not None else 0):",
        [C1 + "test_finalize_order_is_strict_and_a_replay_is_answered_before_the_order_test"],
        "round 1 or a round that skips one is credited before its predecessor.",
    ),
    mutant(
        "credit-cap-reached-is-refused",
        ["CRD-11"],
        ESCROW,
        "self.balances().issued + pending_units + reward.budget_units\n"
        "                > self.policy.max_total_issuance",
        "self.balances().issued + pending_units + reward.budget_units\n"
        "                >= self.policy.max_total_issuance",
        [MODEL],
        "a round that fills the pinned cap exactly is refused (off by one on the cap).",
    ),
    mutant(
        "credit-cap-ignores-pending-shadow-reservations",
        ["CRD-11"],
        ESCROW,
        "self.policy.round_reward_units\n                        * self.db.execute(",
        "0\n                        * self.db.execute(",
        ["tests/ledger/test_escrow_v2.py::test_live_issuance_preserves_pending_shadow_headroom"],
        "a live reward spends the headroom reserved for a pending shadow ordinal.",
    ),
    mutant(
        "credit-duplicate-origin-passes",
        ["CRD-01"],
        ESCROW,
        'raise EscrowError("DUPLICATE_ORIGIN")',
        "pass",
        [C1 + "test_a_colliding_origin_id_is_refused_and_the_whole_finalize_rolls_back"],
        "an origin id that already exists is issued again instead of being refused.",
    ),
    mutant(
        "credit-ledger-transaction-commits-on-failure",
        ["CRD-05", "CRD-01"],
        ESCROW,
        'self.db.execute("ROLLBACK" if failed else "COMMIT")',
        'self.db.execute("COMMIT")',
        [C1 + "test_a_colliding_origin_id_is_refused_and_the_whole_finalize_rolls_back"],
        "a refused finalize keeps the origins it had already issued.",
    ),
    mutant(
        "credit-store-transaction-commits-on-failure",
        ["CRD-05"],
        STORE,
        '            except BaseException:\n                self._db.execute("ROLLBACK")\n'
        "                raise",
        '            except BaseException:\n                self._db.execute("COMMIT")\n'
        "                raise",
        [C2 + "test_a_failure_after_the_ledger_wrote_leaves_the_store_exactly_as_it_was"],
        "a failure after the ledger wrote keeps the credit and its records.",
    ),
    mutant(
        "credit-snapshot-forgets-the-finalized-rounds",
        ["CRD-24", "CRD-01"],
        ESCROW,
        'finals = [list(r) for r in self.db.execute("SELECT * FROM escrow_finalized ORDER BY w")]',
        "finals: list[list[object]] = []",
        [C2 + "test_a_ledger_whose_rows_were_edited_is_refused_when_reopened"],
        "deleting escrow_finalized is no longer detected on restart, so a round can be credited "
        "again.",
    ),
    mutant(
        "credit-restart-skips-the-snapshot-comparison",
        ["CRD-24"],
        ESCROW,
        "if last_state is not None and last_state != self._snapshot():",
        "if False:",
        [C2 + "test_a_ledger_whose_rows_were_edited_is_refused_when_reopened"],
        "edited balances or owners survive a restart.",
    ),
    mutant(
        "credit-move-creates-units",
        ["CRD-24"],
        ESCROW,
        'self._credit(row["origin"], recipient or op.owner, destination, destination_ref, take)',
        'self._credit(row["origin"], recipient or op.owner, destination, destination_ref, '
        "take + 1)",
        [C2 + "test_units_are_conserved_by_every_sequence_of_ledger_operations"],
        "a transfer or lock credits one unit more than it debits.",
    ),
    mutant(
        "credit-miner-audits-itself",
        ["CRD-07"],
        ESCROW,
        "                or any(e.signer == c.hotkey for e in envs)\n",
        "",
        [C1 + "test_a_miner_that_is_also_a_pinned_auditor_cannot_audit_itself"],
        "a pinned auditor key that is also the miner's hotkey signs the verdict that credits it.",
    ),
    mutant(
        "credit-any-signer-audits",
        ["CRD-07"],
        ESCROW,
        '{"ReplayVerdict": lambda s: s in self.auditors}',
        '{"ReplayVerdict": lambda s: True}',
        [MATRIX + "[verdict_by_an_unpinned_auditor]"],
        "a verdict from a key the manifest does not pin credits the miner.",
    ),
    mutant(
        "credit-challenge-target-unchecked",
        ["CRD-07", "CRD-23"],
        ESCROW,
        "                or (challenge.w, challenge.target) != (c.w, c.hotkey)\n",
        "",
        [MATRIX + "[challenge_of_another_miner]"],
        "a challenge issued for another miner or round backs this miner's verdict.",
    ),
    mutant(
        "credit-verdict-challenge-hash-unchecked",
        ["CRD-07", "CRD-23"],
        ESCROW,
        "                or any(x.challenge_hash != digest(challenge) for x in verdict_list)\n",
        "",
        [MATRIX + "[verdict_of_another_challenge]"],
        "a verdict of another challenge is credited as this miner's audit.",
    ),
    mutant(
        "credit-mismatch-verdict-credited",
        ["CRD-07"],
        ESCROW,
        '                    x.result != "MATCH"\n'
        "                    or x.first_bad_leaf is not None",
        "                    False\n                    or x.first_bad_leaf is not None",
        [MATRIX + "[verdict_is_a_mismatch]"],
        "a MISMATCH verdict credits the miner.",
    ),
    mutant(
        "credit-leaves-root-unchecked",
        ["CRD-07"],
        ESCROW,
        "                    or x.recomputed_leaves_root != c.leaves_root\n",
        "",
        [MATRIX + "[verdict_recomputed_another_root]"],
        "a replay of other leaves than the committed ones credits the miner.",
    ),
    mutant(
        "credit-owner-unchecked",
        ["CRD-09", "CRD-12"],
        ESCROW,
        "                or item.owner != self.owner_of(c.hotkey)\n",
        "",
        [MATRIX + "[work_owner_is_not_the_current_owner]"],
        "the coldkey named in the evidence is credited without being the miner's current owner.",
    ),
    mutant(
        "credit-replay-unchecked",
        ["CRD-09"],
        ESCROW,
        "if self.replay_finality(evidence) != final.final_theta_hash_w1:",
        "if False:",
        [C1 + "test_evidence_the_replay_does_not_confirm_or_in_the_wrong_domain_credits_nothing"],
        "a Finalize whose state the independent replay does not confirm is credited.",
    ),
    mutant(
        "credit-included-set-unchecked",
        ["CRD-08"],
        ESCROW,
        "            or set(work) != set(final.included)\n",
        "",
        [MATRIX + "[included_miner_without_audited_work]"],
        "a hotkey listed as included without audited work is accepted next to the audited ones.",
    ),
    mutant(
        "credit-duplicate-work-passes",
        ["CRD-01"],
        ESCROW,
        "                or c.hotkey in work\n",
        "",
        [C1 + "test_one_work_per_hotkey_inside_a_finalize"],
        "the same hotkey presented twice as work is no longer refused.",
    ),
    mutant(
        "credit-duplicate-included-passes",
        ["CRD-01"],
        ESCROW,
        "            or len(set(final.included)) != len(final.included)\n",
        "",
        [C1 + "test_one_work_per_hotkey_inside_a_finalize"],
        "a hotkey listed twice in Finalize.included is no longer refused.",
    ),
    mutant(
        "credit-tokens-not-server-derived",
        ["CRD-10"],
        "src/hypertrain/protocol/messages_v2.py",
        "if self.tokens != sample_count * manifest.training.model.seq_len:",
        "if False:",
        [MATRIX + "[tokens_not_derived_from_the_assignment]"],
        "a miner's declared token count scales its share of the round.",
    ),
    mutant(
        "credit-remainder-to-the-last-hotkeys",
        ["CRD-10"],
        ESCROW,
        "        for h in ordered[:remainder]:",
        "        for h in ordered[len(ordered) - remainder :]:",
        [
            C1
            + "test_a_round_splits_its_budget_exactly_and_hands_the_remainder_to_the_lowest_hotkeys"
        ],
        "the rounding remainder goes to the highest hotkeys, not the lowest ones in byte order.",
    ),
    mutant(
        "credit-budget-unchecked",
        ["CRD-10", "CRD-09"],
        ESCROW,
        "if reward.budget_units != self.policy.round_reward_units:",
        "if False:",
        [REWARD_DISAGREES + "[budget_units-ROUND_BUDGET]"],
        "a reward may name another budget than the round's pinned one.",
    ),
    mutant(
        "credit-verdict-root-unchecked",
        ["CRD-09"],
        ESCROW,
        "if sha256_hex(canonicalize(verdicts)) != reward.verdict_root:",
        "if False:",
        [REWARD_DISAGREES + "[verdict_root-VERDICT_ROOT]"],
        "the reward no longer commits to the verdicts it was minted from.",
    ),
    mutant(
        "credit-envelope-expiry-ignored",
        ["CRD-07"],
        "src/hypertrain/protocol/envelope_v2.py",
        "if now_round > env.exp_drand:",
        "if False:",
        [MATRIX + "[commit_expired_before_acceptance]"],
        "an expired signature is accepted as evidence at the finalize beacon.",
    ),
    mutant(
        "credit-shadow-evidence-credits-a-live-reward",
        ["CRD-09", "CRD-19"],
        ESCROW,
        "                if evidence.shadow:\n"
        '                    raise EscrowError("REWARD_DOMAIN")',
        "                pass",
        [C1 + "test_evidence_the_replay_does_not_confirm_or_in_the_wrong_domain_credits_nothing"],
        "evidence labelled shadow is accepted for a live reward.",
    ),
    mutant(
        "credit-round-settles-with-unresolved-audits",
        ["CRD-08"],
        FINALITY,
        "        or status.unresolved_audits\n",
        "",
        [C2 + "test_a_round_settles_only_when_applied_audited_undisputed_and_not_excluded"],
        "a round with an audit still running settles and is credited.",
    ),
    mutant(
        "credit-round-settles-after-exclusion",
        ["CRD-08"],
        FINALITY,
        '        or status.disposition == "EXCLUDED"\n',
        "",
        [C2 + "test_a_round_settles_only_when_applied_audited_undisputed_and_not_excluded"],
        "an excluded round settles and is credited.",
    ),
    mutant(
        "credit-round-settles-with-open-disputes",
        ["CRD-08"],
        FINALITY,
        "        or status.unresolved_disputes\n",
        "",
        [C2 + "test_a_round_settles_only_when_applied_audited_undisputed_and_not_excluded"],
        "a round with an open dispute settles and is credited.",
    ),
    mutant(
        "credit-round-settles-twice-with-two-finalities",
        ["CRD-08", "CRD-02"],
        FINALITY,
        "if status.resolution_hash is not None and status.resolution_hash != finality_hash:",
        "if False:",
        [C2 + "test_a_round_settles_with_one_finality_hash_for_ever"],
        "a settled round accepts a second, different finality.",
    ),
    mutant(
        "credit-genesis-after-other-events",
        ["CRD-16"],
        ESCROW,
        '                or self.db.execute("SELECT 1 FROM escrow_events LIMIT 1").fetchone()\n',
        "",
        [C2 + "test_genesis_is_test_mode_only_and_only_ever_the_first_event"],
        "a test genesis is accepted after rewards were already issued.",
    ),
    mutant(
        "credit-genesis-above-the-cap",
        ["CRD-11", "CRD-16"],
        ESCROW,
        "                or genesis.total_units > self.policy.max_total_issuance\n",
        "",
        [C2 + "test_genesis_may_issue_up_to_the_pinned_cap_and_not_one_unit_more"],
        "a genesis may issue more than the pinned total issuance.",
    ),
    mutant(
        "credit-genesis-origin-id-unbound",
        ["CRD-16"],
        ESCROW,
        "if o.origin_id != genesis_origin_id(self.run_id, genesis.allocation_hash, i):",
        "if False:",
        [
            C2 + "test_a_genesis_that_is_not_the_pinned_one_is_refused_with_its_code"
            "[origin_id_outside_the_genesis_domain]"
        ],
        "a genesis origin may carry an id outside the genesis domain.",
    ),
    mutant(
        "credit-genesis-units-are-payable",
        ["CRD-16", "CRD-17"],
        ESCROW,
        "AND u.units>0 AND o.hotkey IS NOT NULL AND o.mature_at<=? ",
        "AND u.units>0 AND o.mature_at<=? ",
        [C2 + "test_genesis_replays_conflicts_on_another_body_and_its_units_are_never_payable"],
        "matured genesis and bootstrap collateral is emitted as miner reward.",
    ),
    mutant(
        "credit-second-bootstrap-accepted",
        ["CRD-17"],
        ESCROW,
        "\"SELECT 1 FROM escrow_events WHERE kind='PRODUCTION_BOOTSTRAP'\"",
        "\"SELECT 1 FROM escrow_events WHERE kind='NO_SUCH_KIND'\"",
        [
            "tests/ledger/test_production_bootstrap_v2.py::"
            "test_bootstrap_idempotent_per_operation_and_once_per_run"
        ],
        "a run issues a second production bootstrap under a fresh operation id.",
    ),
    mutant(
        "credit-bootstrap-pin-unchecked",
        ["CRD-17"],
        ESCROW,
        "                bootstrap.allocation_hash != self.policy.bootstrap_allocation_hash\n"
        "                or ",
        "                False\n                or ",
        [BOOTSTRAP_FAULT + "[allocation_is_consistent_but_not_the_pinned_one]"],
        "a bootstrap of an allocation the policy does not pin is accepted.",
    ),
    mutant(
        "credit-bootstrap-ignores-pending-shadow-headroom",
        ["CRD-17", "CRD-11"],
        ESCROW,
        "                + pending * self.policy.round_reward_units\n",
        "",
        [
            "tests/ledger/test_production_bootstrap_v2.py::"
            "test_bootstrap_counts_pending_shadow_reservation_against_cap"
        ],
        "a bootstrap spends the headroom reserved for a pending shadow ordinal.",
    ),
    mutant(
        "credit-shadow-reservation-not-exclusive",
        ["CRD-18"],
        ESCROW,
        "            if pending is not None:\n",
        "            if False:\n",
        [C2 + "test_shadow_ordinals_are_consecutive_unique_bounded_and_reserved_one_at_a_time"],
        "a second shadow reservation is granted while the first is still pending.",
    ),
    mutant(
        "credit-thirteenth-shadow-ordinal",
        ["CRD-18"],
        ESCROW,
        "            if ordinal >= self.policy.shadow_bootstrap_rounds:",
        "            if ordinal > self.policy.shadow_bootstrap_rounds:",
        [C2 + "test_shadow_ordinals_are_consecutive_unique_bounded_and_reserved_one_at_a_time"],
        "the shadow bootstrap is open for one ordinal more than the policy pins.",
    ),
    mutant(
        "credit-shadow-units-are-payable",
        ["CRD-19"],
        ESCROW,
        '            "AND o.origin NOT IN (SELECT origin FROM escrow_shadow_finalized) "\n',
        "",
        [C2 + "test_shadow_and_live_origins_of_one_round_coexist_but_only_live_units_are_emission"],
        "matured shadow collateral is emitted next to the live reward of the same round.",
    ),
    mutant(
        "credit-maturity-ignores-the-finalize-hash",
        ["CRD-20"],
        ESCROW,
        "                        or finality is None\n"
        '                        or status.finality_hash != finality["finalize_hash"]\n',
        "",
        [SETTLE + "[another_finality_hash]"],
        "a settlement bound to another finalize hash releases the origin.",
    ),
    mutant(
        "credit-maturity-ignores-an-open-dispute",
        ["CRD-20"],
        ESCROW,
        "                        status.unresolved\n"
        '                        or status.outcome == "FRAUD"\n'
        "                        or now_beacon < status.release_beacon\n"
        "                        or finality is None",
        '                        status.outcome == "FRAUD"\n'
        "                        or now_beacon < status.release_beacon\n"
        "                        or finality is None",
        [SETTLE + "[unresolved]"],
        "units whose settlement is still unresolved are released.",
    ),
    mutant(
        "credit-maturity-ignores-fraud",
        ["CRD-20"],
        ESCROW,
        '                        or status.outcome == "FRAUD"\n'
        "                        or now_beacon < status.release_beacon\n"
        "                        or finality is None",
        "                        or now_beacon < status.release_beacon\n"
        "                        or finality is None",
        [
            SETTLE + "[fraud]",
            C2 + "test_one_origin_in_fraud_keeps_every_origin_of_the_call_pending",
        ],
        "units whose settlement proves fraud are released.",
    ),
    mutant(
        "credit-maturity-ignores-the-release-beacon",
        ["CRD-20"],
        ESCROW,
        "                        or now_beacon < status.release_beacon\n"
        "                        or finality is None",
        "                        or finality is None",
        [SETTLE + "[release_beacon_not_reached]"],
        "units are released before their dispute window has closed.",
    ),
    mutant(
        "credit-maturity-ignores-the-origin-maturity",
        ["CRD-20"],
        ESCROW,
        'if record is None or now_beacon < record["mature_at"]:',
        "if record is None:",
        [C2 + "test_an_origin_is_not_matured_before_its_own_maturity_round"],
        "an origin is released before the beacon reached its own maturity round.",
    ),
    mutant(
        "credit-unrecorded-settlement-is-accepted",
        ["CRD-20"],
        STORE,
        '        if row is None:\n            raise EscrowError("UNACCEPTED_SETTLEMENT")\n'
        "        return Settlement.model_validate_json(row[0])",
        "        if row is None:\n"
        "            return Settlement(\n"
        "                finality_hash=reference,\n"
        "                closed_dispute_root=reference,\n                release_beacon=0,\n"
        '                unresolved=False,\n                outcome="MATCH",\n            )\n'
        "        return Settlement.model_validate_json(row[0])",
        [C2 + "test_the_service_settlement_is_the_record_the_service_wrote_and_nothing_else"],
        "a reference the service never recorded is treated as a clean settlement.",
    ),
    mutant(
        "credit-rotation-signed-by-any-coldkey",
        ["CRD-12"],
        ADMISSION,
        'Intake(self.escrow.run_id, {"RotateRequest": lambda s: s == history["coldkey"]}).accept(',
        'Intake(self.escrow.run_id, {"RotateRequest": lambda s: True}).accept(',
        [C2 + "test_a_rotation_moves_the_admission_but_never_the_coldkey_that_is_credited"],
        "any key can rotate an admission and so change who is credited for it.",
    ),
    mutant(
        "credit-hotkey-history-ignored-at-join",
        ["CRD-12"],
        ADMISSION,
        '                raise AdmissionError("HOTKEY_HISTORY_EXISTS")',
        "                pass",
        [C2 + "test_a_rotation_moves_the_admission_but_never_the_coldkey_that_is_credited"],
        "a hotkey that was bound to a coldkey can join again under another coldkey.",
    ),
    mutant(
        "credit-owner-share-given-to-the-first-owner",
        ["CRD-14", "CRD-12"],
        ESCROW,
        "                owner=work[h][0],",
        "                owner=work[ordered[0]][0],",
        [C2 + "test_a_coldkey_is_credited_one_share_per_included_hotkey_and_no_more"],
        "every origin of the round is credited to the first miner's coldkey.",
    ),
    mutant(
        "credit-admission-needs-no-own-stake",
        ["CRD-14"],
        ADMISSION,
        "            and units >= required\n",
        "",
        [C2 + "test_each_admission_of_one_coldkey_needs_its_own_funded_lock_to_be_eligible"],
        "an admission is eligible without a lock of its own, so one stake backs many hotkeys.",
    ),
    mutant(
        "credit-services-build-commits-the-open-transaction",
        ["CRD-05"],
        ESCROW,
        '        self.db.execute("PRAGMA synchronous=FULL")\n',
        "",
        [C2 + "test_building_the_services_inside_a_transaction_never_commits_it_half_way"],
        "the service constructors commit an open store transaction on the first call after a "
        "restart.",
    ),
    mutant(
        "credit-finalize-takes-any-included-set",
        ["CRD-08"],
        STORE,
        "if model.w != w or model.included != included:",
        "if model.w != w:",
        [
            C1 + "test_a_finalize_whose_included_set_differs_from_the_audited_set_is_refused_and_"
            "can_be_corrected"
        ],
        "the Finalize's own included list decides who is credited, not the audited set.",
    ),
    mutant(
        "credit-finalize-nonce-conflict-unchecked",
        ["CRD-02", "CRD-23"],
        STORE,
        "if known is not None and known[0] != digest:\n"
        '            raise ChallengeError(409, "conflicting signed operation nonce")',
        'if False:\n            raise ChallengeError(409, "conflicting signed operation nonce")',
        [
            C1 + "test_finalize_settles_a_round_once_and_every_retry_returns_the_stored_result",
            C1 + "test_a_miner_commits_once_per_round_a_replay_returns_the_receipt_and_another_"
            "body_is_refused",
        ],
        "a signed operation re-sent with another body is no longer refused at the reservation.",
    ),
    mutant(
        "credit-finalize-does-not-settle-the-round",
        ["CRD-08", "CRD-02"],
        STORE,
        "settled = settle(status, model, final_hash)",
        "settled = status",
        [C1 + "test_finalize_settles_a_round_once_and_every_retry_returns_the_stored_result"],
        "the credit is minted while the round's finality record stays open.",
    ),
    mutant(
        "credit-audit-lease-recreated",
        ["CRD-23"],
        STORE,
        "                if not self._db.execute(\n"
        '                    "SELECT 1 FROM audit_leases_v2 WHERE id=?", (job_id,)\n'
        "                ).fetchone():",
        "                if True:",
        [C1 + "test_one_audit_lease_per_miner_and_round_completes_once_and_scheduling_adds_none"],
        "scheduling the audits twice creates a second lease for a miner and round.",
    ),
    mutant(
        "credit-entry-point-called-outside-the-store",
        ["CRD-09"],
        "src/hypertrain/challenge/app.py",
        FINALIZE_CALL,
        "        store.for_run(run_id)._services(run_id)[0].reward_finalize(None, None)\n"
        + FINALIZE_CALL,
        [C1 + "test_the_only_callers_of_the_credit_entry_points_are_the_service_handlers"],
        "a route reaches the reward entry point without the store's evidence assembly.",
    ),
    mutant(
        "credit-completed-lease-completes-again",
        ["CRD-23"],
        STORE,
        'if row["state"] == "completed":\n                if row["result"] != verdict.result:',
        'if False:\n                if row["result"] != verdict.result:',
        [C1 + "test_one_audit_lease_per_miner_and_round_completes_once_and_scheduling_adds_none"],
        "a verdict sent again for a completed lease is refused instead of answered from the "
        "stored result.",
    ),
    mutant(
        "credit-tape-replay-unchecked",
        ["CRD-09"],
        STORE,
        'if th(replayed.theta) != record["theta_hash"]:',
        "if False:",
        [C1 + "test_a_finalize_is_refused_when_the_tape_does_not_replay_to_the_claimed_state"],
        "a credit is minted for a state the tape does not replay to.",
    ),
    mutant(
        "credit-replay-is-not-recognised",
        ["CRD-06", "CRD-02"],
        ESCROW,
        '        if row is None:\n            return None\n        if row["request"] != request:',
        '        if True:\n            return None\n        if row["request"] != request:',
        [RESTART],
        "an identical finalize is no longer answered from the stored event.",
    ),
    mutant(
        "credit-conflicting-request-answered-with-the-first-receipt",
        ["CRD-02", "CRD-01"],
        ESCROW,
        '        if row["request"] != request:\n'
        '            raise EscrowError("OPERATION_CONFLICT")\n',
        "",
        [C1 + "test_a_changed_second_finalize_of_a_round_is_refused_and_credits_nothing"],
        "another request under the same operation id is answered with the first receipt.",
    ),
    mutant(
        "credit-a-round-may-be-worth-nothing",
        ["CRD-10"],
        "src/hypertrain/protocol/messages_v2.py",
        "Positive = Annotated[StrictInt, Field(ge=1, le=2**53 - 1)]",
        "Positive = Annotated[StrictInt, Field(ge=0, le=2**53 - 1)]",
        [C1 + "test_a_unit_amount_is_an_exact_integer_between_one_and_two_to_the_53_minus_one[0]"],
        "a zero budget or a zero origin is a valid amount.",
    ),
    mutant(
        "credit-roster-slots-share-their-samples",
        ["CRD-23"],
        "src/hypertrain/data/assignment.py",
        "tuple(eprp(off + rprp(slot * bu + k)) * unit + j for k in range(bu) for j in range(unit))",
        "tuple(eprp(off + rprp(k)) * unit + j for k in range(bu) for j in range(unit))",
        ["tests/data/test_assignment.py::test_slices_disjoint_64_slots_50_rounds"],
        "every roster slot is assigned the same samples, so identical work can be credited to "
        "several hotkeys.",
    ),
    mutant(
        "credit-signer-predicate-skipped",
        ["CRD-09", "CRD-08"],
        "src/hypertrain/protocol/envelope_v2.py",
        "if check is not None and not check(env.signer):",
        "if False:",
        [C1 + "test_a_finalize_that_the_coordinator_did_not_sign_credits_nothing"],
        "a Finalize signed by any key is accepted as the coordinator's, at the store and again at "
        "the ledger.",
    ),
    mutant(
        "credit-service-ledger-runs-a-quorum",
        ["CRD-07"],
        STORE,
        SHADOW_SETTLEMENT_ARGUMENT + "            )\n            escrow.mutex",
        SHADOW_SETTLEMENT_ARGUMENT + "                auditor_quorum=(1, 1),\n"
        "            )\n            escrow.mutex",
        [QUORUM],
        "the service switches the auditor quorum on without supplying the extra verdicts.",
    ),
    mutant(
        "credit-genesis-allocation-not-recomputed",
        ["CRD-16"],
        ESCROW,
        "                or (genesis_allocation_hash(alloc) != genesis.allocation_hash)\n",
        "",
        [PINNED_ALLOCATION + "[genesis]"],
        "a genesis that names the pinned allocation hash may carry other units than the ones it "
        "hashes to.",
    ),
    mutant(
        "credit-bootstrap-allocation-not-recomputed",
        ["CRD-17"],
        ESCROW,
        "                or genesis_allocation_hash(alloc) != bootstrap.allocation_hash\n",
        "",
        [PINNED_ALLOCATION + "[bootstrap]"],
        "a bootstrap that names the pinned allocation hash may carry other units than the ones "
        "it hashes to.",
    ),
    mutant(
        "credit-settlement-lookup-ignores-the-reference",
        ["CRD-20"],
        STORE,
        "\"SELECT data FROM records_v2 WHERE run_id=? AND kind='settlement' AND id=?\"",
        "\"SELECT data FROM records_v2 WHERE run_id=? AND kind='settlement' AND (id=? OR 1)\"",
        [C2 + "test_the_service_settlement_is_the_record_the_service_wrote_and_nothing_else"],
        "any reference is answered with the first settlement the service recorded.",
    ),
    mutant(
        "credit-rotation-migrates-the-earning-hotkey",
        ["CRD-12"],
        ADMISSION,
        "            self.store.db.execute(\n"
        '                "UPDATE admissions_v2 SET hotkey=? WHERE admission_id=? AND hotkey=?",\n',
        "            self.escrow.db.execute(\n"
        '                "UPDATE escrow_origins SET hotkey=? WHERE hotkey=?",\n'
        "                (op.new_hotkey, op.hotkey),\n"
        "            )\n"
        "            self.store.db.execute(\n"
        '                "UPDATE admissions_v2 SET hotkey=? WHERE admission_id=? AND hotkey=?",\n',
        [C1 + "test_a_rotation_after_the_credit_does_not_move_the_credit_to_another_identity"],
        "a rotation re-labels the origins a hotkey already earned with its successor.",
    ),
]
