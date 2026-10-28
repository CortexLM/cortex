"""Mutants of the disputes gap wave (test_gaps_disputes.py).

Two kinds of entry. A `gap-*` mutant is a realistic regression that a pin of the wave must catch.
A `fixproof-*` entry is the opposite: the smallest change of src that closes a hole the wave
records as a strict xfail. The runner calls it KILLED when the xfail then passes (XPASS(strict)
fails the test), which is the machine-checked proof that the hole is what its reason says and that
the desired assertion is reachable. When a maintainer applies the real fix, its `fixproof-*` entry
(whose snippet is then gone) and the xfail marker go together.

Run: scripts/check_ledger_invariant_mutants.py --only <id> ... (see its --help).
"""

from __future__ import annotations

FILE = "tests/ledger_invariants/test_gaps_disputes.py::"
AUDITOR_SILENT = FILE + (
    "test_a_contest_whose_auditor_never_answers_is_closed_as_abandoned_and_nobody_is_convicted"
)
TIMEOUT_FINAL = FILE + "test_a_dispute_closed_by_a_witnessed_timeout_takes_no_further_decision"
EXCLUSION_DUE = FILE + (
    "test_an_exclusion_is_due_for_a_paused_dispute_at_once_and_for_a_running_one_at_the_horizon"
)
NO_RESUME = FILE + (
    "test_an_admission_suspended_on_a_lost_or_open_dispute_cannot_be_resumed_by_the_operator"
)
ORIGIN_RELEASED = FILE + (
    "test_a_reward_origin_vests_at_its_maturity_once_its_miners_contest_is_closed_without_fraud"
)
WINDOWS = FILE + "test_a_rollback_of_one_window_is_not_blocked_by_a_failed_attempt_on_another"
FINAL_OP = FILE + (
    "test_a_dispute_paused_on_a_final_op_holds_both_parties_signed_claims_about_that_op"
)
RESOLUTION_ROUTE = FILE + (
    "test_a_resolution_route_refuses_a_signed_envelope_that_names_no_dispute"
)
SQUAT = FILE + "test_a_client_event_cannot_stop_the_beacon_from_closing_a_stuck_contest"
OWN_MATCH = FILE + (
    "test_a_member_contesting_its_own_matching_verdict_does_not_stop_the_round_of_the_others"
)
BACK_FROM_CONTEST = FILE + (
    "test_a_member_back_from_a_closed_contest_does_not_stop_the_round_of_the_others"
)
ROLLBACK_PAID = FILE + "test_a_round_opened_after_a_two_round_rollback_is_credited"
ORIGIN_HELD = FILE + (
    "test_only_the_origin_of_the_disputed_miner_is_held_and_nothing_is_paid_for_it"
)
BOND_BACK = FILE + "test_a_bond_beside_an_open_dispute_that_never_became_one_is_taken_back_alone"

DISPUTES = "src/hypertrain/challenge/disputes_v2.py"
STORE = "src/hypertrain/challenge/store.py"
ADMISSION = "src/hypertrain/challenge/admission.py"
ESCROW = "src/hypertrain/ledger/escrow_v2.py"

ABANDONED = "        return EXCLUSION_ABANDONED if now > turn.turn_deadline + grace else None"
EXCLUDE_LOSER = "loser=None,\n" + " " * 24 + "evidence_hash=evidence.digest(),"
TERMINAL_GUARD = (
    "            if turn.resolution is not None:\n"
    "                if model != turn.resolution:\n"
    '                    raise DisputeError("RESOLUTION_CONFLICT")\n'
    "                return turn\n"
)
PAUSE_GUARD = (
    "            if turn.resolution is not None:\n"
    '                raise DisputeError("CLOSED_DISPUTE")\n'
)
EXCLUSION_GUARD = (
    "            if not turn.paused and now < turn.absolute_deadline:\n"
    '                raise DisputeError("EXCLUSION_NOT_DUE")\n'
)
PAUSED_ONE_OP = '                            update["paused"] = hi - lo == 1\n'
RESOLUTION_LOOKUP = '            turn = self.get(str(env.body["dispute_id"]))\n'
SETTLEMENT_LOOKUP = (
    "        if row is None:\n"
    '            raise EscrowError("UNACCEPTED_SETTLEMENT")\n'
    "        return Settlement.model_validate_json(row[0])\n"
)
RESERVATION_QUERY = (
    '                    "SELECT 1 FROM records_v2 WHERE run_id=? '
    "AND kind='repair-reservation' \"\n"
    "                    \"AND json_extract(data,'$.state')='RUNNING'\",\n"
    "                    (run_id,),\n"
    "                ).fetchone()"
)
CONVICTION = '                            admission.settle_fraud(result["dispute_id"], now=now)\n'
UNFIT_CALL = "            self._exclude_unfit_members_v2(run_id, w)\n"
UNFIT_SUSPENDED = "            return record.state\n"
UNFIT_SUSPENDED_IS_FIT = (
    '            return None if record.state == "SUSPENDED" else record.state\n'
)
UNFIT_NOT_LIVE = '            return "NOT_LIVE"\n'
UNFIT_NOT_LIVE_IS_FIT = "            return None\n"
CLOSED_ROUND_WITHDRAWN = (
    "            closure = self.round_closure(w) if self.round_closure and w < below else None\n"
)
CLOSED_ROUND_NEVER_WITHDRAWN = "            closure = None\n"
ORIGIN_FOLLOWS_DISPUTE = (
    "        if row is None:\n"
    '            raise EscrowError("UNACCEPTED_SETTLEMENT")\n'
    "        settled = Settlement.model_validate_json(row[0])\n"
    "        origin = self._db.execute(\n"
    '            "SELECT reward_round,hotkey FROM escrow_origins WHERE origin=?", (reference,)\n'
    "        ).fetchone()\n"
    "        if origin is not None and origin[0] is not None:\n"
    "            for (text,) in self._db.execute(\n"
    '                "SELECT turn FROM disputes_v2 WHERE miner=?", (origin[1],)\n'
    "            ):\n"
    "                turn = json.loads(text)\n"
    '                if turn["contest"]["w"] != origin[0]:\n'
    "                    continue\n"
    '                ruling = turn["resolution"]\n'
    "                if ruling is None:\n"
    '                    return settled.model_copy(update={"unresolved": True})\n'
    '                if ruling["reason"] == "FRAUD" and ruling["loser"] == origin[1]:\n'
    '                    return settled.model_copy(update={"outcome": "FRAUD"})\n'
    "        return settled\n"
)
ANY_DISPUTE_HOLDS_ORIGIN = (
    "        if row is None:\n"
    '            raise EscrowError("UNACCEPTED_SETTLEMENT")\n'
    "        settled = Settlement.model_validate_json(row[0])\n"
    "        owner = self._db.execute(\n"
    '            "SELECT hotkey FROM escrow_origins WHERE origin=?", (reference,)\n'
    "        ).fetchone()\n"
    "        if owner is not None and self._db.execute(\n"
    '            "SELECT 1 FROM disputes_v2 WHERE miner=?", (owner[0],)\n'
    "        ).fetchone():\n"
    '            return settled.model_copy(update={"unresolved": True})\n'
    "        return settled\n"
)
UNBOUND_CONTEST_BOND = (
    "        if row is None or Turn.model_validate_json(row[0]).lock_id != lock.operation_id:\n"
    '            raise EscrowError("UNACCEPTED_SETTLEMENT")\n'
)
NEUTRAL_UNBOUND_BOND = (
    "        if row is None or Turn.model_validate_json(row[0]).lock_id != lock.operation_id:\n"
    '            zero = "00" * 32\n'
    "            return Settlement(\n"
    "                finality_hash=zero,\n"
    "                closed_dispute_root=zero,\n"
    "                release_beacon=0,\n"
    "                unresolved=False,\n"
    '                outcome="MATCH",\n'
    "                dispute_id=lock.dispute_id,\n"
    "            )\n"
)
ANY_RESERVATION_HOLDS = (
    '                    "SELECT 1 FROM records_v2 WHERE run_id=? '
    "AND kind='repair-reservation'\",\n"
    "                    (run_id,),\n"
    "                ).fetchone()"
)
EXPIRED_RESERVATION_FREE = (
    '                    "SELECT 1 FROM records_v2 WHERE run_id=? '
    "AND kind='repair-reservation' \"\n"
    "                    \"AND json_extract(data,'$.state')='RUNNING' \"\n"
    "                    \"AND json_extract(data,'$.absolute')>?\",\n"
    "                    (run_id, self._now(self._db)),\n"
    "                ).fetchone()"
)
SETTLEMENT_ID = (
    '                operation_id=sha256_hex(canonicalize([turn.dispute_id, "settle"])),'
)
UNGUESSABLE_SETTLEMENT = (
    '                operation_id=__import__("hmac").new(\n'
    '                    self.key._secret, canonicalize([turn.dispute_id, "settle"]), "sha256"\n'
    "                ).hexdigest(),"
)

MUTANTS = [
    {
        "id": "gap-grace-only-for-a-silent-miner",
        "invariants": ["DIA-14", "DIB-03", "DIA-15"],
        "path": DISPUTES,
        "old": ABANDONED,
        "new": "        if turn.expected_party != turn.contest.miner:\n            return None\n"
        + ABANDONED,
        "tests": [AUDITOR_SILENT + "[round-pending]"],
        "why": "A contest whose silent party is the auditor is never closed by the beacon exit.",
    },
    {
        "id": "gap-exit-blames-the-silent-auditor",
        "invariants": ["DIA-14", "DIA-15"],
        "path": DISPUTES,
        "old": EXCLUDE_LOSER,
        "new": "loser=turn.contest.auditor if turn.expected_party == turn.contest.auditor "
        "else None,\n" + " " * 24 + "evidence_hash=evidence.digest(),",
        "tests": [AUDITOR_SILENT + "[round-pending]"],
        "why": "The infrastructure exclusion convicts the auditor whose turn was abandoned.",
    },
    {
        "id": "gap-a-timeout-can-be-overwritten",
        "invariants": ["DIA-08", "DIB-13"],
        "path": DISPUTES,
        "old": TERMINAL_GUARD,
        "new": TERMINAL_GUARD.replace(
            "if turn.resolution is not None:",
            'if turn.resolution is not None and turn.resolution.reason != "PARTY_TIMEOUT":',
            1,
        ),
        "tests": [TIMEOUT_FINAL + "[miner]"],
        "why": "A referee resolution over registered evidence replaces a witnessed timeout.",
    },
    {
        "id": "gap-a-closed-dispute-can-be-paused",
        "invariants": ["DIA-08", "DIB-13"],
        "path": DISPUTES,
        "old": PAUSE_GUARD,
        "new": "",
        "tests": [TIMEOUT_FINAL + "[miner]"],
        "why": "pause() rewrites a dispute that a timeout already closed.",
    },
    {
        "id": "gap-exclusion-guard-deleted",
        "invariants": ["DIA-08", "CL-44"],
        "path": DISPUTES,
        "old": EXCLUSION_GUARD,
        "new": "",
        "tests": [EXCLUSION_DUE],
        "why": "exclude() closes a running dispute before its absolute horizon.",
    },
    {
        "id": "gap-exclusion-refused-at-the-horizon",
        "invariants": ["DIA-14", "CL-44"],
        "path": DISPUTES,
        "old": EXCLUSION_GUARD,
        "new": EXCLUSION_GUARD.replace("now < turn", "now <= turn", 1),
        "tests": [EXCLUSION_DUE],
        "why": "exclude() waits one round past the horizon that the exit relies on.",
    },
    {
        "id": "gap-a-paused-dispute-waits-for-the-horizon",
        "invariants": ["DIA-14", "CL-44"],
        "path": DISPUTES,
        "old": EXCLUSION_GUARD,
        "new": EXCLUSION_GUARD.replace("not turn.paused and ", "", 1),
        "tests": [EXCLUSION_DUE],
        "why": "A paused dispute nobody can answer keeps its bond locked until the horizon.",
    },
    {
        "id": "gap-resume-ignores-a-pending-dispute",
        "invariants": ["DIB-09", "DIA-23"],
        "path": ADMISSION,
        "old": '            if r.state != "SUSPENDED" or r.pending_dispute:\n',
        "new": '            if r.state != "SUSPENDED":\n',
        "tests": [NO_RESUME + "[fraud-settled]", NO_RESUME + "[unresolved]"],
        "why": "The operator's resume lifts an identity suspended on a lost or open dispute.",
    },
    {
        "id": "gap-any-dispute-holds-a-reward-origin-for-ever",
        "invariants": ["DIB-07", "DIB-17"],
        "path": STORE,
        "old": SETTLEMENT_LOOKUP,
        "new": ANY_DISPUTE_HOLDS_ORIGIN,
        "tests": [ORIGIN_RELEASED + "[MATCH]", ORIGIN_RELEASED + "[AUDITOR-LOSES]"],
        "why": "A reward origin stays pending for ever once its miner filed any contest at all.",
    },
    {
        "id": "gap-a-lost-contest-keeps-the-unvested-reward",
        "invariants": ["DIB-07", "DIB-17"],
        "path": ADMISSION,
        "old": '                    burn_pending=reason == "TRAINING_FRAUD",\n',
        "new": "                    burn_pending=False,\n",
        "tests": [ORIGIN_HELD + "[lost]"],
        "why": "The seizure after a lost contest burns the bond only, so the convicted miner's "
        "origin vests and is paid.",
    },
    {
        "id": "gap-the-resolution-route-never-convicts-the-miner",
        "invariants": ["DIB-07", "DIB-17"],
        "path": STORE,
        "old": CONVICTION,
        "new": "",
        "tests": [ORIGIN_HELD + "[lost]"],
        "why": "A referee ruling of fraud against the miner seizes nothing, so its origin is paid.",
    },
    {
        "id": "gap-a-completed-reservation-holds-the-gate",
        "invariants": ["DIB-22", "DIB-19"],
        "path": STORE,
        "old": RESERVATION_QUERY,
        "new": ANY_RESERVATION_HOLDS,
        "tests": [WINDOWS + "[completed-attempt-of-another-window]"],
        "why": "A finished rollback of one window turns away the rollback of the next one.",
    },
    {
        "id": "gap-the-aggregate-keeps-a-member-that-is-not-eligible-any-more",
        "invariants": ["DIB-04", "DIB-05"],
        "path": STORE,
        "old": UNFIT_CALL,
        "new": "",
        "tests": [OWN_MATCH, BACK_FROM_CONTEST],
        "why": "The aggregate no longer leaves out an audited member whose admission stopped "
        "qualifying it, so that one member answers 403 for the whole roster.",
    },
    {
        "id": "gap-a-suspended-contester-is-taken-for-fit-by-the-aggregate",
        "invariants": ["DIB-04", "DIB-05"],
        "path": STORE,
        "old": UNFIT_SUSPENDED,
        "new": UNFIT_SUSPENDED_IS_FIT,
        "tests": [OWN_MATCH],
        "why": "A member suspended by its own contest is not named unfit, so the real round of "
        "the four others is refused for it.",
    },
    {
        "id": "gap-a-member-back-on-probation-is-taken-for-fit-by-the-aggregate",
        "invariants": ["DIB-04", "DIB-05"],
        "path": STORE,
        "old": UNFIT_NOT_LIVE,
        "new": UNFIT_NOT_LIVE_IS_FIT,
        "tests": [BACK_FROM_CONTEST],
        "why": "A member back from a closed contest, on probation without a clean round, is not "
        "named unfit, so the round of the four others is refused for it.",
    },
    {
        "id": "gap-a-round-closed-by-a-rollback-blocks-the-credit-order",
        "invariants": ["DIB-21", "CRD-04"],
        "path": ESCROW,
        "old": CLOSED_ROUND_WITHDRAWN,
        "new": CLOSED_ROUND_NEVER_WITHDRAWN,
        "tests": [ROLLBACK_PAID],
        "why": "The ledger never withdraws a round that finality closed without a reward, so "
        "the round after a rollback is refused FINALIZE_ORDER for ever.",
    },
    # --- fix proofs: each closes a hole recorded by a strict xfail of the wave -----------------
    {
        "id": "fixproof-final-op-is-answered-on-the-op",
        "invariants": ["DIB-02", "DIA-14"],
        "path": DISPUTES,
        "old": PAUSED_ONE_OP,
        "new": PAUSED_ONE_OP.replace("== 1", "== 1 and tuple(turn.interval) == (lo, hi)"),
        "tests": [FINAL_OP],
        "why": "FIX PROOF: a dispute narrowed to one op asks both parties once more on that op.",
    },
    {
        "id": "fixproof-resolution-without-a-dispute-id-is-refused",
        "invariants": ["CL-59", "DIB-25"],
        "path": DISPUTES,
        "old": RESOLUTION_LOOKUP,
        "new": '            if "dispute_id" not in env.body:\n'
        '                raise DisputeError("RESOLUTION_TYPE_OR_TRANSCRIPT")\n' + RESOLUTION_LOOKUP,
        "tests": [RESOLUTION_ROUTE],
        "why": "FIX PROOF: an envelope without a dispute id is a refusal, not a server error.",
    },
    {
        "id": "fixproof-settlement-id-a-client-cannot-compute",
        "invariants": ["ESC-04", "DIA-14"],
        "path": DISPUTES,
        "old": SETTLEMENT_ID,
        "new": UNGUESSABLE_SETTLEMENT,
        "tests": [SQUAT],
        "why": "FIX PROOF: the release of a dispute bond is keyed by a secret no bystander has.",
    },
    {
        "id": "fixproof-a-reward-origin-follows-its-miners-dispute",
        "invariants": ["DIB-07", "DIB-17"],
        "path": STORE,
        "old": SETTLEMENT_LOOKUP,
        "new": ORIGIN_FOLLOWS_DISPUTE,
        "tests": [ORIGIN_HELD + "[open]"],
        "why": "FIX PROOF: a live reward origin is held while its miner's dispute of it runs.",
    },
    {
        "id": "fixproof-an-unbound-contest-bond-is-releasable",
        "invariants": ["DIA-25", "CL-15"],
        "path": DISPUTES,
        "old": UNBOUND_CONTEST_BOND,
        "new": NEUTRAL_UNBOUND_BOND,
        "tests": [BOND_BACK],
        "why": "FIX PROOF: a contest bond no dispute is bound to is settled as a neutral MATCH.",
    },
    {
        "id": "fixproof-an-expired-reservation-holds-nothing",
        "invariants": ["DIB-22", "DIB-19"],
        "path": STORE,
        "old": RESERVATION_QUERY,
        "new": EXPIRED_RESERVATION_FREE,
        "tests": [WINDOWS + "[failed-attempt-of-another-window]"],
        "why": "FIX PROOF: a repair reservation whose 200 rounds are over blocks no rollback.",
    },
]
