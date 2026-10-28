"""Mutants that re-open what D63 closed: the tiers of the sanction of a no-show (ADR 0010 and
ADR 0011 decision 12, 567fa939, 5adfb637, 4909bf35).

D44 (8c821ded) suspended an admission on its first no-show, and a suspension has no route back.
D63 gives the sanction two tiers at `Admission.seize_bond`: a first no-show adds a strike, burns the
bond and puts the admission on a fresh probation; a second one for other evidence inside the strike
window suspends it; a no-show that reaches an admission already suspended only adds its strike.
The five pins were strict expected failures while the sanction suspended at once; they now hold,
and each mutant undoes one behaviour in a private copy of src so that the runner proves a pin fails
without it:

- NSH-01 (a strike, the bond and PROBATION, not SUSPENDED) fails when the first tier is the old
  suspension and when a no-show books no strike;
- NSH-05 (a fresh probation: reason NO_SHOW, clean_count 0, no screen) fails when the probation
  keeps the trials of the ladder it interrupts and when it carries the reason of a resume;
- NSH-02 (a second silence inside the window suspends) fails when the window is closed at once;
- NSH-07 (a silence long after the last one is a first one again) fails when the window never
  closes;
- NSH-06 (a silence on a suspension keeps its state and its reason and adds its strike) fails when
  the suspension falls to the first tier and when the silence rewrites the reason.

What no pin or mutant covers here: the length of the window (four screens) and the inequality
window >= two ladders of D70 (4909bf35), pinned by tests/challenge/test_strike_window_v2.py and
tests/challenge/test_admission_bond_v2.py, which the lot does not mirror; the burn of the second
tier (the bond is already gone); the void of the trial left open by `_fresh_probation`, since the
silent round of the lot has none; the cause the roster exit names after each tier.

Run from hypertrain/: .venv/bin/python scripts/check_ledger_invariant_mutants.py --only <ids>
"""

from __future__ import annotations

ADMISSION = "src/hypertrain/challenge/admission.py"

R2 = "tests/ledger_invariants/test_exit_roster_2.py::"
FIRST_COSTS = R2 + "test_the_first_no_show_of_an_admission_costs_it_a_strike_and_its_bond"
FIRST_PROBATION = R2 + "test_the_first_no_show_of_an_admission_puts_it_on_probation"
SECOND_SUSPENDS = R2 + "test_a_second_no_show_inside_the_window_suspends_the_admission"
SUSPENSION_KEPT = R2 + "test_a_no_show_adds_its_strike_to_a_suspension_and_keeps_its_reason"
FIRST_AGAIN = R2 + "test_a_no_show_long_after_the_last_one_is_a_first_one_again"

FIRST_TIER = (
    '                elif reason == "NO_SHOW" and not self._repeat_no_show(\n'
    "                    admission_id, evidence_hash, now\n"
    "                ):\n"
    "                    self._fresh_probation(admission_id, reason, now)\n"
)
# The branch of a no-show on a suspended admission, up to its `pass` (the comment that follows it is
# not part of the snippet, so that rewording it breaks nothing).
KEPT_SUSPENSION = (
    '                if reason == "NO_SHOW" and r.state == "SUSPENDED":\n                    pass'
)
STRIKE = (
    "                self.store.db.execute(\n"
    '                    "UPDATE admissions_v2 SET strikes=strikes+1 WHERE admission_id=?",\n'
    "                    (admission_id,),\n"
    "                )\n"
)

MUTANTS = [
    {
        "id": "closed-d63-first-no-show-suspends",
        "invariants": ["NSH-01"],
        "path": ADMISSION,
        "old": FIRST_TIER,
        "new": FIRST_TIER.replace("elif reason ==", "elif False and reason =="),
        "tests": [FIRST_COSTS],
        "why": (
            "The first tier is gone: every no-show of an admission that is not suspended yet "
            "suspends it, as D44 did, and an identity that was only late once has no route back."
        ),
    },
    {
        "id": "closed-d63-first-no-show-books-no-strike",
        "invariants": ["NSH-01"],
        "path": ADMISSION,
        "old": STRIKE,
        "new": (
            '                if reason != "NO_SHOW":\n'
            + "".join("    " + line + "\n" for line in STRIKE.splitlines())
        ),
        "tests": [FIRST_COSTS],
        "why": (
            "A no-show burns the bond and changes the state but adds no strike, so the memory the "
            "second tier reads and the count a suspension carries never grow."
        ),
    },
    {
        "id": "closed-d63-first-no-show-keeps-the-trials-of-the-ladder",
        "invariants": ["NSH-05"],
        "path": ADMISSION,
        "old": "                    self._fresh_probation(admission_id, reason, now)\n",
        "new": (
            '                    self.store.transition(admission_id, "PROBATION", reason, now)\n'
        ),
        "tests": [FIRST_PROBATION],
        "why": (
            "The admission goes to PROBATION without the restart of its trials, so it keeps its "
            "clean count and its work screen and the way back through the trials is a formality."
        ),
    },
    {
        "id": "closed-d63-first-no-show-probation-carries-the-reason-of-a-resume",
        "invariants": ["NSH-05"],
        "path": ADMISSION,
        "old": "                    self._fresh_probation(admission_id, reason, now)\n",
        "new": (
            "                    self._fresh_probation("
            'admission_id, "RESUME_FRESH_PROBATION", now)\n'
        ),
        "tests": [FIRST_PROBATION],
        "why": (
            "The probation a no-show opens is booked as the one a resume opens, so the state "
            "reason misreports its cause."
        ),
    },
    {
        "id": "closed-d63-second-no-show-never-finds-the-first",
        "invariants": ["NSH-02"],
        "path": ADMISSION,
        "old": "        window = no_show_strike_window(self.policy.work_screen_epoch_rounds)\n",
        "new": "        window = 0\n",
        "tests": [SECOND_SUSPENDS],
        "why": (
            "The strike window is closed at once, so an earlier no-show never counts and a miner "
            "that is silent in every round only ever meets the first tier."
        ),
    },
    {
        "id": "closed-d63-the-strike-window-never-closes",
        "invariants": ["NSH-07"],
        "path": ADMISSION,
        "old": "                (admission_id, evidence_hash, now - window),\n",
        "new": "                (admission_id, evidence_hash, -1),\n",
        "tests": [FIRST_AGAIN],
        "why": (
            "An earlier no-show is a strike for ever, so a silence a hundred screens after the "
            "last one suspends the admission instead of starting a new probation."
        ),
    },
    {
        "id": "closed-d63-no-show-on-a-suspension-falls-to-the-first-tier",
        "invariants": ["NSH-06"],
        "path": ADMISSION,
        "old": KEPT_SUSPENSION,
        "new": KEPT_SUSPENSION.replace(
            'if reason == "NO_SHOW" and r.state == "SUSPENDED":', "if False:"
        ),
        "tests": [SUSPENSION_KEPT],
        "why": (
            "A no-show that reaches a suspended admission is judged as a first one and puts it "
            "back on probation: a silence lifts a suspension earned for something else."
        ),
    },
    {
        "id": "closed-d63-no-show-on-a-suspension-rewrites-its-reason",
        "invariants": ["NSH-06"],
        "path": ADMISSION,
        "old": KEPT_SUSPENSION,
        "new": KEPT_SUSPENSION.replace(
            "pass", 'self.store.transition(admission_id, "SUSPENDED", reason, now)'
        ),
        "tests": [SUSPENSION_KEPT],
        "why": (
            "A no-show on a suspended admission suspends it again for NO_SHOW, so the cause the "
            "suspension was earned for is overwritten."
        ),
    },
]
