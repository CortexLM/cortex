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

What no test or mutant covers: the order in which the store locks are taken. Every path that holds
two of them takes the root first and the children in load order (read in review, not pinned), and
a child lock is now held until the root commits.

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
]
