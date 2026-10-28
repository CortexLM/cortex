"""Mutants that re-open what D70 closed: the exit reaper retires a round that nobody can carry
(e06cb795, 523d585d, ADR 0011 part 8 decisions 7 to 11).

A round whose members cannot reach a tape (the survivors of its commit and delta windows cannot
carry one update) used to stay open for ever: the opening of the next round read the applied state
of this one, which never exists (409 'missing accepted applied: 0'), and no later round was ever
credited. `ChallengeStore._reap_round_v2` now asks the planner (`exit_v2.plan_round`) and hands its
answer to `_retire_or_wait_v2`, which retires the round as a null round: a null tape that carries
the previous state, the anchors of the roster carried to the next round, the round closed. The two
pins of FIN-22 were strict expected failures until audit-integrity-2 landed; each mutant reverts
one behaviour in a private copy of src, so that the runner proves a pin fails without it:

- the pin of test_gaps_finality (a member that never committed leaves three survivors who cannot
  carry the round) fails when the reaper only marks the round and never retires it;
- the pin of test_exit_roster (a round whose every audit job ran out of both windows) fails when
  the planner never finds that the survivors cannot carry, and when a retirement carries no anchor
  to the next round (the next opening is refused for the anchor it cannot restore).

The kills are assertions of the pins: the opening of the round after the retired one answers 409
instead of 200, or refuses for the missing anchor. Other reverts of the same retirement (the null
tape not written, the round not closed, the survivors not recomputed by the planner) fail the pins
through the controls that state the scenario, not through the asserted behaviour, and are not
listed.

What no pin or mutant covers here: the retirement for AUDIT_WINDOW_MISSED (a round nobody
scheduled audits for: the same pins retire it through INFEASIBLE first), the reaper that waits when
the primitive refuses (an earlier round still open) and journals any other failure, the check of
the survivors against the facts of the round in `_retire_round_v2`, the `retired` record that
answers a second retirement, and the survivors of the delta window (a member that committed and
delivered nothing). tests/challenge/test_exit_reaper_v2.py and test_retire_round_v2.py hold them.

Run from hypertrain/: .venv/bin/python scripts/check_ledger_invariant_mutants.py --only <ids>
"""

from __future__ import annotations

STORE = "src/hypertrain/challenge/store.py"
EXIT = "src/hypertrain/challenge/exit_v2.py"

NEVER_COMMITTED = (
    "tests/ledger_invariants/test_gaps_finality.py::"
    "test_a_round_with_a_member_that_never_committed_can_be_left_behind"
)
NOBODY_AUDITED = (
    "tests/ledger_invariants/test_exit_roster.py::"
    "test_a_round_after_a_round_nobody_audited_is_opened_and_credited"
)

MUTANTS = [
    {
        "id": "closed-d70-the-reaper-only-marks-a-round-that-cannot-go-on",
        "invariants": ["FIN-22", "R-C2", "DSP-01"],
        "path": STORE,
        "old": "            self._retire_or_wait_v2(run_id, w, plan.retire, plan.survivors)\n",
        "new": (
            '            self._put_changed_v2(run_id, "round-exit", str(w), '
            '{"state": plan.retire})\n'
        ),
        "tests": [NEVER_COMMITTED],
        "why": (
            "The planner says the round cannot go on and the reaper writes a marker and retires "
            "nothing, so the round stays open and the opening of the next one is refused for the "
            "applied state this one never reaches."
        ),
    },
    {
        "id": "closed-d70-the-planner-never-finds-that-the-survivors-cannot-carry",
        "invariants": ["FIN-22", "R-C2"],
        "path": EXIT,
        "old": '        retire = "INFEASIBLE"\n',
        "new": "        retire = None\n",
        "tests": [NOBODY_AUDITED],
        "why": (
            "A round whose survivors cannot carry one update is never named for retirement, so a "
            "round nobody audited stays open and no later round is opened or credited."
        ),
    },
    {
        "id": "closed-d70-a-retirement-carries-no-anchor-to-the-next-round",
        "invariants": ["FIN-22", "R-C2"],
        "path": STORE,
        "old": (
            "        for hotkey, anchor in carried.items():\n"
            '            self._put_record_v2(run_id, "anchor", f"{w}:{hotkey}", anchor)\n'
        ),
        "new": "",
        "tests": [NOBODY_AUDITED],
        "why": (
            "The retired round is closed with its null tape but the anchors of its roster are "
            "not carried, so the opening of the next round cannot restore them and is refused."
        ),
    },
]
