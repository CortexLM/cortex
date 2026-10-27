"""Mutants of the exit_disputes package: one realistic regression each of the dispute exit.

Run: scripts/check_ledger_invariant_mutants.py --only <id> ... (see its --help).
"""

from __future__ import annotations

FILE = "tests/ledger_invariants/test_exit_disputes.py::"
CLOSE = FILE + "test_a_contest_nobody_can_decide_is_closed_by_the_beacon_and_frees_every_hold"
TIMEOUT = FILE + "test_the_timeout_route_needs_the_contesters_receipt_but_the_beacon_exit_does_not"
HOLD = FILE + "test_the_hold_of_a_round_is_the_count_of_its_open_disputes_after_every_closing_route"
QUEUE = FILE + "test_contests_nobody_answers_free_the_32_slot_queue_for_the_next_honest_contest"
LIFT = "tests/challenge/test_dispute_exit_v2.py::test_closed_contest_lifts_the_suspension"

DISPUTES = "src/hypertrain/challenge/disputes_v2.py"
STORE = "src/hypertrain/challenge/store.py"
FINALITY = "src/hypertrain/challenge/finality_v2.py"

MUTANTS = [
    {
        "id": "exit-ignores-the-dead-end",
        "invariants": ["DIA-14", "DIB-03", "FIN-20", "DS-03"],
        "path": DISPUTES,
        "old": "            return None if turn.at_final_op else EXCLUSION_FROZEN",
        "new": "            return None",
        "tests": [CLOSE + "[paused-step-round-pending]"],
        "why": "A paused turn off the final op is no longer a dead end: it waits for the horizon.",
    },
    {
        "id": "exit-closes-at-the-horizon-round",
        "invariants": ["DIA-14", "DIB-03"],
        "path": DISPUTES,
        "old": "        if now > turn.absolute_deadline:\n            return EXCLUSION_HORIZON",
        "new": "        if now >= turn.absolute_deadline:\n            return EXCLUSION_HORIZON",
        "tests": [CLOSE + "[paused-final-op-round-pending]"],
        "why": "The server closes at the last round the referee may still resolve in.",
    },
    {
        "id": "exit-grace-dropped",
        "invariants": ["DIA-14", "DIA-15", "DS-03"],
        "path": DISPUTES,
        "old": "        return EXCLUSION_ABANDONED if now > turn.turn_deadline + grace else None",
        "new": "        return EXCLUSION_ABANDONED if now > turn.turn_deadline else None",
        "tests": [CLOSE + "[never-answered-round-pending]"],
        "why": "The abandoned turn is closed at its deadline, the timeout window is gone.",
    },
    {
        "id": "exit-grace-tripled",
        "invariants": ["DIA-14", "DIB-03", "FIN-20"],
        "path": DISPUTES,
        "old": "ABANDONMENT_GRACE = 1",
        "new": "ABANDONMENT_GRACE = 3",
        "tests": [CLOSE + "[never-answered-round-pending]"],
        "why": "A silent contest keeps the round blocked three times longer than documented.",
    },
    {
        "id": "exit-keeps-the-bond-locked",
        "invariants": ["DIA-14", "DIB-03", "FIN-20", "DS-03"],
        "path": STORE,
        "old": "                        disputes.settle_lock(turn.dispute_id, now=now)\n"
        "                        self._recount_disputes_v2(run_id, turn.contest.w)\n",
        "new": "                        self._recount_disputes_v2(run_id, turn.contest.w)\n",
        "tests": [CLOSE + "[never-answered-round-pending]"],
        "why": "The exit resolves the dispute but forgets to settle the contest bond.",
    },
    {
        "id": "exit-keeps-the-round-blocked",
        "invariants": ["DIA-14", "DIB-03", "FIN-20", "DS-03", "DIA-16"],
        "path": STORE,
        "old": "                        self._recount_disputes_v2(run_id, turn.contest.w)\n",
        "new": "                        pass\n",
        "tests": [CLOSE + "[never-answered-round-pending]"],
        "why": "The exit closes the dispute without recounting the hold on its round.",
    },
    {
        "id": "exit-leaves-the-miner-suspended",
        "invariants": ["DIA-14", "DS-03", "DS-02"],
        "path": STORE,
        "old": "admission.resolve_pending(turn.contest.miner, turn.dispute_id, now=now)",
        "new": "pass",
        "tests": [CLOSE + "[never-answered-round-pending]"],
        "why": "A closed contest keeps its opener SUSPENDED with a pending dispute.",
    },
    {
        "id": "exit-is-never-run",
        "invariants": ["DIA-14", "DIB-03", "FIN-20", "DS-03", "DS-06"],
        "path": STORE,
        "old": "            self._close_blocked_disputes_v2(run_id)\n",
        "new": "",
        "tests": [CLOSE + "[paused-step-round-pending]"],
        "why": "push_beacon no longer calls the reaper, so nothing closes a blocked contest.",
    },
    {
        "id": "exit-names-the-contester-loser",
        "invariants": ["DIA-14", "DIA-15", "DS-03"],
        "path": DISPUTES,
        "old": "loser=None,\n" + " " * 24 + "evidence_hash=evidence.digest(),",
        "new": "loser=turn.contest.miner,\n" + " " * 24 + "evidence_hash=evidence.digest(),",
        "tests": [CLOSE + "[acked-then-silent-round-pending]"],
        "why": "The infrastructure exclusion blames the contester for a stall nobody proved.",
    },
    {
        "id": "exit-forgets-to-pause-an-abandoned-turn",
        "invariants": ["DIA-14", "DIB-03", "FIN-20"],
        "path": STORE,
        "old": "disputes.pause(turn.dispute_id)  # exclude() is due only when paused",
        "new": "pass  # exclude() is due only when paused",
        "tests": [CLOSE + "[never-answered-round-pending]"],
        "why": "exclude() refuses an unpaused turn before its horizon: the contest stays open.",
    },
    {
        "id": "timeout-convicts-without-a-receipt",
        "invariants": ["DIA-15"],
        "path": DISPUTES,
        "old": "            if not delivered:\n",
        "new": "            if False:\n",
        "tests": [TIMEOUT],
        "why": "The witnessed timeout convicts a party whose receipt of the turn was never signed.",
    },
    {
        "id": "settle-ignores-the-dispute-hold",
        "invariants": ["DIA-14", "DIA-16", "DS-03"],
        "path": FINALITY,
        "old": "        or status.unresolved_disputes\n",
        "new": "",
        "tests": [CLOSE + "[paused-step-round-pending]"],
        "why": "A round settles while one of its disputes is still open.",
    },
    {
        "id": "dispute-route-skips-the-recount",
        "invariants": ["DIA-16", "DIB-18"],
        "path": STORE,
        "old": '                        if action in ("dispute", "resolution"):',
        "new": '                        if action == "resolution":',
        "tests": [HOLD],
        "why": "Opening a contest no longer raises the hold on its round.",
    },
    {
        "id": "resolution-route-skips-the-recount",
        "invariants": ["DIA-16", "DIB-18"],
        "path": STORE,
        "old": '                        if action in ("dispute", "resolution"):',
        "new": '                        if action == "dispute":',
        "tests": [HOLD],
        "why": "The referee's resolution closes a dispute without lowering the hold.",
    },
    {
        "id": "timeout-route-skips-the-recount",
        "invariants": ["DIA-16", "DIB-18"],
        "path": STORE,
        "old": "                self._recount_disputes_v2(run_id, result.contest.w)",
        "new": "                pass",
        "tests": [HOLD],
        "why": "The operator's timeout closes a dispute without lowering the hold.",
    },
    {
        "id": "recount-counts-closed-disputes",
        "invariants": ["DIA-16", "DIB-18"],
        "path": STORE,
        "old": "            \"WHERE json_extract(turn,'$.contest.w')=? \"\n"
        "            \"AND json_extract(turn,'$.resolution') IS NULL\",\n"
        "            (w,),",
        "new": "            \"WHERE json_extract(turn,'$.contest.w')=?\",\n            (w,),",
        "tests": [HOLD],
        "why": "The hold counts every dispute of the round, resolved ones included.",
    },
    {
        "id": "recount-counts-later-rounds",
        "invariants": ["DIA-16", "DIB-18"],
        "path": STORE,
        "old": "            \"WHERE json_extract(turn,'$.contest.w')=? \"\n"
        "            \"AND json_extract(turn,'$.resolution') IS NULL\",\n"
        "            (w,),",
        "new": "            \"WHERE json_extract(turn,'$.contest.w')>=? \"\n"
        "            \"AND json_extract(turn,'$.resolution') IS NULL\",\n"
        "            (w,),",
        "tests": [HOLD],
        "why": "The hold of a round also counts the open disputes of the rounds after it.",
    },
    {
        "id": "queue-counts-closed-disputes",
        "invariants": ["DS-06"],
        "path": DISPUTES,
        "old": "sum(t.resolution is None for t in active) >= self.policy.max_open",
        "new": "len(active) >= self.policy.max_open",
        "tests": [QUEUE],
        "why": "Closed disputes keep their slot, so 32 stalled contests fill the queue for good.",
    },
    {
        "id": "exit-grace-dropped-closes-the-timeout-window",
        "invariants": ["DIA-15"],
        "path": DISPUTES,
        "old": "        return EXCLUSION_ABANDONED if now > turn.turn_deadline + grace else None",
        "new": "        return EXCLUSION_ABANDONED if now > turn.turn_deadline else None",
        "tests": [TIMEOUT],
        "why": "The beacon closes the contest before the operator's witnessed timeout window ends.",
    },
    {
        "id": "exit-leaves-the-miner-suspended-existing-pin",
        "invariants": ["DS-02", "DIA-14"],
        "path": STORE,
        "old": "admission.resolve_pending(turn.contest.miner, turn.dispute_id, now=now)",
        "new": "pass",
        "tests": [LIFT],
        "why": "A closed contest keeps its opener SUSPENDED (pinned by the existing exit test).",
    },
    {
        "id": "exit-is-never-run-queue",
        "invariants": ["DS-06"],
        "path": STORE,
        "old": "            self._close_blocked_disputes_v2(run_id)\n",
        "new": "",
        "tests": [QUEUE],
        "why": "push_beacon no longer calls the reaper: 32 stalled contests pin the queue.",
    },
    {
        "id": "resolution-route-keeps-the-bond-locked",
        "invariants": ["DIA-14", "DIA-16"],
        "path": STORE,
        "old": 'disputes.settle_lock(result["dispute_id"], now=now)',
        "new": "pass",
        "tests": [HOLD],
        "why": "The referee's resolution closes the dispute but never settles the contest bond.",
    },
    {
        "id": "timeout-route-keeps-the-bond-locked",
        "invariants": ["DIA-14", "DIA-16"],
        "path": STORE,
        "old": "disputes.settle_lock(dispute_id, now=self._now(self._db))",
        "new": "pass",
        "tests": [HOLD],
        "why": "The operator's timeout closes the dispute but never settles the contest bond.",
    },
]
