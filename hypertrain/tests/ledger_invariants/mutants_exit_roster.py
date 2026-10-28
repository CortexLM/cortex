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
COOLDOWN = "src/hypertrain/challenge/audit_cooldown_v2.py"

DROPPED = (
    'finality["unresolved_audits"] = max(0, int(str(finality["unresolved_audits"])) - 1)\n'
    '        self._put_record_v2(run_id, "finality", str(row["w"]), finality)'
)

# The retirement of a round that nobody can carry is where the jobs of a round nobody served end
# (the exit reaper closes the round before its audit horizon is reached).
RETIRED_JOBS = (
    "        self._db.execute(\n"
    "            \"UPDATE audit_leases_v2 SET state='exhausted',result='ROUND_RETIRED' \"\n"
    "            \"WHERE run_id=? AND w=? AND state IN ('queued','running')\",\n"
    "            (run_id, w),\n"
    "        )\n"
)
RETIRED_JOBS_STRIKE = RETIRED_JOBS + (
    "        self._db.execute(\n"
    '            "UPDATE admissions_v2 SET strikes=strikes+1 WHERE hotkey IN "\n'
    '            "(SELECT hotkey FROM audit_leases_v2 "\n'
    "            \"WHERE run_id=? AND w=? AND result='ROUND_RETIRED')\",\n"
    "            (run_id, w),\n"
    "        )\n"
)
NO_SHOW_EVIDENCE = '        evidence = f"ht-no-show/1|{run_id}|{w}|{hotkey}|{stage}"\n'

# NSH-03 (D83): the lines of lease_v2 that find a leaf object the master lost, the helpers behind
# them and the reaper that retires the round. Each `old` occurs once in its file.
EXIT = "src/hypertrain/challenge/exit_v2.py"
PARK = "raise self._park_lost_leaves_v2(run_id, candidate, leaves_record, now) from None\n"
HANDLER = "                except ObjectNotFound:\n                    "
UNREADABLE = (
    "            except StoreError:\n"
    "                pass  # unreadable, not lost: the next ask for the job meets the error\n"
)
FRESH_READ = (
    "            try:\n"
    '                self.objects.get(str(record["object"]))\n'
    "            except ObjectNotFound:\n"
    '                lost.append(identity.split(":", 1)[1])\n' + UNREADABLE
)
REQUEUE = "            self._requeue_restored_leaves_v2(run_id, w, lost)\n"
LOST_FAULT = "        return CommittedRefusal(503,"
MEMBER = "f\"{row['w']}:{row['hotkey']}\""  # the identity of the member in the park
EXCLUDED = (
    "        self._put_record_v2(\n"
    f'            run_id, "excluded", {MEMBER}, {{"reason": "INFRASTRUCTURE"}}\n'
    "        )\n"
)
STRUCK = (
    "        self._db.execute(\n"
    "            \"UPDATE admissions_v2 SET strikes=strikes+1 WHERE hotkey=?\", (row['hotkey'],)\n"
    "        )\n"
)
BOOKED = (
    "        admission = self._db.execute(\n"
    "            \"SELECT admission_id FROM admissions_v2 WHERE hotkey=?\", (row['hotkey'],)\n"
    "        ).fetchone()[0]\n"
    "        self._book_sanction_v2(\n"
    f'            run_id, {MEMBER}, admission, "leaves", "MINER", "0" * 64\n'
    "        )\n"
)
SAME_OBJECT = (
    '            if self._record_v2(run_id, "leaves", identity)["object"] != record["object"]:\n'
    "                continue\n"
)
UNGUARDED = "\"WHERE id=? AND state='exhausted' AND result='LEAF_OBJECT_LOST'\","
COUNTED = (
    '        finality = self._record_v2(run_id, "finality", str(row["w"]))\n'
    '        finality["unresolved_audits"] = max(0, int(finality["unresolved_audits"]) - 1)\n'
    '        self._put_record_v2(run_id, "finality", str(row["w"]), finality)\n'
)
FORGOTTEN = (
    "                lost_leaves=bool(lost)\n"
    "                and self._db.execute(\n"
    '                    "SELECT 1 FROM records_v2 "\n'
    "                    \"WHERE run_id=? AND kind='round-exit' AND id=?\",\n"
    "                    (run_id, str(w)),\n"
    "                ).fetchone()\n"
    "                is None,\n"
)
REFUSED = (
    T2 + "test_a_leaf_object_the_master_lost_is_refused_by_name_and_the_audits_behind_it_lease"
)
RETIRED = T2 + "test_a_lost_leaf_object_retires_the_round_at_the_next_push"
NOBODY = T2 + "test_the_retirement_for_a_lost_leaf_object_sanctions_nobody_and_moves_no_unit"
NEXT = T2 + "test_the_round_after_one_with_a_lost_leaf_object_opens_and_its_audits_lease"
RESTORED = T2 + "test_a_restored_leaf_object_puts_its_job_back_and_the_round_goes_on"
UNREAD = T2 + "test_a_leaf_object_that_reads_back_corrupt_is_not_a_loss_and_pins_no_round"
REPUBLISHED = T2 + "test_a_leaf_object_republished_after_the_loss_is_not_the_one_that_was_lost"
KEPT = T2 + "test_a_lease_refused_for_another_reason_commits_nothing"
REVIVED = T2 + "test_a_restored_leaf_object_does_not_revive_a_job_of_a_retired_round"
WAITS = (
    T2 + "test_a_lost_leaf_object_waits_for_the_round_before_it_and_is_retired_when_that_one_closes"
)
PLAN = (
    "tests/challenge/test_exit_plan_v2.py::"
    "test_a_lost_leaf_object_retires_a_round_that_can_still_carry_unless_it_cannot"
)
PRIMITIVE = (
    "tests/challenge/test_retire_round_v2.py::"
    "test_a_round_is_retired_for_a_leaf_object_only_while_the_master_has_lost_it"
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
        "old": RETIRED_JOBS,
        "new": RETIRED_JOBS_STRIKE,
        "tests": [T2 + "test_a_loss_of_leaves_on_the_master_sanctions_nobody"],
        "why": "A job the master could not serve ends in a strike for its miner when its round is "
        "retired, as if the silence were the miner's.",
    },
    {
        "id": "nsh03-lost-leaf-object-is-a-bare-500",
        "invariants": ["NSH-03"],
        "path": STORE,
        "old": HANDLER + PARK,
        "new": HANDLER.replace("ObjectNotFound", "KeyError") + PARK,
        "tests": [REFUSED],
        "why": "A leaf object the master lost escapes lease_v2 as ObjectNotFound: a bare 500.",
    },
    {
        "id": "nsh03-lost-leaf-object-is-a-silent-skip",
        "invariants": ["NSH-03"],
        "path": STORE,
        "old": HANDLER + PARK,
        "new": HANDLER
        + "self._park_lost_leaves_v2(run_id, candidate, leaves_record, now)\n"
        + "                    continue\n",
        "tests": [REFUSED],
        "why": "The job is parked and the ask is served the next one: the loss is never named.",
    },
    {
        "id": "nsh03-lost-leaf-refusal-rolls-back-its-journal",
        "invariants": ["NSH-03"],
        "path": STORE,
        "old": "            except CommittedRefusal as error:\n                refusal = error\n",
        "new": "            except CommittedRefusal:\n                raise\n",
        "tests": [REFUSED],
        "why": "The 503 is raised inside the transaction that wrote the journal and the parking.",
    },
    {
        "id": "nsh03-lost-leaf-job-is-not-parked",
        "invariants": ["NSH-03"],
        "path": STORE,
        "old": "SET state='exhausted',result='LEAF_OBJECT_LOST' WHERE id=?",
        "new": "SET result='LEAF_OBJECT_LOST' WHERE id=?",
        "tests": [REFUSED],
        "why": "The job whose object is gone stays queued and the queue behind it starves.",
    },
    {
        "id": "nsh03-lost-leaf-member-is-excluded",
        "invariants": ["NSH-03"],
        "path": STORE,
        "old": LOST_FAULT,
        "new": EXCLUDED + LOST_FAULT,
        "tests": [REFUSED, RETIRED, NOBODY],
        "why": "The member whose object the master lost is left out of the round it did serve.",
    },
    {
        "id": "nsh03-lost-leaf-member-is-struck",
        "invariants": ["NSH-03"],
        "path": STORE,
        "old": LOST_FAULT,
        "new": STRUCK + LOST_FAULT,
        "tests": [NOBODY],
        "why": "The member whose object the master lost is struck as if it had not published it.",
    },
    {
        "id": "nsh03-lost-leaf-member-is-booked-a-no-show",
        "invariants": ["NSH-03"],
        "path": STORE,
        "old": LOST_FAULT,
        "new": BOOKED + LOST_FAULT,
        "tests": [NOBODY],
        "why": "The missing object is booked as the member's no-show: the sweeper seizes its bond.",
    },
    {
        "id": "nsh03-lost-leaf-round-is-not-retired",
        "invariants": ["NSH-03"],
        "path": EXIT,
        "old": "    elif s.lost_leaves:\n",
        "new": "    elif False:\n",
        "tests": [RETIRED, NOBODY, NEXT, PLAN],
        "why": "The planner never asks to retire a round that lost a leaf object: it starves.",
    },
    {
        "id": "nsh03-lost-leaf-retirement-is-refused-as-an-audit-window",
        "invariants": ["NSH-03"],
        "path": STORE,
        "old": '        elif cause == "LEAF_OBJECT_LOST":\n',
        "new": "        elif False:\n",
        "tests": [RETIRED, PRIMITIVE],
        "why": "The retirement falls into the audit-window branch and is refused: audits were "
        "scheduled.",
    },
    {
        "id": "nsh03-lost-leaf-retirement-without-a-fresh-read",
        "invariants": ["NSH-03"],
        "path": STORE,
        "old": FRESH_READ,
        "new": '            lost.append(identity.split(":", 1)[1])\n',
        "tests": [RESTORED, PRIMITIVE],
        "why": "A record of a loss outlives it: an object that reads again retires the round.",
    },
    {
        "id": "nsh03-lost-leaf-unreadable-object-pins-the-round",
        "invariants": ["NSH-03"],
        "path": STORE,
        "old": UNREADABLE,
        "new": "",
        "tests": [UNREAD],
        "why": "An object that is there and unreadable fails the reaper's turn: the job stays "
        "parked and the round pending for good.",
    },
    {
        "id": "nsh03-lost-leaf-restored-job-is-not-queued-again",
        "invariants": ["NSH-03"],
        "path": STORE,
        "old": REQUEUE,
        "new": "            pass\n",
        "tests": [RESTORED],
        "why": "A job parked for an object that reads again stays parked and pins its round.",
    },
    {
        "id": "nsh03-lost-leaf-republished-object-still-counts-as-lost",
        "invariants": ["NSH-03"],
        "path": STORE,
        "old": SAME_OBJECT,
        "new": "",
        "tests": [REPUBLISHED],
        "why": "A member that published its leaves again keeps a retired round for an object its "
        "audit would not read.",
    },
    {
        "id": "nsh03-lost-leaf-requeue-resets-a-leased-job",
        "invariants": ["NSH-03"],
        "path": STORE,
        "old": UNGUARDED,
        "new": '"WHERE id=?",',
        "tests": [RESTORED],
        "why": "The record of a healed loss puts a job that was leased since back in the queue at "
        "every push.",
    },
    {
        "id": "nsh03-lost-leaf-park-counts-the-audit-as-settled",
        "invariants": ["NSH-03"],
        "path": STORE,
        "old": LOST_FAULT,
        "new": COUNTED + LOST_FAULT,
        "tests": [REFUSED],
        "why": "A parked job counts as an audit nobody owes, so the round could finish with one "
        "outstanding.",
    },
    {
        "id": "nsh03-lease-tx-commits-every-refusal",
        "invariants": ["NSH-03", "ES-08"],
        "path": STORE,
        "old": "            except CommittedRefusal as error:\n                refusal = error\n",
        "new": "            except ChallengeError as error:\n                refusal = error\n",
        "tests": [KEPT],
        "why": "Any refusal of a lease request keeps the writes of its transaction, the Receipt of "
        "a refused ask included.",
    },
    {
        "id": "nsh03-lost-leaf-retired-round-takes-its-job-back",
        "invariants": ["NSH-03"],
        "path": STORE,
        "old": "        if not status.applied:  # a retired round takes no parked job back\n",
        "new": "        if True:  # a retired round takes no parked job back\n",
        "tests": [REVIVED],
        "why": "A retired round that an open dispute keeps open gets its parked job back to lease.",
    },
    {
        "id": "nsh03-lost-leaf-retirement-is-forgotten-once-asked",
        "invariants": ["NSH-03"],
        "path": STORE,
        "old": "                lost_leaves=bool(lost),\n",
        "new": FORGOTTEN,
        "tests": [WAITS],
        "why": "A retirement the exit had to postpone is forgotten: the round starves on.",
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
        "old": NO_SHOW_EVIDENCE,
        "new": '        if stage == "commit":\n            return\n' + NO_SHOW_EVIDENCE,
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
        "path": STORE,
        "old": NO_SHOW_EVIDENCE,
        "new": '        if stage == "delta":\n            return\n' + NO_SHOW_EVIDENCE,
        "tests": [NO_SHOW_E2E + "test_a_member_that_commits_but_never_uploads_a_delta_is_left_out"],
        "why": "An audited member whose delta never arrives stops the aggregation of the round.",
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
