"""Mutation table of the `audit` package: one small regression of src/hypertrain per entry."""

from __future__ import annotations

A1 = "tests/ledger_invariants/test_audit.py::"
A2 = "tests/ledger_invariants/test_audit_2.py::"
STORE = "src/hypertrain/challenge/store.py"
ESCROW = "src/hypertrain/ledger/escrow_v2.py"
TAPE = "src/hypertrain/aggregator/tape_v2.py"

NOT_AN_AUDITOR = A1 + "test_a_registered_miner_that_is_not_an_auditor_gets_no_lease"
OWN_JOB = A1 + "test_an_allowlisted_miner_is_never_leased_its_own_job"
TRIED = A1 + "test_an_auditor_that_already_tried_a_job_is_not_leased_it_again"
CAPS = A1 + "test_an_auditor_holds_one_lease_and_the_store_at_most_two"
PINNED = A1 + "test_only_the_pinned_auditor_may_execute_or_complete_a_live_lease"
FORGED = A1 + "test_the_executed_verdict_of_a_forged_commit_is_the_masters_mismatch"
FLIPS = A1 + "test_an_auditor_cannot_sign_a_verdict_other_than_the_masters"
LAPSED = A1 + "test_a_lapsed_lease_cannot_be_executed_or_completed_by_its_old_holder"
TAKEN_BACK = A1 + "test_a_lease_that_was_taken_back_cannot_be_completed_before_it_expires"
ATTEMPTS = A1 + "test_a_lapsed_job_is_retried_once_then_given_one_more_window_then_dropped"
RELEASED = A1 + "test_a_job_leased_again_cannot_be_settled_with_the_first_attempts_verdict"
NONCE = A1 + "test_an_execute_request_must_carry_the_nonce_of_the_live_lease"
BOUNDARY = A1 + "test_a_result_at_the_lease_expiry_round_is_refused_even_before_the_sweep"
FINAL = A1 + "test_a_completed_job_is_final_and_a_repeated_completion_changes_nothing"
COVERAGE = A1 + "test_every_roster_member_has_exactly_one_full_audit_job"
SAMPLED = A1 + "test_a_round_cannot_be_opened_with_a_sampled_audit_probability"

EXCLUSION = A2 + "test_a_mismatch_completes_as_an_exclusion_and_leaves_no_successor_state"
TAPE_GATE = A2 + "test_the_tape_refuses_an_input_whose_replay_is_not_a_full_match_of_its_commit"
UNAUDITED = A2 + "test_a_round_with_an_unaudited_member_cannot_be_aggregated"
SURVIVORS = A2 + "test_a_second_exclusion_leaves_too_few_members_to_aggregate"
NO_RECORD = A2 + "test_finalize_refuses_a_member_without_an_audit_record"
UNRESOLVED = A2 + "test_a_round_cannot_settle_while_an_audit_is_counted_unresolved"
ABSENT = A2 + "test_an_excluded_member_is_absent_from_the_tape_the_included_set_and_the_origins"
FAILED_REAUDIT = (
    "        self._db.execute(\n"
    "            \"UPDATE audit_leases_v2 SET state='exhausted',"
    "result='REAUDIT_FAILED' WHERE id=?\",\n"
    '            (row["id"],),\n'
    "        )\n"
)
STRIKE = (
    "        self._db.execute(\n"
    '            "UPDATE admissions_v2 SET strikes=strikes+1 WHERE hotkey=?", (row["hotkey"],)\n'
    "        )\n"
)
UNVERIFIED_DROP = (
    "        self._drop_audit_job_v2(\n"
    '            row["run_id"], row, "UNVERIFIED", "AUDIT_INFRASTRUCTURE",\n'
    '            sha256_hex(row["id"].encode()),\n'
    "        )\n"
)
NOBODY_SIGNS = (
    A2 + "test_a_mismatch_the_master_replayed_and_no_auditor_signed_costs_its_miner_the_round_only"
)
COUNTER = A2 + "test_the_audit_counter_equals_the_jobs_not_yet_terminal"
DECREMENTS = A2 + "test_each_job_decrements_the_counter_once_whether_it_completes_or_is_exhausted"
SETTLED = A2 + "test_a_settled_round_is_untouched_by_late_audit_events"
HORIZON = A2 + "test_after_the_last_window_nothing_late_can_change_the_dropped_round"
SCHEDULE_HORIZON = A2 + "test_audits_cannot_be_scheduled_once_the_horizon_is_one_round_away"
ROLLBACK = A2 + "test_a_rollback_waits_for_every_audit_of_both_rounds"
ALLOCATION = A2 + "test_the_allocation_depends_only_on_the_roster_never_on_what_a_miner_signed"
ENTITLEMENT = A2 + "test_a_commit_cannot_declare_its_own_entitlement"
ESCROW_FAULTS = A2 + "test_a_verdict_that_is_not_an_authorised_match_of_the_commit_mints_nothing"
SELF_SIGNED = (
    A2 + "test_a_verdict_signed_by_the_miner_it_judges_mints_nothing_even_if_it_is_allowlisted"
)
OUTSIDE_THE_SET = (
    A2 + "test_a_verdict_of_a_key_outside_the_auditor_set_or_an_expired_one_mints_nothing"
)
OTHER_CHALLENGE = A2 + "test_a_verdict_that_answers_another_challenge_mints_nothing"
NO_OWNER = A2 + "test_a_completion_is_refused_for_a_lease_that_has_no_owner"
SINGLE_COMMIT = A2 + "test_a_commit_is_single_assignment_per_member_and_round"
DELTA_BINDING = A2 + "test_a_delta_manifest_must_carry_the_hash_and_size_of_the_commit"
DISPATCH = (
    "tests/challenge/test_role_dispatch_consumers_v2.py::"
    "test_dispatch_replay_env_is_actual_remote_hardware"
)
PRODUCTION_CPU = (
    "tests/challenge/test_production_economics_v2.py::test_bootstrap_route_refuses_cpu_backend"
)
COMPARATOR = A2 + "test_the_audit_decision_is_bound_to_every_committed_hash"
ANOTHER_AUDITOR = A1 + "test_a_verdict_signed_by_another_allowlisted_auditor_is_refused"
PAYS = A2 + "test_a_finalized_round_pays_the_matched_members_and_not_the_excluded_one"
HORIZON_EXHAUSTS = HORIZON
UNSCHEDULED = (
    A2 + "test_without_scheduled_audits_the_counter_stays_at_the_roster_and_the_round_waits"
)
DECLARED_TOKENS = A2 + "test_a_commit_that_declares_more_tokens_than_its_assignment_mints_nothing"

MUTANTS: list[dict[str, object]] = [
    {
        "id": "aud-lease-allowlist-dropped",
        "invariants": ["A2W-02", "AU-04"],
        "path": STORE,
        "old": "if env.signer not in manifest.training.auditors:\n"
        '                raise ChallengeError(403, "auditor not allowlisted")',
        "new": 'if False:\n                raise ChallengeError(403, "auditor not allowlisted")',
        "tests": [NOT_AN_AUDITOR, PINNED],
        "why": "any registered key, a miner included, is leased audit work.",
    },
    {
        "id": "aud-lease-own-job-served-to-its-miner",
        "invariants": ["A2W-02", "A2W-03"],
        "path": STORE,
        "old": 'or env.signer == candidate["hotkey"]\n'
        "                ):\n                    continue",
        "new": "or False\n                ):\n                    continue",
        "tests": [OWN_JOB],
        "why": "an allowlisted miner is leased its own job and judges itself.",
    },
    {
        "id": "aud-lease-tried-auditor-served-again",
        "invariants": ["A2W-02"],
        "path": STORE,
        "old": 'env.signer in json.loads(candidate["tried"])\n'
        "                    or env.signer == ",
        "new": "False\n                    or env.signer == ",
        "tests": [TRIED],
        "why": "an auditor that already tried a job is offered it again.",
    },
    {
        "id": "aud-lease-store-cap-of-three",
        "invariants": ["A2W-02"],
        "path": STORE,
        "old": ").fetchone()[0]\n                >= 2\n            ):\n                return None",
        "new": ").fetchone()[0]\n                >= 3\n            ):\n                return None",
        "tests": [CAPS],
        "why": "three replays may run on the master at once.",
    },
    {
        "id": "aud-lease-one-per-auditor-dropped",
        "invariants": ["A2W-02"],
        "path": STORE,
        "old": 'raise ChallengeError(409, "one live lease per auditor")',
        "new": "pass",
        "tests": [CAPS],
        "why": "an auditor holds several leases at once.",
    },
    {
        "id": "aud-lease-silent-auditor-not-cooled-down",
        "invariants": ["A2W-05"],
        "path": STORE,
        "old": 'raise ChallengeError(403, f"auditor cooldown until round {until}")',
        "new": "pass",
        "tests": [LAPSED],
        "why": "an auditor whose lease lapsed asks for work again as if nothing happened.",
    },
    {
        "id": "aud-lease-third-attempt",
        "invariants": ["A2W-02", "A2W-12"],
        "path": STORE,
        "old": 'if row["attempts"] >= 2\n            or now >= row["absolute"]',
        "new": 'if row["attempts"] >= 3\n            or now >= row["absolute"]',
        "tests": [ATTEMPTS],
        "why": "a job whose second attempt lapses is queued for a third one.",
    },
    {
        "id": "aud-execute-owner-check-dropped",
        "invariants": ["A2W-05", "AU-04"],
        "path": STORE,
        "old": 'if row is None or row["state"] != "running" or row["auditor"] != env.signer:',
        "new": 'if row is None or row["state"] != "running":',
        "tests": [PINNED],
        "why": "the first request of a stranger is not stopped by the lease owner check.",
    },
    {
        "id": "aud-execute-state-check-dropped",
        "invariants": ["A2W-05", "A2W-06"],
        "path": STORE,
        "old": 'if row is None or row["state"] != "running" or row["auditor"] != env.signer:',
        "new": 'if row is None or row["auditor"] != env.signer:',
        "tests": [LAPSED, FINAL],
        "why": "the master replays again for a lapsed or finished lease.",
    },
    {
        "id": "aud-execute-nonce-check-dropped",
        "invariants": ["A2W-05"],
        "path": STORE,
        "old": 'if request.commit_hash != row["nonce"]:\n'
        '                raise ChallengeError(403, "audit lease nonce differs")',
        "new": 'if False:\n                raise ChallengeError(403, "audit lease nonce differs")',
        "tests": [NONCE],
        "why": "an execute request names no lease in particular and is served anyway.",
    },
    {
        "id": "aud-execute-stores-a-pass",
        "invariants": ["A2W-04"],
        "path": STORE,
        "old": "result=outcome.result,",
        "new": 'result="MATCH",',
        "tests": [FORGED],
        "why": "the verdict the master stores no longer is the result of its own replay.",
    },
    {
        "id": "aud-complete-verdict-equality-dropped",
        "invariants": ["A2W-04"],
        "path": STORE,
        "old": 'if verdict.model_dump(mode="json") != expected:\n'
        "                raise ChallengeError(422, ",
        "new": "if False:\n                raise ChallengeError(422, ",
        "tests": [FLIPS],
        "why": "the auditor signs a pass for a forged commit and it is recorded.",
    },
    {
        "id": "aud-complete-state-check-dropped",
        "invariants": ["A2W-05", "A2W-13"],
        "path": STORE,
        "old": 'if row["state"] != "running" or now >= row["expires"] or now >= row["absolute"]:',
        "new": 'if now >= row["expires"] or now >= row["absolute"]:',
        "tests": [TAKEN_BACK],
        "why": "a holder whose lease was taken back completes the job anyway.",
    },
    {
        "id": "aud-complete-expiry-round-accepted",
        "invariants": ["A2W-05"],
        "path": STORE,
        "old": 'if row["state"] != "running" or now >= row["expires"] or now >= row["absolute"]:',
        "new": 'if row["state"] != "running" or now > row["expires"] or now >= row["absolute"]:',
        "tests": [BOUNDARY],
        "why": "a result is accepted in the very round the lease expires.",
    },
    {
        "id": "aud-complete-completed-branch-dropped",
        "invariants": ["A2W-06"],
        "path": STORE,
        "old": 'if row["state"] == "completed":\n                if row["result"] != ',
        "new": 'if False:\n                if row["result"] != ',
        "tests": [FINAL],
        "why": "a repeated completion is no longer answered with the stored result.",
    },
    {
        "id": "aud-complete-counter-not-decremented",
        "invariants": ["A2W-12", "A2W-06"],
        "path": STORE,
        "old": '(finality["unresolved_audits"])) - 1)\n            if verdict.result != "MATCH":',
        "new": '(finality["unresolved_audits"])))\n            if verdict.result != "MATCH":',
        "tests": [COUNTER, DECREMENTS],
        "why": "a completed audit stays counted as open and the round never settles.",
    },
    {
        "id": "aud-sweep-counter-not-decremented",
        "invariants": ["A2W-12"],
        "path": STORE,
        "old": '(finality["unresolved_audits"])) - 1)\n        self._put_record_v2(',
        "new": '(finality["unresolved_audits"])))\n        self._put_record_v2(',
        "tests": [DECREMENTS],
        "why": "an exhausted audit stays counted as open.",
    },
    {
        "id": "aud-complete-mismatch-not-excluded",
        "invariants": ["A2W-01", "A2W-11"],
        "path": STORE,
        "old": 'if verdict.result != "MATCH":\n                self._put_record_v2(',
        "new": "if False:\n                self._put_record_v2(",
        "tests": [EXCLUSION],
        "why": "a mismatch is recorded as a replay but the miner stays in the working set.",
    },
    {
        "id": "aud-sweep-rescans-finished-jobs",
        "invariants": ["A2W-06", "A2W-13", "A2W-15"],
        "path": STORE,
        "old": "AND state IN ('running','queued')\",\n"
        "            (run_id,),\n"
        "        ).fetchall()\n"
        "        for row in rows:\n"
        '            if row["state"] == "running" and now >= self._lease_end_v2(run_id, row):\n'
        "                self._retire_audit_attempt_v2(run_id, row, now, "
        "self._expiry_cause_v2(run_id, row))\n"
        '            elif row["state"] == "queued" and now >= row["absolute"]:',
        "new": "AND state IN ('running','queued','completed')\",\n"
        "            (run_id,),\n"
        "        ).fetchall()\n"
        "        for row in rows:\n"
        '            if row["state"] == "running" and now >= self._lease_end_v2(run_id, row):\n'
        "                self._retire_audit_attempt_v2(run_id, row, now, "
        "self._expiry_cause_v2(run_id, row))\n"
        '            elif row["state"] != "running" and now >= row["absolute"]:',
        "tests": [FINAL, SETTLED],
        "why": "the expiry sweep exhausts and excludes members of rounds that were audited.",
    },
    {
        "id": "aud-schedule-horizon-ignored",
        "invariants": ["A2W-13"],
        "path": STORE,
        "old": "if absolute <= now + 1:\n                raise ChallengeError(503, ",
        "new": "if False:\n                raise ChallengeError(503, ",
        "tests": [SCHEDULE_HORIZON],
        "why": "jobs are created after the point where they could still be replayed.",
    },
    {
        "id": "aud-schedule-skips-the-last-member",
        "invariants": ["A2W-09"],
        "path": STORE,
        "old": "for entry, identity, job_id, _commit in missing:",
        "new": "for entry, identity, job_id, _commit in missing[:-1]:",
        "tests": [COVERAGE],
        "why": "one roster member is never given an audit job.",
    },
    {
        "id": "aud-open-accepts-a-sampled-roster",
        "invariants": ["A2W-09"],
        "path": STORE,
        "old": ") != (r.admission_id, r.coldkey, r.state, f32hex(1.0)):",
        "new": ") != (r.admission_id, r.coldkey, r.state, entry.q_i):",
        "tests": [SAMPLED],
        "why": "a roster that audits its members with probability below one is opened.",
    },
    {
        "id": "aud-aggregate-skips-the-audit-gate",
        "invariants": ["A2W-10"],
        "path": STORE,
        "old": "inputs = self.verified_inputs_v2(run_id, w, now)\n"
        "        self._require_survivors_v2(",
        "new": "inputs = []\n        self._require_survivors_v2(",
        "tests": [UNAUDITED],
        "why": "aggregation no longer requires every member to have an audit outcome.",
    },
    {
        "id": "aud-finalize-trusts-the-requested-included-set",
        "invariants": ["A2W-10", "A2W-11"],
        "path": STORE,
        "old": "            included = self._unconvicted_v2(\n"
        '                item["roster"]["hotkey"]\n'
        '                for item in self._record_v2(run_id, "tape-inputs", str(w))["inputs"]\n'
        "            )\n",
        "new": "            included = sorted(model.included)\n",
        "tests": [ABSENT],
        "why": "the coordinator's included set replaces the audited one.",
    },
    {
        "id": "aud-finalize-pays-around-a-missing-verdict",
        "invariants": ["A2W-10"],
        "path": STORE,
        "old": "            works = []\n            for hotkey in included:",
        "new": "            works = []\n"
        "            included = [\n"
        "                h\n"
        "                for h in included\n"
        "                if self._db.execute(\n"
        '                    "SELECT 1 FROM records_v2 "\n'
        "                    \"WHERE run_id=? AND kind='verdict' AND id=?\",\n"
        '                    (run_id, f"{w}:{h}"),\n'
        "                ).fetchone()\n"
        "            ]\n"
        "            for hotkey in included:",
        "tests": [NO_RECORD],
        "why": "a member whose audit verdict is missing is left out and the others are paid, "
        "instead of the finalize being refused.",
    },
    {
        "id": "aud-verified-inputs-keeps-excluded-members",
        "invariants": ["A2W-11"],
        "path": STORE,
        "old": "            if excluded:\n                continue",
        "new": "            if False:\n                continue",
        "tests": [EXCLUSION],
        "why": "an excluded member stays in the working set of the round.",
    },
    {
        "id": "aud-settle-ignores-open-audits",
        "invariants": ["A2W-10", "A2W-13"],
        "path": "src/hypertrain/challenge/finality_v2.py",
        "old": "        or status.unresolved_audits\n        or status.unresolved_disputes\n",
        "new": "        or status.unresolved_disputes\n",
        "tests": [UNRESOLVED],
        "why": "a round with an open audit settles and mints.",
    },
    {
        "id": "aud-rollback-ignores-open-audits",
        "invariants": ["A2W-13"],
        "path": STORE,
        "old": "if not status.applied or status.unresolved_audits or status.unresolved_disputes:",
        "new": "if not status.applied or status.unresolved_disputes:",
        "tests": [ROLLBACK],
        "why": "a rollback starts while an audit of either round is open.",
    },
    {
        "id": "aud-rollback-rewrites-a-settled-round",
        "invariants": ["A2W-13"],
        "path": STORE,
        "old": 'if status.disposition == "SETTLED":\n                    raise Chall',
        "new": "if False:\n                    raise Chall",
        "tests": [SETTLED],
        "why": "late fraud rewrites the history of a finalized round.",
    },
    {
        "id": "aud-tape-accepts-any-replay-result",
        "invariants": ["A2W-01"],
        "path": TAPE,
        "old": '        r.result != "MATCH"\n        or r.audit_mode != "anchored-full"',
        "new": '        r.audit_mode != "anchored-full"',
        "tests": [TAPE_GATE],
        "why": "a replay that is not a match enters the tape.",
    },
    {
        "id": "aud-tape-leaves-root-compared-to-itself",
        "invariants": ["A2W-01", "A2W-07"],
        "path": TAPE,
        "old": "            r.assignment_hash,\n            r.leaves_root,",
        "new": "            r.assignment_hash,\n            c.leaves_root,",
        "tests": [TAPE_GATE],
        "why": "the audited leaves root is no longer compared to the committed one.",
    },
    {
        "id": "aud-escrow-credits-a-mismatch",
        "invariants": ["A2W-01"],
        "path": ESCROW,
        "old": '                    x.result != "MATCH"\n                    or x.first_bad',
        "new": "                    x.first_bad",
        "tests": [ESCROW_FAULTS],
        "why": "the escrow mints for a verdict whose result is not MATCH.",
    },
    {
        "id": "aud-escrow-credits-another-leaves-root",
        "invariants": ["A2W-01", "A2W-07"],
        "path": ESCROW,
        "old": "                    or x.recomputed_leaves_root != c.leaves_root\n",
        "new": "",
        "tests": [ESCROW_FAULTS],
        "why": "the escrow mints for a verdict about other leaves than the committed ones.",
    },
    {
        "id": "aud-escrow-credits-another-image",
        "invariants": ["A2W-01", "A2W-08"],
        "path": ESCROW,
        "old": "                    or x.replay_env.image_digest\n"
        "                    != self.manifest.training.reference_spec.image_digest\n",
        "new": "",
        "tests": [ESCROW_FAULTS],
        "why": "the escrow mints for a replay run in an image other than the pinned one.",
    },
    {
        "id": "aud-escrow-credits-a-self-signed-verdict",
        "invariants": ["A2W-03"],
        "path": ESCROW,
        "old": "                or any(e.signer == c.hotkey for e in envs)\n",
        "new": "",
        "tests": [SELF_SIGNED],
        "why": "a miner that is in the auditor set is credited on its own verdict.",
    },
    {
        "id": "aud-escrow-credits-another-challenge",
        "invariants": ["A2W-01"],
        "path": ESCROW,
        "old": "!= digest(challenge) for x in verdict_list)\n            ):",
        "new": "!= digest(challenge) and False for x in verdict_list)\n            ):",
        "tests": [OTHER_CHALLENGE],
        "why": "the escrow mints for a verdict that answers some other challenge.",
    },
    {
        "id": "aud-allocation-ordered-by-miner-label",
        "invariants": ["A2W-14"],
        "path": "src/hypertrain/aggregator/weighted_v2.py",
        "old": 'ordered = sorted(candidates, key=lambda c: c.roster.hotkey.encode("utf-8"))',
        "new": "ordered = sorted(candidates, key=lambda c: c.commit_hash)",
        "tests": [ALLOCATION],
        "why": "which member gets the odd unit of the quantum depends on a hash the miner signed.",
    },
    {
        "id": "aud-commit-declares-its-tokens",
        "invariants": ["A2W-14"],
        "path": "src/hypertrain/protocol/messages_v2.py",
        "old": "if self.tokens != sample_count * manifest.training.model.seq_len:",
        "new": "if False:",
        "tests": [ENTITLEMENT],
        "why": "a miner chooses the token count its reward is split by.",
    },
    {
        "id": "aud-audit-ignores-the-delta-hash",
        "invariants": ["A2W-07"],
        "path": "src/hypertrain/auditor/worker.py",
        "old": '        and summary["delta_hash"] == commit.delta_hash\n',
        "new": "",
        "tests": [COMPARATOR],
        "why": "a MATCH no longer certifies the delta that will be aggregated.",
    },
    {
        "id": "aud-audit-ignores-the-error-feedback-input",
        "invariants": ["A2W-07"],
        "path": "src/hypertrain/auditor/worker.py",
        "old": '        and summary["ef_out_hash"] == commit.ef_out_hash\n'
        "        and state_hash(ef) == commit.ef_in_hash\n",
        "new": '        and summary["ef_out_hash"] == commit.ef_out_hash\n',
        "tests": [COMPARATOR],
        "why": "a MATCH no longer certifies the error feedback the miner started from.",
    },
    {
        "id": "aud-audit-ignores-a-bad-leaf",
        "invariants": ["A2W-07"],
        "path": "src/hypertrain/auditor/worker.py",
        "old": '        first is None\n        and summary["leaves_root"] == commit.leaves_root',
        "new": '        summary["leaves_root"] == commit.leaves_root',
        "tests": [COMPARATOR],
        "why": "a leaf that differs from the committed preimage does not fail the audit.",
    },
    {
        "id": "aud-signed-operation-conflict-ignored",
        "invariants": ["A2W-07", "A2W-06"],
        "path": STORE,
        "old": "if known is not None and known[0] != digest:",
        "new": "if False:",
        "tests": [SINGLE_COMMIT, FINAL],
        "why": "a signer may put a second body under the identity of an operation it signed.",
    },
    {
        "id": "aud-delta-manifest-binding-dropped",
        "invariants": ["A2W-07"],
        "path": STORE,
        "old": 'raise ChallengeError(422, "delta/master acceptance differs from commit")',
        "new": "pass",
        "tests": [DELTA_BINDING],
        "why": "a delta manifest is accepted for a hash and size the commit does not name.",
    },
    {
        "id": "aud-contest-accepts-an-ownerless-lease",
        "invariants": ["AU-04", "A2W-23"],
        "path": STORE,
        "old": 'auditor=row["auditor"],\n                referee=referee,',
        "new": 'auditor=row["auditor"] or referee,\n                referee=referee,',
        "tests": [NO_OWNER],
        "why": "the one check that refuses a completion for an ownerless lease is relaxed.",
    },
    {
        "id": "aud-escrow-credits-another-driver",
        "invariants": ["A2W-01", "A2W-08"],
        "path": ESCROW,
        "old": "                    or x.replay_env.driver\n"
        "                    not in self.manifest.training.reference_spec.driver_allowlist\n",
        "new": "",
        "tests": [ESCROW_FAULTS],
        "why": "the escrow mints for a replay run on a driver the run does not allow.",
    },
    {
        "id": "aud-dispatch-image-digest-ignored",
        "invariants": ["A2W-08"],
        "path": STORE,
        "old": '            or env.get("image_digest") != ref.image_digest\n',
        "new": "",
        "tests": [DISPATCH],
        "why": "the hardware record of a dispatch is accepted for an image other than the pinned.",
    },
    {
        "id": "aud-production-run-replays-on-cpu",
        "invariants": ["A2W-08"],
        "path": STORE,
        "old": 'if econ.ledger_mode != "test":\n                    raise ChallengeError(409, "p',
        "new": 'if False:\n                    raise ChallengeError(409, "p',
        "tests": [PRODUCTION_CPU],
        "why": "a production run without a CUDA qualification is replayed on the CPU placeholder.",
    },
    {
        "id": "aud-escrow-credits-any-verdict-signer",
        "invariants": ["A2W-01", "A2W-03"],
        "path": ESCROW,
        "old": '{"ReplayVerdict": lambda s: s in self.auditors}',
        "new": '{"ReplayVerdict": lambda s: True}',
        "tests": [OUTSIDE_THE_SET],
        "why": "the escrow credits a verdict signed by a key that is not an auditor.",
    },
    {
        "id": "aud-complete-accepts-another-allowlisted-auditor",
        "invariants": ["A2W-04", "A2W-05"],
        "path": STORE,
        "old": 'verdict = self._authenticated_v2(run_id, raw, "ReplayVerdict", row["auditor"])',
        "new": "sender = envelope_v2.parse_envelope(raw).signer\n"
        "            verdict = self._authenticated_v2(\n"
        "                run_id,\n"
        "                raw,\n"
        '                "ReplayVerdict",\n'
        '                sender if sender in manifest.training.auditors else row["auditor"],\n'
        "            )",
        "tests": [ANOTHER_AUDITOR],
        "why": "any allowlisted auditor, not only the holder, completes a lease.",
    },
    {
        "id": "aud-sweep-decrements-twice",
        "invariants": ["A2W-12"],
        "path": STORE,
        "old": '(finality["unresolved_audits"])) - 1)\n        self._put_record_v2(',
        "new": '(finality["unresolved_audits"])) - 2)\n        self._put_record_v2(',
        "tests": [DECREMENTS],
        "why": "an exhausted job takes two units off the open-audit counter, which the clamp at "
        "zero hides when every job is exhausted at once.",
    },
    {
        "id": "aud-tape-accepts-a-sampled-replay",
        "invariants": ["A2W-01", "A2W-09"],
        "path": TAPE,
        "old": '        r.result != "MATCH"\n        or r.audit_mode != "anchored-full"\n'
        "        or (",
        "new": '        r.result != "MATCH"\n        or (',
        "tests": [TAPE_GATE],
        "why": "a replay that audited a sample of the leaves enters the tape as a full audit.",
    },
    {
        "id": "aud-escrow-credits-declared-tokens",
        "invariants": ["A2W-14"],
        "path": ESCROW,
        "old": "            c.validate_assignment(self.manifest, len(item.sample_ids))\n",
        "new": "",
        "tests": [DECLARED_TOKENS],
        "why": "the reward path credits the token count a commit declares, not the server's.",
    },
    {
        "id": "aud-reward-owner-is-the-miner-hotkey",
        "invariants": ["A2W-11", "A2W-14"],
        "path": ESCROW,
        "old": "owner=work[h][0],",
        "new": "owner=h,",
        "tests": [PAYS],
        "why": "the reward origins of the matched members are owned by their hotkeys, not by "
        "the coldkeys that funded them.",
    },
    {
        "id": "aud-horizon-leaves-queued-jobs",
        "invariants": ["A2W-12", "A2W-13"],
        "path": STORE,
        "old": 'elif row["state"] == "queued" and now >= row["absolute"]:',
        "new": "elif False:",
        "tests": [HORIZON_EXHAUSTS],
        "why": "a job that never found an auditor is still queued after the audit horizon.",
    },
    {
        "id": "aud-round-opens-with-no-open-audit",
        "invariants": ["A2W-12"],
        "path": STORE,
        "old": '"unresolved_audits": len(body.roster),',
        "new": '"unresolved_audits": 0,',
        "tests": [UNSCHEDULED],
        "why": "a round opens with no audit counted as open, so the settle guard on open audits "
        "has nothing to refuse.",
    },
    {
        "id": "aud-rearm-without-limit",
        "invariants": ["A2W-12", "A2W-13"],
        "path": STORE,
        "old": "if windows >= policy.max_attempts:",
        "new": "if False:",
        "tests": [ATTEMPTS, HORIZON],
        "why": "a job nobody audits is given a new window for ever, so its audit stays counted as "
        "open and the round never resolves.",
    },
    {
        "id": "aud-dropped-job-voids-the-round",
        "invariants": ["A2W-13"],
        "path": STORE,
        "old": '(finality["unresolved_audits"])) - 1)\n'
        '        self._put_record_v2(run_id, "finality", str(row["w"]), finality)',
        "new": '(finality["unresolved_audits"])) - 1)\n'
        '        finality["disposition"] = "EXCLUDED"\n'
        '        self._put_record_v2(run_id, "finality", str(row["w"]), finality)',
        "tests": [ATTEMPTS, HORIZON],
        "why": "the infrastructure failure of one audit voids the round of every miner.",
    },
    {
        "id": "aud-aggregate-skips-the-survivor-check",
        "invariants": ["A2W-11"],
        "path": STORE,
        "old": "        self._require_survivors_v2(manifest, [x.roster for x in inputs])\n",
        "new": "",
        "tests": [SURVIVORS],
        "why": "a round whose remaining members cannot carry it reaches the allocator instead of "
        "being refused by name.",
    },
    {
        "id": "aud-lease-reuses-the-nonce-of-the-first-attempt",
        "invariants": ["A2W-05"],
        "path": STORE,
        "old": "nonce = secrets.token_hex(32)",
        "new": 'nonce = row["nonce"] or secrets.token_hex(32)',
        "tests": [RELEASED],
        "why": "the second attempt of a job runs under the nonce of the first, so the stale "
        "geometry and verdict of the first attempt settle it.",
    },
    {
        "id": "aud-lease-keeps-the-first-holder",
        "invariants": ["A2W-05"],
        "path": STORE,
        "old": '"auditor=?,nonce=?,expires=?,tried=? WHERE id=?",',
        "new": '"auditor=COALESCE(auditor,?),nonce=?,expires=?,tried=? WHERE id=?",',
        "tests": [RELEASED],
        "why": "a job leased again still names its first holder as the owner of the lease.",
    },
    {
        "id": "aud-an-unsigned-mismatch-is-excluded-as-unverified",
        "invariants": ["A2W-23"],
        "path": STORE,
        "old": FAILED_REAUDIT,
        "new": UNVERIFIED_DROP,
        "tests": [NOBODY_SIGNS],
        "why": "a mismatch the master replayed and no auditor signed is held against its miner "
        "as the fraud exclusion.",
    },
    {
        "id": "aud-an-unsigned-mismatch-strikes-its-miner",
        "invariants": ["A2W-23", "NSH-04"],
        "path": STORE,
        "old": FAILED_REAUDIT,
        "new": FAILED_REAUDIT + STRIKE,
        "tests": [NOBODY_SIGNS],
        "why": "a job nobody signed ends in a strike for its miner, as if the silence were the "
        "miner's.",
    },
]
