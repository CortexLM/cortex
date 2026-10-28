"""Mutants that re-open the holes roster-contract-b closed: each one brings back, in one place, a
behaviour a fix removed. 45b49351 refuses a rotation while a round lists the admission, a28532d4
leaves out an audited member that is no longer eligible and the round goes on, 62b56dcf makes
finalize pay the inputs of the applied tape.

The pins were strict expected failures until the fixes landed and now hold them. These mutants prove
it on the merged tree: reverting one behaviour of a fix in a private copy of src must fail the pins.

Two pins are held by two independent guards and no single-file mutant fails them: a member rotated
after its audits is refused by the guard of 45b49351 (admission.py) and, were the rotation accepted,
left out as ROTATED by a28532d4 (store.py). Taking either away leaves the round going on, so the
pins test_a_hotkey_rotated_after_the_audits_does_not_stall_the_round and the [rotation] case of
test_a_member_that_loses_its_standing_after_its_audit_never_freezes_the_round are named by no
mutant here. The opening check of 20b177ac (a roster screen that ends before the audit window) is
pinned by no test of the lot: every fixture of the lot sets a screen far in the future.

Run from hypertrain/: .venv/bin/python scripts/check_ledger_invariant_mutants.py --only <ids>
"""

from __future__ import annotations

ADMISSION = "src/hypertrain/challenge/admission.py"
STORE = "src/hypertrain/challenge/store.py"

ROSTER = "tests/ledger_invariants/test_exit_roster_2.py::"
LOCK = "tests/ledger_invariants/test_gaps_admission_lock_2.py::"
DISPUTES = "tests/ledger_invariants/test_gaps_disputes.py::"
FINALITY = "tests/ledger_invariants/test_gaps_finality_2.py::"

CANNOT_ROTATE = ROSTER + "test_an_admission_an_open_round_lists_cannot_rotate"
LAPSED_AT_AGGREGATE = ROSTER + "test_a_member_whose_screen_lapsed_is_excluded_and_the_round_goes_on"
LAPSE_CASE = (
    LOCK
    + "test_a_member_that_loses_its_standing_after_its_audit_never_freezes_the_round[screen-lapse]"
)
OWN_CONTEST = (
    DISPUTES
    + "test_a_member_contesting_its_own_matching_verdict_does_not_stop_the_round_of_the_others"
)
CLOSED_CONTEST = (
    DISPUTES + "test_a_member_back_from_a_closed_contest_does_not_stop_the_round_of_the_others"
)
LAPSED_AT_FINALIZE = (
    FINALITY
    + "test_a_member_whose_screen_lapsed_after_the_aggregation_does_not_freeze_the_finalize"
)
STAKE_FELL = LOCK + "test_a_finalize_after_the_aggregation_pays_a_member_whose_stake_fell"
SANCTIONED_AT_FINALIZE = (
    "tests/ledger_invariants/test_finality_2.py::"
    "test_a_member_sanctioned_after_the_aggregation_is_paid_for_the_round_it_was_verified_in"
)

MUTANTS = [
    {
        "id": "closed-rcb-rotation-allowed-while-a-round-lists-the-admission",
        "invariants": ["ROS-01", "RTA-01"],
        "path": ADMISSION,
        "old": (
            "            if self.open_rounds(r.admission_id):\n"
            '                raise AdmissionError("ROTATION_ROUND_OPEN")\n'
        ),
        "new": "",
        "tests": [CANNOT_ROTATE],
        "why": (
            "Without the guard an owner moves an admission to another hotkey while a round's roster"
            " still names the old one, so the rotation answers 200 instead of 409."
        ),
    },
    {
        "id": "closed-rcb-rotation-guard-counts-only-rounds-without-a-record",
        "invariants": ["ROS-01", "RTA-01"],
        "path": ADMISSION,
        "old": "            if status is None or not status.closed\n",
        "new": "            if status is None\n",
        "tests": [CANNOT_ROTATE],
        "why": (
            "A round that is open but already has its finality record no longer counts as open, so"
            " the rotation of an admission that a running round lists is accepted."
        ),
    },
    {
        "id": "closed-rcb-aggregate-does-not-leave-out-members-that-lost-their-standing",
        "invariants": ["AD-07", "A2W-21", "LCK-18", "DIB-04", "DIB-05"],
        "path": STORE,
        "old": "        self._exclude_unfit_members_v2(run_id, w, now)\n",
        "new": "",
        "tests": [LAPSE_CASE, LAPSED_AT_AGGREGATE, OWN_CONTEST, CLOSED_CONTEST],
        "why": (
            "Aggregate no longer leaves an audited member that is no longer eligible out of the"
            " round, so verified_inputs_v2 answers 403 for the whole roster and the run waits for a"
            " member that cannot come back."
        ),
    },
    {
        "id": "closed-rcb-a-lapsed-screen-is-no-cause-to-leave-the-round",
        "invariants": ["AD-07", "A2W-21", "LCK-18"],
        "path": STORE,
        "old": '        if now > record.screen_until:\n            return "SCREEN_LAPSED"\n',
        "new": "        if now > record.screen_until:\n            return None\n",
        "tests": [LAPSE_CASE, LAPSED_AT_AGGREGATE],
        "why": (
            "A member whose work screen ended after its audit is judged fit, stays in the roster"
            " and refuses the aggregation of the four others, and a lapsed screen is never renewed."
        ),
    },
    {
        "id": "closed-rcb-a-suspended-member-is-no-cause-to-leave-the-round",
        "invariants": ["DSP-04", "DIB-04"],
        "path": STORE,
        "old": (
            '        if record.state in ("BANNED", "SUSPENDED"):\n            return record.state\n'
        ),
        "new": ('        if record.state in ("BANNED", "SUSPENDED"):\n            return None\n'),
        "tests": [OWN_CONTEST],
        "why": (
            "A member suspended by its own contest is judged fit, stays in the roster and the"
            " aggregation of the four others answers 403 until a verdict the contest cannot win."
        ),
    },
    {
        "id": "closed-rcb-a-member-back-on-probation-is-no-cause-to-leave-the-round",
        "invariants": ["DSP-04", "DIB-04", "DIB-05"],
        "path": STORE,
        "old": '            return "NOT_LIVE"\n',
        "new": "            return None\n",
        "tests": [CLOSED_CONTEST],
        "why": (
            "A member that came back from a closed contest on probation without a clean trial is"
            " judged fit, so the aggregation of the four others answers 403 for it."
        ),
    },
    {
        "id": "closed-rcb-finalize-pays-the-live-verified-set-not-the-applied-tape",
        "invariants": [
            "A2W-21",
            "AD-07",
            "CL-06",
            "LCK-18",
            "LCK-01",
            "T-LOCK-04",
            "FIN-03",
            "FIN-18",
        ],
        "path": STORE,
        "old": (
            "            included = self._unconvicted_v2(\n"
            '                item["roster"]["hotkey"]\n'
            '                for item in self._record_v2(run_id, "tape-inputs", str(w))["inputs"]\n'
            "            )\n"
        ),
        "new": (
            "            included = self._unconvicted_v2(\n"
            "                x.roster.hotkey for x in self.verified_inputs_v2(run_id, w)\n"
            "            )\n"
        ),
        "tests": [LAPSED_AT_FINALIZE, STAKE_FELL, SANCTIONED_AT_FINALIZE],
        "why": (
            "Finalize re-derives the paid set from the live admissions, so a member that lost its"
            " place after the round was applied makes it answer 403 for ever and the round never"
            " settles."
        ),
    },
]
