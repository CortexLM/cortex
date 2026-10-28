"""Mutants of the admission_http package: each one removes or weakens a defence the tests pin."""

ADMISSION = "src/hypertrain/challenge/admission.py"
ENVELOPE = "src/hypertrain/protocol/envelope_v2.py"
STORE = "src/hypertrain/challenge/store.py"
APP = "src/hypertrain/challenge/app.py"
HTTP = "tests/ledger_invariants/test_admission_http.py"
INPROCESS = "tests/ledger_invariants/test_admission_http_2.py"
EXISTING = "tests/challenge/test_admission_v2.py"
HARDENING = "tests/challenge/test_admission_hardening_v2.py"
TRIAL_EXPORT = "tests/challenge/test_trial_export.py"

JOIN_FAULT = HTTP + "::test_sg06_a_join_with_one_defect_is_refused_and_leaves_no_trace"
REPLAY = HTTP + "::test_sg06_repeating_a_join_returns_the_same_admission_and_spends_no_quota"
PROPERTY = INPROCESS + "::test_ad05_sg06_identity_rules_hold_for_any_sequence_of_joins"
HOTKEY_ONCE = HTTP + "::test_ad05_a_hotkey_is_admitted_once_ever_even_after_it_moved_to_another_key"
COLDKEY_ONE = (
    HTTP + "::test_ad05_a_coldkey_has_one_non_active_admission_and_an_active_one_may_apply_again"
)
FINALIZE_BOUND = (
    INPROCESS + "::test_ad02_a_finalize_not_bound_to_this_participation_credits_nothing"
)
CREDITED_ONCE = INPROCESS + "::test_ad02_a_finalized_trial_is_credited_once_at_any_later_beacon"
REPLAYED_OVER_HTTP = (
    HTTP + "::test_ad02_a_finalized_trial_credits_one_participation_however_often_it_is_replayed"
)
POLL_KEEPS_PROOF = EXISTING + "::test_poll_after_deadline_keeps_miner_whose_proof_is_accepted"
REAPER_BOUNDARY = (
    EXISTING + "::test_reaper_closes_only_unproven_trials_past_deadline_and_penalises_nobody"
)
STRANGER_POLL = (
    HTTP + "::test_gt01_a_poll_after_the_deadline_by_anyone_expires_or_suspends_nobody[APPLIED]"
)
STRANGER_POLL_ACTIVE = STRANGER_POLL.replace("[APPLIED]", "[ACTIVE]")
NEWCOMER_SERVED = (
    INPROCESS + "::test_gt03_a_newcomer_is_served_past_the_deadline_of_unproven_trials"
)
CLOCK_CLOSES = HTTP + "::test_gt03_the_clock_closes_the_trial_of_a_suspended_or_banned_holder"
PENDING_BOUND = HTTP + "::test_ad01_the_pending_places_are_bounded_and_a_refusal_changes_nothing"
ROTATE_OWNER = HTTP + "::test_sg08_only_the_coldkey_of_the_admission_can_rotate_it"
CURRENT_OWNER = HTTP + "::test_gt01_after_a_rotation_only_the_current_hotkey_can_ask_for_the_trial"
ROTATE_CONSENT = HTTP + "::test_lck17_a_rotation_onto_a_hotkey_that_never_agreed_is_refused"
# What ADR 0010 closed and tests/challenge pins more strongly than a copy over HTTP would
# (PIN_EXISTING): the runner proves below that each of them fails under its mutant.
BANNED_ROTATION = EXISTING + "::test_banned_admission_cannot_rotate"
ROTATION_WINDOW = EXISTING + "::test_rotation_chain_is_bounded_per_coldkey_and_window"
DEAD_PLACES = EXISTING + "::test_unfunded_applied_rows_release_their_places_once_the_window_is_dead"
FUNDED_KEPT = EXISTING + "::test_funded_or_outbox_pending_applied_row_is_never_reaped"
OWNER_ONLY = TRIAL_EXPORT + "::test_challenge_route_refuses_everyone_but_the_owner"
CAPTURE_BOUND = (
    TRIAL_EXPORT
    + "::test_an_owner_request_may_expire_only_so_far_ahead[challenge-ChallengeRequest]"
)

MUTANTS = [
    {
        "id": "join-policy-check-removed",
        "invariants": ["SG-06"],
        "path": ADMISSION,
        "old": """        if request.policy_hash != digest(self.policy):
            raise AdmissionError("POLICY_CHANGED")""",
        "new": """        if False:
            raise AdmissionError("POLICY_CHANGED")""",
        "tests": [JOIN_FAULT + "[stale_policy]"],
        "why": "a join signed against a superseded admission policy is admitted.",
    },
    {
        "id": "join-signature-check-removed",
        "invariants": ["SG-06"],
        "path": ADMISSION,
        "old": """        if not verify_join(request, self.escrow.run_id, now):
            raise AdmissionError("DUAL_SIGNATURE_RUN_OR_EXPIRY")""",
        "new": """        if False:
            raise AdmissionError("DUAL_SIGNATURE_RUN_OR_EXPIRY")""",
        "tests": [JOIN_FAULT + "[unsigned]"],
        "why": "an application without any valid signature is admitted.",
    },
    {
        "id": "join-signatures-checked-after-state",
        "invariants": ["OR-01"],
        "path": ADMISSION,
        "old": """        if not verify_join(request, self.escrow.run_id, now):
            raise AdmissionError("DUAL_SIGNATURE_RUN_OR_EXPIRY")""",
        "new": """        if self.store.db.execute(
            "SELECT 1 FROM admission_history WHERE hotkey=?", (request.hotkey,)
        ).fetchone():
            raise AdmissionError("HOTKEY_HISTORY_EXISTS")
        if not verify_join(request, self.escrow.run_id, now):
            raise AdmissionError("DUAL_SIGNATURE_RUN_OR_EXPIRY")""",
        "tests": [
            HTTP + "::test_or01_a_forged_join_gets_the_same_refusal_whoever_is_already_admitted"
        ],
        "why": "an unsigned request learns whether a hotkey is already admitted.",
    },
    {
        "id": "join-cold-signature-not-verified",
        "invariants": ["SG-06"],
        "path": ENVELOPE,
        "old": "verify(decode_hotkey(request.coldkey), msg, bytes.fromhex(request.cold_sig))",
        "new": "True",
        "tests": [JOIN_FAULT + "[cold_signature_of_another_key]"],
        "why": "the coldkey's consent to a join is never checked.",
    },
    {
        "id": "join-hot-signature-not-verified",
        "invariants": ["SG-06"],
        "path": ENVELOPE,
        "old": """        return verify(
            decode_hotkey(request.hotkey), msg, bytes.fromhex(request.hot_sig)
        ) and verify(""",
        "new": """        return True and verify(""",
        "tests": [JOIN_FAULT + "[hot_signature_of_another_key]"],
        "why": "the hotkey's consent to a join is never checked.",
    },
    {
        "id": "join-run-id-not-checked",
        "invariants": ["SG-06"],
        "path": ENVELOPE,
        "old": "    if request.run_id != run_id or not 1 <= now_round <= request.expires_beacon:",
        "new": "    if not 1 <= now_round <= request.expires_beacon:",
        "tests": [JOIN_FAULT + "[signed_for_another_run]"],
        "why": "an application signed for another run is admitted to this one.",
    },
    {
        "id": "join-expiry-not-checked",
        "invariants": ["SG-06"],
        "path": ENVELOPE,
        "old": "    if request.run_id != run_id or not 1 <= now_round <= request.expires_beacon:",
        "new": "    if request.run_id != run_id:",
        "tests": [JOIN_FAULT + "[expired_one_round_ago]"],
        "why": "an application past its expires_beacon is admitted.",
    },
    {
        "id": "join-expiry-excludes-its-last-round",
        "invariants": ["SG-06"],
        "path": ENVELOPE,
        "old": "    if request.run_id != run_id or not 1 <= now_round <= request.expires_beacon:",
        "new": "    if request.run_id != run_id or not 1 <= now_round < request.expires_beacon:",
        "tests": [JOIN_FAULT + "[expired_one_round_ago]"],
        "why": "an application is refused on the very round it still is valid.",
    },
    {
        "id": "join-replay-lookup-skipped",
        "invariants": ["SG-06"],
        "path": ADMISSION,
        "old": """            if old:
                self.store.reserve(key, request_hash, old["receipt"])
                return self.store.by_id(old["receipt"])""",
        "new": """            if False:
                self.store.reserve(key, request_hash, old["receipt"])
                return self.store.by_id(old["receipt"])""",
        "tests": [REPLAY, PROPERTY],
        "why": "a retried join is judged as a new application instead of answered from its record.",
    },
    {
        "id": "join-replay-charges-quota",
        "invariants": ["SG-06"],
        "path": ADMISSION,
        "old": """            if old:
                self.store.reserve(key, request_hash, old["receipt"])
                return self.store.by_id(old["receipt"])""",
        "new": """            if old:
                self.store.quota(
                    request.coldkey, now, self.policy.join_per_beacon, self.policy.join_burst
                )
                self.store.reserve(key, request_hash, old["receipt"])
                return self.store.by_id(old["receipt"])""",
        "tests": [REPLAY, PROPERTY],
        "why": "every retry of an accepted join spends the coldkey's join quota.",
    },
    {
        "id": "join-hotkey-history-ignored",
        "invariants": ["AD-05"],
        "path": ADMISSION,
        "old": """                raise AdmissionError("HOTKEY_HISTORY_EXISTS")""",
        "new": """                pass""",
        "tests": [HOTKEY_ONCE, PROPERTY],
        "why": "a hotkey that already has an admission, or had one, is admitted again.",
    },
    {
        "id": "join-coldkey-guard-ignored",
        "invariants": ["AD-05"],
        "path": ADMISSION,
        "old": """                raise AdmissionError("COLDKEY_PROBATION_EXISTS")""",
        "new": """                pass""",
        "tests": [COLDKEY_ONE, PROPERTY],
        "why": "a coldkey holds several non-ACTIVE admissions at once.",
    },
    {
        "id": "join-coldkey-guard-counts-active-rows",
        "invariants": ["AD-05"],
        "path": ADMISSION,
        "old": " AND state!='ACTIVE'",
        "new": "",
        "tests": [COLDKEY_ONE, PROPERTY],
        "why": "a coldkey whose admission graduated can never apply again.",
    },
    {
        "id": "join-pending-capacity-off-by-one",
        "invariants": ["AD-01"],
        "path": ADMISSION,
        "old": "            if count >= self.policy.max_pending:",
        "new": "            if count > self.policy.max_pending:",
        "tests": [PENDING_BOUND],
        "why": "the 33rd pending application is admitted.",
    },
    {
        "id": "rotation-signer-not-bound-to-the-coldkey",
        "invariants": ["SG-08"],
        "path": ADMISSION,
        "old": """{"RotateRequest": lambda s: s == history["coldkey"]}""",
        "new": """{"RotateRequest": lambda s: True}""",
        "tests": [ROTATE_OWNER],
        "why": "any signer can move another participant's admission.",
    },
    {
        "id": "rotation-target-history-ignored",
        "invariants": ["AD-05"],
        "path": ADMISSION,
        "old": """                raise AdmissionError("NEW_HOTKEY_HISTORY")""",
        "new": """                pass""",
        "tests": [HOTKEY_ONCE],
        "why": "an admission rotates onto a hotkey that already belongs to an admission.",
    },
    {
        "id": "recover-names-the-missing-pending-row",
        "invariants": ["OR-01"],
        "path": STORE,
        "old": """                        if row is None or row[0] != auth.commit_hash:
                            raise ChallengeError(
                                403, "recovery reference differs from exact current pending dispute"
                            )""",
        "new": """                        if row is None:
                            raise ChallengeError(403, "no pending dispute for this hotkey")
                        if row[0] != auth.commit_hash:
                            raise ChallengeError(
                                403, "recovery reference differs from exact current pending dispute"
                            )""",
        "tests": [HTTP + "::test_or01_recover_refuses_alike_whatever_the_admission_holds"],
        "why": "recover tells a hotkey whether it has a pending dispute row at all.",
    },
    {
        "id": "reaper-expires-a-proven-trial",
        "invariants": ["GT-02"],
        "path": ADMISSION,
        "old": "AND a.proof_digest IS NULL ",
        "new": "",
        "tests": [POLL_KEEPS_PROOF],
        "why": "the clock expires a trial whose proof was accepted and whose finalization is due.",
    },
    {
        "id": "proof-replay-after-deadline-refused",
        "invariants": ["GT-02"],
        "path": ADMISSION,
        "old": "screen_env.exp_drand) if replay else now",
        "new": "screen_env.exp_drand) if False else now",
        "tests": [POLL_KEEPS_PROOF],
        "why": "a finalization that failed after the deadline can no longer be retried.",
    },
    {
        "id": "reaper-closes-the-deadline-round",
        "invariants": ["GT-03"],
        "path": ADMISSION,
        "old": "$.deadline_beacon')<?",
        "new": "$.deadline_beacon')<=?",
        "tests": [REAPER_BOUNDARY],
        "why": "the clock closes a trial on the round that still accepts its proof.",
    },
    {
        "id": "reaper-suspends-the-holder",
        "invariants": ["GT-03", "LCK-20"],
        "path": ADMISSION,
        "old": "            return epochs",
        "new": """            for epoch in epochs:
                holder = self.store.db.execute(
                    "SELECT admission_id FROM admission_trials WHERE epoch=?", (epoch,)
                ).fetchone()[0]
                self.store.transition(holder, "SUSPENDED", "CHALLENGE_EXPIRED", now)
            return epochs""",
        "tests": [REAPER_BOUNDARY, STRANGER_POLL],
        "why": "an unproven trial past its deadline suspends its holder, who has no route back.",
    },
    {
        "id": "beacon-push-does-not-reap",
        "invariants": ["GT-03", "LCK-21"],
        "path": STORE,
        "old": """                admission.reap_expired_trials(now=self._now(self._db))""",
        "new": """                pass""",
        "tests": [
            HARDENING + "::test_beacon_push_frees_the_places_of_unproven_challenges",
            STRANGER_POLL,
            CLOCK_CLOSES,
        ],
        "why": "two unproven trials hold both trial places until someone asks for their challenge.",
    },
    {
        "id": "challenge-does-not-reap",
        "invariants": ["GT-03", "LCK-21"],
        "path": ADMISSION,
        "old": """            self.reap_expired_trials(now=now)
            if row["challenge"]:""",
        "new": """            if row["challenge"]:""",
        "tests": [NEWCOMER_SERVED],
        "why": "a newcomer's challenge is refused TRIAL_CAPACITY by trials past their deadline.",
    },
    {
        "id": "trial-credited-on-every-finalize",
        "invariants": ["AD-02"],
        "path": ADMISSION,
        "old": """            if trial["finalized_beacon"] is not None:
                return self.status(r.hotkey, now=now)""",
        "new": """            if False:
                return self.status(r.hotkey, now=now)""",
        "tests": [CREDITED_ONCE, REPLAYED_OVER_HTTP],
        "why": "replaying a Finalize at a later beacon credits the same participation again.",
    },
    {
        "id": "finalize-epoch-not-bound",
        "invariants": ["AD-02"],
        "path": ADMISSION,
        "old": """                final.w != row["trial_epoch"]
                or final.included != [r.hotkey]""",
        "new": """                final.included != [r.hotkey]""",
        "tests": [FINALIZE_BOUND + "[another_epoch-changes0]"],
        "why": "a Finalize of another trial epoch credits this participation.",
    },
    {
        "id": "finalize-participant-not-bound",
        "invariants": ["AD-02"],
        "path": ADMISSION,
        "old": """                or final.included != [r.hotkey]""",
        "new": """""",
        "tests": [FINALIZE_BOUND + "[another_participant-changes1]"],
        "why": "a Finalize that includes another hotkey credits this one.",
    },
    {
        "id": "finalize-reference-not-bound",
        "invariants": ["AD-02", "AD-03"],
        "path": ADMISSION,
        "old": """                or final.entitlements_root != digest(expected)""",
        "new": """""",
        "tests": [FINALIZE_BOUND + "[another_reference-changes2]"],
        "why": "a Finalize that does not cite the committed reference credits the participation.",
    },
    {
        "id": "finalize-state-hash-not-bound",
        "invariants": ["AD-02"],
        "path": ADMISSION,
        "old": """                or final.final_theta_hash_w1 != state_hash(theta)""",
        "new": """""",
        "tests": [FINALIZE_BOUND + "[another_final_state-changes3]"],
        "why": "a Finalize with another final state hash credits the participation.",
    },
    {
        "id": "finalize-signer-not-the-coordinator",
        "invariants": ["AD-02"],
        "path": ADMISSION,
        "old": """{"Finalize": lambda s: s == self.coord.ss58}""",
        "new": """{"Finalize": lambda s: True}""",
        "tests": [
            INPROCESS + "::test_ad02_a_finalize_not_signed_by_the_coordinator_credits_nothing"
        ],
        "why": "the miner credits its own participation with a Finalize it signed itself.",
    },
    {
        "id": "proof-accepted-without-a-committed-reference",
        "invariants": ["AD-03"],
        "path": ADMISSION,
        "old": """                raise AdmissionError("REFERENCE_NOT_COMMITTED")""",
        "new": """                pass""",
        "tests": [
            INPROCESS
            + "::test_ad03_a_proof_arriving_before_the_service_reference_is_committed_earns_nothing"
        ],
        "why": "a proof is compared to a reference that was never committed.",
    },
    {
        "id": "proof-older-than-the-reference-accepted",
        "invariants": ["AD-03"],
        "path": ADMISSION,
        "old": """            if now < row["reference_beacon"]:""",
        "new": """            if False:""",
        "tests": [
            INPROCESS + "::test_ad03_a_proof_dated_before_the_reference_commitment_is_refused"
        ],
        "why": "a proof stamped before the reference commitment is compared to it.",
    },
    {
        "id": "proof-matches-whatever-it-says",
        "invariants": ["AD-03"],
        "path": ADMISSION,
        "old": """            if expected != proof:""",
        "new": """            if False:""",
        "tests": [
            INPROCESS
            + "::test_ad03_a_proof_that_differs_from_the_committed_reference_earns_nothing"
        ],
        "why": "a proof that differs from the service reference is accepted as a match.",
    },
    {
        "id": "proof-screen-binding-removed",
        "invariants": ["AD-03"],
        "path": ADMISSION,
        "old": """                raise AdmissionError("WORK_SCREEN_BINDING")""",
        "new": """                pass""",
        "tests": [EXISTING + "::test_proof_rejects_when_not_bound_to_reference[nonce]"],
        "why": "a proof whose screen is bound to another challenge nonce is judged anyway.",
    },
    {
        "id": "proof-after-the-deadline-accepted",
        "invariants": ["AD-03", "GT-02"],
        "path": ADMISSION,
        "old": "or (now > c.deadline_beacon and not replay)",
        "new": "or False",
        "tests": [EXISTING + "::test_proof_rejects_when_not_bound_to_reference[late]"],
        "why": "a new proof after the challenge deadline is accepted, not only the exact replay.",
    },
    {
        "id": "proof-credits-before-finalization",
        "invariants": ["AD-03", "AD-02"],
        "path": ADMISSION,
        "old": """"PROBATION", "FULL_REFERENCE_MATCH", now)""",
        "new": """"PROBATION", "FULL_REFERENCE_MATCH", now)
                self.store.db.execute(
                    "UPDATE admissions_v2 SET clean_count=clean_count+1 WHERE admission_id=?",
                    (r.admission_id,),
                )""",
        "tests": [
            INPROCESS
            + "::test_ad03_an_equal_proof_is_accepted_and_credited_only_by_the_finalization"
        ],
        "why": "accepting a proof credits a clean participation before the coordinator finalizes.",
    },
    {
        "id": "join-pending-capacity-ignores-probation",
        "invariants": ["AD-01", "LCK-21"],
        "path": ADMISSION,
        "old": "\"SELECT COUNT(*) FROM admissions_v2 WHERE state IN ('APPLIED','PROBATION')\"",
        "new": "\"SELECT COUNT(*) FROM admissions_v2 WHERE state IN ('APPLIED')\"",
        "tests": [PENDING_BOUND],
        "why": "an application that passed its trial no longer holds a pending place.",
    },
    {
        "id": "join-coldkey-guard-ignores-probation",
        "invariants": ["AD-05"],
        "path": ADMISSION,
        "old": " AND state!='ACTIVE'",
        "new": " AND state NOT IN ('ACTIVE','PROBATION')",
        "tests": [COLDKEY_ONE],
        "why": "a coldkey on probation applies again with a fresh hotkey.",
    },
    {
        "id": "join-coldkey-guard-ignores-suspended",
        "invariants": ["AD-05"],
        "path": ADMISSION,
        "old": " AND state!='ACTIVE'",
        "new": " AND state NOT IN ('ACTIVE','SUSPENDED')",
        "tests": [COLDKEY_ONE],
        "why": "a coldkey whose admission is suspended applies again with a fresh hotkey.",
    },
    {
        "id": "join-coldkey-guard-ignores-banned",
        "invariants": ["AD-05"],
        "path": ADMISSION,
        "old": " AND state!='ACTIVE'",
        "new": " AND state NOT IN ('ACTIVE','BANNED')",
        "tests": [COLDKEY_ONE],
        "why": "a coldkey whose admission is banned applies again with a fresh hotkey.",
    },
    {
        "id": "join-hotkey-history-ignores-active-admissions",
        "invariants": ["AD-05"],
        "path": ADMISSION,
        "old": """"SELECT 1 FROM admission_history WHERE hotkey=?", (request.hotkey,)""",
        "new": (
            '"SELECT 1 FROM admission_history h JOIN admissions_v2 a '
            "ON a.admission_id=h.admission_id WHERE h.hotkey=? AND a.state!='ACTIVE'\", "
            "(request.hotkey,)"
        ),
        "tests": [PROPERTY],
        "why": "the hotkey of an ACTIVE miner is admitted again under another coldkey.",
    },
    {
        "id": "rotation-signer-any-coldkey-with-an-admission",
        "invariants": ["SG-08"],
        "path": ADMISSION,
        "old": """{"RotateRequest": lambda s: s == history["coldkey"]}""",
        "new": (
            """{"RotateRequest": lambda s: s in {"""
            """r[0] for r in self.store.db.execute("SELECT coldkey FROM admission_history")}}"""
        ),
        "tests": [ROTATE_OWNER],
        "why": "the owner of any admission can move another participant's admission.",
    },
    {
        "id": "join-replay-refreshes-the-seed",
        "invariants": ["SG-06"],
        "path": ADMISSION,
        "old": """                return self.store.by_id(old["receipt"])""",
        "new": """                self.store.db.execute(
                    "UPDATE admissions_v2 SET seed_beacon=? WHERE admission_id=?",
                    (now + 1, old["receipt"]),
                )
                return self.store.by_id(old["receipt"])""",
        "tests": [REPLAY],
        "why": "a retried join at a later beacon moves the seed round of the admission it returns.",
    },
    {
        "id": "reaper-skips-suspended-holders",
        "invariants": ["GT-03", "LCK-21"],
        "path": ADMISSION,
        "old": "AND a.proof_digest IS NULL ",
        "new": "AND a.proof_digest IS NULL AND a.state!='SUSPENDED' ",
        "tests": [CLOCK_CLOSES + "[SUSPENDED]"],
        "why": "a trial held by a suspended identity keeps its place for good.",
    },
    {
        "id": "reaper-skips-banned-holders",
        "invariants": ["GT-03", "LCK-21"],
        "path": ADMISSION,
        "old": "AND a.proof_digest IS NULL ",
        "new": "AND a.proof_digest IS NULL AND a.state!='BANNED' ",
        "tests": [CLOCK_CLOSES + "[BANNED]"],
        "why": "a trial held by a banned identity keeps its place for good.",
    },
    {
        "id": "reaper-suspends-active-holders",
        "invariants": ["GT-03", "LCK-20"],
        "path": ADMISSION,
        "old": "            return epochs",
        "new": """            for epoch in epochs:
                holder = self.store.db.execute(
                    "SELECT admission_id FROM admission_trials WHERE epoch=?", (epoch,)
                ).fetchone()[0]
                if self.store.by_id(holder).state == "ACTIVE":
                    self.store.transition(holder, "SUSPENDED", "CHALLENGE_EXPIRED", now)
            return epochs""",
        "tests": [STRANGER_POLL_ACTIVE],
        "why": "an ACTIVE miner whose trial lapsed is suspended by the clock.",
    },
    {
        "id": "finalize-accepts-an-additional-participant",
        "invariants": ["AD-02"],
        "path": ADMISSION,
        "old": "final.included != [r.hotkey]",
        "new": "r.hotkey not in final.included",
        "tests": [FINALIZE_BOUND + "[an_additional_participant-changes4]"],
        "why": "a Finalize that also includes other hotkeys credits this participation.",
    },
    {
        "id": "proof-replay-credits-again",
        "invariants": ["AD-02"],
        "path": ADMISSION,
        "old": """                    raise AdmissionError("PROOF_REPLAY_CONFLICT")
                return r""",
        "new": """                    raise AdmissionError("PROOF_REPLAY_CONFLICT")
                self.store.db.execute(
                    "UPDATE admissions_v2 SET clean_count=clean_count+1 WHERE admission_id=?",
                    (r.admission_id,),
                )
                return r""",
        "tests": [REPLAYED_OVER_HTTP],
        "why": "re-posting the accepted proof credits a clean participation again.",
    },
    {
        "id": "rotation-consent-not-verified",
        "invariants": ["LCK-17", "SG-08"],
        "path": ADMISSION,
        "old": (
            'not verify_rotate(op, history["coldkey"], self.escrow.run_id, env.exp_drand):\n'
            '                raise AdmissionError("ROTATION_CONSENT")'
        ),
        "new": 'False:\n                raise AdmissionError("ROTATION_CONSENT")',
        "tests": [ROTATE_CONSENT],
        "why": "the coldkey alone rebinds an admission to a registered hotkey that never agreed.",
    },
    {
        "id": "rotation-consent-checked-against-the-old-hotkey",
        "invariants": ["LCK-17", "SG-08"],
        "path": ENVELOPE,
        "old": """            decode_hotkey(request.new_hotkey),
            rotate_signing_message(""",
        "new": """            decode_hotkey(request.hotkey),
            rotate_signing_message(""",
        "tests": [ROTATE_CONSENT],
        "why": "the consent is verified against the hotkey being replaced, not the one it binds.",
    },
    {
        "id": "rotation-of-a-banned-admission-allowed",
        "invariants": ["SG-08"],
        "path": ADMISSION,
        "old": """            if r.state == "BANNED":
                raise AdmissionError("TERMINAL_BANNED")
            self.store.quota(""",
        "new": """            self.store.quota(""",
        "tests": [BANNED_ROTATION],
        "why": "a banned admission moves to a fresh hotkey and keeps operating under it.",
    },
    {
        "id": "rotation-quota-removed",
        "invariants": ["SG-08"],
        "path": ADMISSION,
        "old": """            self.store.quota(
                f"rotate:{r.coldkey}:{now // self.policy.work_screen_epoch_rounds}",
                now,
                0,
                self.policy.join_burst,
                code="ROTATION_QUOTA",
            )""",
        "new": """            pass""",
        "tests": [ROTATION_WINDOW],
        "why": "one coldkey chains rotations without limit inside a window.",
    },
    {
        "id": "rotation-quota-off-by-one",
        "invariants": ["SG-08"],
        "path": ADMISSION,
        "old": """                0,
                self.policy.join_burst,
                code="ROTATION_QUOTA",""",
        "new": """                0,
                self.policy.join_burst + 1,
                code="ROTATION_QUOTA",""",
        "tests": [ROTATION_WINDOW],
        "why": "a coldkey gets one rotation more per window than the policy burst.",
    },
    {
        "id": "join-does-not-retire-dead-applications",
        "invariants": ["AD-01", "LCK-21"],
        "path": ADMISSION,
        "old": """            self.reap_dead_applications(now=now)
""",
        "new": """            pass
""",
        "tests": [DEAD_PLACES],
        "why": "unfunded applications past their challenge window keep their pending places.",
    },
    {
        "id": "dead-applications-retired-while-the-window-is-open",
        "invariants": ["AD-01", "LCK-21"],
        "path": ADMISSION,
        "old": """"WHERE state='APPLIED' AND seed_beacon+?<? ORDER BY admission_id",""",
        "new": """"WHERE state='APPLIED' AND seed_beacon+?<=? ORDER BY admission_id",""",
        "tests": [DEAD_PLACES],
        "why": "an application is retired on the last round it can still ask for its challenge.",
    },
    {
        "id": "dead-applications-retired-even-when-funded",
        "invariants": ["AD-01", "LCK-21"],
        "path": ADMISSION,
        "old": """                if self._unfunded(row["admission_id"], row["coldkey"]):
                    self.store.transition(""",
        "new": """                if True:
                    self.store.transition(""",
        "tests": [FUNDED_KEPT],
        "why": "an application that locked a bond is suspended by the clock while it holds stake.",
    },
    {
        "id": "challenge-route-owner-check-removed",
        "invariants": ["GT-01", "LCK-20"],
        "path": APP,
        "old": """        await owner_request(run_id, admission_id, request, ChallengeRequest)""",
        "new": """        pass""",
        "tests": [OWNER_ONLY],
        "why": "anyone opens a rival's trial and takes one of the run's trial places.",
    },
    {
        "id": "owner-request-signer-not-the-admissions-hotkey",
        "invariants": ["GT-01", "LCK-20"],
        "path": APP,
        "old": """{message.__name__: hotkey.__eq__}""",
        "new": """{message.__name__: lambda signer: True}""",
        "tests": [OWNER_ONLY],
        "why": "a registered rival signing the request as itself opens the owner's trial.",
    },
    {
        "id": "owner-request-not-bound-to-the-admission",
        "invariants": ["GT-01", "LCK-20"],
        "path": APP,
        "old": """asked.admission_id == admission_id""",
        "new": """True""",
        "tests": [OWNER_ONLY],
        "why": "the owner's request for one admission opens the trial of another of its own.",
    },
    {
        "id": "owner-request-accepted-for-the-other-route",
        "invariants": ["GT-01", "LCK-20"],
        "path": APP,
        "old": (
            "            asked = envelope_v2.Intake(run_id, {message.__name__: hotkey.__eq__})"
            ".accept(raw, now)\n"
            "            valid = isinstance(asked, message) and asked.admission_id == admission_id"
        ),
        "new": """            asked = envelope_v2.Intake(
                run_id, {"ChallengeRequest": hotkey.__eq__, "JobRequest": hotkey.__eq__}
            ).accept(raw, now)
            valid = asked.admission_id == admission_id""",
        "tests": [OWNER_ONLY],
        "why": "a captured job request of the owner opens its trial.",
    },
    {
        "id": "owner-is-the-hotkey-the-admission-started-with",
        "invariants": ["GT-01", "LCK-20"],
        "path": APP,
        "old": """        hotkey = (await run(admission.store.by_id, admission_id)).hotkey""",
        "new": """        hotkey = await run(
            lambda: admission.store.db.execute(
                "SELECT hotkey FROM admission_history WHERE admission_id=? ORDER BY rowid",
                (admission_id,),
            ).fetchone()[0]
        )""",
        "tests": [CURRENT_OWNER],
        "why": "the hotkey an admission left still asks for its trial, its current one cannot.",
    },
    {
        "id": "owner-request-expiry-unbounded",
        "invariants": ["GT-01", "LCK-20"],
        "path": APP,
        "old": (
            "            if envelope_v2.parse_envelope(raw).exp_drand"
            " > now + OWNER_REQUEST_MAX_ROUNDS:\n"
            '                raise ExpiredError("expiry too far ahead")'
        ),
        "new": """            pass""",
        "tests": [CAPTURE_BOUND],
        "why": "an owner request signed to expire far ahead stays good for a replay of a capture.",
    },
    {
        "id": "late-challenge-suspends-the-owner",
        "invariants": ["GT-01", "LCK-20"],
        "path": ADMISSION,
        "old": """            if now > seed + self.policy.challenge_rounds:
                raise AdmissionError("CHALLENGE_EXPIRED")""",
        "new": """            if now > seed + self.policy.challenge_rounds:
                self.store.transition(admission_id, "SUSPENDED", "CHALLENGE_EXPIRED", now)
                return {}""",
        "tests": [STRANGER_POLL, STRANGER_POLL_ACTIVE],
        "why": "an owner asking for its challenge after the window is suspended for good.",
    },
]
