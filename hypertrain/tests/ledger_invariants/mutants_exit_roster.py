"""Mutants of the exit_roster package: one small regression of src each, killed by named tests.

Most of what this package first found as holes was closed by v3 (audit-integrity ADR 0011), where
the closing commits ship their own tests. Their mutants stay here and name those tests: a mutant
that reverts the fix has to be killed by the test that replaced the one this package used to carry.
"""

T = "tests/ledger_invariants/test_exit_roster.py::"
T2 = "tests/ledger_invariants/test_exit_roster_2.py::"
LIVENESS = "tests/challenge/test_audit_liveness_v2.py::"
NO_SHOW = "tests/challenge/test_no_show_v2.py::"
NO_SHOW_E2E = "tests/e2e/test_no_show_resume_v2.py::"
STORE = "src/hypertrain/challenge/store.py"
EXIT = "src/hypertrain/challenge/exit_v2.py"
COOLDOWN = "src/hypertrain/challenge/audit_cooldown_v2.py"

DROPPED = (
    'finality["unresolved_audits"] = max(0, int(str(finality["unresolved_audits"])) - 1)\n'
    '        self._put_record_v2(run_id, "finality", str(row["w"]), finality)'
)

STRIKE = (
    "\n        self._db.execute("
    '\n            "UPDATE admissions_v2 SET strikes=strikes+1 WHERE hotkey=?", (row["hotkey"],)'
    "\n        )"
)

MUTANTS = [
    {
        "id": "exit-roster-queue-served-backwards",
        "invariants": ["A2W-18"],
        "path": STORE,
        "old": "\"AND state='queued' ORDER BY created,id\",",
        "new": "\"AND state='queued' ORDER BY created,id DESC\",",
        "tests": [T + "test_the_job_first_in_the_queue_is_the_first_to_be_leased"],
        "why": "The lease goes to the last job of the queue instead of the first.",
    },
    {
        "id": "exit-roster-live-lease-guard-removed",
        "invariants": ["A2W-24", "SG-10"],
        "path": STORE,
        "old": 'raise ChallengeError(409, "one live lease per auditor")',
        "new": "pass",
        "tests": [T + "test_an_ask_repeated_while_its_lease_runs_is_refused_and_grants_nothing"],
        "why": "An auditor that holds a running lease is granted a second one for the same ask.",
    },
    {
        "id": "exit-roster-conflicting-ask-accepted",
        "invariants": ["A2W-24", "SG-10"],
        "path": STORE,
        "old": "if known is not None and known[0] != digest:",
        "new": "if known is not None and known[0] != known[0]:",
        "tests": [T + "test_the_same_ask_nonce_with_another_body_is_refused_and_grants_nothing"],
        "why": "A second body under a known signed nonce is accepted instead of refused.",
    },
    {
        "id": "exit-roster-excluded-member-not-skipped",
        "invariants": ["A2W-21", "FIN-19"],
        "path": STORE,
        "old": "            if excluded:\n                continue",
        "new": "            if False:\n                continue",
        "tests": [T + "test_an_excluded_member_is_left_out_of_the_tape_and_the_others_are_applied"],
        "why": "The aggregation reads the records of a member that has an exclusion record.",
    },
    {
        "id": "exit-roster-horizon-one-round-late",
        "invariants": ["A2W-20", "FIN-22"],
        "path": STORE,
        "old": '            elif row["state"] == "queued" and now >= row["absolute"]:',
        "new": '            elif row["state"] == "queued" and now > row["absolute"]:',
        "tests": [
            T + "test_every_roster_member_is_in_a_terminal_audit_state_once_the_horizon_has_passed",
            LIVENESS + "test_a_job_nobody_audits_in_two_windows_drops_its_miner_and_nothing_else",
        ],
        "why": "A queued job survives the round its absolute deadline names.",
    },
    {
        "id": "exit-roster-horizon-keeps-the-audit-counter",
        "invariants": ["A2W-20", "FIN-22"],
        "path": STORE,
        "old": DROPPED,
        "new": DROPPED.replace(" - 1)", ")"),
        "tests": [
            T + "test_every_roster_member_is_in_a_terminal_audit_state_once_the_horizon_has_passed",
            LIVENESS + "test_a_job_nobody_audits_in_two_windows_drops_its_miner_and_nothing_else",
        ],
        "why": "A job that left its round leaves the audit counter of that round unchanged.",
    },
    {
        "id": "exit-roster-horizon-blames-the-miner",
        "invariants": ["A2W-20", "FIN-22"],
        "path": STORE,
        "old": '                "INFRASTRUCTURE",\n                "AUDIT_INFRASTRUCTURE",\n',
        "new": '                "UNVERIFIED",\n                "AUDIT_INFRASTRUCTURE",\n',
        "tests": [
            T + "test_every_roster_member_is_in_a_terminal_audit_state_once_the_horizon_has_passed",
            LIVENESS + "test_a_job_nobody_audits_in_two_windows_drops_its_miner_and_nothing_else",
        ],
        "why": "A job nobody served excludes its member as a failed audit.",
    },
    {
        "id": "exit-roster-dropped-job-voids-the-round",
        "invariants": ["FIN-09", "FIN-19", "CRD-04", "A2W-22"],
        "path": STORE,
        "old": DROPPED,
        "new": DROPPED.replace(
            "\n        self._put_record_v2",
            '\n        finality["disposition"] = "EXCLUDED"'
            '\n        finality["rollback_complete"] = not finality["applied"]'
            "\n        self._put_record_v2",
        ),
        "tests": [
            T + "test_no_applied_round_is_closed_by_exclusion_without_a_rollback_record",
            T + "test_the_survivors_of_a_round_with_an_exhausted_member_are_settled",
        ],
        "why": "The member that left a round at the horizon takes the whole round with it, as an "
        "EXCLUDED disposition that finality refuses to settle.",
    },
    {
        "id": "exit-roster-dropped-job-strikes-its-miner",
        "invariants": ["NSH-04"],
        "path": STORE,
        "old": DROPPED,
        "new": DROPPED + STRIKE,
        "tests": [T2 + "test_a_loss_of_leaves_on_the_master_sanctions_nobody"],
        "why": "A job the master could not serve ends in a strike for its miner, as if the silence "
        "were the miner's.",
    },
    {
        "id": "exit-roster-ledger-demands-a-round-nobody-finalized",
        "invariants": ["CRD-04", "FIN-08", "A2W-22"],
        "path": "src/hypertrain/ledger/escrow_v2.py",
        "old": "            while w in accounted:\n                w += 1\n",
        "new": "            while w in accounted:\n                w += 2\n",
        "tests": [T + "test_a_round_after_a_round_with_a_lost_member_is_credited"],
        "why": "The ledger waits for a round that no finalize will ever credit, so the round after "
        "a settled one is refused FINALIZE_ORDER.",
    },
    {
        "id": "exit-roster-no-show-not-left-out",
        "invariants": ["FIN-21", "A2W-20", "LV-06"],
        "path": STORE,
        "old": "            self._exclude_no_shows_v2(run_id, manifest, w, body.roster)\n",
        "new": "            pass\n",
        "tests": [
            T + "test_every_roster_member_is_in_a_terminal_audit_state_once_the_horizon_has_passed",
            NO_SHOW + "test_a_member_that_never_commits_no_longer_stalls_the_round",
            NO_SHOW_E2E + "test_round_proceeds_to_w_plus_2_after_a_member_never_commits",
        ],
        "why": "The audits of a round with a member that never committed raise 409 for everybody.",
    },
    {
        "id": "exit-roster-delta-stage-not-wired",
        "invariants": ["A2W-21"],
        "path": EXIT,
        "old": "    if now >= s.schedule.d_upload:\n        out += [",
        "new": "    if False:\n        out += [",
        "tests": [NO_SHOW_E2E + "test_a_member_that_commits_but_never_uploads_a_delta_is_left_out"],
        "why": "The push that closes delta intake no longer leaves out an audited member whose "
        "delta never arrived.",
    },
    {
        "id": "exit-roster-job-without-leaves-blocks-the-queue",
        "invariants": ["A2W-18", "FIN-22"],
        "path": STORE,
        "old": "continue  # leaves have no deadline: leasable once posted, never blocking",
        "new": "pass",
        "tests": [NO_SHOW + "test_a_job_without_leaves_does_not_starve_the_audits_behind_it"],
        "why": "The head of the queue is read before its prerequisites and refuses every ask.",
    },
    {
        "id": "exit-roster-every-expiry-is-the-auditors-silence",
        "invariants": ["A2W-19", "LV-05"],
        "path": STORE,
        "old": 'if marker is not None and json.loads(marker[0])["state"] == "REQUESTED":',
        "new": "if False:",
        "tests": [
            LIVENESS + "test_a_preparation_refusal_by_the_master_is_not_the_auditors_silence",
            LIVENESS + "test_an_expiry_while_the_executor_ran_is_not_the_auditors_silence",
        ],
        "why": "An auditor that asked for the execution is put on cooldown for the master's own "
        "refusal.",
    },
    {
        "id": "exit-roster-cooldown-is-for-ever",
        "invariants": ["A2W-19", "LV-05"],
        "path": COOLDOWN,
        "old": 'if record is None or now >= record["until"]:',
        "new": "if record is None:",
        "tests": [LIVENESS + "test_a_silent_auditor_is_refused_only_until_the_cooldown_ends"],
        "why": "One lapsed lease refuses the auditor for ever, as the disable record did.",
    },
    {
        "id": "exit-roster-last-auditor-refused",
        "invariants": ["A2W-19", "LV-05"],
        "path": COOLDOWN,
        "old": '"until": now + length if free else now,',
        "new": '"until": now + length,',
        "tests": [LIVENESS + "test_two_silent_auditors_never_leave_the_run_without_an_auditor"],
        "why": "Two silent auditors end all auditing: the last free auditor is refused too.",
    },
]
