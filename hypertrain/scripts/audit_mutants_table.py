"""The mutants of scripts/check_audit_mutants.py: (name, path, old, new, tests) rows.

`path` is relative to hypertrain/, `old` must occur exactly once in it, and `tests` are pytest node
ids relative to hypertrain/ that must fail once `old` is replaced by `new`. This first table covers
the comparison of the production audit (auditor/worker.py:196-210), the one check that catches
every cheat accepted at intake; the re-audit of a withheld signature (ADR 0011 Part 9) follows it.
The intake, sanction and ledger defences were mutation-proved by hand in private copies (see ADR
0009) and are the next rows to add.
"""

from __future__ import annotations

WORKER = "src/hypertrain/auditor/worker.py"
STORE = "src/hypertrain/challenge/store.py"
MESSAGES = "src/hypertrain/protocol/messages_v2.py"
COOLDOWN = "src/hypertrain/challenge/audit_cooldown_v2.py"
EXIT = "src/hypertrain/challenge/exit_v2.py"
LIVE = "scripts/ops/live_operator.py"
CHEATS = "tests/audit_cheaters/test_cheats_audit.py::test_cheat_is_mismatch_at_the_expected_leaf"
WITHHELD = "tests/challenge/test_audit_withheld_signature_v2.py::"
LAST_WINDOW = (
    WITHHELD
    + "test_a_mismatch_withheld_in_the_last_window_still_gets_a_lease_for_the_other_auditor"
)
NOBODY_LEFT = WITHHELD + "test_a_reaudit_window_fails_the_job_as_soon_as_no_auditor_is_left_to_try"
BOTH_WITHHOLD = (
    WITHHELD + "test_when_both_auditors_withhold_the_miner_is_dropped_once_and_never_sanctioned"
)
MATCH_HOLDER = (
    WITHHELD + "test_a_match_the_first_holder_withheld_does_not_keep_it_from_the_reaudit_of_the_job"
)
OTHER_JOB = (
    WITHHELD + "test_a_withhold_on_another_job_does_not_make_its_auditor_a_withholder_of_this_one"
)
MINER_AUDITOR = (
    WITHHELD + "test_a_job_whose_only_other_auditor_is_its_miner_is_re_armed_when_the_first_lapses"
)
NTH_BARS = WITHHELD + "test_the_nth_withheld_mismatch_bars_the_auditor_and_the_one_before_does_not"
ONLY_MISMATCH = WITHHELD + "test_only_a_withheld_mismatch_counts_toward_the_bar"
BAR_LENGTHS = (
    "tests/challenge/test_audit_cooldown_v2.py::"
    "test_the_third_withheld_mismatch_bars_for_two_absolute_windows"
)
REVOKING = "tests/challenge/test_audit_revocation_v2.py::"
REVOKED = (
    REVOKING
    + "test_an_unsigned_verdict_is_revoked_when_the_sign_window_ends_and_goes_to_the_other_auditor"
)
FROM_STORED = (
    REVOKING
    + "test_the_sign_window_counts_from_the_stored_verdict_and_never_revokes_a_replay_in_progress"
)
COOLS_FROM = REVOKING + "test_a_revocation_cools_the_auditor_down_from_the_revocation_round"
LATE_POLL = (
    REVOKING + "test_a_poll_that_notices_the_revocation_late_does_not_push_the_cooldown_further"
)
ADMITTED = (
    REVOKING
    + "test_a_service_admitted_run_revokes_an_unsigned_verdict_and_drops_the_job_without_a_sanction"
)
NOT_AGAIN = (
    REVOKING
    + "test_the_holder_of_a_revoked_lease_is_not_handed_the_job_again_when_its_cooldown_ends"
)
REAL_SIGNATURE = (
    REVOKING + "test_a_signature_inside_the_sign_window_is_untouched_and_a_later_one_is_refused"
)
SIGN_DEADLINE = (
    "tests/challenge/test_audit_cooldown_v2.py::"
    "test_a_stored_verdict_is_due_twenty_rounds_after_it_was_stored_and_a_replay_never_is"
)
RETIRE_EXPIRED = (
    "                self._retire_audit_attempt_v2("
    "run_id, row, now, self._expiry_cause_v2(run_id, row))\n"
)
FORGET_TRIED = (
    "                self._db.execute(\n"
    "                    \"UPDATE audit_leases_v2 SET tried='[]' WHERE id=?\", (row['id'],)\n"
    "                )\n"
)
LAST_FREE = (
    "tests/challenge/test_audit_cooldown_v2.py::"
    "test_the_last_free_auditor_is_not_barred_whatever_it_withheld"
)
POLICY = "tests/protocol/test_network_v2.py::"
OMITTED = POLICY + "test_audit_policy_without_the_reaudit_window_keeps_its_digest_and_run_id"
WINDOW_RANGE = (
    POLICY + "test_the_reaudit_window_defaults_to_one_lease_and_stays_inside_the_absolute_one"
)
WINDOW_FORMULA = (
    POLICY + "test_the_published_reaudit_window_is_twice_the_measured_replay_in_beacon_rounds"
)
REAUDIT = "tests/challenge/test_audit_reaudit_window_v2.py::"
WINDOW_AT_DETECTION = (
    REAUDIT + "test_the_window_starts_at_the_detection_not_at_the_lease_and_ends_with_the_commit"
)
SECOND_FAILS = (
    REAUDIT + "test_the_second_auditor_withholding_too_drops_its_member_and_blames_nobody"
)
UNUSED_WINDOW = REAUDIT + "test_a_window_nobody_uses_ends_in_a_drop_at_its_bound_and_not_before"
COMMIT_ENDS = (
    REAUDIT + "test_a_commit_that_ends_before_the_window_can_open_drops_the_member_at_the_detection"
)
LATE_OBSERVATION = REAUDIT + "test_a_late_observation_leaves_the_window_where_the_detection_put_it"
NOTICED_AFTER = (
    REAUDIT + "test_a_detection_noticed_after_its_window_is_over_fails_the_job_without_leasing_it"
)
MINER_IS_AUDITOR = (
    REAUDIT + "test_a_miner_that_is_the_only_other_auditor_cannot_be_re_audited_and_is_dropped"
)
SERVICE_RUN = (
    REAUDIT + "test_a_service_admitted_run_opens_no_window_and_keeps_the_infrastructure_drop"
)
LIVE_TESTS = "tests/ops/test_live_operator.py::"
SHORT_GAP = LIVE_TESTS + "test_open_refuses_a_gap_one_round_short_of_the_minimum_and_names_it"
MINIMUM_GAP = LIVE_TESTS + "test_open_accepts_the_minimum_gap_when_the_manifest_raises_the_cap"
CAP_LEFT_AT_200 = (
    LIVE_TESTS + "test_open_refuses_the_256_s_minimum_when_the_manifest_leaves_the_cap_at_200"
)
TEST_ONLY = LIVE_TESTS + "test_a_cap_above_the_default_is_for_a_test_manifest_only"
GAP_CAP = (
    POLICY + "test_the_gap_cap_is_a_policy_parameter_omitted_when_absent_and_bounded_by_the_exit"
)
SHAPES = (
    "tests/ops/test_live_operator.py::test_cli_open_with_an_auditable_window_reaches_the_master"
)
GAP_FN = (
    "tests/challenge/test_audit_cooldown_v2.py::"
    "test_the_whole_reaudit_window_must_fit_after_the_end_of_the_first_lease_before_d_final"
)
GAP_TESTS = "tests/challenge/test_audit_reaudit_gap_v2.py::"
AT_THE_MINIMUM = (
    GAP_TESTS
    + "test_the_reaudit_at_the_minimum_gap_is_decided_inside_the_slack_and_drops_the_member_past_it"
)
D87_E2E = (
    "tests/e2e/test_reaudit_failed_v2_e2e.py::"
    "test_a_failed_reaudit_drops_its_member_pays_the_honest_and_sanctions_the_next_forger"
)
PLAN = "tests/challenge/test_exit_plan_v2.py::"
PLAN_BOUND = (
    PLAN + "test_the_reaudit_wait_ends_one_window_of_its_rounds_after_the_second_audit_window"
)
PLAN_COMMIT = PLAN + "test_a_reaudit_window_never_outlives_its_wait_or_the_commit"
SERVICE_ACCEPTS = "tests/ops/test_manifest_v2.py::test_build_then_service_accepts"
BUILD_REFUSES = "tests/ops/test_manifest_v2.py::test_build_refuses_invalid_inputs"
MANIFEST_TOOL = "src/hypertrain/ops/manifest_v2.py"
PRODUCTION_CAP = POLICY + "test_a_production_manifest_cannot_raise_the_gap_cap_above_the_default"
BUILDER_CAP = (
    "tests/ops/test_manifest_v2.py::"
    "test_the_builder_refuses_a_production_manifest_that_lifts_a_test_only_bound"
    "[production-2-gap_cap]"
)
SERVICE_CAP = (
    "tests/ops/test_manifest_v2.py::"
    "test_the_service_refuses_a_production_manifest_that_lifts_a_test_only_bound[gap_cap]"
)
FAIL_REAUDIT = (
    "            self._fail_reaudit_v2(run_id, row)  "
    "# the second auditor failed too: the member leaves\n"
)
GAP_CAP_FIELD = (
    "    gap_cap_rounds: Annotated[StrictInt, Field(ge=DEFAULT_GAP_CAP_ROUNDS, le=2**53 - 1)]"
    " | None = (\n"
    "        Field(default=None, exclude_if=lambda v: v is None)\n    )\n"
)
REAUDIT_FIELD = (
    "    reaudit_rounds: Annotated[StrictInt, Field(ge=2, le=2**53 - 1)] | None = Field(\n"
    "        default=None, exclude_if=lambda v: v is None\n    )\n"
)

TABLE: tuple[tuple[str, str, str, str, tuple[str, ...]], ...] = (
    (
        "audit does not compare the leaf preimages",
        WORKER,
        "if a != b), None\n",
        "if False), None\n",
        (f"{CHEATS}[altered-weights-one-ulp-first-step]",),
    ),
    (
        "audit does not compare the leaves root",
        WORKER,
        '        and summary["leaves_root"] == commit.leaves_root\n',
        "",
        (f"{CHEATS}[commit-lie-leaves-root]",),
    ),
    (
        "audit does not compare the delta hash",
        WORKER,
        '        and summary["delta_hash"] == commit.delta_hash\n',
        "",
        (f"{CHEATS}[forged-delta-valid-hash]", f"{CHEATS}[commit-lie-delta-hash]"),
    ),
    (
        "audit does not compare the final theta hash",
        WORKER,
        '        and summary["final_theta_hash"] == commit.final_theta_hash\n',
        "",
        (f"{CHEATS}[commit-lie-final-theta-hash]",),
    ),
    (
        "audit does not compare the EF out hash",
        WORKER,
        '        and summary["ef_out_hash"] == commit.ef_out_hash\n',
        "",
        (f"{CHEATS}[commit-lie-ef-out-hash]",),
    ),
    (
        "re-audit skipped: a withheld MISMATCH is retired as an ordinary lapse",
        STORE,
        '        if failure.get("withheld") == "MISMATCH" and '
        "self._reaudit_wanted_v2(run_id, row):\n"
        "            self._open_reaudit_v2(run_id, row, now)\n"
        "            return\n",
        "",
        (WINDOW_AT_DETECTION, SECOND_FAILS),
    ),
    (
        "withholders forgotten when the window opens: nobody is excluded from the job",
        STORE,
        '        withholders = self._withholders_v2(run_id, row["id"])\n',
        "        withholders: set[str] = set()\n",
        (NOT_AGAIN, LAST_WINDOW, NOBODY_LEFT),
    ),
    (
        "same auditor allowed: the window forgets who withheld",
        STORE,
        '(_dumps(sorted(withholders)), until, _dumps(signed), row["id"])',
        '("[]", until, _dumps(signed), row["id"])',
        (NOT_AGAIN, LAST_WINDOW),
    ),
    (
        "the withheld MISMATCH is not recorded on the failure row",
        STORE,
        '                failure["withheld"] = stored\n',
        "                pass\n",
        (WINDOW_AT_DETECTION, LAST_WINDOW, NTH_BARS + "[True]"),
    ),
    (
        "sanction on first detection: the lapse books the miner",
        STORE,
        '                failure["withheld"] = stored\n',
        '                failure["withheld"] = stored\n'
        '            if stored == "MISMATCH":\n'
        "                self._put_record_v2(\n"
        '                    run_id,\n                    "exit-sanction",\n'
        "                    f\"{row['w']}:{row['hotkey']}\",\n"
        '                    {"state": "PENDING", "blame": "MINER", "auditor": row["auditor"]},\n'
        "                )\n",
        (LAST_WINDOW, WINDOW_AT_DETECTION),
    ),
    (
        "strike counter off by one: the bar starts after the third",
        COOLDOWN,
        "barred = withheld >= WITHHELD_STRIKES\n",
        "barred = withheld > WITHHELD_STRIKES\n",
        (BAR_LENGTHS, NTH_BARS + "[True]"),
    ),
    (
        "strike counter off by one: the bar starts at the second",
        COOLDOWN,
        "barred = withheld >= WITHHELD_STRIKES\n",
        "barred = withheld >= WITHHELD_STRIKES - 1\n",
        (BAR_LENGTHS, NTH_BARS + "[False]"),
    ),
    (
        "the last free auditor is barred too",
        COOLDOWN,
        '"until": now + length if free else now,',
        '"until": now + length,',
        (LAST_FREE,),
    ),
    (
        "a withheld MATCH counts toward the bar",
        STORE,
        'withheld=failure.get("withheld") == "MISMATCH")',
        'withheld=failure.get("withheld") is not None)',
        (ONLY_MISMATCH + "[withheld-match]",),
    ),
    (
        "an ordinary silence is barred once the count is past the threshold",
        STORE,
        "        count = 0\n        if withheld:\n",
        "        count = 0\n        if True:\n",
        (ONLY_MISMATCH + "[without-execution]",),
    ),
    (
        "the withholds of every auditor count against each one",
        STORE,
        "\"AND json_extract(data,'$.auditor')=? \"\n",
        "\"AND ?<>'' \"\n",
        (NTH_BARS + "[False]",),
    ),
    (
        "nobody-left rule removed: the job idles in a window nobody can serve",
        STORE,
        '            or not self._auditors_left_v2(run_id, row, json.loads(row["tried"]))\n',
        "",
        (NOBODY_LEFT, SECOND_FAILS),
    ),
    (
        "a stale verdict is attributed to a holder that never asked for the execution",
        STORE,
        '        if marker is None or marker["state"] != "EXECUTED" or stored is None:\n',
        "        if stored is None:\n",
        (NOBODY_LEFT,),
    ),
    (
        "a withheld MATCH makes its holder a withholder of the job",
        STORE,
        "='MISMATCH'\",\n                (run_id, f\"{job_id}",
        ' IS NOT NULL",\n                (run_id, f"{job_id}',
        (MATCH_HOLDER,),
    ),
    (
        "the withholders of a job include those of the jobs that sort after it",
        STORE,
        '(run_id, f"{job_id}:", f"{job_id};"),',
        '(run_id, f"{job_id}:", "\\U0010ffff"),',
        (OTHER_JOB + "[above]",),
    ),
    (
        "the withholders of a job include those of the jobs that sort before it",
        STORE,
        '(run_id, f"{job_id}:", f"{job_id};"),',
        '(run_id, "", f"{job_id};"),',
        (OTHER_JOB + "[below]",),
    ),
    (
        "the miner counts as an auditor left to try its own job",
        STORE,
        'return any(a not in taken and a != row["hotkey"] for a in auditors)',
        "return any(a not in taken for a in auditors)",
        (MINER_AUDITOR,),
    ),
    (
        "window opened for nobody: the miner is the only other auditor",
        STORE,
        "        if until <= max(detected, now) + 1 or "
        "not self._auditors_left_v2(run_id, row, withholders):\n",
        "        if until <= max(detected, now) + 1:\n",
        (MINER_IS_AUDITOR,),
    ),
    (
        "revocation removed: an executed lease runs to its expiry whoever holds it",
        STORE,
        "signed_by = audit_cooldown_v2.sign_deadline(self._execution_marker_v2(run_id, row))",
        "signed_by = None",
        (REVOKED, ADMITTED),
    ),
    (
        "sign window counted from the claim, not from the stored verdict",
        STORE,
        "audit_cooldown_v2.sign_deadline(self._execution_marker_v2(run_id, row))",
        "audit_cooldown_v2.sign_deadline(\n"
        "            (m := self._execution_marker_v2(run_id, row))\n"
        '            and {**m, "at": row["created"]}\n'
        "        )",
        (FROM_STORED, REVOKED),
    ),
    (
        "revocation sanctions the miner: the revoked lease books an exclusion",
        STORE,
        RETIRE_EXPIRED,
        RETIRE_EXPIRED + '                if now < row["expires"]:\n'
        "                    self._put_record_v2(\n"
        '                        run_id,\n                        "excluded",\n'
        "                        f\"{row['w']}:{row['hotkey']}\",\n"
        '                        {"reason": "UNVERIFIED", "evidence_hash": row["id"]},\n'
        "                    )\n",
        (REVOKED,),
    ),
    (
        "same auditor re-picked after the revocation: tried is forgotten",
        STORE,
        RETIRE_EXPIRED,
        RETIRE_EXPIRED + FORGET_TRIED,
        (NOT_AGAIN,),
    ),
    (
        "sign window one round short: an honest signature on the last round is refused",
        COOLDOWN,
        'return int(marker["at"]) + SIGN_WINDOW_ROUNDS',
        'return int(marker["at"]) + SIGN_WINDOW_ROUNDS - 1',
        (SIGN_DEADLINE, REVOKED, REAL_SIGNATURE + "[inside-the-window]"),
    ),
    (
        "sign window one round long: the revocation comes a round late",
        COOLDOWN,
        'return int(marker["at"]) + SIGN_WINDOW_ROUNDS',
        'return int(marker["at"]) + SIGN_WINDOW_ROUNDS + 1',
        (SIGN_DEADLINE, REVOKED, REAL_SIGNATURE + "[after-the-window]"),
    ),
    (
        "revocation of a REQUESTED marker: a replay in progress loses its lease",
        COOLDOWN,
        'if marker is None or marker["state"] != "EXECUTED":',
        "if marker is None:",
        (SIGN_DEADLINE, FROM_STORED),
    ),
    (
        "the cooldown starts at the expiry, not at the revocation",
        STORE,
        "silent_since = self._lease_end_v2(run_id, row)",
        'silent_since = row["expires"]',
        (REVOKED, COOLS_FROM, LATE_POLL),
    ),
    (
        "reaudit_rounds always serialised: the exclusion is dropped and every run id moves",
        MESSAGES,
        REAUDIT_FIELD,
        REAUDIT_FIELD.replace("default=None, exclude_if=lambda v: v is None", "default=None"),
        (OMITTED, SERVICE_ACCEPTS + "[None]"),
    ),
    (
        "re-audit window ceiling removed: a window past two leases is accepted",
        MESSAGES,
        "self.lease_rounds <= self.reaudit_window() <= self.absolute_rounds()",
        "self.lease_rounds <= self.reaudit_window()",
        (
            WINDOW_RANGE,
            BUILD_REFUSES + "[reaudit-audit re-audit window must be between one and two leases]",
        ),
    ),
    (
        "re-audit window floor removed: a window shorter than a lease is accepted",
        MESSAGES,
        "self.lease_rounds <= self.reaudit_window() <= self.absolute_rounds()",
        "self.reaudit_window() <= self.absolute_rounds()",
        (WINDOW_RANGE,),
    ),
    (
        "an absent re-audit window means two leases instead of one",
        MESSAGES,
        "return self.lease_rounds if self.reaudit_rounds is None else self.reaudit_rounds",
        "return self.absolute_rounds() if self.reaudit_rounds is None else self.reaudit_rounds",
        (WINDOW_RANGE,),
    ),
    (
        "published re-audit window without the one-lease floor",
        MESSAGES,
        "return max(lease_rounds, -(-2 * replay_seconds // BEACON_PERIOD_SECONDS))",
        "return -(-2 * replay_seconds // BEACON_PERIOD_SECONDS)",
        (WINDOW_FORMULA + "[144-100]",),
    ),
    (
        "published re-audit window rounded down instead of up",
        MESSAGES,
        "return max(lease_rounds, -(-2 * replay_seconds // BEACON_PERIOD_SECONDS))",
        "return max(lease_rounds, 2 * replay_seconds // BEACON_PERIOD_SECONDS)",
        (WINDOW_FORMULA + "[200-134]", WINDOW_FORMULA + "[256-171]"),
    ),
    (
        "window from the lease open: the detection is the claim plus one lease",
        STORE,
        "        detected = self._lease_end_v2(run_id, row)\n",
        '        detected = int(row["expires"]) - policy.lease_rounds\n',
        (WINDOW_AT_DETECTION, LATE_OBSERVATION),
    ),
    (
        "window opened at the observation: the detection is the round that noticed it",
        STORE,
        "        detected = self._lease_end_v2(run_id, row)\n",
        "        detected = now\n",
        (LATE_OBSERVATION, NOTICED_AFTER),
    ),
    (
        "no bound: the window of a re-audit never ends",
        STORE,
        '(_dumps(sorted(withholders)), until, _dumps(signed), row["id"])',
        '(_dumps(sorted(withholders)), 10**9, _dumps(signed), row["id"])',
        (UNUSED_WINDOW, WINDOW_AT_DETECTION),
    ),
    (
        "window ignores the commit: it outlasts the expiry of what it audits",
        EXIT,
        "    return min(commit_exp, max(detected + reaudit_rounds, absolute))\n",
        "    return max(detected + reaudit_rounds, absolute)\n",
        (WINDOW_AT_DETECTION + "[clipped-by-the-commit]", COMMIT_ENDS, PLAN_COMMIT),
    ),
    (
        "window refused when the detection plus the window is past the commit",
        STORE,
        "        if until <= max(detected, now) + 1 or "
        "not self._auditors_left_v2(run_id, row, withholders):\n",
        "        if until <= max(detected, now) + 1 or detected + policy.reaudit_window() > "
        "sealed.exp_drand or not self._auditors_left_v2(run_id, row, withholders):\n",
        (WINDOW_AT_DETECTION + "[clipped-by-the-commit]",),
    ),
    (
        "a window with no rounds left is leased: the guard on the round that notices it is gone",
        STORE,
        "        if until <= max(detected, now) + 1 or "
        "not self._auditors_left_v2(run_id, row, withholders):\n",
        "        if until <= detected + 1 or "
        "not self._auditors_left_v2(run_id, row, withholders):\n",
        (NOTICED_AFTER,),
    ),
    (
        "sanction on the second failure: the failed re-audit books the miner",
        STORE,
        FAIL_REAUDIT,
        "            self._fail_reaudit_v2(run_id, row)\n"
        "            self._put_record_v2(\n"
        '                run_id,\n                "exit-sanction",\n'
        "                f\"{row['w']}:{row['hotkey']}\",\n"
        '                {"state": "PENDING", "blame": "MINER", "auditor": row["auditor"]},\n'
        "            )\n",
        (SECOND_FAILS, UNUSED_WINDOW),
    ),
    (
        "funnel removed: the second failure re-arms the job instead of failing it",
        STORE,
        "        if self._reaudit_v2(run_id, row) is not None:\n"
        + FAIL_REAUDIT
        + "            return\n",
        "",
        (SECOND_FAILS, UNUSED_WINDOW),
    ),
    (
        "the wait for a re-audit ends with the second audit window",
        EXIT,
        "        return self.audits_end + self.reaudit_rounds\n",
        "        return self.audits_end\n",
        (PLAN_BOUND,),
    ),
    (
        "a service-admitted run opens a window it could not retire a round for",
        STORE,
        "            and not self._db.execute(\n"
        '                "SELECT 1 FROM records_v2 WHERE run_id=? AND kind='
        "'service-admission' AND id='run'\",\n"
        "                (run_id,),\n"
        "            ).fetchone()\n",
        "",
        (SERVICE_RUN,),
    ),
    (
        "gap guard asked of a policy that pins no window: the tiny-cpu shapes are refused",
        LIVE,
        "    if audit.reaudit_rounds is None:\n"
        "        return\n"
        "    needed = reaudit_gap_needed(audit.lease_rounds, audit.reaudit_rounds)\n",
        "    needed = reaudit_gap_needed(audit.lease_rounds, audit.reaudit_rounds or "
        "audit.lease_rounds)\n",
        (SHAPES + "[98]",),
    ),
    (
        "gap guard off by one: the exact gap is refused",
        LIVE,
        "    if gap < needed:\n",
        "    if gap <= needed:\n",
        (MINIMUM_GAP + "[144s]", MINIMUM_GAP + "[256s]"),
    ),
    (
        "gap guard lets a gap that cannot decide a miner through",
        LIVE,
        "    if gap < needed:\n",
        "    if gap < needed - 10:\n",
        (SHORT_GAP + "[144s]", SHORT_GAP + "[256s]"),
    ),
    (
        "gap needed ignores that the first lease is capped at one lease",
        COOLDOWN,
        "1 + min(lease_rounds, replay + SIGN_WINDOW_ROUNDS) + reaudit_rounds + SIGN_MARGIN_ROUNDS",
        "1 + (replay + SIGN_WINDOW_ROUNDS) + reaudit_rounds + SIGN_MARGIN_ROUNDS",
        (GAP_FN,),
    ),
    (
        "gap needed leaves out the margin of the signature",
        COOLDOWN,
        "1 + min(lease_rounds, replay + SIGN_WINDOW_ROUNDS) + reaudit_rounds + SIGN_MARGIN_ROUNDS",
        "1 + min(lease_rounds, replay + SIGN_WINDOW_ROUNDS) + reaudit_rounds",
        (GAP_FN, SHORT_GAP + "[256s]"),
    ),
    (
        "sign window of 21 rounds: a stored verdict is revoked a round late",
        COOLDOWN,
        "SIGN_WINDOW_ROUNDS = 20\n",
        "SIGN_WINDOW_ROUNDS = 21\n",
        (SIGN_DEADLINE, REVOKED),
    ),
    (
        "whole round retired by one failed re-audit (D84, replaced by D87): the round is voided",
        STORE,
        "        self._drop_audit_job_v2(\n"
        "            run_id,\n"
        "            row,\n"
        '            "INFRASTRUCTURE",\n'
        '            "REAUDIT_FAILED",\n'
        "            sha256_hex(f\"ht-reaudit-failed/1|{row['id']}\".encode()),\n"
        "        )\n",
        "        self._drop_audit_job_v2(\n"
        "            run_id,\n"
        "            row,\n"
        '            "INFRASTRUCTURE",\n'
        '            "REAUDIT_FAILED",\n'
        "            sha256_hex(f\"ht-reaudit-failed/1|{row['id']}\".encode()),\n"
        "        )\n"
        "        self._put_record_v2(\n"
        '            run_id, "retired", str(row["w"]), {"cause": "REAUDIT_FAILED"}\n'
        "        )\n",
        (D87_E2E,),
    ),
    (
        "sibling unpaid: a failed re-audit cancels the other audits of its round",
        STORE,
        "            sha256_hex(f\"ht-reaudit-failed/1|{row['id']}\".encode()),\n        )\n",
        "            sha256_hex(f\"ht-reaudit-failed/1|{row['id']}\".encode()),\n        )\n"
        "        self._db.execute(\n"
        "            \"UPDATE audit_leases_v2 SET state='exhausted',result='ROUND_RETIRED' \"\n"
        "            \"WHERE run_id=? AND w=? AND state IN ('queued','running')\",\n"
        '            (run_id, row["w"]),\n'
        "        )\n",
        (D87_E2E,),
    ),
    (
        "a later MISMATCH is not sanctioned: a failed re-audit closes its round to the sanction",
        STORE,
        '            if int(row["id"].split(":", 1)[0]) not in unclosed:\n'
        "                continue\n"
        "            try:\n"
        "                with escrow.tx():\n"
        "                    self._book_mismatch_v2(",
        '            if int(row["id"].split(":", 1)[0]) not in unclosed or self._db.execute(\n'
        '                "SELECT 1 FROM audit_leases_v2 WHERE run_id=? AND w=? "\n'
        "                \"AND result='REAUDIT_FAILED'\",\n"
        '                (run_id, int(row["id"].split(":", 1)[0])),\n'
        "            ).fetchone():\n"
        "                continue\n"
        "            try:\n"
        "                with escrow.tx():\n"
        "                    self._book_mismatch_v2(",
        (D87_E2E,),
    ),
    (
        "a failed re-audit drops its member with the evidence of an ordinary exhaustion",
        STORE,
        "            sha256_hex(f\"ht-reaudit-failed/1|{row['id']}\".encode()),\n",
        '            sha256_hex(row["id"].encode()),\n',
        (SECOND_FAILS, D87_E2E),
    ),
    (
        "gap_cap_rounds always serialised: the exclusion is dropped and every run id moves",
        MESSAGES,
        GAP_CAP_FIELD,
        GAP_CAP_FIELD.replace(
            "Field(default=None, exclude_if=lambda v: v is None)", "Field(default=None)"
        ),
        (GAP_CAP,),
    ),
    (
        "gap cap ceiling removed: a round may outlast the last audit wait",
        MESSAGES,
        "        if self.gap_cap() > 2 * self.absolute_rounds() + self.reaudit_window():\n",
        "        if False:\n",
        (GAP_CAP,),
    ),
    (
        "gap cap raised by a production manifest is accepted",
        LIVE,
        '    if audit.gap_cap() > DEFAULT_GAP_CAP_ROUNDS and ledger_mode != "test":\n',
        "    if False:\n",
        (TEST_ONLY,),
    ),
    (
        "a production ledger mode may raise the cap of the audit policy",
        MESSAGES,
        "        if self.gap_cap() > DEFAULT_GAP_CAP_ROUNDS and ledger_mode not in "
        '(None, "test"):\n',
        "        if False:\n",
        (PRODUCTION_CAP, BUILDER_CAP, SERVICE_CAP),
    ),
    (
        "the manifest builder does not tell the audit policy its ledger mode",
        MANIFEST_TOOL,
        "    audit.validate_training(validated, ledger_mode=economics.ledger_mode)\n",
        "    audit.validate_training(validated)\n",
        (BUILDER_CAP,),
    ),
    (
        "the service does not tell the audit policy its ledger mode when it creates a run",
        STORE,
        "                        policy.validate_training(model.training, "
        "ledger_mode=ledger_mode)\n",
        "                        policy.validate_training(model.training)\n",
        (SERVICE_CAP,),
    ),
    (
        "the cap of the policy is ignored: the tool keeps the literal 200",
        LIVE,
        "    elif not 2 <= gap <= cap or offsets[4] <= offsets[2]:\n",
        "    elif not 2 <= gap <= DEFAULT_GAP_CAP_ROUNDS or offsets[4] <= offsets[2]:\n",
        (MINIMUM_GAP + "[256s]", TEST_ONLY),
    ),
    (
        "the cap is not named when the minimum gap is above it",
        LIVE,
        "                if needed > audit.gap_cap()\n",
        "                if False\n",
        (CAP_LEFT_AT_200,),
    ),
)
