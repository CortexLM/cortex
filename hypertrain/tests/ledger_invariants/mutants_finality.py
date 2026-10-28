"""Mutants of the finality package: one small regression each, which a named test must catch."""

from __future__ import annotations

T = "tests/ledger_invariants/test_finality.py::"
T2 = "tests/ledger_invariants/test_finality_2.py::"
FINALITY = "src/hypertrain/challenge/finality_v2.py"
ESCROW = "src/hypertrain/ledger/escrow_v2.py"
STORE = "src/hypertrain/challenge/store.py"
ROLLBACK_ON_ERROR = (
    "            except BaseException:\n"
    '                self._db.execute("ROLLBACK")\n'
    "                raise\n"
)

SETTLE = T + "test_a_round_that_may_not_settle_is_refused_by_settle"
NEVER_BACK = T + "test_a_round_status_never_moves_backwards"
EXCLUDE = T + "test_a_settled_round_cannot_be_excluded_and_a_conflicting_exclusion_is_refused"
OPEN_GATE = T + "test_a_round_does_not_open_without_a_complete_closed_history"
SCHEDULE = T + "test_no_schedule_of_the_gates_leaves_a_third_round_unsettled"
APPLY_GATE = T + "test_a_round_is_applied_only_on_an_applied_predecessor"
ROLLBACK_PURE = T + "test_a_rollback_without_complete_authenticated_replay_is_refused"
IN_ORDER = T + "test_every_round_is_credited_once_and_in_order_whatever_is_attempted"
ORIGIN_ONCE = T + "test_an_origin_that_already_exists_is_never_credited_again"
WORK_SET = T + "test_a_finalize_whose_included_set_and_work_differ_credits_nobody"
AUDIT_PROVEN = T + "test_a_miner_is_credited_only_for_work_a_pinned_auditor_matched"
PINNED_AUDITOR = T + "test_a_verdict_of_an_unpinned_auditor_credits_nobody"
TX_COMMIT = T + "test_a_transaction_commits_on_success_and_is_closed"
TX_ROLLBACK = T + "test_a_transaction_rolls_back_whatever_ends_it_and_is_closed"
FINALIZE_MATRIX = T2 + "test_finalize_refuses_everything_but_the_coordinators_complete_audited_set"
STORE_REFUSED = T2 + "test_finalize_refuses_a_round_that_may_not_settle_and_writes_nothing"
REPAIR_REFUSED = T2 + "test_a_repair_is_model_history_and_is_never_finalized_for_a_reward"
PAYS_AUDITED = T2 + "test_finalize_pays_the_four_audited_miners_and_nobody_excluded"
ELIGIBLE = T2 + "test_a_miner_convicted_after_its_round_was_audited_is_never_credited_by_a_finalize"
ATOMIC = T2 + "test_a_failure_inside_finalize_leaves_nothing_behind_and_the_retry_settles_once"
ONCE = T2 + "test_a_round_is_credited_once_however_often_its_finalize_is_sent"
OPENING = T2 + "test_a_round_opening_is_immutable_idempotent_and_sequential"
THIRD_ROUND = T2 + "test_a_third_round_opens_only_after_the_first_is_settled"
ROLLBACK_MATRIX = (
    T2 + "test_a_rollback_without_the_coordinator_or_an_accepted_cause_changes_nothing"
)
ROLLBACK_SETTLED = T2 + "test_a_settled_round_is_never_rolled_back"
ROLLBACK_REPLAY = (
    T2 + "test_a_stored_rollback_is_replayed_and_a_different_body_for_the_round_is_refused"
)
LEGACY = T + "test_legacy_finalize_is_journaled_and_recorded_together_and_only_once"
LEDGER_FINALITY = (
    T + "test_the_ledger_credits_nothing_for_a_finalize_it_cannot_authenticate_and_replay"
)
CAUSE_REFUSED = T2 + "test_a_rollback_cause_the_referee_did_not_sign_with_evidence_is_refused"
CAUSE_SET = T2 + "test_a_rollback_that_cites_a_cause_it_does_not_rest_on_is_refused"
EXCLUDE_CLOSES = T + "test_an_exclusion_closes_a_round_only_when_no_applied_tape_is_left_to_repair"
CHALLENGE_SIGNER = T + "test_an_audit_challenge_the_reward_authority_did_not_sign_credits_nobody"
SELF_AUDIT = T + "test_a_miner_is_not_credited_for_a_verdict_it_signed_itself"


def mutant(
    id: str, invariants: list[str], path: str, old: str, new: str, tests: list[str], why: str
) -> dict[str, object]:
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
        "settle-ignores-an-unapplied-round",
        ["FIN-02"],
        FINALITY,
        "        not status.applied\n        or status.unresolved_audits\n",
        "        False\n        or status.unresolved_audits\n",
        [SETTLE],
        "A round with no applied tape is settled and rewards are minted for a model never built.",
    ),
    mutant(
        "settle-ignores-open-audits",
        ["FIN-02"],
        FINALITY,
        "        or status.unresolved_audits\n",
        "",
        [SETTLE],
        "A round settles while an audit of one of its miners is still unresolved.",
    ),
    mutant(
        "settle-ignores-open-disputes",
        ["FIN-02"],
        FINALITY,
        "        or status.unresolved_disputes\n",
        "",
        [SETTLE],
        "A round settles while a contest of its verdict is still open.",
    ),
    mutant(
        "settle-accepts-an-excluded-round",
        ["FIN-02", "FIN-10"],
        FINALITY,
        '        or status.disposition == "EXCLUDED"\n',
        "",
        [SETTLE, NEVER_BACK],
        "An excluded round moves to SETTLED and is paid.",
    ),
    mutant(
        "settle-ignores-the-applied-theta",
        ["FIN-01", "FIN-02"],
        FINALITY,
        "(status.w, status.state_hash) != (final.w, final.final_theta_hash_w1)",
        "(status.w,) != (final.w,)",
        [SETTLE],
        "A Finalize that names another resulting theta settles the applied round.",
    ),
    mutant(
        "settle-accepts-another-finalize-over-a-resolution",
        ["FIN-02"],
        FINALITY,
        "    if status.resolution_hash is not None and status.resolution_hash != finality_hash:\n"
        '        raise FinalityError("conflicting finality")\n',
        "",
        [SETTLE, NEVER_BACK],
        "A second, different Finalize overwrites the resolution a round already carries.",
    ),
    mutant(
        "exclude-rewrites-a-settled-round",
        ["FIN-15", "FIN-10"],
        FINALITY,
        '    if status.disposition == "SETTLED":\n'
        '        raise FinalityError("finalized late fraud cannot rewrite model history")\n',
        "",
        [EXCLUDE, NEVER_BACK],
        "Late fraud turns a settled round into an excluded one.",
    ),
    mutant(
        "exclude-overwrites-the-resolution",
        ["FIN-10"],
        FINALITY,
        "    if status.resolution_hash is not None and status.resolution_hash != evidence_hash:\n"
        '        raise FinalityError("conflicting exclusion")\n',
        "",
        [EXCLUDE, NEVER_BACK],
        "A second exclusion with other evidence replaces the evidence of the first.",
    ),
    mutant(
        "open-lets-a-third-round-in",
        ["FIN-12"],
        FINALITY,
        "    if any(not r.closed for r in ordered if r.w <= w - 2):",
        "    if any(not r.closed for r in ordered if r.w < w - 2):",
        [OPEN_GATE, SCHEDULE],
        "Round w opens while round w-2 is still unsettled: three speculative rounds.",
    ),
    mutant(
        "open-accepts-a-gap-in-the-history",
        ["FIN-12", "FIN-13"],
        FINALITY,
        "    if [r.w for r in ordered] != list(range(w)):",
        "    if len(ordered) < w:",
        [OPEN_GATE],
        "A round opens although its history has a gap or already contains the round.",
    ),
    mutant(
        "apply-without-an-applied-predecessor",
        ["FIN-12"],
        FINALITY,
        "    if w and not next(r for r in history if r.w == w - 1).applied:\n"
        '        raise FinalityError("predecessor not applied")\n',
        "",
        [APPLY_GATE],
        "A round is aggregated on a predecessor whose tape was never applied.",
    ),
    mutant(
        "rollback-without-the-replayed-outputs",
        ["FIN-14"],
        FINALITY,
        "        or (rollback.new_theta_hash_w2, rollback.new_outer_state_hash)\n"
        "        != (replayed_theta_hash, replayed_outer_hash)\n",
        "",
        [ROLLBACK_PURE],
        "A rollback completes with signed outputs that differ from the repair actually replayed.",
    ),
    mutant(
        "rollback-without-a-matching-cause",
        ["FIN-14"],
        FINALITY,
        "        or (status.resolution_hash not in rollback.cause_hashes)\n",
        "",
        [ROLLBACK_PURE],
        "A rollback completes an exclusion whose cause it does not cite.",
    ),
    mutant(
        "ledger-mints-rounds-out-of-order",
        ["FIN-07", "FIN-06"],
        ESCROW,
        "            if number != (last + 1 if last is not None else 0):",
        "            if number < (last + 1 if last is not None else 0):",
        [IN_ORDER],
        "A round is rewarded before its predecessor, leaving a gap.",
    ),
    mutant(
        "ledger-accepts-a-conflicting-finalize-for-a-round",
        ["FIN-06"],
        ESCROW,
        '        if row["request"] != request:\n'
        '            raise EscrowError("OPERATION_CONFLICT")\n',
        "",
        [IN_ORDER],
        "A different Finalize of an already rewarded round returns the old receipt as accepted.",
    ),
    mutant(
        "ledger-reissues-an-existing-origin",
        ["FIN-06"],
        ESCROW,
        '            raise EscrowError("DUPLICATE_ORIGIN")',
        "            pass",
        [ORIGIN_ONCE],
        "An origin id that already exists is no longer refused by the ledger.",
    ),
    mutant(
        "ledger-pays-an-included-miner-without-work",
        ["FIN-18", "FIN-03"],
        ESCROW,
        "            or set(work) != set(final.included)\n",
        "",
        [WORK_SET],
        "A Finalize that names a miner with no verified work is paid for the others.",
    ),
    mutant(
        "ledger-pays-work-the-finalize-did-not-include",
        ["FIN-18"],
        ESCROW,
        "                or c.hotkey not in final.included\n",
        "",
        [WORK_SET],
        "Verified work of a miner outside the signed included set reaches the allocation check.",
    ),
    mutant(
        "ledger-ignores-the-audit-result",
        ["FIN-03"],
        ESCROW,
        '                    x.result != "MATCH"\n',
        "                    False\n",
        [AUDIT_PROVEN],
        "A miner whose audit is MISMATCH is credited.",
    ),
    mutant(
        "ledger-ignores-a-bad-leaf",
        ["FIN-03"],
        ESCROW,
        "                    or x.first_bad_leaf is not None\n",
        "",
        [AUDIT_PROVEN],
        "A MATCH verdict that names a bad leaf is credited.",
    ),
    mutant(
        "ledger-ignores-the-recomputed-leaves-root",
        ["FIN-03"],
        ESCROW,
        "                    or x.recomputed_leaves_root != c.leaves_root\n",
        "",
        [AUDIT_PROVEN],
        "A verdict that recomputed another leaves root than the commit's is credited.",
    ),
    mutant(
        "ledger-ignores-the-challenge-binding",
        ["FIN-03"],
        ESCROW,
        "                or any(x.challenge_hash != digest(challenge) for x in verdict_list)\n",
        "",
        [AUDIT_PROVEN],
        "A verdict of another audit challenge is credited to this commit.",
    ),
    mutant(
        "ledger-accepts-any-signer-as-auditor",
        ["FIN-03"],
        ESCROW,
        'Intake(self.run_id, {"ReplayVerdict": lambda s: s in self.auditors}).accept(',
        'Intake(self.run_id, {"ReplayVerdict": lambda s: True}).accept(',
        [PINNED_AUDITOR],
        "A verdict signed by a key outside the pinned auditors is credited.",
    ),
    mutant(
        "tx-keeps-the-transaction-open-after-an-error",
        ["FIN-25"],
        STORE,
        ROLLBACK_ON_ERROR,
        "            except BaseException:\n                raise\n",
        [TX_ROLLBACK],
        "An exception inside a store transaction leaves the connection inside it.",
    ),
    mutant(
        "tx-rolls-back-exceptions-only",
        ["FIN-25"],
        STORE,
        ROLLBACK_ON_ERROR,
        ROLLBACK_ON_ERROR.replace("BaseException", "Exception"),
        [TX_ROLLBACK],
        "An interrupt (a BaseException) inside a store transaction is not rolled back.",
    ),
    mutant(
        "tx-never-commits",
        ["FIN-25"],
        STORE,
        '            self._db.execute("COMMIT")\n        if notify:',
        "            pass\n        if notify:",
        [TX_COMMIT],
        "A successful store transaction is left open instead of committed.",
    ),
    mutant(
        "finalize-ignores-the-included-set",
        ["FIN-01", "FIN-18"],
        STORE,
        "if model.w != w or model.included != included:",
        "if model.w != w:",
        [FINALIZE_MATRIX],
        "A Finalize that omits or adds a miner is accepted as the audited set.",
    ),
    mutant(
        "finalize-accepts-any-signer",
        ["FIN-01"],
        STORE,
        'model = self._authenticated_v2(run_id, raw, "Finalize", manifest.training.coord_pubkey)',
        "model = self._authenticated_v2(\n"
        '                run_id, raw, "Finalize", envelope_v2.parse_envelope(raw).signer\n'
        "            )",
        [FINALIZE_MATRIX],
        "A Finalize signed by a miner or an auditor settles the round.",
    ),
    mutant(
        "finalize-settles-without-the-settle-gate",
        ["FIN-02"],
        STORE,
        "settled = settle(status, model, final_hash)",
        'settled = RoundStatus(**{**asdict(status), "disposition": "SETTLED", '
        '"resolution_hash": final_hash})',
        [STORE_REFUSED, FINALIZE_MATRIX],
        "The store marks a round SETTLED without asking settle() whether it may be.",
    ),
    mutant(
        "finalize-mints-for-a-repair",
        ["FIN-02"],
        STORE,
        '            if applied.get("kind") == "repair":\n'
        "                raise ChallengeError(\n"
        '                    409, "repair evidence is model-only; cannot mint miner rewards"',
        "            if False:\n"
        "                raise ChallengeError(\n"
        '                    409, "repair evidence is model-only; cannot mint miner rewards"',
        [REPAIR_REFUSED],
        "A repaired round, whose model was rewritten by a rollback, is rewarded.",
    ),
    mutant(
        "finalize-includes-the-whole-roster",
        ["FIN-03", "FIN-18"],
        STORE,
        "included = sorted(x.roster.hotkey for x in self.verified_inputs_v2(run_id, w))",
        'included = sorted(e["hotkey"] for e in self._record_v2(run_id, "round", str(w))'
        '["body"]["roster"])',
        [PAYS_AUDITED],
        "The included set comes from the opening roster, so an excluded miner is part of it.",
    ),
    mutant(
        "finalize-skips-the-eligibility-check",
        ["FIN-03"],
        STORE,
        "if not status.eligible or status.record.admission_id != entry.admission_id:",
        "if False:",
        [ELIGIBLE],
        "A miner whose admission is no longer eligible is still finalized and paid.",
    ),
    mutant(
        "finalize-commits-before-the-ledger",
        ["FIN-04"],
        STORE,
        '            self._put_record_v2(run_id, "finality", str(w), asdict(settled))\n',
        '            self._put_record_v2(run_id, "finality", str(w), asdict(settled))\n'
        '            self._db.execute("COMMIT")\n'
        '            self._db.execute("BEGIN IMMEDIATE")\n',
        [ATOMIC],
        "The SETTLED row is committed before the ledger credit: a ledger failure leaves it behind.",
    ),
    mutant(
        "finalize-replays-a-conflicting-body",
        ["FIN-06"],
        STORE,
        'if json.loads(previous_final[0])["body"] != model.model_dump(mode="json"):',
        "if False:",
        [ONCE],
        "A different Finalize of a settled round returns the stored result instead of a conflict.",
    ),
    mutant(
        "signed-operations-ignore-a-conflicting-body",
        ["FIN-06", "FIN-13"],
        STORE,
        "if known is not None and known[0] != digest:",
        "if False:",
        [ONCE, OPENING],
        "The single-use reservation of a signed message no longer refuses a different body.",
    ),
    mutant(
        "open-overwrites-a-different-opening",
        ["FIN-13"],
        STORE,
        'if body_digest(json.loads(prior[0])["body"]) != body.digest():',
        "if False:",
        [OPENING],
        "A different RoundOpenV2 for an opened round replaces it instead of conflicting.",
    ),
    mutant(
        "open-gates-on-a-truncated-history",
        ["FIN-12"],
        STORE,
        "require_open(run_id, body.w, history)",
        "require_open(run_id, body.w, history[-1:])",
        [THIRD_ROUND],
        "The w+2 gate of an opening sees only the newest round.",
    ),
    mutant(
        "rollback-accepts-any-signer",
        ["FIN-14"],
        STORE,
        'model = self._authenticated_v2(run_id, raw, "Rollback", self._coord().ss58)',
        "model = self._authenticated_v2(\n"
        '                run_id, raw, "Rollback", envelope_v2.parse_envelope(raw).signer\n'
        "            )",
        [ROLLBACK_MATRIX],
        "A Rollback signed by a miner reaches the repair.",
    ),
    mutant(
        "rollback-accepts-a-miner-without-a-cause",
        ["FIN-14"],
        STORE,
        "if len(matched) != 1:",
        "if len(matched) > 1:",
        [ROLLBACK_MATRIX],
        "A miner with no accepted referee resolution is removed from the model.",
    ),
    mutant(
        "rollback-rewrites-a-settled-round",
        ["FIN-15"],
        STORE,
        'if status.disposition == "SETTLED":',
        "if False:",
        [ROLLBACK_SETTLED],
        "A rollback is allowed to start on a round whose rewards were already minted.",
    ),
    mutant(
        "rollback-checks-only-the-first-round",
        ["FIN-15"],
        STORE,
        "for w in (model.w, model.w + 1):",
        "for w in (model.w,):",
        [ROLLBACK_SETTLED],
        "The second round of the window is not checked for settlement before a rollback.",
    ),
    mutant(
        "rollback-accepts-a-different-body-for-a-repaired-round",
        ["FIN-16"],
        STORE,
        'if saved["request"]["body"] != model.model_dump(mode="json"):',
        "if False:",
        [ROLLBACK_REPLAY],
        "A different Rollback for a repaired round returns the stored result as accepted.",
    ),
    mutant(
        "legacy-finalizes-before-d-final",
        ["FIN-27"],
        STORE,
        '            if self._now(db) < body.d_final or row["selected"] is None:',
        '            if row["selected"] is None:',
        [LEGACY],
        "A legacy round is finalized, and its journal appended, before d_final.",
    ),
    mutant(
        "ledger-accepts-a-finalize-the-coordinator-did-not-sign",
        ["FIN-01"],
        ESCROW,
        "            or evidence.finalize.signer != self.policy.reward_authority\n",
        "",
        [LEDGER_FINALITY],
        "The ledger no longer compares the finalize signer with the pinned reward authority.",
    ),
    mutant(
        "ledger-skips-the-finality-replay",
        ["FIN-01"],
        ESCROW,
        "        if self.replay_finality(evidence) != final.final_theta_hash_w1:\n"
        '            raise EscrowError("FINALITY_REPLAY")\n',
        "",
        [LEDGER_FINALITY],
        "Rewards are minted for a Finalize whose theta the replay did not reproduce.",
    ),
    mutant(
        "rollback-accepts-a-cause-naming-another-loser",
        ["FIN-14"],
        STORE,
        '                    or resolution["loser"] != hotkey\n',
        "",
        [CAUSE_REFUSED],
        "A miner is removed from the model on a resolution that convicted somebody else.",
    ),
    mutant(
        "rollback-accepts-a-forged-referee-signature",
        ["FIN-14"],
        STORE,
        "                    not envelope_v2.verify_envelope(signed)\n",
        "                    False\n",
        [CAUSE_REFUSED],
        "A resolution whose signature does not verify is accepted as the referee's.",
    ),
    mutant(
        "rollback-accepts-a-cause-without-registered-evidence",
        ["FIN-14"],
        STORE,
        "                if independent is None:\n",
        "                if False:\n",
        [CAUSE_REFUSED],
        "A resolution without registered independent referee work removes a miner.",
    ),
    mutant(
        "exclude-closes-an-applied-round-at-once",
        ["FIN-12", "FIN-10"],
        FINALITY,
        "        rollback_complete=not status.applied,\n",
        "        rollback_complete=True,\n",
        [EXCLUDE_CLOSES],
        "An applied round that is excluded counts as repaired before any rollback ran.",
    ),
    mutant(
        "exclude-never-closes-an-unapplied-round",
        ["FIN-12"],
        FINALITY,
        "        rollback_complete=not status.applied,\n",
        "        rollback_complete=False,\n",
        [EXCLUDE_CLOSES],
        "An excluded round that never applied waits for a rollback that has nothing to repair.",
    ),
    mutant(
        "ledger-ignores-the-reference-image",
        ["FIN-03"],
        ESCROW,
        "                    or x.replay_env.image_digest\n"
        "                    != self.manifest.training.reference_spec.image_digest\n",
        "",
        [AUDIT_PROVEN],
        "A verdict replayed in another image is credited.",
    ),
    mutant(
        "ledger-ignores-the-driver-allowlist",
        ["FIN-03"],
        ESCROW,
        "                    or x.replay_env.driver\n"
        "                    not in self.manifest.training.reference_spec.driver_allowlist\n",
        "",
        [AUDIT_PROVEN],
        "A verdict replayed with an unlisted driver is credited.",
    ),
    mutant(
        "ledger-accepts-a-self-audit",
        ["FIN-03"],
        ESCROW,
        "                or any(e.signer == c.hotkey for e in envs)\n",
        "",
        [SELF_AUDIT],
        "A miner that is also a pinned auditor is credited on a verdict it signed itself.",
    ),
    mutant(
        "ledger-accepts-a-challenge-of-any-signer",
        ["FIN-03"],
        ESCROW,
        'self.run_id, {"AuditChallengeV2": lambda s: s == self.policy.reward_authority}',
        'self.run_id, {"AuditChallengeV2": lambda s: True}',
        [CHALLENGE_SIGNER],
        "An audit challenge the reward authority did not sign binds the verdict to a commit.",
    ),
    mutant(
        "finalize-writes-no-settlement-records",
        ["FIN-04"],
        STORE,
        "            for origin in origins:\n"
        "                self._put_record_v2(\n"
        "                    run_id,\n"
        '                    "settlement",',
        "            for origin in []:\n"
        "                self._put_record_v2(\n"
        "                    run_id,\n"
        '                    "settlement",',
        [PAYS_AUDITED],
        "A reward origin is credited without the settlement record that lets it vest.",
    ),
    mutant(
        "finalize-accepts-an-expired-envelope",
        ["FIN-01"],
        "src/hypertrain/protocol/envelope_v2.py",
        "        if now_round > env.exp_drand:\n",
        "        if False:\n",
        [FINALIZE_MATRIX],
        "A Finalize whose signed expiry has passed settles the round.",
    ),
    mutant(
        "open-starts-from-any-public-theta",
        ["FIN-13"],
        STORE,
        "            if body.theta_hash != state_hash(theta):\n",
        "            if False:\n",
        [OPENING],
        "A round opens on a start state that is not the predecessor's authenticated output.",
    ),
    mutant(
        "rollback-ignores-open-disputes",
        ["FIN-14"],
        STORE,
        "if not status.applied or status.unresolved_audits or status.unresolved_disputes:",
        "if not status.applied or status.unresolved_audits:",
        [ROLLBACK_MATRIX],
        "A rollback starts while a dispute of one of the two rounds is open.",
    ),
    mutant(
        "rollback-ignores-open-audits",
        ["FIN-14"],
        STORE,
        "if not status.applied or status.unresolved_audits or status.unresolved_disputes:",
        "if not status.applied or status.unresolved_disputes:",
        [ROLLBACK_MATRIX],
        "A rollback starts while an audit of one of the two rounds is open.",
    ),
    mutant(
        "rollback-ignores-an-unapplied-round",
        ["FIN-14"],
        STORE,
        "if not status.applied or status.unresolved_audits or status.unresolved_disputes:",
        "if status.unresolved_audits or status.unresolved_disputes:",
        [ROLLBACK_MATRIX],
        "A rollback starts on a round whose tape was never applied.",
    ),
    mutant(
        "rollback-ignores-the-recomputed-steps",
        ["FIN-14"],
        STORE,
        'if model.recomputed != ["agg_w", "step_w", "agg_w1", "step_w1"]:',
        "if False:",
        [ROLLBACK_MATRIX],
        "A rollback that lists fewer recomputed steps reaches the repair.",
    ),
    mutant(
        "rollback-ignores-the-original-theta",
        ["FIN-14"],
        STORE,
        'if model.old_theta_hash_w2 != old["theta_hash"]:',
        "if False:",
        [ROLLBACK_MATRIX],
        "A rollback of another original theta reaches the repair.",
    ),
    mutant(
        "rollback-reaches-past-the-window",
        ["FIN-14"],
        STORE,
        '"AND CAST(id AS INTEGER)>?",',
        '"AND CAST(id AS INTEGER)>?+99",',
        [ROLLBACK_MATRIX],
        "A rollback reaches past the two-round speculative window.",
    ),
    mutant(
        "rollback-accepts-causes-it-does-not-rest-on",
        ["FIN-14"],
        STORE,
        'if {e["evidence_hash"] for e in evidence} != set(model.cause_hashes):',
        "if False:",
        [CAUSE_SET],
        "A rollback that signs causes beyond the ones that exclude its miners is accepted.",
    ),
    mutant(
        "rollback-accepts-a-resolution-of-another-signer",
        ["FIN-14"],
        STORE,
        '                        signed["signer"] != turn["contest"]["referee"]\n'
        '                        or signed["signer"] not in policy.referees\n',
        "                        False\n",
        [CAUSE_REFUSED],
        "A resolution signed by a key that was not the contest's referee is a cause.",
    ),
    mutant(
        "legacy-finalizes-a-wrong-included-set",
        ["FIN-27"],
        STORE,
        "            if list(m.included) != included:\n",
        "            if False:\n",
        [LEGACY],
        "A legacy Finalize naming other miners than the audited ones appends to the journal.",
    ),
]
