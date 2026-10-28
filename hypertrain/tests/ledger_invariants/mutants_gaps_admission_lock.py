"""Mutants of the admission-lock gap tests: one small regression of src/hypertrain per pinned
defence of test_gaps_admission_lock.py. Holes (strict xfails) have no mutant: their proof is a
private-copy fix that turns them into XPASS(strict), recorded in the group's result rows.

Snippets are the exact source text, indentation included, so they sit at column 0 below.
Run from hypertrain/: .venv/bin/python scripts/check_ledger_invariant_mutants.py --only <ids>
"""

from __future__ import annotations

ADMISSION = "src/hypertrain/challenge/admission.py"
STORE = "src/hypertrain/challenge/store.py"
LEDGER = "src/hypertrain/ledger/escrow_v2.py"

GAPS = "tests/ledger_invariants/test_gaps_admission_lock.py::"
ONE_STAKE_ONE_ADMISSION = GAPS + "test_one_stake_backs_exactly_one_admission_of_a_coldkey"
ACCEPT_STAKE = GAPS + "test_an_accept_is_refused_when_the_stake_of_the_miner_fell_after_the_opening"
PAID_AFTER_AGGREGATION = (
    "tests/ledger_invariants/test_gaps_admission_lock_2.py::"
    "test_a_finalize_after_the_aggregation_pays_a_member_whose_stake_fell"
)
DISPUTE_WON = GAPS + "test_a_reward_origin_vests_after_a_dispute_the_miner_won"
ROTATION_LAB = GAPS + "test_a_hotkey_without_an_admission_history_cannot_be_rotated"
ROTATION_ROUTE = (
    GAPS + "test_the_rotation_route_answers_409_for_a_hotkey_without_an_admission_history"
)
RESERVATION = GAPS + "test_a_reservation_names_the_accepted_execution_it_cannot_find"
BEACON_WORK = GAPS + "test_one_poisoned_shadow_origin_never_stops_the_work_of_a_beacon[none]"

STAKE_OF_ONE_ADMISSION = "            \"AND json_extract(e.request,'$.admission_id')=? \"\n"
STAKE_OF_THE_COLDKEY = '            "AND ? IS NOT NULL "\n'
ACCEPT_CHECK = """\
                    not status.eligible
                    or model.work_screen_hash"""
ACCEPT_BLIND_TO_THE_STAKE = """\
                    not (status.eligible or status.funding.locked_units > 0)
                    or model.work_screen_hash"""
FINALIZE_PAYS_THE_TAPE = """\
            included = sorted(
                item["roster"]["hotkey"]
                for item in self._record_v2(run_id, "tape-inputs", str(w))["inputs"]
            )
"""
FINALIZE_ASKS_THE_ADMISSIONS_AGAIN = """\
            included = sorted(x.roster.hotkey for x in self.verified_inputs_v2(run_id, w))
"""
SETTLEMENT_OF_AN_ORIGIN = "        return Settlement.model_validate_json(row[0])\n"
SETTLEMENT_HELD_BY_ANY_DISPUTE = """\
        settled = Settlement.model_validate_json(row[0])
        miner = self._db.execute(
            "SELECT hotkey FROM escrow_origins WHERE origin=?", (reference,)
        ).fetchone()
        if miner is not None and self._db.execute(
            "SELECT 1 FROM disputes_v2 WHERE miner=?", (miner[0],)
        ).fetchone():
            return settled.model_copy(update={"unresolved": True})
        return settled
"""
HISTORY_GUARD = """\
            if history is None:
                raise AdmissionError("UNKNOWN_ROTATION_HISTORY")
"""
EXECUTION_RECORD = (
    '                accepted = self._record_v2(run_id, "shadow-execution", operation_hash)\n'
)
EXECUTION_OR_RUNTIME_CUSTODY = """\
                try:
                    accepted = self._record_v2(run_id, "shadow-execution", operation_hash)
                except ChallengeError:
                    accepted = self._record_v2(run_id, "runtime-custody", operation_hash)
"""

MISSING_RECORD = """\
        if row is None:
            raise ChallengeError(409, f"missing accepted {kind}: {identity}")
"""
RUNTIME_CUSTODY_FALLBACK = """\
        if row is None and kind == "shadow-execution":
            row = self._db.execute(
                "SELECT data FROM records_v2 WHERE run_id=? AND kind='runtime-custody' AND id=?",
                (run_id, identity),
            ).fetchone()
        if row is None:
            raise ChallengeError(409, f"missing accepted {kind}: {identity}")
"""
LEASE_EXPIRY = "                self._expire_leases_v2(run_id, br.round)\n"
DISPUTE_EXIT = "            self._close_blocked_disputes_v2(run_id)\n"
TRIAL_REAPER = "                admission.reap_expired_trials(now=self._now(self._db))\n"
NOTHING_16 = "                pass\n"
NOTHING_12 = "            pass\n"

MUTANTS = [
    {
        "id": "gal-stake-pooled-by-coldkey",
        "invariants": ["LCK-11", "LCK-10", "T-LOCK-06"],
        "path": LEDGER,
        "old": STAKE_OF_ONE_ADMISSION,
        "new": STAKE_OF_THE_COLDKEY,
        "tests": [ONE_STAKE_ONE_ADMISSION],
        "why": "the stake of one admission counts the locks of every admission of its coldkey.",
    },
    {
        "id": "gal-accept-is-blind-to-the-stake",
        "invariants": ["LCK-02", "LCK-14", "T-LOCK-03"],
        "path": STORE,
        "old": ACCEPT_CHECK,
        "new": ACCEPT_BLIND_TO_THE_STAKE,
        "tests": [ACCEPT_STAKE],
        "why": "an accept is served to a member holding part of the required stake.",
    },
    {
        "id": "gal-finalize-asks-the-admissions-again-after-the-aggregation",
        "invariants": ["LCK-01", "T-LOCK-04"],
        "path": STORE,
        "old": FINALIZE_PAYS_THE_TAPE,
        "new": FINALIZE_ASKS_THE_ADMISSIONS_AGAIN,
        "tests": [PAID_AFTER_AGGREGATION],
        "why": "the finalize of an applied round re-derives its members from the live admissions, "
        "so one member whose stake fell after the aggregation freezes the round for all.",
    },
    {
        "id": "gal-a-settled-dispute-holds-the-origin-for-ever",
        "invariants": ["LCK-09", "T-LOCK-10"],
        "path": STORE,
        "old": SETTLEMENT_OF_AN_ORIGIN,
        "new": SETTLEMENT_HELD_BY_ANY_DISPUTE,
        "tests": [DISPUTE_WON],
        "why": "a dispute settled for the miner still keeps the reward origin from vesting.",
    },
    {
        "id": "gal-rotation-trusts-a-hotkey-without-history",
        "invariants": ["LCK-16", "T-LOCK-19"],
        "path": ADMISSION,
        "old": HISTORY_GUARD,
        "new": "",
        "tests": [ROTATION_LAB, ROTATION_ROUTE],
        "why": "a rotation of a hotkey that never held an admission fails on a missing row.",
    },
    {
        "id": "gal-reservation-also-reads-private-runtime-custody",
        "invariants": ["SHD-06", "SHD-11", "T-LOCK-24"],
        "path": STORE,
        "old": EXECUTION_RECORD,
        "new": EXECUTION_OR_RUNTIME_CUSTODY,
        "tests": [RESERVATION + "[unknown-hashes]"],
        "why": "a reservation that misses an accepted execution names runtime custody instead.",
    },
    {
        "id": "gal-an-accepted-execution-lookup-falls-back-to-runtime-custody",
        "invariants": ["SHD-06", "SHD-11", "T-LOCK-24"],
        "path": STORE,
        "old": MISSING_RECORD,
        "new": RUNTIME_CUSTODY_FALLBACK,
        "tests": [RESERVATION + "[private-runtime-custody]"],
        "why": "every lookup of an accepted execution takes private runtime custody for one.",
    },
    {
        "id": "gal-a-beacon-expires-no-lease",
        "invariants": ["SHD-19", "T-LOCK-27"],
        "path": STORE,
        "old": LEASE_EXPIRY,
        "new": NOTHING_16,
        "tests": [BEACON_WORK],
        "why": "a beacon push matures what is due but expires no audit lease.",
    },
    {
        "id": "gal-a-beacon-closes-no-blocked-dispute",
        "invariants": ["SHD-19", "T-LOCK-27"],
        "path": STORE,
        "old": DISPUTE_EXIT,
        "new": NOTHING_12,
        "tests": [BEACON_WORK],
        "why": "a beacon push leaves every blocked dispute open.",
    },
    {
        "id": "gal-a-beacon-reaps-no-trial",
        "invariants": ["SHD-19", "T-LOCK-27"],
        "path": STORE,
        "old": TRIAL_REAPER,
        "new": NOTHING_16,
        "tests": [BEACON_WORK],
        "why": "a beacon push never frees the place of a trial that lapsed.",
    },
]
