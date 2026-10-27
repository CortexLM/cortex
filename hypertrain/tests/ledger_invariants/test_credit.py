"""Credit paths of the ledger and of finalize_v2 (Lens CRD-01..CRD-10, CRD-12, CRD-23): a round
credits each miner once, in order, only for audited MATCH work, with an allocation the ledger
recomputes itself, to the coldkey bound at join.

Every test states a defence the protocol must hold: the ledger refuses X and its durable state
is unchanged, or it credits once. The rounds are signed with test keys and no model execution
(`support_credit.build_plan`); the replay callback is a stand-in, the training replay itself is
covered by the real reward of tests/ledger.
"""

from __future__ import annotations

import ast
import json
import sqlite3
from collections.abc import Callable, Iterator
from dataclasses import replace
from pathlib import Path
from types import ModuleType
from typing import Any

import pytest
from hypothesis import example, given
from hypothesis import strategies as st
from pydantic import ValidationError

import hypertrain
from hypertrain.challenge.store import ChallengeError
from hypertrain.ledger.escrow_v2 import EscrowError, EscrowReceipt, EscrowV2, digest
from hypertrain.miner.admission import sign_rotation
from hypertrain.protocol.envelope_v2 import seal
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.messages import Finalize
from hypertrain.protocol.messages_v2 import (
    OriginAllocation,
    RewardFinalize,
    RotateRequest,
)

from .support import H, memory_db
from .support_credit import (
    BUDGET,
    HOTKEYS,
    STRANGER,
    AuditedRound,
    Plan,
    build_audited_round,
    build_plan,
    credited,
    database_digest,
    event_count,
    finalize_refusal,
    ledger_for,
    ledger_rows,
    miner,
    open_ledger,
    owner,
    refusal,
    repin,
    reward_origins,
    split_units,
)

SRC = Path(hypertrain.__file__).resolve().parent


@pytest.fixture(scope="module")
def setup(li_escrow: ModuleType) -> Any:
    return li_escrow.setup("production")


@pytest.fixture(scope="module")
def two_rounds_cap(li_escrow: ModuleType) -> Any:
    """Room for exactly two rounds of issuance: the third finalize meets the pinned cap."""
    return repin(li_escrow.setup("production"), max_total_issuance=2 * BUDGET)


# --- CRD-01 / CRD-02 / CRD-03 / CRD-06 / CRD-11: one credit per round, in order -----------------


@pytest.fixture(scope="module")
def signed_rounds(li_escrow: ModuleType, two_rounds_cap: Any) -> dict[tuple[int, int], Any]:
    """(round, variant) -> (reward, evidence): two competing Finalize bodies for rounds 0..2."""
    rounds = {}
    for w in range(3):
        for variant in range(2):
            plan = build_plan(
                li_escrow, two_rounds_cap, HOTKEYS, w=w, entitlements_root=f"{variant + 1:02x}" * 32
            )
            rounds[(w, variant)] = (plan, *plan.build())
    return rounds


ATTEMPT = st.one_of(st.tuples(st.integers(0, 2), st.integers(0, 1)), st.just("restart"))


@given(attempts=st.lists(ATTEMPT, max_size=9))
@example(attempts=[(0, 0), (0, 1), (1, 1), (1, 0), (2, 0), (2, 1)])
@example(attempts=[(2, 0), (1, 0), (0, 1), "restart", (2, 1), (1, 1), (0, 0), (1, 0), (2, 0)])
def test_finalize_attempts_in_any_order_credit_each_round_once_and_follow_the_ledger_model(
    li_escrow: ModuleType,
    two_rounds_cap: Any,
    signed_rounds: dict[tuple[int, int], Any],
    attempts: list[Any],
) -> None:
    """CRD-01 CRD-02 CRD-03 CRD-06 CRD-11: whatever the order of finalizes, variants, replays and
    restarts, a (round, miner) holds at most one live origin, a round is credited once in strict
    order, an identical replay returns the first receipt, another body is OPERATION_CONFLICT, a
    skipped or past round is FINALIZE_ORDER, and issuance never passes the pinned cap."""
    plan = signed_rounds[(0, 0)][0]
    database = memory_db()

    def engine() -> EscrowV2:
        return open_ledger(li_escrow, two_rounds_cap, database, plan.owners, plan.replay)

    escrow = engine()
    applied: dict[int, tuple[int, EscrowReceipt]] = {}
    cap = two_rounds_cap.policy.max_total_issuance
    for step in attempts:
        if step == "restart":
            escrow = engine()
            continue
        w, variant = step
        _, reward, evidence = signed_rounds[(w, variant)]
        before = ledger_rows(escrow)
        try:
            outcome: Any = escrow.reward_finalize(reward, evidence)
        except EscrowError as error:
            outcome = error.code
        if w in applied:
            first_variant, first = applied[w]
            expected: Any = first if variant == first_variant else "OPERATION_CONFLICT"
        elif w != len(applied):
            expected = "FINALIZE_ORDER"
        elif (len(applied) + 1) * BUDGET > cap:
            expected = "TOTAL_ISSUANCE_BUDGET"
        else:
            expected = "ACCEPTED"
        if expected == "ACCEPTED":
            assert isinstance(outcome, EscrowReceipt)
            applied[w] = (variant, outcome)
        else:
            assert outcome == expected
            assert ledger_rows(escrow) == before
        assert reward_origins(escrow) == sorted((n, m.ss58) for n in applied for m in HOTKEYS)
        assert [tuple(r) for r in escrow.db.execute("SELECT * FROM escrow_finalized")] == [
            (n, signed_rounds[(n, applied[n][0])][1].finalize_hash) for n in sorted(applied)
        ]
        balances = escrow.balances()
        assert balances.issued == len(applied) * BUDGET <= cap and balances.conserved()
    escrow.verify()


def test_a_changed_second_finalize_of_a_round_is_refused_and_credits_nothing(
    li_escrow: ModuleType, two_rounds_cap: Any, signed_rounds: dict[tuple[int, int], Any]
) -> None:
    """CRD-01 CRD-02: another validly signed Finalize body for the same round (a different
    entitlements_root, hence other origin ids) is OPERATION_CONFLICT; the first stays."""
    plan, first, first_evidence = signed_rounds[(0, 0)]
    _, other, other_evidence = signed_rounds[(0, 1)]
    assert set(first.origin_ids).isdisjoint(other.origin_ids)
    escrow = ledger_for(li_escrow, two_rounds_cap, plan)
    escrow.reward_finalize(first, first_evidence)
    rows = ledger_rows(escrow)
    assert refusal(lambda: escrow.reward_finalize(other, other_evidence)) == "OPERATION_CONFLICT"
    assert ledger_rows(escrow) == rows
    assert escrow.db.execute("SELECT finalize_hash FROM escrow_finalized").fetchone()[0] == (
        first.finalize_hash
    )
    assert [o for _, o in reward_origins(escrow)] == sorted(m.ss58 for m in HOTKEYS)


def test_finalize_order_is_strict_and_a_replay_is_answered_before_the_order_test(
    li_escrow: ModuleType, two_rounds_cap: Any, signed_rounds: dict[tuple[int, int], Any]
) -> None:
    """CRD-03: round 1 cannot be credited before round 0, round 2 cannot skip round 1; once
    applied, a round is answered from its stored receipt whatever the head of the chain is."""
    plan, zero, zero_evidence = signed_rounds[(0, 0)]
    _, one, one_evidence = signed_rounds[(1, 0)]
    _, two, two_evidence = signed_rounds[(2, 0)]
    escrow = ledger_for(li_escrow, two_rounds_cap, plan)
    assert refusal(lambda: escrow.reward_finalize(one, one_evidence)) == "FINALIZE_ORDER"
    assert event_count(escrow) == 0 and escrow.balances().issued == 0
    first = escrow.reward_finalize(zero, zero_evidence)
    assert refusal(lambda: escrow.reward_finalize(two, two_evidence)) == "FINALIZE_ORDER"
    assert escrow.balances().issued == BUDGET
    escrow.reward_finalize(one, one_evidence)
    assert escrow.reward_finalize(zero, zero_evidence) == first
    assert [r[0] for r in escrow.db.execute("SELECT w FROM escrow_finalized ORDER BY w")] == [0, 1]


def test_an_identical_finalize_after_a_restart_returns_the_stored_receipt_and_moves_nothing(
    li_escrow: ModuleType, setup: Any, tmp_path: Path
) -> None:
    """CRD-02 CRD-06: the same finalize, resent after the ledger file was closed and reopened,
    returns the first receipt byte for byte and writes nothing; a different request for the same
    round (here another maturity) is OPERATION_CONFLICT before and after the restart."""
    plan = build_plan(li_escrow, setup, HOTKEYS)
    reward, evidence = plan.build()
    changed, _ = plan.build(mature_at=21)
    path = tmp_path / "ledger.db"
    escrow = ledger_for(li_escrow, setup, plan, path)
    receipt = escrow.reward_finalize(reward, evidence)
    assert refusal(lambda: escrow.reward_finalize(changed, evidence)) == "OPERATION_CONFLICT"
    rows = ledger_rows(escrow)
    escrow.db.close()
    reopened = ledger_for(li_escrow, setup, plan, path)
    assert reopened.reward_finalize(reward, evidence) == receipt
    assert refusal(lambda: reopened.reward_finalize(changed, evidence)) == "OPERATION_CONFLICT"
    assert ledger_rows(reopened) == rows
    assert event_count(reopened, "REWARD_FINALIZE") == 1


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="CRD-06+CRD-D08: an identical live finalize is re-authenticated by the current owner, "
    "replay and signature callbacks before the stored receipt is looked up, so a retry after "
    "the owner mapping changed or a callback became unavailable is refused instead of "
    "answered from the immutable event (the store's finalize-result cache hides it)",
)
def test_an_identical_finalize_is_answered_from_the_stored_event_when_a_callback_fails(
    li_escrow: ModuleType, setup: Any
) -> None:
    plan = build_plan(li_escrow, setup, HOTKEYS)
    reward, evidence = plan.build()
    escrow = ledger_for(li_escrow, setup, plan)
    receipt = escrow.reward_finalize(reward, evidence)
    assert escrow.reward_finalize(reward, evidence) == receipt  # control: healthy callbacks

    def unavailable(hotkey: str) -> str:
        raise ChallengeError(403, "hotkey is not current accepted identity")

    escrow.owner_of = unavailable
    try:
        replayed: Any = escrow.reward_finalize(reward, evidence)
    except (EscrowError, ChallengeError) as error:
        replayed = error
    assert replayed == receipt, f"the retry was refused with {replayed!r}"


def test_one_work_per_hotkey_inside_a_finalize(li_escrow: ModuleType, setup: Any) -> None:
    """CRD-01: the same hotkey presented twice as work, or listed twice as included, is refused
    even when the reward is signed for exactly that evidence; the one-work control is accepted."""
    plan = build_plan(li_escrow, setup, [miner(0)])
    reward, evidence = plan.build()
    twice = replace(evidence, works=(evidence.works[0], evidence.works[0]))
    duplicated_work = plan.reward(twice)
    plan.included = [miner(0).ss58, miner(0).ss58]
    listed_twice, listed_evidence = plan.build()
    for label, signed, facts, code in (
        ("work", duplicated_work, twice, "UNVERIFIED_REWARD_WORK"),
        ("included", listed_twice, listed_evidence, "NO_VERIFIED_WORK_OR_EXCLUSIONS"),
    ):
        escrow = ledger_for(li_escrow, setup, plan)
        assert finalize_refusal(escrow, signed, facts) == code, label
        assert escrow.balances().issued == 0 and event_count(escrow) == 0, label
    control = ledger_for(li_escrow, setup, plan)
    control.reward_finalize(reward, evidence)
    assert control.balances().issued == BUDGET and reward_origins(control) == [(0, miner(0).ss58)]


def test_a_colliding_origin_id_is_refused_and_the_whole_finalize_rolls_back(
    li_escrow: ModuleType, setup: Any
) -> None:
    """CRD-01 CRD-05: an origin id that already exists is DUPLICATE_ORIGIN, and the origin the
    same call had already issued before reaching it disappears with the transaction."""
    plan = build_plan(li_escrow, setup, HOTKEYS)
    reward, evidence = plan.build()
    escrow = ledger_for(li_escrow, setup, plan)
    taken = reward.origin_ids[1]  # the second origin of the split
    escrow.db.execute("INSERT INTO escrow_origins VALUES(?,?,?,?,?)", (taken, 1, 0, 9, "x"))
    escrow.db.execute(
        "INSERT INTO escrow_units VALUES(?,?,?,?,?)",
        (taken, owner(5).ss58, "reward_pending", "", 1),
    )
    rows = ledger_rows(escrow)
    assert refusal(lambda: escrow.reward_finalize(reward, evidence)) == "DUPLICATE_ORIGIN"
    assert ledger_rows(escrow) == rows
    assert not escrow.db.in_transaction


# --- CRD-07 / CRD-09 / CRD-10: what a credit needs, and what the ledger recomputes ----------------


def alter_item(plan: Plan, **commit: Any) -> None:
    item = plan.items[0]
    item.commit = item.commit.model_copy(update=commit)


def rebind(plan: Plan, **challenge: Any) -> None:
    """Change the challenge and keep the verdict bound to it, so only the change can refuse."""
    item = plan.items[0]
    item.challenge = item.challenge.model_copy(update=challenge)
    item.verdict = item.verdict.model_copy(update={"challenge_hash": digest(item.challenge)})


def verdict_change(**changes: Any) -> Callable[[Plan], None]:
    def apply(plan: Plan) -> None:
        item = plan.items[0]
        item.verdict = item.verdict.model_copy(update=changes)

    return apply


def env_change(**changes: Any) -> Callable[[Plan], None]:
    def apply(plan: Plan) -> None:
        item = plan.items[0]
        item.verdict = item.verdict.model_copy(
            update={"replay_env": item.verdict.replay_env.model_copy(update=changes)}
        )

    return apply


def set_attr(**fields: Any) -> Callable[[Plan], None]:
    def apply(plan: Plan) -> None:
        for name, value in fields.items():
            target = plan.items[0] if hasattr(plan.items[0], name) else plan
            setattr(target, name, value)

    return apply


OTHER_IMAGE = "sha256:" + "22" * 32

# name -> (how the plan is altered before it is signed, what the ledger answers)
REFUSALS: dict[str, tuple[Callable[[Plan], None], str]] = {
    "verdict_by_an_unpinned_auditor": (
        set_attr(verdict_signer=STRANGER),
        "UnauthorizedSigner",
    ),
    "challenge_by_a_stranger": (set_attr(challenge_signer=STRANGER), "UnauthorizedSigner"),
    "commit_by_another_key": (set_attr(commit_signer=STRANGER), "UnauthorizedSigner"),
    "verdict_of_another_challenge": (
        verdict_change(challenge_hash=H),
        "REWARD_AUDIT_BINDING",
    ),
    "challenge_of_another_miner": (
        lambda plan: rebind(plan, target=STRANGER.ss58),
        "REWARD_AUDIT_BINDING",
    ),
    "verdict_is_a_mismatch": (verdict_change(result="MISMATCH"), "UNVERIFIED_REWARD_WORK"),
    "verdict_names_a_bad_leaf": (verdict_change(first_bad_leaf=0), "UNVERIFIED_REWARD_WORK"),
    "verdict_recomputed_another_root": (
        verdict_change(recomputed_leaves_root=H),
        "UNVERIFIED_REWARD_WORK",
    ),
    "verdict_from_another_image": (
        env_change(image_digest=OTHER_IMAGE),
        "UNVERIFIED_REWARD_WORK",
    ),
    "verdict_from_a_driver_off_the_allowlist": (
        env_change(driver="driver-x"),
        "UNVERIFIED_REWARD_WORK",
    ),
    "commit_of_another_round": (
        lambda plan: (alter_item(plan, w=1), rebind(plan, w=1)),
        "UNVERIFIED_REWARD_WORK",
    ),
    "audited_miner_missing_from_included": (
        lambda plan: setattr(plan, "included", []),
        "UNVERIFIED_REWARD_WORK",
    ),
    "included_miner_without_audited_work": (
        lambda plan: setattr(plan, "included", sorted([plan.items[0].hotkey, STRANGER.ss58])),
        "NO_VERIFIED_WORK_OR_EXCLUSIONS",
    ),
    "tokens_not_derived_from_the_assignment": (
        lambda plan: alter_item(plan, tokens=8),
        "ValueError",
    ),
    "work_owner_is_not_the_current_owner": (
        set_attr(owner=owner(8).ss58),
        "UNVERIFIED_REWARD_WORK",
    ),
    "samples_differ_from_the_assignment": (set_attr(sample_ids=(1,)), "UNVERIFIED_REWARD_WORK"),
    "finalize_signed_by_a_stranger": (set_attr(finalize_signer=STRANGER), "FINALITY_AUTHORITY"),
    "finalized_before_the_commit_expiry": (set_attr(commit_exp=5), "ExpiredError"),
    "finalized_after_the_verdict_expiry": (set_attr(verdict_exp=5), "ExpiredError"),
    "finalized_after_the_challenge_expiry": (set_attr(challenge_exp=5), "ExpiredError"),
    "finalized_after_the_finalize_expiry": (set_attr(finalize_exp=5), "ExpiredError"),
}


@pytest.mark.parametrize("name", [*REFUSALS, "control"])
def test_no_credit_without_audited_match_work_bound_to_this_round_miner_and_owner(
    li_escrow: ModuleType, setup: Any, name: str
) -> None:
    """CRD-07 CRD-08 CRD-09: each way the evidence can be wrong is refused with its own answer
    and leaves the ledger untouched; the unaltered, re-signed control is credited."""
    plan = build_plan(li_escrow, setup, [miner(0)])
    owners = dict(plan.owners)  # the service's view, whatever the evidence claims
    alter, expected = REFUSALS.get(name, (lambda _: None, ""))
    alter(plan)
    reward, evidence = plan.build()
    escrow = ledger_for(li_escrow, setup, plan, owners=owners)
    if name == "control":
        escrow.reward_finalize(reward, evidence)
        assert escrow.balances().issued == BUDGET
        return
    rows = ledger_rows(escrow)
    assert refusal(lambda: escrow.reward_finalize(reward, evidence)) == expected
    assert ledger_rows(escrow) == rows


def test_a_miner_that_is_also_a_pinned_auditor_cannot_audit_itself(
    li_escrow: ModuleType, setup: Any
) -> None:
    """CRD-07: a verdict signed by the audited miner is refused even when the manifest pins that
    key as an auditor; the same verdict by the real auditor is credited."""
    plan = build_plan(li_escrow, setup, [miner(0)])
    honest, honest_evidence = plan.build()
    plan.items[0].verdict_signer = miner(0)
    reward, evidence = plan.build()
    escrow = ledger_for(li_escrow, setup, plan)
    escrow.auditors = escrow.auditors | {miner(0).ss58}
    assert refusal(lambda: escrow.reward_finalize(reward, evidence)) == "UNVERIFIED_REWARD_WORK"
    assert escrow.balances().issued == 0 and event_count(escrow) == 0
    escrow.reward_finalize(honest, honest_evidence)  # control: the independent auditor
    assert escrow.balances().issued == BUDGET


def keyword_names(source: str, constructor: str) -> list[set[str]]:
    """The keyword names of every call to `constructor(` in `source`."""
    return [
        {keyword.arg for keyword in node.keywords if keyword.arg}
        for node in ast.walk(ast.parse(source))
        if isinstance(node, ast.Call)
        and isinstance(node.func, ast.Name)
        and node.func.id == constructor
    ]


def test_the_service_credits_on_one_verdict_per_miner_and_a_quorum_ledger_would_refuse_it(
    li_escrow: ModuleType, setup: Any
) -> None:
    """CRD-07 (condition: production runs without an auditor quorum, so one pinned auditor's
    MATCH is the whole audit). The service builds its ledger without a quorum and assembles each
    RewardWork without extra verdicts; a ledger that did run a quorum refuses that evidence, so
    switching it on without changing the service fails closed instead of crediting."""
    source = (SRC / "challenge/store.py").read_text()
    assert keyword_names(source, "EscrowV2")
    assert all("auditor_quorum" not in names for names in keyword_names(source, "EscrowV2"))
    assert all("extra_verdicts" not in names for names in keyword_names(source, "RewardWork"))
    plan = build_plan(li_escrow, setup, [miner(0)])
    reward, evidence = plan.build()
    auditor = li_escrow.AUDITOR.ss58
    quorum = ledger_for(li_escrow, setup, plan, owners={**plan.owners, auditor: owner(7).ss58})
    quorum.auditor_quorum = (2, 2)
    assert finalize_refusal(quorum, reward, evidence) == "QUORUM_UNMET"
    assert quorum.balances().issued == 0 and event_count(quorum) == 0
    single = ledger_for(li_escrow, setup, plan)
    single.reward_finalize(reward, evidence)
    assert single.balances().issued == BUDGET


@pytest.mark.parametrize(
    ("change", "code"),
    [
        ({"verdict_root": H}, "VERDICT_ROOT"),
        ({"origin_ids": [H]}, "REWARD_ALLOCATION"),
        ({"allocation_hash": H}, "REWARD_ALLOCATION"),
        ({"budget_units": BUDGET - 1}, "ROUND_BUDGET"),
        ({"tape_hash": H}, "FINALITY_BINDING_OR_VESTING"),
        ({"finalize_hash": H}, "FINALITY_BINDING_OR_VESTING"),
        ({"w": 1}, "FINALITY_BINDING_OR_VESTING"),
        ({"mature_at": 19}, "FINALITY_BINDING_OR_VESTING"),
    ],
    ids=lambda value: next(iter(value)) if isinstance(value, dict) else str(value),
)
def test_a_signed_reward_that_disagrees_with_the_evidence_credits_nothing(
    li_escrow: ModuleType, setup: Any, change: dict[str, Any], code: str
) -> None:
    """CRD-09: the reward is only a claim; roots, hashes, budget and vesting it names are
    checked against the evidence the service built, and a re-signed lie is refused."""
    plan = build_plan(li_escrow, setup, [miner(0)])
    reward, evidence = plan.build(**change)
    escrow = ledger_for(li_escrow, setup, plan)
    assert refusal(lambda: escrow.reward_finalize(reward, evidence)) == code
    assert ledger_rows(escrow) == ledger_rows(ledger_for(li_escrow, setup, plan))


def test_evidence_the_replay_does_not_confirm_or_in_the_wrong_domain_credits_nothing(
    li_escrow: ModuleType, setup: Any
) -> None:
    """CRD-09: the finalized state must equal the independently replayed one, a signature of
    another origin type is no authority, and shadow evidence is not live evidence."""
    plan = build_plan(li_escrow, setup, [miner(0)])
    reward, evidence = plan.build()
    stranger_signed, _ = plan.build(signer=STRANGER)
    escrow = ledger_for(li_escrow, setup, plan)
    escrow.replay_finality = lambda _: H
    rows = ledger_rows(escrow)
    assert refusal(lambda: escrow.reward_finalize(reward, evidence)) == "FINALITY_REPLAY"
    escrow.replay_finality = plan.replay
    assert refusal(lambda: escrow.reward_finalize(stranger_signed, evidence)) == "ORIGIN_AUTHORITY"
    shadowed = replace(evidence, shadow=True)
    assert refusal(lambda: escrow.reward_finalize(reward, shadowed)) == "REWARD_DOMAIN"
    assert ledger_rows(escrow) == rows


def callers_of(method: str) -> set[tuple[str, str]]:
    """(file, enclosing function) of every call to `.method(` in the source tree."""
    found = set()
    for path in sorted(SRC.rglob("*.py")):
        tree = ast.parse(path.read_text())
        for function in ast.walk(tree):
            if not isinstance(function, ast.FunctionDef | ast.AsyncFunctionDef):
                continue
            for node in ast.walk(function):
                if (
                    isinstance(node, ast.Call)
                    and isinstance(node.func, ast.Attribute)
                    and node.func.attr == method
                ):
                    found.add((str(path.relative_to(SRC)), function.name))
    return found


def test_the_only_callers_of_the_credit_entry_points_are_the_service_handlers() -> None:
    """CRD-09: reward_finalize, test_genesis and production_bootstrap are public methods; the
    safety argument is that only coordinator-gated store handlers, which assemble the evidence
    from the store's own records, reach them. A new caller must be reviewed on purpose."""
    assert callers_of("reward_finalize") == {
        ("challenge/store.py", "finalize_v2"),
        ("challenge/store.py", "shadow_reward_v2"),
    }
    assert callers_of("test_genesis") == {("challenge/store.py", "genesis_v2")}
    assert callers_of("production_bootstrap") == {("challenge/store.py", "production_bootstrap_v2")}


@given(n=st.integers(1, 12), reverse=st.booleans())
@example(n=7, reverse=False)
@example(n=3, reverse=True)
def test_a_round_splits_its_budget_exactly_and_hands_the_remainder_to_the_lowest_hotkeys(
    li_escrow: ModuleType, setup: Any, n: int, reverse: bool
) -> None:
    """CRD-10: for any number of equally entitled miners the origins are integers that sum to
    the round budget, differ by at most one unit, and the remainder goes to the first hotkeys in
    UTF-8 order; a signed allocation that hands it elsewhere is REWARD_ALLOCATION."""
    keys = [miner(i) for i in range(n)]
    plan = build_plan(li_escrow, setup, keys[::-1] if reverse else keys)
    reward, evidence = plan.build()
    escrow = ledger_for(li_escrow, setup, plan)
    escrow.reward_finalize(reward, evidence)
    units = {
        row["hotkey"]: row["issued"]
        for row in escrow.db.execute("SELECT hotkey,issued FROM escrow_origins")
    }
    base, remainder = divmod(BUDGET, n)
    ranked = sorted(units, key=lambda hotkey: hotkey.encode())
    assert sum(units.values()) == BUDGET == escrow.balances().reward_pending
    assert [units[h] for h in ranked] == [base + 1] * remainder + [base] * (n - remainder)
    assert units == split_units(BUDGET, list(units))
    if 0 < remainder < n:
        swapped = {h: base for h in ranked} | {ranked[-1]: base + remainder}
        wrong = plan.reward(evidence, units=swapped)
        fresh = ledger_for(li_escrow, setup, plan)
        assert refusal(lambda: fresh.reward_finalize(wrong, evidence)) == "REWARD_ALLOCATION"
        assert fresh.balances().issued == 0


@pytest.mark.parametrize("amount", [0, -1, 2**53, True, 1.0, "5", None])
def test_a_unit_amount_is_an_exact_integer_between_one_and_two_to_the_53_minus_one(
    li_escrow: ModuleType, setup: Any, amount: Any
) -> None:
    """CRD-10: the amounts of a round (its budget and each origin) are not zero, negative,
    fractional, boolean, textual or beyond the integers every verifier can represent."""
    plan = build_plan(li_escrow, setup, [miner(0)])
    reward, evidence = plan.build()
    origin = OriginAllocation(
        origin_id=reward.origin_ids[0], owner=owner(0).ss58, units=1, mature_at=20
    )
    with pytest.raises(ValidationError):
        RewardFinalize.model_validate({**reward.body(), "budget_units": amount})
    with pytest.raises(ValidationError):
        OriginAllocation.model_validate({**origin.body(), "units": amount})
    for edge in (1, 2**53 - 1):
        assert OriginAllocation.model_validate({**origin.body(), "units": edge}).units == edge
        assert RewardFinalize.model_validate({**reward.body(), "budget_units": edge})


# --- the store: finalize_v2 on a real audited and aggregated round (CRD-02/05/08/09/12/23) -----


@pytest.fixture(scope="module")
def audited_round(tmp_path_factory: pytest.TempPathFactory) -> Iterator[AuditedRound]:
    yield from build_audited_round(tmp_path_factory)


def test_finalize_settles_a_round_once_and_every_retry_returns_the_stored_result(
    audited_round: AuditedRound,
) -> None:
    """CRD-02 CRD-06 CRD-10 CRD-12 CRD-20: the coordinator's Finalize credits the four audited
    miners an equal quarter each, to the coldkey bound at join, and records the settlement of each
    origin. The same Finalize again, also after a restart, returns the stored answer and changes
    nothing; another Finalize body for the round is a 409 that changes nothing."""
    round_ = audited_round
    round_.restore()
    store = round_.store
    first = round_.finalize()
    assert first.status_code == 200, first.text
    assert first.json()["status"] == "SETTLED" and first.json()["reward_receipt"]
    owners = {
        key.ss58: cold.ss58
        for key, cold in zip(round_.fixtures.HOT, round_.fixtures.COLD, strict=True)
    }
    assert credited(round_) == [(h, BUDGET // 4, 0, owners[h]) for h in round_.hotkeys]
    final_hash = digest(Finalize.model_validate(round_.finalize_request()["body"]))
    records = store._db.execute("SELECT id,data FROM records_v2 WHERE kind='settlement'").fetchall()
    assert {r["id"] for r in records} == {
        row["origin"]
        for row in store._db.execute("SELECT origin FROM escrow_origins WHERE hotkey IS NOT NULL")
    }
    assert {json.loads(r["data"])["finality_hash"] for r in records} == {final_hash}
    finality = store._record_v2(round_.run_id, "finality", "0")
    assert (finality["disposition"], finality["resolution_hash"]) == ("SETTLED", final_hash)
    settled = database_digest(store)

    again = round_.finalize()
    assert (again.status_code, again.json()) == (200, first.json())
    store._services_v2.clear()  # a restarted master: the stored answer needs no service
    restarted = round_.finalize()
    assert (restarted.status_code, restarted.json()) == (200, first.json())
    other = round_.finalize(entitlements_root="22" * 32)
    assert (other.status_code, other.json()["detail"]) == (
        409,
        "conflicting signed operation nonce",
    )
    assert database_digest(store) == settled


def test_a_finalize_whose_included_set_differs_from_the_audited_set_is_refused_and_can_be_corrected(
    audited_round: AuditedRound,
) -> None:
    """CRD-08 CRD-02: the included set must be exactly the miners the service derived, one
    missing or one stranger is a 422; the refusal persists nothing, not even the Finalize's
    signed-operation reservation, so the corrected Finalize is then accepted."""
    round_ = audited_round
    round_.restore()
    before = database_digest(round_.store)
    for included in (round_.hotkeys[1:], sorted([*round_.hotkeys, STRANGER.ss58])):
        refused = round_.finalize(included=included)
        assert refused.status_code == 422, refused.text
        assert "included set differs" in refused.json()["detail"]
        assert database_digest(round_.store) == before
    corrected = round_.finalize()
    assert corrected.status_code == 200 and len(credited(round_)) == 4


def test_a_finalize_that_the_coordinator_did_not_sign_credits_nothing(
    audited_round: AuditedRound,
) -> None:
    """CRD-09 CRD-08: the Finalize is the one input the caller supplies, and only the coordinator
    key pinned in the manifest can sign it; another key, here a pinned auditor, is refused and
    nothing is persisted."""
    round_ = audited_round
    round_.restore()
    before = database_digest(round_.store)
    auditor = round_.fixtures.AUDITORS[0]
    forged = round_.network.signed(
        auditor, "Finalize", Finalize(**round_.finalize_request()["body"])
    )
    refused = round_.network.client.post(
        round_.network.url + "/admin/rounds/0/finalize",
        json=forged,
        headers=round_.fixtures.admin(),
    )
    assert (refused.status_code, refused.json()["detail"]) == (
        409,
        "signer is not authorized for message role",
    )
    assert database_digest(round_.store) == before and credited(round_) == []


def test_a_ledger_refusal_after_the_ledger_wrote_leaves_the_round_untouched_and_retryable(
    audited_round: AuditedRound, monkeypatch: pytest.MonkeyPatch
) -> None:
    """CRD-05 CRD-02: finalize_v2 settles the round, stores the Finalize and its replay record,
    credits the ledger and records the settlements in one transaction. A refusal raised after
    the ledger wrote undoes all of it; the same Finalize is then accepted."""
    round_ = audited_round
    round_.restore()
    store = round_.store
    escrow = store._services(round_.run_id)[0]
    before = database_digest(store)
    real = escrow.reward_finalize

    def refuse_after_writing(reward: Any, evidence: Any) -> Any:
        real(reward, evidence)
        raise EscrowError("TOTAL_ISSUANCE_BUDGET")

    monkeypatch.setattr(escrow, "reward_finalize", refuse_after_writing)
    refused = round_.finalize()
    assert (refused.status_code, refused.json()["detail"]) == (409, "TOTAL_ISSUANCE_BUDGET")
    assert database_digest(store) == before and credited(round_) == []
    monkeypatch.undo()
    assert round_.finalize().status_code == 200
    assert len(credited(round_)) == 4


def test_a_stored_verdict_that_no_pinned_auditor_signed_mints_nothing(
    audited_round: AuditedRound,
) -> None:
    """CRD-09 CRD-07: the evidence is read from the service's records but every signature in it
    is checked again when the credit is made; a verdict record re-signed by a stranger is
    refused, the round stays unsettled and nothing is credited."""
    round_ = audited_round
    round_.restore()
    store, run = round_.store, round_.run_id
    identity = f"0:{round_.hotkeys[0]}"
    stored = store._record_v2(run, "verdict", identity)
    forged = seal(STRANGER, "ReplayVerdict", run, stored["body"], 10_000)
    with store._tx():
        store._put_record_v2(run, "verdict", identity, forged)
    before = database_digest(store)
    refused = round_.finalize()
    assert (refused.status_code, refused.json()["detail"]) == (
        409,
        "signer is not authorized for message role",
    )
    assert database_digest(store) == before and credited(round_) == []


def test_a_rotation_after_the_credit_does_not_move_the_credit_to_another_identity(
    audited_round: AuditedRound,
) -> None:
    """CRD-12: the origins keep the hotkey that earned them and the coldkey that owned it when
    the work was done, whatever the admission becomes afterwards."""
    round_ = audited_round
    round_.restore()
    assert round_.finalize().status_code == 200
    before = credited(round_)
    first = round_.fixtures.HOT[0]
    request = RotateRequest(
        hotkey=first.ss58, new_hotkey=round_.fixtures.REFEREE.ss58, operation_id="df" * 32
    )
    rotated = round_.network.client.post(
        round_.network.url + "/admission/" + first.ss58 + "/rotate",
        json=sign_rotation(round_.fixtures.COLD[0], round_.run_id, request, 10_000),
    )
    assert rotated.status_code == 200, rotated.text
    assert credited(round_) == before
    assert round_.store._owner_v2(round_.run_id, round_.fixtures.REFEREE.ss58) == (
        round_.fixtures.COLD[0].ss58
    )


def test_one_audit_lease_per_miner_and_round_completes_once_and_scheduling_adds_none(
    audited_round: AuditedRound,
) -> None:
    """CRD-23: a round has exactly one audit lease per committed miner under a name derived from
    (run, round, miner), each completed once, over samples no other miner of the round holds (the
    condition that makes identical work impossible to credit twice). Scheduling the audits again
    creates no second lease, completing a lease again returns its stored result, and a contrary
    verdict for it is a 409; none of the three changes the store."""
    round_ = audited_round
    round_.restore()
    store, run, network = round_.store, round_.run_id, round_.network
    leases = store._db.execute("SELECT id,hotkey,state FROM audit_leases_v2 WHERE w=0").fetchall()
    assert sorted(r["hotkey"] for r in leases) == round_.hotkeys
    assert {r["id"] for r in leases} == {
        sha256_hex(f"ht-audit-job/2|{run}|0|{hotkey}".encode()) for hotkey in round_.hotkeys
    }
    assert {r["state"] for r in leases} == {"completed"}
    samples = [
        set(store._record_v2(run, "assignment", f"0:{hotkey}")["samples"])
        for hotkey in round_.hotkeys
    ]
    assert sum(len(held) for held in samples) == len(set().union(*samples)) > 0
    before = database_digest(store)
    again = network.client.post(
        network.url + "/admin/rounds/0/audits", headers=round_.fixtures.admin()
    )
    assert again.status_code == 200, again.text
    lease = next(r for r in leases if r["hotkey"] == round_.hotkeys[0])
    verdict = store._record_v2(run, "verdict", f"0:{lease['hotkey']}")
    complete = network.url + "/worker/jobs/" + lease["id"] + "/complete"
    repeated = network.client.post(complete, json=verdict)
    assert (repeated.status_code, repeated.json()) == (200, {"result": "MATCH"})
    auditor = next(k for k in round_.fixtures.AUDITORS if k.ss58 == verdict["signer"])
    contrary = {**verdict["body"], "result": "MISMATCH", "first_bad_leaf": 0}
    refused = network.client.post(complete, json=network.signed(auditor, "ReplayVerdict", contrary))
    assert (refused.status_code, refused.json()["detail"]) == (
        409,
        "conflicting signed operation nonce",
    )
    assert database_digest(store) == before


def test_a_finalize_is_refused_when_the_tape_does_not_replay_to_the_claimed_state(
    audited_round: AuditedRound,
) -> None:
    """CRD-09: the stored acceptance of a round is not the authority for its state: the tape is
    replayed again when the credit is made. A Finalize that agrees with a tampered applied record
    is refused, and nothing is credited."""
    round_ = audited_round
    round_.restore()
    store, run = round_.store, round_.run_id
    forged = "ee" * 32
    with store._tx():
        applied = store._record_v2(run, "applied", "0")
        store._put_record_v2(run, "applied", "0", {**applied, "theta_hash": forged})
        finality = store._record_v2(run, "finality", "0")
        store._put_record_v2(run, "finality", "0", {**finality, "state_hash": forged})
    before = database_digest(store)
    refused = round_.finalize(final_theta_hash_w1=forged)
    assert (refused.status_code, refused.json()["detail"]) == (
        409,
        "FINALITY_TAPE_REPLAY_MISMATCH",
    )
    assert database_digest(store) == before and credited(round_) == []


def test_a_miner_commits_once_per_round_a_replay_returns_the_receipt_and_another_body_is_refused(
    audited_round: AuditedRound,
) -> None:
    """CRD-23: the commit a delta is credited through is accepted once per (round, miner): the
    same body again returns the first receipt, a different body is a 409, and neither rewrites
    the accepted commit."""
    round_ = audited_round
    round_.restore()
    store, network = round_.store, round_.network
    hot = round_.fixtures.HOT[0]

    def accepted() -> list[tuple[Any, ...]]:
        rows = store._db.execute(
            "SELECT * FROM accepted_v2 WHERE key LIKE '%CommitV2%' ORDER BY key"
        ).fetchall()
        return [tuple(row) for row in rows]

    stored = store._record_v2(round_.run_id, "commit", f"0:{hot.ss58}")
    first = accepted()
    same = network.client.post(
        network.url + "/commit", json=network.signed(hot, "CommitV2", stored["body"])
    )
    assert same.status_code == 200 and same.json()["body"]["w"] == 0
    assert accepted() == first
    settled = database_digest(store)
    other = network.client.post(
        network.url + "/commit",
        json=network.signed(hot, "CommitV2", {**stored["body"], "leaves_root": "ab" * 32}),
    )
    assert (other.status_code, other.json()["detail"]) == (
        409,
        "conflicting signed operation nonce",
    )
    assert database_digest(store) == settled
    assert store._record_v2(round_.run_id, "commit", f"0:{hot.ss58}") == stored


@pytest.mark.xfail(
    strict=True,
    raises=sqlite3.OperationalError,
    reason="CRD-05+CRD-D03: the v2 services of a restarted store are built by the first handler "
    "that needs them; finalize_v2 is one, and builds them inside its transaction, where "
    "EscrowV2.__init__ changes PRAGMA synchronous and fails, so the first Finalize after a "
    "restart is a server error until another route has built the services",
)
def test_the_first_finalize_after_a_restart_is_accepted(audited_round: AuditedRound) -> None:
    round_ = audited_round
    round_.restore()
    assert round_.finalize().status_code == 200  # control: the services are warm
    round_.restore()
    round_.store._services_v2.clear()  # the process was restarted; no route has run yet
    assert round_.finalize().status_code == 200
