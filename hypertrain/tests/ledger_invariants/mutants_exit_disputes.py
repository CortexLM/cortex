"""Mutants of the exit_disputes package: one realistic regression each of the dispute exit.

Run: scripts/check_ledger_invariant_mutants.py --only <id> ... (see its --help).
"""

from __future__ import annotations

FILE = "tests/ledger_invariants/test_exit_disputes.py::"
CLOSE = FILE + "test_a_contest_nobody_can_decide_is_closed_by_the_beacon_and_frees_every_hold"
HOLD = FILE + "test_the_hold_of_a_round_is_the_count_of_its_open_disputes_after_every_closing_route"
QUEUE = FILE + "test_contests_nobody_answers_free_the_32_slot_queue_for_the_next_honest_contest"
# Tests of the base that already pin an invariant of this package: the mutants below name them.
LIFT = "tests/challenge/test_dispute_exit_v2.py::test_closed_contest_lifts_the_suspension"
GRACE = "tests/challenge/test_dispute_exit_v2.py::test_blocked_cause_table[in-grace]"
TIMEOUT = "tests/challenge/test_disputes_v2.py::test_timeout_when_authenticated_delivery_healthy"
ORDER = "tests/challenge/test_dispute_lock_order_v2.py::"

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
        "tests": [TIMEOUT + "[False]", TIMEOUT + "[True]"],
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
    {
        "id": "exit-skips-settled-rounds",
        "invariants": ["DIA-14", "FIN-20"],
        "path": STORE,
        "old": (
            "                cause = disputes.blocked_cause(turn, now)\n"
            "                if cause is None:\n"
            "                    continue\n"
        ),
        "new": (
            "                cause = disputes.blocked_cause(turn, now)\n"
            "                if cause is None or self._record_v2(\n"
            '                    run_id, "finality", str(turn.contest.w)\n'
            '                ).get("disposition") == "SETTLED":\n'
            "                    continue\n"
        ),
        "tests": [CLOSE + "[never-answered-round-settled]"],
        "why": "The exit leaves the contests of a settled round open, so round 2 stays blocked.",
    },
    {
        "id": "exit-skips-turns-the-party-acknowledged",
        "invariants": ["DIA-14", "DIA-15"],
        "path": STORE,
        "old": (
            "                cause = disputes.blocked_cause(turn, now)\n"
            "                if cause is None:\n"
            "                    continue\n"
        ),
        "new": (
            "                cause = disputes.blocked_cause(turn, now)\n"
            "                if cause is None or self._db.execute(\n"
            '                    "SELECT 1 FROM dispute_acks_v2 LIMIT 1"\n'
            "                ).fetchone():\n"
            "                    continue\n"
        ),
        "tests": [CLOSE + "[acked-then-silent-round-pending]"],
        "why": "A turn whose party acknowledged it is left to the operator's timeout for ever.",
    },
    {
        "id": "exit-grace-boundary-inclusive",
        "invariants": ["DIA-15"],
        "path": DISPUTES,
        "old": "        return EXCLUSION_ABANDONED if now > turn.turn_deadline + grace else None",
        "new": "        return EXCLUSION_ABANDONED if now >= turn.turn_deadline + grace else None",
        "tests": [CLOSE + "[never-answered-round-pending]", GRACE],
        "why": "The last round of the operator's timeout window already belongs to the beacon.",
    },
    {
        "id": "exit-stops-after-one-closure",
        "invariants": ["DS-06", "DIA-14"],
        "path": STORE,
        "old": (
            "                        self._recount_disputes_v2(run_id, turn.contest.w)\n"
            "                except Exception as error:"
            "  # one poisoned dispute must not stop the beacon\n"
        ),
        "new": (
            "                        self._recount_disputes_v2(run_id, turn.contest.w)\n"
            "                        break\n"
            "                except Exception as error:"
            "  # one poisoned dispute must not stop the beacon\n"
        ),
        "tests": [QUEUE],
        "why": "One dispute is closed per beacon round: 32 stalled contests pin the queue for 32.",
    },
    {
        "id": "exit-lifts-the-audit-exclusion",
        "invariants": ["DIA-14"],
        "path": STORE,
        "old": (
            "                        disputes.settle_lock(turn.dispute_id, now=now)\n"
            "                        self._recount_disputes_v2(run_id, turn.contest.w)\n"
        ),
        "new": (
            "                        disputes.settle_lock(turn.dispute_id, now=now)\n"
            "                        self._recount_disputes_v2(run_id, turn.contest.w)\n"
            "                        self._db.execute(\n"
            '                            "DELETE FROM records_v2 WHERE "\n'
            "                            \"kind='excluded' AND id=?\",\n"
            '                            (f"{turn.contest.w}:{turn.contest.miner}",),\n'
            "                        )\n"
        ),
        "tests": [CLOSE + "[never-answered-round-pending]"],
        "why": "The server lifts the audit's exclusion when it closes a contest nobody decided.",
    },
    {
        "id": "resolution-route-decrements-the-hold",
        "invariants": ["DIA-16", "DIB-18"],
        "path": STORE,
        "old": (
            '                        if action in ("dispute", "resolution"):\n'
            "                            self._recount_disputes_v2(run_id, "
            'result["contest"]["w"])\n'
        ),
        "new": (
            '                        if action == "dispute":\n'
            "                            self._recount_disputes_v2(run_id, "
            'result["contest"]["w"])\n'
            '                        if action == "resolution":\n'
            '                            w = result["contest"]["w"]\n'
            '                            hold = self._record_v2(run_id, "finality", str(w))\n'
            '                            hold["unresolved_disputes"] -= 1\n'
            '                            self._put_record_v2(run_id, "finality", str(w), hold)\n'
        ),
        "tests": [HOLD],
        "why": "A resolution lowers the hold by one, not a recount: a replay subtracts twice.",
    },
    {
        "id": "poll-holds-the-condition-while-it-reads",
        "invariants": ["DIA-22", "DS-04"],
        "path": DISPUTES,
        "old": "            found = available()\n",
        "new": "            with self.condition:\n                found = available()\n",
        "tests": [
            ORDER
            + "test_a_mutator_that_notifies_under_the_store_lock_does_not_deadlock_a_parked_poller"
        ],
        "why": "The long poll asks for the store lock inside the condition again (ADR 0011 pt 7).",
    },
    {
        "id": "poll-reads-the-table-inside-the-leaf",
        "invariants": ["DIA-22", "DS-04"],
        "path": DISPUTES,
        "old": "            found = available()\n",
        "new": "            with self._leaf():\n                found = available()\n",
        "tests": [
            ORDER
            + "test_a_poller_parked_on_the_condition_is_woken_by_a_mutator_holding_the_store_lock"
        ],
        "why": "The long poll reads the event table in the leaf: the order check fires, no event.",
    },
    {
        "id": "notify-does-not-wake-the-poll",
        "invariants": ["DIA-22", "DS-04"],
        "path": DISPUTES,
        "old": "            self._beats += 1\n",
        "new": "            pass\n",
        "tests": [
            ORDER
            + "test_a_poller_parked_on_the_condition_is_woken_by_a_mutator_holding_the_store_lock"
        ],
        "why": "A dispute write wakes nobody: a parked long poll sleeps until its own timeout.",
    },
    {
        "id": "exit-seizes-the-bond-of-a-stalled-contester",
        "invariants": ["DIA-14", "DS-03"],
        "path": STORE,
        "old": (
            "                            admission.resolve_pending("
            "turn.contest.miner, turn.dispute_id, now=now)\n"
        ),
        "new": (
            "                            admission.resolve_pending("
            "turn.contest.miner, turn.dispute_id, now=now)\n"
            "                            record = admission.store.record(turn.contest.miner)\n"
            "                            admission.seize_bond(\n"
            "                                record.admission_id,\n"
            '                                reason="AUDIT_MISMATCH",\n'
            "                                evidence_hash=turn.dispute_id,\n"
            "                                now=now,\n"
            "                            )\n"
        ),
        "tests": [
            FILE + "test_the_beacon_exit_never_sanctions_the_bonded_miner_who_opened_the_contest"
        ],
        "why": "The exit treats a stalled contest as a confirmed audit mismatch: its bond burns.",
    },
    {
        "id": "exit-burns-the-admission-bond-of-a-stalled-contester",
        "invariants": ["DIA-14", "DS-03"],
        "path": STORE,
        "old": (
            "                            admission.resolve_pending("
            "turn.contest.miner, turn.dispute_id, now=now)\n"
        ),
        "new": (
            "                            admission.resolve_pending("
            "turn.contest.miner, turn.dispute_id, now=now)\n"
            "                            record = admission.store.record(turn.contest.miner)\n"
            "                            admission.seize_bond(\n"
            "                                record.admission_id,\n"
            '                                reason="NO_SHOW",\n'
            "                                evidence_hash=turn.dispute_id,\n"
            "                                now=now,\n"
            "                            )\n"
        ),
        "tests": [
            FILE + "test_the_beacon_exit_never_sanctions_the_bonded_miner_who_opened_the_contest"
        ],
        "why": "The exit burns the admission bond alone, with no strike and no state change.",
    },
]
