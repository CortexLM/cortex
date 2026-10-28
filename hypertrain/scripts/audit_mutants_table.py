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
COOLDOWN = "src/hypertrain/challenge/audit_cooldown_v2.py"
CHEATS = "tests/audit_cheaters/test_cheats_audit.py::test_cheat_is_mismatch_at_the_expected_leaf"
WITHHELD = "tests/challenge/test_audit_withheld_signature_v2.py::"
NOT_HANDED_AGAIN = (
    WITHHELD
    + "test_the_auditor_that_withheld_a_mismatch_is_not_handed_the_job_again_after_the_rearm"
)
LAST_WINDOW = (
    WITHHELD
    + "test_a_mismatch_withheld_in_the_last_window_still_gets_a_lease_for_the_other_auditor"
)
NOBODY_LEFT = WITHHELD + "test_a_third_window_ends_the_job_as_soon_as_no_auditor_is_left_to_try"
BOTH_WITHHOLD = (
    WITHHELD + "test_when_both_auditors_withhold_the_miner_is_dropped_once_and_never_sanctioned"
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
        "re-audit skipped: the re-arm ignores the auditors that withheld",
        STORE,
        '        withholders = self._withholders_v2(run_id, row["id"])\n',
        "        withholders: set[str] = set()\n",
        (NOT_HANDED_AGAIN, LAST_WINDOW, NOBODY_LEFT),
    ),
    (
        "same auditor allowed: the re-arm forgets who withheld",
        STORE,
        '(_dumps(sorted(withholders)), now, absolute, _dumps(signed), row["id"])',
        '("[]", now, absolute, _dumps(signed), row["id"])',
        (NOT_HANDED_AGAIN, LAST_WINDOW),
    ),
    (
        "the withheld MISMATCH is not recorded on the failure row",
        STORE,
        '                failure["withheld"] = stored\n',
        "                pass\n",
        (NOT_HANDED_AGAIN, LAST_WINDOW, NTH_BARS + "[True]"),
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
        (LAST_WINDOW, BOTH_WITHHOLD),
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
        "extra window removed: a withheld MISMATCH in the last window is dropped",
        STORE,
        "extra = audit_cooldown_v2.WITHHELD_EXTRA_WINDOWS if withholders else 0",
        "extra = 0",
        (LAST_WINDOW,),
    ),
    (
        "nobody-left rule removed: the job idles in a window nobody can serve",
        STORE,
        '            or not self._auditors_left_v2(run_id, row, json.loads(row["tried"]))\n',
        "",
        (NOBODY_LEFT,),
    ),
    (
        "re-arm with nobody left to try the job",
        STORE,
        "        if withholders and not self._auditors_left_v2(run_id, row, withholders):\n"
        "            return False\n",
        "",
        (BOTH_WITHHOLD,),
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
)
