"""Mutants that re-open what hotfix 4 (D109/D114) closed: the weights of a state directory that
hosts a retired run, and a hosted store that pays before the root has an answer.

S21 (run 0ef600b4, TEST): `GET /internal/v1/get_weights` answered 409 RUN_MANIFEST_RETIRED because
a stale hosted run had a stored manifest the model refuses, and the hosted stores asked before it
kept 1,000,000 units marked paid for an answer nobody was served. Two behaviours, one mutant each:

- a retired run is left out of the escrows its store pays, booked as `weights-skipped`, and the
  other runs are answered (`tests/challenge/test_weights_hosted_runs_v2.py`);
- the answer transaction of every hosted store ends with the root's, root last (WGT-15, WGT-D6): the
  strict xfail of the unretried epoch (`test_weights_2.py`) is flipped, and the pin that asserted
  the old intermediate state (`test_gaps_weights.py`) asserts that the child keeps nothing. The same
  patch is one mutant per pin, because the runner stops at the first failing test of a mutant.

The hardening lot (v3-weights-hardening) adds the mutants Lens left alive (L10, L5, L11), a COMMIT
that raises with the transaction open (N1), whose fix also flips the strict xfail FIN-25 + D-FIN-11
of test_finality.py, and a v1 journal replay that must equal an unfailed answer. L10 is equivalent
on every store the code can build (one run per store, one run per escrow ledger): its pin writes a
second run by hand. The charge window of `_service_charge_v2` (a 409 while another thread holds a
transaction of the store) was a strict xfail of tests/challenge/test_service_charge_window_v2.py;
ADR 0017 T2 reads the guard under the store lock (an owned body: the hash moves). One mutant per
window test reads it without the lock again. Another patch holds the lock across the charge's
`_tx`, the wrap the ADR first proposed: that `_tx` is then not outermost and writes no staged
round, which the forwarded-push and staged-deadline tests each kill (one mutant per test). The
guard mutant pins what the guard still adds, the mapped 409 for a charge inside its caller's own
transaction. Lens's M31 (T2 gate) refuses whoever holds the lock instead: a charge made under the
lock with no transaction open, which `_tx` serves, kills it.

What no test or mutant covers: the order in which the store locks are taken. Every path that holds
two of them takes the root first and the children in load order (read in review, not pinned), and
a child lock is now held until the root commits. Nor a charge whose caller holds the lock while a
round is staged: its `_tx` is not outermost, so that round is neither written nor seen by its
deadline check. No caller charges under the lock (the two charges run from the capacity attempt,
`store.py:5480-5486` and `:10500-10506`, built after their transaction closed), so the lock-holder
control charges with nothing staged.

Run from hypertrain/: .venv/bin/python scripts/check_ledger_invariant_mutants.py --only <ids>
"""

from __future__ import annotations

STORE = "src/hypertrain/challenge/store.py"

HOSTED = "tests/challenge/test_weights_hosted_runs_v2.py::"
RETIRED_CHILD = HOSTED + "test_a_retired_hosted_run_does_not_stop_the_weights_of_the_others"
RETIRED_ROOT = HOSTED + "test_a_retired_run_in_the_root_does_not_stop_the_weights_of_its_children"
SECOND_CALL = HOSTED + "test_a_second_call_for_the_same_epoch_replays_the_first_answer"
HOLD_THE_EPOCH = HOSTED + "test_children_that_already_hold_the_epoch_are_replayed_not_paid_again"
ROOT_FAILS = HOSTED + "test_a_failed_call_leaves_no_child_paid_and_the_retry_pays_each_once"
CHILD_FAILS = HOSTED + "test_a_child_that_fails_leaves_the_children_asked_before_it_unpaid"
HALF_COMMIT = (
    HOSTED + "test_a_commit_that_fails_halfway_is_repaired_by_the_replay_and_pays_each_once"
)
COMMIT_LEFT_OPEN = HOSTED + "test_a_store_whose_commit_fails_is_left_outside_its_transaction"
COMMIT_FAILS = (
    "tests/ledger_invariants/test_finality.py::"
    "test_a_transaction_whose_commit_fails_is_rolled_back_and_closed"
)
BESIDE_A_LIVE_RUN = (
    HOSTED + "test_a_retired_run_beside_a_live_one_in_the_same_store_does_not_stop_its_pay"
)
SKIP_PER_EPOCH = HOSTED + "test_a_skip_is_recorded_once_per_epoch_under_that_epoch"
NOTHING_TO_PAY = HOSTED + "test_a_run_with_nothing_to_pay_is_not_built_by_the_weights_call"
JOURNAL_REPLAY = (
    HOSTED + "test_the_retry_of_a_journaled_epoch_is_the_answer_of_a_call_that_never_failed"
)
WINDOW = "tests/challenge/test_service_charge_window_v2.py::"
OWN_TRANSACTION = WINDOW + "test_a_charge_made_inside_its_own_open_transaction_is_still_refused"
WEIGHTS_WINDOW = (
    WINDOW + "test_a_charge_in_the_weights_answer_window_of_its_store_waits_and_is_granted"
)
GENESIS_PUSH = (
    WINDOW + "test_a_genesis_charge_during_a_beacon_push_of_its_store_waits_and_is_granted"
)
FORWARDED_PUSH = (
    WINDOW
    + "test_a_charge_during_a_forwarded_push_waits_and_the_round_forwarded_meanwhile_is_written"
)
STAGED_DEADLINE = (
    WINDOW
    + "test_a_charge_behind_a_lock_holder_checks_its_deadline_with_the_round_staged_meanwhile"
)
LOCK_HOLDER = (
    WINDOW
    + "test_a_charge_made_by_a_thread_that_holds_the_store_lock_without_a_transaction_is_granted"
)
UNRETRIED = (
    "tests/ledger_invariants/test_weights_2.py::"
    "test_units_a_child_paid_while_the_root_failed_to_persist_reach_a_delivered_answer[False]"
)
ROOT_FAILED = (
    "tests/ledger_invariants/test_gaps_weights.py::"
    "test_units_a_child_would_pay_when_the_root_fails_to_persist_are_emitted_once_by_the_retry"
)

ASK_CHILDREN = (
    "                c._epoch_answer(stack, epoch, epoch_at, computed_at)\n"
    "                for c in self._children.values()\n"
)
ASK_CHILDREN_ALONE = (
    "                c.weights(epoch, epoch_at, computed_at)\n"
    "                for c in self._children.values()\n"
)
CHARGE_GUARD = (
    "            if self._db.in_transaction:\n"
    '                raise ChallengeError(409, "SERVICE_CHARGE_'
    'REQUIRES_COMMITTED_TRANSACTION")\n'
)
CHARGE_GUARD_UNLOCKED = (
    "        if self._db.in_transaction:\n"
    '            raise ChallengeError(409, "SERVICE_CHARGE_'
    'REQUIRES_COMMITTED_TRANSACTION")\n'
)
CHARGE_TX = (
    '        with self._tx():\n            preparation = operation == "genesis" and w == -1\n'
)

MUTANTS = [
    {
        "id": "d109-retired-run-aborts-the-call",
        "invariants": ["WGT-15"],
        "path": STORE,
        "old": "except RunRetired as error:",
        "new": "except KeyError as error:",
        "tests": [RETIRED_CHILD, RETIRED_ROOT, SECOND_CALL, HOLD_THE_EPOCH],
        "why": "a hosted run whose manifest is refused raises RunRetired out of the whole call.",
    },
    {
        "id": "d109-skip-not-journaled",
        "invariants": ["WGT-15"],
        "path": STORE,
        "old": "for run_id, reason in retired:",
        "new": "for run_id, reason in []:",
        "tests": [RETIRED_CHILD],
        "why": "the retired run is skipped without a trace of which run, epoch or why.",
    },
    {
        "id": "d109-skip-without-reason",
        "invariants": ["WGT-15"],
        "path": STORE,
        "old": '{"epoch": epoch, "reason": reason}',
        "new": '{"epoch": epoch, "reason": ""}',
        "tests": [RETIRED_CHILD],
        "why": "the skip is journaled without the refusal that caused it.",
    },
    {
        "id": "d109-answer-not-persisted",
        "invariants": ["WGT-15"],
        "path": STORE,
        "old": 'db.execute("INSERT INTO weights_runs VALUES(?, ?)", (epoch, merged))',
        "new": 'db.execute("SELECT 1")',
        "tests": [SECOND_CALL, HOLD_THE_EPOCH],
        "why": "no store keeps its epoch answer, so a second call recomputes and pays nothing.",
    },
    {
        "id": "d109-children-commit-alone",
        "invariants": ["WGT-15"],
        "path": STORE,
        "old": ASK_CHILDREN,
        "new": ASK_CHILDREN_ALONE,
        "tests": [ROOT_FAILS, CHILD_FAILS],
        "why": "a hosted store commits its payout and its answer before the root has its own.",
    },
    {
        "id": "d109-unretried-epoch-loses-child-units",
        "invariants": ["WGT-15"],
        "path": STORE,
        "old": ASK_CHILDREN,
        "new": ASK_CHILDREN_ALONE,
        "tests": [UNRETRIED],
        "why": "units a child marked paid in a failed call reach no answer once Cortex moves on.",
    },
    {
        "id": "d109-root-failure-leaves-child-paid",
        "invariants": ["WGT-15"],
        "path": STORE,
        "old": ASK_CHILDREN,
        "new": ASK_CHILDREN_ALONE,
        "tests": [ROOT_FAILED],
        "why": "the child keeps its payout and answer when the root fails to persist the merge.",
    },
    {
        "id": "d109-root-commits-first",
        "invariants": ["WGT-15"],
        "path": STORE,
        "old": "with self._tx_until(stack) as db:",
        "new": "with (self._tx() if self._children else self._tx_until(stack)) as db:",
        "tests": [HALF_COMMIT],
        "why": "the root commits before the hosted stores: a hosted commit that fails leaves the "
        "root with an answer that pays units the hosted store never kept.",
    },
    {
        "id": "d109-retired-run-stops-its-store",
        "invariants": ["WGT-15"],
        "path": STORE,
        "old": "            if escrows:\n",
        "new": "            if escrows and not retired:\n",
        "tests": [BESIDE_A_LIVE_RUN],
        "why": "a retired run stops the pay of a live run its store also holds (Lens L10).",
    },
    {
        "id": "d109-skip-keyed-off-its-epoch",
        "invariants": ["WGT-15"],
        "path": STORE,
        "old": '"weights-skipped", str(epoch),',
        "new": '"weights-skipped", str(epoch + 1),',
        "tests": [SKIP_PER_EPOCH],
        "why": "the skip is filed under another epoch's id (Lens L5).",
    },
    {
        "id": "d109-idle-run-built",
        "invariants": ["WGT-15"],
        "path": STORE,
        "old": 'if live and json.loads(r[1]).get("manifest_version") == 2:',
        "new": 'if json.loads(r[1]).get("manifest_version") == 2:',
        "tests": [NOTHING_TO_PAY],
        "why": "the services of a run with nothing to pay are built: a retired one is booked as "
        "skipped, one that cannot be built refuses the epoch for every run (Lens L11).",
    },
    {
        "id": "d109-hosted-journal-forgets-a-failed-epoch",
        "invariants": ["WGT-15"],
        "path": STORE,
        "old": (
            "        with ExitStack() as stack:\n"
            "            return self._epoch_answer(stack, epoch, epoch_at, computed_at)\n"
        ),
        "new": (
            "        try:\n"
            "            with ExitStack() as stack:\n"
            "                return self._epoch_answer(stack, epoch, epoch_at, computed_at)\n"
            "        except BaseException:\n"
            "            for c in self._children.values():\n"
            "                c.ledger._engine.answers.pop(epoch, None)\n"
            "            raise\n"
        ),
        "tests": [JOURNAL_REPLAY],
        "why": "a hosted store's v1 journal forgets a failed epoch, so the retry journals it again "
        "with its own clock and stores an answer an unfailed call never stores.",
    },
    {
        "id": "d109-charge-inside-its-own-transaction-allowed",
        "invariants": ["WGT-15"],
        "path": STORE,
        "old": CHARGE_GUARD,
        "new": "            pass\n",
        "tests": [OWN_TRANSACTION],
        "why": "a service charge made inside its caller's own transaction is no longer refused "
        "with the mapped 409: the nested BEGIN raises a bare sqlite error instead.",
    },
    *(
        {
            "id": f"d109-charge-guard-read-without-the-lock-{pin}",
            "invariants": ["WGT-15"],
            "path": STORE,
            "old": "        with self._lock:\n" + CHARGE_GUARD,
            "new": CHARGE_GUARD_UNLOCKED,
            "tests": [test],
            "why": "the charge reads in_transaction without the lock again: a transaction another "
            "thread holds on the store refuses it with 409 and burns its attempt 1.",
        }
        for pin, test in (
            ("weights", WEIGHTS_WINDOW),
            ("genesis-push", GENESIS_PUSH),
            ("forwarded-push", FORWARDED_PUSH),
            ("staged-deadline", STAGED_DEADLINE),
        )
    ),
    *(
        {
            "id": f"d109-charge-holds-the-lock-across-its-tx-{pin}",
            "invariants": ["WGT-15"],
            "path": STORE,
            "old": CHARGE_TX,
            "new": CHARGE_TX.replace("with self._tx()", "with self._lock, self._tx()"),
            "tests": [test],
            "why": why,
        }
        for pin, test, why in (
            (
                "keeps-staged-rounds",
                FORWARDED_PUSH,
                "the charge's `_tx` is not outermost: a round forwarded during the charge stays "
                "staged after it, where an outermost transaction writes it.",
            ),
            (
                "misses-the-deadline",
                STAGED_DEADLINE,
                "the charge's `_tx` is not outermost: its deadline check reads the view without "
                "the round staged behind a lock holder and grants a charge past its deadline.",
            ),
        )
    ),
    {
        "id": "d109-charge-refuses-a-lock-holder-without-a-transaction",
        "invariants": ["WGT-15"],
        "path": STORE,
        "old": "        with self._lock:\n" + CHARGE_GUARD,
        "new": CHARGE_GUARD_UNLOCKED.replace("self._db.in_transaction", "self._lock._is_owned()"),
        "tests": [LOCK_HOLDER],
        "why": "a caller that holds the store lock with no transaction open is refused with the "
        "409 of a charge inside its own transaction, though `_tx` serves it (Lens M31).",
    },
    {
        "id": "d109-failed-commit-left-open",
        "invariants": ["WGT-15", "FIN-25"],
        "path": STORE,
        "old": (
            "                    if self._db.in_transaction:\n"
            '                        self._db.execute("ROLLBACK")\n'
        ),
        "new": "                    pass\n",
        "tests": [COMMIT_LEFT_OPEN],
        "why": "a COMMIT that raised leaves the store inside its transaction: it refuses every "
        "later BEGIN, so the replay that repairs a half-committed epoch cannot run on it.",
    },
    {
        "id": "d109-failed-commit-left-open-flips-fin25",
        "invariants": ["WGT-15", "FIN-25"],
        "path": STORE,
        "old": (
            "                    if self._db.in_transaction:\n"
            '                        self._db.execute("ROLLBACK")\n'
        ),
        "new": "                    pass\n",
        "tests": [COMMIT_FAILS],
        "why": "the same patch for the FIN-25 + D-FIN-11 pin the fix flipped: the connection is "
        "left inside a transaction whose COMMIT raised.",
    },
]
