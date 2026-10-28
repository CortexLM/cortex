"""Mutants of the weights gap tests: one small regression each, which a named test must catch.

Only tests that pass today are named: a strict xfail cannot kill a mutant.
"""

from __future__ import annotations

G = "tests/ledger_invariants/test_gaps_weights.py::"
G2 = "tests/ledger_invariants/test_gaps_weights_2.py::"
STORE = "src/hypertrain/challenge/store.py"
APP = "src/hypertrain/challenge/app.py"
ESCROW = "src/hypertrain/ledger/escrow_v2.py"
FINALITY = "src/hypertrain/challenge/finality_v2.py"

MALFORMED = (
    G + "test_a_malformed_or_out_of_range_epoch_or_epoch_at_is_refused_with_422_and_leaves_no_trace"
)
BETWEEN_PHASES = (
    G + "test_an_answer_that_lands_between_the_two_phases_is_replayed_not_failed_or_paid_twice"
)
CHILD_BUILD = (
    G
    + "test_a_child_that_cannot_build_its_services_fails_the_ask_and_the_retry_pays_each_store_once"
)
DEFAULT_CLOCK = (
    G + "test_an_omitted_epoch_at_is_the_end_of_the_epoch_and_the_clock_that_vests_v2_units"
)
FIRST_ANSWER = (
    G + "test_a_first_answer_stays_final_when_the_escrow_gains_a_payable_origin_and_after_a_restart"
)
EMPTY_EPOCH = G + "test_an_epoch_with_nothing_to_pay_is_answered_while_the_metagraph_is_down"
SETTLED = (
    G2 + "test_a_settled_round_pays_nothing_until_the_beacon_vests_it_and_then_each_audited_miner"
)
LATE_ROLLBACK = (
    G2 + "test_a_late_rollback_of_a_settled_round_moves_no_stored_answer_and_pays_nothing"
)
REFUSED = G2 + "test_a_round_whose_finalize_was_refused_pays_nothing_whatever_the_epoch_at"
SUSPENDED = (
    G2 + "test_an_earner_whose_admission_is_suspended_with_a_pending_dispute_is_not_paid[False]"
)


def mutant(
    id: str, invariants: list[str], path: str, old: str, new: str, tests: list[str], why: str
) -> dict[str, object]:
    return {
        "id": id,
        "invariants": invariants,
        "path": path,
        "old": old,
        "new": new,
        "tests": tests,
        "why": why,
    }


def bound(name: str, old: str, new: str, why: str) -> dict[str, object]:
    return mutant(f"gw-{name}", ["WGT-09"], APP, old, new, [MALFORMED], why)


EPOCH = "        epoch: Annotated[int, Query(ge=0, lt=2**63)],\n"
EPOCH_AT = "        epoch_at: Annotated[int | None, Query(ge=0, lt=2**63)] = None,\n"


def settle_clause(name: str, clause: str, case: str, why: str) -> dict[str, object]:
    return mutant(
        f"gw-settle-ignores-{name}",
        ["FIN-02", "WGT-02"],
        FINALITY,
        clause,
        "",
        [REFUSED + f"[{case}]"],
        why,
    )


MUTANTS = [
    bound(
        "epoch-has-no-lower-bound",
        EPOCH,
        "        epoch: Annotated[int, Query(lt=2**63)],\n",
        "a negative epoch reaches the store and surfaces as an unmapped ledger error (500).",
    ),
    bound(
        "epoch-has-no-upper-bound",
        EPOCH,
        "        epoch: Annotated[int, Query(ge=0)],\n",
        "an epoch of 2**63 and more reaches the store and surfaces as an unmapped error (500).",
    ),
    bound(
        "epoch-at-has-no-lower-bound",
        EPOCH_AT,
        "        epoch_at: Annotated[int | None, Query(lt=2**63)] = None,\n",
        "a negative epoch_at reaches the store and surfaces as an unmapped ledger error (500).",
    ),
    bound(
        "epoch-at-has-no-upper-bound",
        EPOCH_AT,
        "        epoch_at: Annotated[int | None, Query(ge=0)] = None,\n",
        "an epoch_at of 2**63 and more reaches the store and surfaces as an unmapped error (500).",
    ),
    mutant(
        "gw-write-transaction-does-not-recheck-the-stored-answer",
        ["WGT-06", "WGT-13"],
        STORE,
        "            if stored is not None:\n"
        "                return bytes(stored[0])\n"
        "            # Answered before multi-run hosting.\n",
        "            # Answered before multi-run hosting.\n",
        [BETWEEN_PHASES],
        "an answer that lands between the two phases is not replayed: the second writer fails on "
        "the primary key of weights_runs.",
    ),
    mutant(
        "gw-a-failing-child-is-skipped",
        ["WGT-15", "WGT-07"],
        STORE,
        "            answers = [answer] + [\n"
        "                c.weights(epoch, epoch_at, computed_at) for c in self._children.values()\n"
        "            ]\n",
        "            def soft(c: ChallengeStore) -> bytes | None:\n"
        "                try:\n"
        "                    return c.weights(epoch, epoch_at, computed_at)\n"
        "                except ChallengeError:\n"
        "                    return None\n\n"
        "            answers = [answer] + [\n"
        "                a for c in self._children.values() if (a := soft(c)) is not None\n"
        "            ]\n",
        [CHILD_BUILD],
        "a child that cannot build its services is left out of the merged answer, which the root "
        "stores for good, so the child's units are never emitted.",
    ),
    mutant(
        "gw-v2-units-vest-by-the-start-of-the-epoch",
        ["WGT-09", "WGT-03"],
        STORE,
        "                answer = self._pay_v2(escrows, epoch, epoch_at, answer)\n",
        "                answer = self._pay_v2(\n"
        "                    escrows,\n"
        "                    epoch,\n"
        "                    self.params.genesis_unix + epoch * self.params.epoch_seconds,\n"
        "                    answer,\n"
        "                )\n",
        [DEFAULT_CLOCK],
        "the v2 units vest by the start of the epoch while the answer is dated at its end.",
    ),
    mutant(
        "gw-reopened-store-replays-only-through-the-v2-services",
        ["WGT-06", "WGT-20"],
        STORE,
        "            if stored is not None:  # final: a replay needs no v2 services\n"
        "                return bytes(stored[0])\n",
        "            if False:\n                return bytes(stored[0])\n",
        [FIRST_ANSWER],
        "a store opened again builds the v2 services before it returns a stored answer.",
    ),
    mutant(
        "gw-route-asks-the-metagraph-for-an-empty-epoch",
        ["WGT-16"],
        APP,
        '            await metagraph.hotkeys() if json.loads(text)["weights"] else {}\n',
        "            await metagraph.hotkeys()\n",
        [EMPTY_EPOCH],
        "an epoch that pays nothing needs the metagraph too: an outage fails it with a 503.",
    ),
    mutant(
        "gw-route-answers-a-weighty-epoch-without-the-metagraph",
        ["WGT-16"],
        APP,
        '            await metagraph.hotkeys() if json.loads(text)["weights"] else {}\n',
        "            {}\n",
        [EMPTY_EPOCH],
        "an epoch that lists weight is answered without the master: every hotkey looks "
        "deregistered and is dropped.",
    ),
    mutant(
        "gw-beacon-push-vests-no-reward-unit",
        ["WGT-02", "WGT-03"],
        STORE,
        "                    \"AND u.bucket='reward_pending' AND u.units>0\",\n",
        "                    \"AND u.bucket='nowhere' AND u.units>0\",\n",
        [SETTLED, LATE_ROLLBACK, SUSPENDED],
        "the beacon never vests a settled reward, so no audited miner is ever emitted.",
    ),
    mutant(
        "gw-payable-lists-pending-units",
        ["WGT-02", "WGT-03"],
        ESCROW,
        "WHERE u.bucket='available' ",
        "WHERE u.bucket IN ('available','reward_pending') ",
        [SETTLED],
        "a settled reward is payable before the beacon vested it, as soon as the epoch's time "
        "reaches its maturity round.",
    ),
    mutant(
        "gw-rollback-of-a-settled-round-is-not-refused",
        ["WGT-17", "FIN-22"],
        STORE,
        '                if status.disposition == "SETTLED":\n'
        '                    raise ChallengeError(409, "finalized late fraud is money-only")\n',
        "",
        [LATE_ROLLBACK],
        "a rollback of a settled round gets past the money-only refusal.",
    ),
    settle_clause(
        "an-open-audit",
        "        or status.unresolved_audits\n",
        "an audit is open",
        "a round with an open audit settles, so its reward is issued before the audits close.",
    ),
    settle_clause(
        "an-excluded-round",
        '        or status.disposition == "EXCLUDED"\n',
        "the round is excluded",
        "an excluded round settles and issues a reward.",
    ),
    settle_clause(
        "a-missing-tape",
        "        not status.applied\n        or ",
        "no tape was applied",
        "a round whose tape was never applied settles and issues a reward.",
    ),
]
