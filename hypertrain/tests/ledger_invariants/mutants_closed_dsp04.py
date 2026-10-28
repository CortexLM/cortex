"""Mutants that re-open what DSP-04 closed: the contest of a verdict that upholds the work is
refused (b0ac0ab5, ADR 0011 part 8).

Only the miner itself can contest its verdict, so the contest of a MATCH could only suspend the
contestant and pin its round: for the price of a refundable bond a member could take itself out of
its round at the moment it chose. `ChallengeStore._contest_v2`, which `DisputesV2` calls to load the
accepted contest, now answers 409 NOTHING_TO_CONTEST when the audit lease of (run, round, miner)
holds MATCH. The two pins were strict expected failures until audit-integrity-2 landed; each
mutant reverts one behaviour in a private copy of src, so that the runner proves a pin fails
without it:

- DIA-06 (the contest of a MATCH verdict is a client error and leaves the miner as it was) fails
  when the check is gone and when the callback of the dispute service loads the bare record again;
- DIB-04 + DS-02 (a contest never wedges the aggregation of its opener's round) fails when the
  check reads the lease of another round, which finds no row and lets the contest through. Both pins
  fail under each of the three, checked by hand without -x; the runner stops at the first.

What no pin or mutant covers here: the refusal of the contest of a verdict whose sanction is
already applied (SANCTION_APPLIED, a few lines under the check, from the D81 lot).

Run from hypertrain/: .venv/bin/python scripts/check_ledger_invariant_mutants.py --only <ids>
"""

from __future__ import annotations

STORE = "src/hypertrain/challenge/store.py"

EXIT = "tests/ledger_invariants/test_exit_disputes.py::"
REFUSED = EXIT + "test_a_contest_on_a_matching_verdict_is_refused_before_it_suspends_anyone"
NOT_WEDGED = EXIT + "test_a_contest_on_a_matching_verdict_never_wedges_the_aggregation_of_its_round"

MUTANTS = [
    {
        "id": "closed-dsp04-a-contest-on-a-matching-verdict-is-admitted",
        "invariants": ["DIA-06", "DIB-04", "DS-02"],
        "path": STORE,
        "old": '        if row is not None and row[0] == "MATCH":\n',
        "new": "        if False:\n",
        "tests": [REFUSED],
        "why": (
            "The contest of a verdict that upholds the work is taken, so the miner that filed it "
            "is suspended and the aggregation of its round answers 403 for everybody."
        ),
    },
    {
        "id": "closed-dsp04-the-refusal-reads-the-lease-of-another-round",
        "invariants": ["DIA-06", "DIB-04", "DS-02"],
        "path": STORE,
        "old": (
            "            (run_id, contest.w, contest.miner),\n"
            "        ).fetchone()\n"
            "        if row is not None"
        ),
        "new": (
            "            (run_id, contest.w + 1, contest.miner),\n"
            "        ).fetchone()\n"
            "        if row is not None"
        ),
        "tests": [NOT_WEDGED],
        "why": (
            "The check looks for the lease of the round after the contested one, finds no row "
            "and lets the contest through, so a member can pin its own round."
        ),
    },
    {
        "id": "closed-dsp04-the-dispute-service-loads-the-bare-contest-record",
        "invariants": ["DIA-06", "DIB-04", "DS-02"],
        "path": STORE,
        "old": "                contest_of=lambda h: self._contest_v2(run_id, h),\n",
        "new": (
            "                contest_of=lambda h: Contest.model_validate("
            'self._record_v2(run_id, "contest", h)),\n'
        ),
        "tests": [REFUSED],
        "why": (
            "The dispute service is wired to the accepted record without the check, as before "
            "the fix, so no contest is ever refused for the verdict it contests."
        ),
    },
]
