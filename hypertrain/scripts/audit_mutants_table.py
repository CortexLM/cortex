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
    REAUDIT + "test_the_second_auditor_withholding_too_retires_the_round_and_blames_nobody"
)
UNUSED_WINDOW = (
    REAUDIT + "test_a_window_nobody_uses_ends_in_a_retirement_at_its_bound_and_not_before"
)
COMMIT_ENDS = (
    REAUDIT
    + "test_a_commit_that_ends_before_the_window_can_open_retires_the_round_at_the_detection"
)
LATE_OBSERVATION = REAUDIT + "test_a_late_observation_leaves_the_window_where_the_detection_put_it"
NOTICED_AFTER = (
    REAUDIT + "test_a_detection_noticed_after_its_window_is_over_fails_the_job_without_leasing_it"
)
MINER_IS_AUDITOR = (
    REAUDIT
    + "test_a_miner_that_is_the_only_other_auditor_cannot_be_re_audited_and_the_round_is_retired"
)
SERVICE_RUN = (
    REAUDIT + "test_a_service_admitted_run_opens_no_window_and_keeps_the_infrastructure_drop"
)
GUARD = (
    "tests/ops/test_live_operator.py::"
    "test_open_refuses_a_gap_in_which_a_pinned_reaudit_window_cannot_decide_a_miner"
)
SHAPES = (
    "tests/ops/test_live_operator.py::test_cli_open_with_an_auditable_window_reaches_the_master"
)
GAP_FN = (
    "tests/challenge/test_audit_cooldown_v2.py::"
    "test_a_second_replay_needs_the_first_lease_to_end_and_its_own_replay_before_d_final"
)
GAP200 = (
    "tests/challenge/test_audit_reaudit_gap_200_v2.py::"
    "test_the_reaudit_at_gap_200_is_decided_inside_the_slack_and_retires_the_round_past_it"
)
PLAN = "tests/challenge/test_exit_plan_v2.py::"
PLAN_FAILED = PLAN + "test_a_failed_reaudit_retires_a_round_that_can_still_carry_and_blames_nobody"
PLAN_BOUND = (
    PLAN + "test_the_reaudit_wait_ends_one_window_of_its_rounds_after_the_second_audit_window"
)
PLAN_COMMIT = PLAN + "test_a_reaudit_window_never_outlives_its_wait_or_the_commit"
RETIRE = (
    "tests/challenge/test_retire_round_v2.py::"
    "test_a_round_is_retired_for_a_failed_reaudit_only_when_a_job_of_it_failed"
)
SERVICE_ACCEPTS = "tests/ops/test_manifest_v2.py::test_build_then_service_accepts"
BUILD_REFUSES = "tests/ops/test_manifest_v2.py::test_build_refuses_invalid_inputs"
FAIL_REAUDIT = (
    "            self._fail_reaudit_v2(row)  "
    "# the second auditor failed too: the round, not the miner\n"
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
        "window opened for nobody: the miner is the only other auditor",
        STORE,
        "        if until <= max(detected, now) + 1 or "
        "not self._auditors_left_v2(run_id, row, withholders):\n",
        "        if until <= max(detected, now) + 1:\n",
        (MINER_IS_AUDITOR,),
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
        "            self._fail_reaudit_v2(row)\n"
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
        "planner ignores a failed re-audit",
        EXIT,
        "    elif s.reaudit_failed:\n",
        "    elif False:\n",
        (PLAN_FAILED, SECOND_FAILS),
    ),
    (
        "retirement does not re-derive a failed re-audit from the rows",
        STORE,
        "            if not self._failed_reaudits_v2(run_id, w):\n"
        '                raise refuse("no re-audit of the round has failed")\n',
        "            pass\n",
        (RETIRE,),
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
        (GUARD + "[191-False]",),
    ),
    (
        "gap guard lets a gap that cannot decide a miner through",
        LIVE,
        "    if gap < needed:\n",
        "    if gap < needed - 10:\n",
        (GUARD + "[190-True]",),
    ),
    (
        "gap needed ignores that the first lease is capped at one lease",
        COOLDOWN,
        "1 + min(lease_rounds, replay + SIGN_WINDOW_ROUNDS) + replay + SIGN_MARGIN_ROUNDS",
        "1 + (replay + SIGN_WINDOW_ROUNDS) + replay + SIGN_MARGIN_ROUNDS",
        (GAP_FN,),
    ),
    (
        "gap needed leaves out the margin of the signature",
        COOLDOWN,
        "1 + min(lease_rounds, replay + SIGN_WINDOW_ROUNDS) + replay + SIGN_MARGIN_ROUNDS",
        "1 + min(lease_rounds, replay + SIGN_WINDOW_ROUNDS) + replay",
        (GAP_FN, GUARD + "[190-True]"),
    ),
    (
        "sign window of 21 rounds: the slack of the 144 s replay is one round short",
        COOLDOWN,
        "SIGN_WINDOW_ROUNDS = 20\n",
        "SIGN_WINDOW_ROUNDS = 21\n",
        (GAP200 + "[144s-last-round-of-slack]",),
    ),
)
