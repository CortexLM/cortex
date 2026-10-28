"""Mutants of the locks package: one small regression of src/hypertrain per pinned defence.

Snippets are the exact source text, indentation included, so they sit at column 0 below.
Run from hypertrain/: .venv/bin/python scripts/check_ledger_invariant_mutants.py --only <ids>
"""

from __future__ import annotations

ADMISSION = "src/hypertrain/challenge/admission.py"
STORE = "src/hypertrain/challenge/store.py"
LEDGER = "src/hypertrain/ledger/escrow_v2.py"
DISPUTES = "src/hypertrain/challenge/disputes_v2.py"
PROTOCOL = "src/hypertrain/protocol/envelope_v2.py"

LINKAGE_TEST = (
    "tests/challenge/test_admission_v2.py::test_pending_recovery_rejects_wrong_accepted_linkage"
)
LEDGER_RELEASE = "tests/ledger/test_escrow_v2.py::test_release_rejects_when_disputed_or_unvested"
ROUTES = "tests/ledger_invariants/test_locks.py::"
API = "tests/ledger_invariants/test_locks_2.py::"
EXISTING = "tests/challenge/test_admission_v2.py::"

ELIGIBILITY = API + "test_eligibility_needs_current_stake_standing_and_screen"
REQUIRED_STAKE = API + "test_the_required_stake_is_the_largest_of_its_three_terms"
ROLE_KEYS = API + "test_no_role_key_is_exempt_from_the_stake"
GRADUATION = API + "test_graduation_needs_funding_clean_finalizations_and_every_canary_block"
ROTATION = API + "test_rotation_keeps_the_standing_and_the_history_of_both_keys"
SECOND_APPLICATION = (
    API + "test_a_second_application_is_refused_for_a_known_hotkey_or_an_unsettled_coldkey"
)
COLDKEY_STATE = (
    API + "test_a_coldkey_with_a_probation_or_suspended_admission_cannot_admit_another_hotkey"
)
RECOVERY = API + "test_recovery_needs_a_resolved_nonfraud_record_at_or_past_its_release_round"
BANNED = (
    API + "test_a_banned_admission_cannot_be_resumed_suspended_challenged_or_graduated_or_reapplied"
)
ROSTER = ROUTES + "test_a_round_opens_only_when_every_roster_member_is_currently_funded"
ROSTER_ENTRY = ROUTES + "test_a_roster_entry_must_equal_the_admission_it_names"
POLICY = API + "test_a_join_signed_for_another_policy_is_refused_and_writes_nothing"
ACCEPT = ROUTES + "test_an_accept_is_refused_when_the_miner_lost_its_standing_after_the_opening"
FINALIZE_INPUT = (
    ROUTES + "test_a_finalize_input_is_refused_for_a_member_who_lost_standing_after_the_opening"
)
FINALIZED_ROUND = (
    ROUTES
    + "test_a_finalized_round_answers_a_replay_with_its_stored_result_and_refuses_another_body"
)
VESTING = ROUTES + "test_a_reward_origin_vests_only_as_its_settlement_record_allows"
BOND = (
    ROUTES + "test_the_owner_releases_a_contest_bond_only_with_the_terms_of_its_own_closed_dispute"
)
ROUTE_BACK = (
    ROUTES
    + "test_a_suspended_miner_recovers_once_from_a_closed_dispute_and_not_before_its_release_round"
)
FRAUD_CLOSED = (
    ROUTES + "test_a_miner_who_lost_a_fraud_contest_has_no_route_back_and_no_new_application"
)
LAPSE = ROUTES + "test_a_lapsed_challenge_window_suspends_nobody_and_frees_the_trial_place"
NAMED_AFTER = (
    ROUTES + "test_a_live_admission_keeps_its_stake_when_the_lock_is_named_after_a_settled_object"
)
LIVE_STAKE = (
    ROUTES + "test_the_stake_of_a_live_admission_is_released_only_once_it_is_no_longer_live"
)
FRAUD_BURNS_REWARD = (
    ROUTES + "test_fraud_proven_against_a_staked_miner_burns_the_reward_origin_it_never_vested"
)
ROSTERED_STAKE = ROUTES + "test_the_stake_of_a_rostered_member_is_held_until_its_round_closes"
HARDENING = "tests/challenge/test_admission_hardening_v2.py::"
BOND_ROUTES = HARDENING + "test_owner_takes_the_bond_back_through_the_routes"
BOND_UNITS = "tests/challenge/test_admission_bond_v2.py::"
BORROWED_SETTLEMENT = BOND_UNITS + "test_a_lock_cannot_borrow_the_settlement_of_another_id"
FRAUD_ROUTES = HARDENING + "test_referee_fraud_against_the_miner_seizes_the_bond_through_the_routes"

STAKE_TERM = """\
            and units >= required
"""
STAKE_TERM_EXEMPT = """\
            and (units >= required or hotkey == self.coord.ss58)
"""
PENDING_TERM = """\
            and not r.pending_dispute
            and now <= r.screen_until
"""
PENDING_TERM_GONE = """\
            and now <= r.screen_until
"""
SCREEN_TERM = """\
            and now <= r.screen_until
"""
GOAL_TERM = """\
            (p.G_max_units * 10**18 + denominator - 1) // denominator
            if denominator
            else p.G_max_units + units + 1,
"""
COLLECTIBLE_TERM = (
    "            self.escrow.manifest.training.verify.s_min_units(p.R_collectible_units),\n"
)
SCREEN_AND_STAKE = """\
            and now <= r.screen_until
            and units >= required
"""
SCREEN_ENDS_EARLY = """\
            and now < r.screen_until
            and units >= required
"""
OUTBOX_TERM = """\
            and not self.store.pending_outbox(r.admission_id)
"""
BOUND_TERM = """\
            and funding.conservative_bound
"""
GRADUATE_CONDITION = """\
                and r.canary_blocks == (1, 2, 3)
                and s.eligible
"""
GRADUATE_WITHOUT_FUNDING = """\
                and r.canary_blocks == (1, 2, 3)
"""
GRADUATE_STATE = """\
                r.state == "PROBATION"
                and r.clean_count >= self.policy.clean_finalizations
"""
GRADUATE_ANY_LIVE_STATE = """\
                r.state in ("PROBATION", "ACTIVE")
                and r.clean_count >= self.policy.clean_finalizations
"""
GRADUATE_WITHOUT_CANARIES = """\
                and s.eligible
"""
CLEAN_TERM = """\
                and r.clean_count >= self.policy.clean_finalizations
"""
ROSTER_CHECK = """\
                if not status.eligible or (
                    entry.admission_id,
"""
ROSTER_CHECK_GONE = """\
                if (
                    entry.admission_id,
"""
ACCEPT_CHECK = """\
                    not status.eligible
                    or model.work_screen_hash"""
ACCEPT_CHECK_GONE = """\
                    model.work_screen_hash"""
INPUT_CHECK = """\
            if not status.eligible or status.record.admission_id != entry.admission_id:
"""
INPUT_CHECK_GONE = """\
            if status.record.admission_id != entry.admission_id:
"""
NEW_KEY_HISTORY = """\
                "INSERT INTO admission_history VALUES(?,?,?)",
                (op.new_hotkey, r.admission_id, r.coldkey),
"""
OLD_KEY_HISTORY_TWICE = """\
                "INSERT OR IGNORE INTO admission_history VALUES(?,?,?)",
                (op.hotkey, r.admission_id, r.coldkey),
"""
ROTATE_UPDATE = '"UPDATE admissions_v2 SET hotkey=? WHERE admission_id=? AND hotkey=?"'
ROTATE_UPDATE_RESET = (
    '"UPDATE admissions_v2 SET hotkey=?,strikes=0 WHERE admission_id=? AND hotkey=?"'
)
SPENT_KEY_REFUSAL = """\
                raise AdmissionError("NEW_HOTKEY_HISTORY")
"""
HISTORY_REFUSAL = """\
                raise AdmissionError("HOTKEY_HISTORY_EXISTS")
"""
COLDKEY_REFUSAL = """\
                raise AdmissionError("COLDKEY_PROBATION_EXISTS")
"""
PASS = """\
                pass
"""
COLDKEY_QUERY = "\"SELECT 1 FROM admissions_v2 WHERE coldkey=? AND state!='ACTIVE' \"\n"
COLDKEY_QUERY_ANY = '"SELECT 1 FROM admissions_v2 WHERE coldkey=? "\n'
COLDKEY_QUERY_NOT_SUSPENDED = (
    "\"SELECT 1 FROM admissions_v2 WHERE coldkey=? AND state NOT IN ('ACTIVE','SUSPENDED') \"\n"
)
COLDKEY_QUERY_NOT_PROBATION = (
    "\"SELECT 1 FROM admissions_v2 WHERE coldkey=? AND state NOT IN ('ACTIVE','PROBATION') \"\n"
)
COLD_SIGNATURE = (
    ") and verify(decode_hotkey(request.coldkey), msg, bytes.fromhex(request.cold_sig))"
)
FRAUD_REFUSAL = """\
                or result.outcome == "FRAUD"
"""
UNRESOLVED_REFUSAL = """\
                or result.unresolved
"""
RELEASE_ROUND_REFUSAL = """\
                or now < result.release_beacon
"""
LINKAGE = """\
                    result.dispute_id,
                    result.evidence_hash,
                )
                != (
                    self.escrow.run_id,
                    r.admission_id,
                    r.coldkey,
                    pending["dispute_id"],
                    pending["evidence_hash"],
"""
LINKAGE_WITHOUT_EVIDENCE = """\
                    result.dispute_id,
                )
                != (
                    self.escrow.run_id,
                    r.admission_id,
                    r.coldkey,
                    pending["dispute_id"],
"""
RECOVER_BINDING = """\
                        if row is None or row[0] != auth.commit_hash:
"""
RECOVER_UNBOUND = """\
                        if row is None:
"""
BANNED_GUARD = """\
            r = self.store.record(hotkey)
            if r.state == "BANNED":
                raise AdmissionError("TERMINAL_BANNED")
"""
BANNED_GUARD_GONE = """\
            r = self.store.record(hotkey)
            if r.state == "BANNED":
                pass
"""
YOUNG_PROBATION = (
    '        live_phase = r.state == "ACTIVE" or (r.state == "PROBATION" and r.clean_count >= 4)\n'
)
YOUNG_PROBATION_LIVE = '        live_phase = r.state == "ACTIVE" or (r.state == "PROBATION")\n'
RESUME_STATE = """\
            if r.state != "SUSPENDED" or r.pending_dispute:
"""
RESUME_ANY_STATE = """\
            if r.pending_dispute:
"""
CHALLENGE_STATE = """\
            if r.state not in ("APPLIED", "PROBATION", "ACTIVE") or r.pending_dispute:
"""
COLDKEY_GROUP = """\
                    entry.coldkey_group,
                    entry.state,
                    entry.q_i,
                ) != (r.admission_id, r.coldkey, r.state, f32hex(1.0)):
"""
COLDKEY_GROUP_GONE = """\
                    entry.state,
                    entry.q_i,
                ) != (r.admission_id, r.state, f32hex(1.0)):
"""
WEIGHT_CEILING = """\
                if entry.eligible_weight != 4194304:
                    raise ChallengeError(422, "live eligible ceiling must be Q/4")
"""
WEIGHT_CEILING_CHECK = "                if entry.eligible_weight != 4194304:\n"
WEIGHT_BELOW_ONLY = "                if entry.eligible_weight < 4194304:\n"
POLICY_REFUSAL = """\
            raise AdmissionError("POLICY_CHANGED")
"""
POLICY_PASS = """\
            pass
"""
FINALIZE_REPLAY = """\
                return self._record_v2(run_id, "finalize-result", str(w))
"""
MATURE_UNRESOLVED = """\
                        status.unresolved
                        or status.outcome == "FRAUD"
"""
MATURE_UNRESOLVED_GONE = """\
                        status.outcome == "FRAUD"
"""
MATURE_FRAUD = """\
                        or status.outcome == "FRAUD"
                        or now_beacon < status.release_beacon
"""
MATURE_FRAUD_GONE = """\
                        or now_beacon < status.release_beacon
"""
MATURE_ROUND = """\
                        or now_beacon < status.release_beacon
                        or finality is None
"""
MATURE_ROUND_GONE = """\
                        or finality is None
"""
MATURE_BINDING = """\
                        or finality is None
                        or status.finality_hash != finality["finalize_hash"]
"""
RELEASE_OPEN = """\
            if (
                status.unresolved
                or status.outcome == "FRAUD"
                or now_beacon < status.release_beacon
"""
RELEASE_OPEN_GONE = """\
            if (
                status.outcome == "FRAUD"
                or now_beacon < status.release_beacon
"""
RELEASE_TERMS = """\
                or now_beacon < status.release_beacon
                or (op.finality_hash, op.closed_dispute_root)
                != (status.finality_hash, status.closed_dispute_root)
            ):
                raise EscrowError("DISPUTED_OR_UNVESTED")"""
RELEASE_WITHOUT_ROUND = """\
                or (op.finality_hash, op.closed_dispute_root)
                != (status.finality_hash, status.closed_dispute_root)
            ):
                raise EscrowError("DISPUTED_OR_UNVESTED")"""
RELEASE_WITHOUT_FINALITY_HASH = """\
                or op.closed_dispute_root != status.closed_dispute_root
            ):
                raise EscrowError("DISPUTED_OR_UNVESTED")"""
RELEASE_WITHOUT_TERMS = """\
                or now_beacon < status.release_beacon
            ):
                raise EscrowError("DISPUTED_OR_UNVESTED")"""
SLASH_ON_FRAUD = """\
                and status.loser == turn.contest.miner
"""
SLASH_NEVER = """\
                and status.loser == turn.contest.auditor
"""
LOCK_KIND_RESOLVER = "            return self.admission_settlement(lock_id)\n"
LOCK_GENERIC_RESOLVER = "            return self.settlement(lock_id)\n"
LIVE_HOLD = "            release = max(release, r.screen_until + 1)\n"
OPEN_ROUND_HOLD = "            unresolved = unresolved or status is None or not status.closed\n"
OPEN_ROUND_IGNORED = "            unresolved = unresolved or status is None\n"
CONVICTION = '                            admission.settle_fraud(result["dispute_id"], now=now)\n'
BAN_ON_FRAUD = '            if reason == "TRAINING_FRAUD" and r.state != "BANNED":\n'
BURN_PENDING = '            if original.kind == "LOCK_ADMISSION" and burn_pending:\n'
RELEASABLE_WHEN_QUIET = "            unresolved=unresolved or unvested is not None,\n"
EXPIRE = """\
                self.store.db.execute(
                    "UPDATE admission_trial_results SET outcome='EXPIRED' WHERE epoch=?", (epoch,)
                )
"""
EXPIRE_AND_SUSPEND = (
    EXPIRE
    + """\
                self.store.db.execute(
                    "UPDATE admissions_v2 SET state='SUSPENDED' WHERE trial_epoch=?",
                    (epoch,),
                )
"""
)

MUTANTS = [
    # --- LCK-01, LCK-14: what eligibility reads --------------------------------------------
    {
        "id": "locks-eligibility-ignores-the-stake",
        "invariants": ["LCK-01", "LCK-14"],
        "path": ADMISSION,
        "old": STAKE_TERM,
        "new": "            and units >= 0\n",
        "tests": [ELIGIBILITY + "[no-stake]", ELIGIBILITY + "[one-unit-short]"],
        "why": "an admission with no stake or one unit short of it is eligible for a reward.",
    },
    {
        "id": "locks-eligibility-exempts-the-coordinator",
        "invariants": ["LCK-14"],
        "path": ADMISSION,
        "old": STAKE_TERM,
        "new": STAKE_TERM_EXEMPT,
        "tests": [ROLE_KEYS + "[no-stake-coordinator]", ROLE_KEYS + "[one-unit-short-coordinator]"],
        "why": "the coordinator key is eligible without any stake.",
    },
    {
        "id": "locks-eligibility-ignores-a-pending-dispute",
        "invariants": ["LCK-01"],
        "path": ADMISSION,
        "old": PENDING_TERM,
        "new": PENDING_TERM_GONE,
        "tests": [ELIGIBILITY + "[pending-dispute]"],
        "why": "a miner under a pending dispute stays eligible.",
    },
    {
        "id": "locks-eligibility-ignores-a-lapsed-screen",
        "invariants": ["LCK-01", "LCK-02"],
        "path": ADMISSION,
        "old": SCREEN_TERM,
        "new": "",
        "tests": [ELIGIBILITY + "[screen-lapsed]", ACCEPT + "[screen]"],
        "why": "a work screen that has expired still makes the miner eligible.",
    },
    {
        "id": "locks-eligibility-counts-a-young-probation",
        "invariants": ["LCK-01"],
        "path": ADMISSION,
        "old": YOUNG_PROBATION,
        "new": YOUNG_PROBATION_LIVE,
        "tests": [ELIGIBILITY + "[probation-below-four]"],
        "why": "a PROBATION miner with fewer than four clean finalizations is live.",
    },
    {
        "id": "locks-eligibility-ignores-an-unapplied-lock",
        "invariants": ["LCK-01"],
        "path": ADMISSION,
        "old": OUTBOX_TERM,
        "new": "",
        "tests": [ELIGIBILITY + "[lock-not-yet-applied]"],
        "why": "a lock accepted but not yet applied to the ledger does not stop eligibility.",
    },
    {
        "id": "locks-eligibility-ignores-the-conservative-bound",
        "invariants": ["LCK-01"],
        "path": ADMISSION,
        "old": BOUND_TERM,
        "new": "",
        "tests": [ELIGIBILITY + "[bound-not-conservative]"],
        "why": "a run whose bound is not conservative still pays an admission.",
    },
    {
        "id": "locks-eligibility-drops-the-goal-term",
        "invariants": ["LCK-01"],
        "path": ADMISSION,
        "old": GOAL_TERM,
        "new": "            0,\n",
        "tests": [REQUIRED_STAKE + "[goal-over-the-stake-fraction-one-unit-short]"],
        "why": "the stake that covers the run's goal is not required: a smaller stake is eligible.",
    },
    {
        "id": "locks-eligibility-drops-the-collectible-reward-term",
        "invariants": ["LCK-01"],
        "path": ADMISSION,
        "old": COLLECTIBLE_TERM,
        "new": "            0,\n",
        "tests": [REQUIRED_STAKE + "[collectible-reward-multiple-one-unit-short]"],
        "why": "the multiple of the collectible reward is not required: a smaller stake counts.",
    },
    {
        "id": "locks-eligibility-ends-the-screen-a-round-early",
        "invariants": ["LCK-01"],
        "path": ADMISSION,
        "old": SCREEN_AND_STAKE,
        "new": SCREEN_ENDS_EARLY,
        "tests": [ELIGIBILITY + "[screen-ends-this-round]"],
        "why": "a miner whose screen ends this very round is already ineligible.",
    },
    # --- LCK-03: ACTIVE only through graduate -----------------------------------------------
    {
        "id": "locks-graduation-skips-funding",
        "invariants": ["LCK-03"],
        "path": ADMISSION,
        "old": GRADUATE_CONDITION,
        "new": GRADUATE_WITHOUT_FUNDING,
        "tests": [GRADUATION + "[stake-one-short]", GRADUATION + "[screen-lapsed]"],
        "why": "an unfunded or unscreened PROBATION miner graduates to ACTIVE.",
    },
    {
        "id": "locks-graduation-skips-clean-finalizations",
        "invariants": ["LCK-03"],
        "path": ADMISSION,
        "old": CLEAN_TERM,
        "new": "",
        "tests": [GRADUATION + "[eleven-clean-finalizations]"],
        "why": "a miner graduates before its twelfth clean finalization.",
    },
    {
        "id": "locks-graduation-skips-the-canary-blocks",
        "invariants": ["LCK-03"],
        "path": ADMISSION,
        "old": GRADUATE_CONDITION,
        "new": GRADUATE_WITHOUT_CANARIES,
        "tests": [GRADUATION + "[canary-block-missing]"],
        "why": "a miner graduates with a canary block missing.",
    },
    {
        "id": "locks-graduation-repeats-for-an-active-admission",
        "invariants": ["LCK-03"],
        "path": ADMISSION,
        "old": GRADUATE_STATE,
        "new": GRADUATE_ANY_LIVE_STATE,
        "tests": [GRADUATION + "[already-active]"],
        "why": "an ACTIVE admission is graduated again: a second ACTIVE transition on record.",
    },
    # --- LCK-02: roster, accept and finalize inputs ----------------------------------------
    {
        "id": "locks-roster-accepts-an-ineligible-member",
        "invariants": ["LCK-02", "LCK-14"],
        "path": STORE,
        "old": ROSTER_CHECK,
        "new": ROSTER_CHECK_GONE,
        "tests": [ROSTER + "[underfunded]", ROSTER + "[unfunded]", ROSTER + "[referee-unfunded]"],
        "why": "a round opens with a roster member that holds no current funded eligibility.",
    },
    {
        "id": "locks-roster-ignores-the-coldkey-group",
        "invariants": ["LCK-02"],
        "path": STORE,
        "old": COLDKEY_GROUP,
        "new": COLDKEY_GROUP_GONE,
        "tests": [ROSTER_ENTRY + "[coldkey-group]"],
        "why": "a roster entry names another coldkey group than the admission it lists.",
    },
    {
        "id": "locks-roster-takes-any-weight",
        "invariants": ["LCK-02"],
        "path": STORE,
        "old": WEIGHT_CEILING,
        "new": "",
        "tests": [ROSTER_ENTRY + "[weight-below]", ROSTER_ENTRY + "[weight-above]"],
        "why": "a roster entry carries an eligible weight other than the live ceiling Q/4.",
    },
    {
        "id": "locks-roster-takes-a-weight-above-the-ceiling",
        "invariants": ["LCK-02"],
        "path": STORE,
        "old": WEIGHT_CEILING_CHECK,
        "new": WEIGHT_BELOW_ONLY,
        "tests": [ROSTER_ENTRY + "[weight-above]"],
        "why": "a roster entry carries more eligible weight than the live ceiling Q/4.",
    },
    {
        "id": "locks-accept-skips-eligibility",
        "invariants": ["LCK-02"],
        "path": STORE,
        "old": ACCEPT_CHECK,
        "new": ACCEPT_CHECK_GONE,
        "tests": [ACCEPT + "[screen]", ACCEPT + "[pending-dispute]"],
        "why": "an accept is taken from a miner whose screen lapsed or who is under dispute.",
    },
    {
        "id": "locks-finalize-input-skips-eligibility",
        "invariants": ["LCK-01"],
        "path": STORE,
        "old": INPUT_CHECK,
        "new": INPUT_CHECK_GONE,
        "tests": [FINALIZE_INPUT],
        "why": "a reward input is built for a member that is no longer funded and eligible.",
    },
    # --- LCK-16, LCK-23: rotation and join --------------------------------------------------
    {
        "id": "locks-rotation-forgets-the-new-key",
        "invariants": ["LCK-16", "LCK-23"],
        "path": ADMISSION,
        "old": NEW_KEY_HISTORY,
        "new": OLD_KEY_HISTORY_TWICE,
        "tests": [ROTATION],
        "why": "the key rotated to has no history row, so it cannot be rotated away from again.",
    },
    {
        "id": "locks-rotation-resets-the-strikes",
        "invariants": ["LCK-16"],
        "path": ADMISSION,
        "old": ROTATE_UPDATE,
        "new": ROTATE_UPDATE_RESET,
        "tests": [ROTATION, EXISTING + "test_rotation_carries_locks_strikes_dispute_and_history"],
        "why": "rotating the hotkey launders the admission's strikes.",
    },
    {
        "id": "locks-rotation-returns-to-a-spent-key",
        "invariants": ["LCK-16", "LCK-23"],
        "path": ADMISSION,
        "old": SPENT_KEY_REFUSAL,
        "new": PASS,
        "tests": [ROTATION],
        "why": "a rotation back to a key that already belonged to an admission is not refused.",
    },
    {
        "id": "locks-join-forgets-the-hotkey-history",
        "invariants": ["LCK-23"],
        "path": ADMISSION,
        "old": HISTORY_REFUSAL,
        "new": PASS,
        "tests": [SECOND_APPLICATION, BANNED],
        "why": "a hotkey that already had an admission applies again.",
    },
    {
        "id": "locks-join-allows-a-second-unsettled-coldkey",
        "invariants": ["LCK-23"],
        "path": ADMISSION,
        "old": COLDKEY_REFUSAL,
        "new": PASS,
        "tests": [SECOND_APPLICATION, BANNED],
        "why": "a coldkey holds a second admission while its first is not ACTIVE.",
    },
    {
        "id": "locks-join-lets-a-suspended-coldkey-apply-again",
        "invariants": ["LCK-23"],
        "path": ADMISSION,
        "old": COLDKEY_QUERY,
        "new": COLDKEY_QUERY_NOT_SUSPENDED,
        "tests": [COLDKEY_STATE + "[SUSPENDED]"],
        "why": "a coldkey whose admission is suspended starts again with a fresh admission.",
    },
    {
        "id": "locks-join-lets-a-probation-coldkey-apply-again",
        "invariants": ["LCK-23"],
        "path": ADMISSION,
        "old": COLDKEY_QUERY,
        "new": COLDKEY_QUERY_NOT_PROBATION,
        "tests": [COLDKEY_STATE + "[PROBATION]"],
        "why": "a coldkey whose admission is still on probation holds a second admission.",
    },
    {
        "id": "locks-join-blocks-an-active-coldkey",
        "invariants": ["LCK-23"],
        "path": ADMISSION,
        "old": COLDKEY_QUERY,
        "new": COLDKEY_QUERY_ANY,
        "tests": [SECOND_APPLICATION],
        "why": "a coldkey with an ACTIVE admission may not admit another hotkey.",
    },
    {
        "id": "locks-join-ignores-a-policy-change",
        "invariants": ["LCK-23"],
        "path": ADMISSION,
        "old": POLICY_REFUSAL,
        "new": POLICY_PASS,
        "tests": [POLICY],
        "why": "a join signed for another admission policy is accepted.",
    },
    {
        "id": "locks-join-accepts-one-signature",
        "invariants": ["LCK-23"],
        "path": PROTOCOL,
        "old": COLD_SIGNATURE,
        "new": ") and True",
        "tests": [EXISTING + "test_join_rejects_when_auth_or_replay_wrong[cold_sig]"],
        "why": "a join is accepted without the coldkey's signature.",
    },
    # --- LCK-22, AD-08, DIB-09: recovery -----------------------------------------------------
    {
        "id": "locks-recovery-accepts-a-fraud-record",
        "invariants": ["LCK-22", "AD-08", "DIB-09"],
        "path": ADMISSION,
        "old": FRAUD_REFUSAL,
        "new": "",
        "tests": [RECOVERY + "[fraud]"],
        "why": "a miner whose dispute ended in FRAUD is resumed.",
    },
    {
        "id": "locks-recovery-accepts-an-unresolved-record",
        "invariants": ["LCK-22", "AD-08"],
        "path": ADMISSION,
        "old": UNRESOLVED_REFUSAL,
        "new": "",
        "tests": [RECOVERY + "[unresolved]"],
        "why": "a miner is resumed while its dispute is still open.",
    },
    {
        "id": "locks-recovery-ignores-the-release-round",
        "invariants": ["LCK-22", "AD-08"],
        "path": ADMISSION,
        "old": RELEASE_ROUND_REFUSAL,
        "new": "",
        "tests": [RECOVERY + "[before-the-release-round]", ROUTE_BACK],
        "why": "a miner is resumed before the release round of its settlement.",
    },
    {
        "id": "locks-recovery-ignores-the-evidence-linkage",
        "invariants": ["LCK-22"],
        "path": ADMISSION,
        "old": LINKAGE,
        "new": LINKAGE_WITHOUT_EVIDENCE,
        "tests": [LINKAGE_TEST + "[evidence_hash-" + "11" * 32 + "]"],
        "why": "a settlement for other evidence than the pending incident resumes the miner.",
    },
    {
        "id": "locks-recover-route-takes-any-reference",
        "invariants": ["LCK-22", "AD-08"],
        "path": STORE,
        "old": RECOVER_BINDING,
        "new": RECOVER_UNBOUND,
        "tests": [ROUTE_BACK],
        "why": "the recovery route answers for a reference that is not the pending dispute.",
    },
    # --- AD-09 neighbour, DIB-09: BANNED is terminal -----------------------------------------
    {
        "id": "locks-a-banned-admission-can-be-suspended",
        "invariants": ["AD-09", "DIB-09"],
        "path": ADMISSION,
        "old": BANNED_GUARD,
        "new": BANNED_GUARD_GONE,
        "tests": [BANNED],
        "why": "a dispute hook turns a BANNED admission into a recoverable SUSPENDED one.",
    },
    {
        "id": "locks-a-banned-admission-can-resume",
        "invariants": ["AD-09", "DIB-09"],
        "path": ADMISSION,
        "old": RESUME_STATE,
        "new": RESUME_ANY_STATE,
        "tests": [BANNED],
        "why": "resume accepts any state, a BANNED miner starts a fresh probation.",
    },
    {
        "id": "locks-a-banned-admission-can-be-challenged",
        "invariants": ["AD-09", "DIB-09"],
        "path": ADMISSION,
        "old": CHALLENGE_STATE,
        "new": RESUME_ANY_STATE,
        "tests": [BANNED],
        "why": "a BANNED admission is served a new trial.",
    },
    # --- LCK-07, LCK-09: the settlement record gates vesting ---------------------------------
    {
        "id": "locks-finalize-recomputes-a-finalized-round",
        "invariants": ["LCK-07"],
        "path": STORE,
        "old": FINALIZE_REPLAY,
        "new": PASS,
        "tests": [FINALIZED_ROUND],
        "why": "a replayed Finalize of a finalized round is processed again instead of answered.",
    },
    {
        "id": "locks-vesting-ignores-an-unresolved-record",
        "invariants": ["LCK-07", "LCK-09"],
        "path": LEDGER,
        "old": MATURE_UNRESOLVED,
        "new": MATURE_UNRESOLVED_GONE,
        "tests": [VESTING + "[unresolved]"],
        "why": "an origin whose settlement is unresolved vests.",
    },
    {
        "id": "locks-vesting-ignores-a-fraud-record",
        "invariants": ["LCK-07", "LCK-09", "CRD-22"],
        "path": LEDGER,
        "old": MATURE_FRAUD,
        "new": MATURE_FRAUD_GONE,
        "tests": [VESTING + "[fraud]"],
        "why": "an origin whose settlement is FRAUD vests.",
    },
    {
        "id": "locks-vesting-ignores-the-release-round",
        "invariants": ["LCK-07", "LCK-09"],
        "path": LEDGER,
        "old": MATURE_ROUND,
        "new": MATURE_ROUND_GONE,
        "tests": [VESTING + "[match-held-to-the-release-round]"],
        "why": "an origin vests before the release round its settlement names.",
    },
    {
        "id": "locks-vesting-ignores-the-finalization-binding",
        "invariants": ["LCK-07"],
        "path": LEDGER,
        "old": MATURE_BINDING,
        "new": MATURE_ROUND_GONE,
        "tests": [VESTING + "[another-finalization]"],
        "why": "a settlement of another finalization lets an origin of this round vest.",
    },
    # --- ESC-12 control: the contest bond settles through the real callback ------------------
    {
        "id": "locks-release-ignores-an-open-dispute",
        "invariants": ["ESC-10", "ESC-12"],
        "path": LEDGER,
        "old": RELEASE_OPEN,
        "new": RELEASE_OPEN_GONE,
        "tests": [LEDGER_RELEASE + "[True-10]"],
        "why": "a bond is released while its dispute is still open.",
    },
    {
        "id": "locks-release-ignores-the-release-round",
        "invariants": ["ESC-10", "ESC-12"],
        "path": LEDGER,
        "old": RELEASE_TERMS,
        "new": RELEASE_WITHOUT_ROUND,
        "tests": [BOND],
        "why": "a bond is released before the round its dispute closed at.",
    },
    {
        "id": "locks-release-ignores-the-settlement-terms",
        "invariants": ["ESC-10", "ESC-12"],
        "path": LEDGER,
        "old": RELEASE_TERMS,
        "new": RELEASE_WITHOUT_TERMS,
        "tests": [BOND],
        "why": "a release is accepted with terms that are not those of the settlement.",
    },
    {
        "id": "locks-release-ignores-the-finality-hash",
        "invariants": ["ESC-10", "ESC-12"],
        "path": LEDGER,
        "old": RELEASE_TERMS,
        "new": RELEASE_WITHOUT_FINALITY_HASH,
        "tests": [BOND],
        "why": "a release naming another finalization than the settlement's is accepted.",
    },
    {
        "id": "locks-a-lost-contest-never-burns-the-bond",
        "invariants": ["LCK-06", "DIB-08"],
        "path": DISPUTES,
        "old": SLASH_ON_FRAUD,
        "new": SLASH_NEVER,
        "tests": [FRAUD_CLOSED],
        "why": "a FRAUD ruling against the miner releases its contest bond instead of burning it.",
    },
    # --- ESC-10, ES-05, LCK-04, LCK-06, ESC-12: the admission bond (ADR 0010 part 6) ---------
    {
        "id": "locks-an-admission-lock-takes-the-single-settlement",
        "invariants": ["ESC-10", "ES-05", "LCK-04"],
        "path": LEDGER,
        "old": LOCK_KIND_RESOLVER,
        "new": LOCK_GENERIC_RESOLVER,
        "tests": [NAMED_AFTER + "[a settled reward origin]", BORROWED_SETTLEMENT],
        "why": "an admission lock is settled by the callback that answers by id for every object.",
    },
    {
        "id": "locks-an-admission-lock-takes-the-settlement-of-a-closed-dispute",
        "invariants": ["ES-05", "LCK-04"],
        "path": LEDGER,
        "old": LOCK_KIND_RESOLVER,
        "new": LOCK_GENERIC_RESOLVER,
        "tests": [NAMED_AFTER + "[a closed dispute]"],
        "why": "a live admission's stake named like a closed dispute is released by that dispute.",
    },
    {
        "id": "locks-a-live-admission-releases-its-stake",
        "invariants": ["ES-05", "LCK-04"],
        "path": ADMISSION,
        "old": LIVE_HOLD,
        "new": "            release = release\n",
        "tests": [LIVE_STAKE],
        "why": "the stake that makes a live miner eligible is releasable before its screen ends.",
    },
    {
        "id": "locks-a-rostered-member-releases-its-stake-before-its-round-closes",
        "invariants": ["ES-05", "LCK-04", "T-LOCK-04"],
        "path": ADMISSION,
        "old": OPEN_ROUND_HOLD,
        "new": OPEN_ROUND_IGNORED,
        "tests": [ROSTERED_STAKE],
        "why": "the stake of a member of an open round is releasable once its screen has lapsed.",
    },
    {
        "id": "locks-the-stake-of-a-quiet-admission-is-never-releasable",
        "invariants": ["ESC-12", "CRD-21", "ES-04", "BND-19"],
        "path": ADMISSION,
        "old": RELEASABLE_WHEN_QUIET,
        "new": "            unresolved=True,\n",
        "tests": [BOND_ROUTES, LIVE_STAKE],
        "why": "an admission with no open obligation can never take its stake back.",
    },
    {
        "id": "locks-a-fraud-resolution-convicts-no-admission",
        "invariants": ["LCK-06", "AD-09", "DIA-23", "DIB-08", "BND-19", "ES-04"],
        "path": STORE,
        "old": CONVICTION,
        "new": "",
        "tests": [FRAUD_ROUTES + "[True]", FRAUD_BURNS_REWARD],
        "why": "a referee verdict of fraud burns the contest bond and leaves the admission whole.",
    },
    {
        "id": "locks-a-convicted-admission-is-not-banned",
        "invariants": ["LCK-06", "AD-09", "DIA-23"],
        "path": ADMISSION,
        "old": BAN_ON_FRAUD,
        "new": "            if False:\n",
        "tests": [FRAUD_ROUTES + "[True]"],
        "why": "a miner convicted of training fraud is burned but stays free to train.",
    },
    {
        "id": "locks-a-convicted-miner-keeps-its-unvested-reward",
        "invariants": ["CRD-22", "LCK-09", "DIB-07"],
        "path": LEDGER,
        "old": BURN_PENDING,
        "new": "            if False:\n",
        "tests": [FRAUD_BURNS_REWARD],
        "why": "proven fraud leaves the reward origin of the miner to vest.",
    },
    # --- AD-06: an expired challenge window suspends nobody ----------------------------------
    {
        "id": "locks-the-reaper-suspends-the-holder",
        "invariants": ["AD-06"],
        "path": ADMISSION,
        "old": EXPIRE,
        "new": EXPIRE_AND_SUSPEND,
        "tests": [LAPSE],
        "why": "an unproven trial past its deadline suspends its holder, who then has no way back.",
    },
]
