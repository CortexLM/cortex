"""Origins that are not a round's reward, settlement and conservation (Lens CRD-05, CRD-08,
CRD-11..CRD-24): genesis, bootstrap and shadow issuance, the settlement a maturity needs, the cap,
conservation, and the store-side owner and atomicity conditions of a credit.

Every test states a defence the protocol must hold: the ledger or the store refuses X and its
durable state is unchanged, units are conserved, or a credit is made once. Test keys only.
"""

from __future__ import annotations

import sqlite3
from collections.abc import Callable
from dataclasses import replace
from functools import partial
from pathlib import Path
from types import ModuleType
from typing import Any

import pytest
from hypothesis import example, given
from hypothesis import strategies as st

from env_pin_fixtures import honest_env
from hypertrain.aggregator.core import FinalityError
from hypertrain.challenge.finality_v2 import RoundStatus, settle
from hypertrain.challenge.store import ChallengeError
from hypertrain.ledger.escrow_v2 import (
    EscrowV2,
    Settlement,
    bootstrap_origin_id,
    digest,
    genesis_allocation_hash,
    genesis_origin_id,
)
from hypertrain.miner.admission import sign_join
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages import Finalize
from hypertrain.protocol.messages_v2 import (
    EscrowLock,
    EscrowOperation,
    EscrowRelease,
    EscrowTransfer,
    HardwareHint,
)

from .support import (
    SERVICE_FIXTURE,
    H,
    hosted_reward,
    load_fixture_module,
    match_settlement,
    memory_db,
    registered_owners,
    registered_settlements,
    service_world,
)
from .support_credit import (
    BUDGET,
    HOTKEYS,
    STRANGER,
    build_plan,
    database_digest,
    event_count,
    issued_id,
    ledger_for,
    ledger_rows,
    make_bootstrap,
    make_genesis,
    miner,
    owner,
    refusal,
    repin,
    reserve_shadow_round,
    reward_origins,
    rotation_envelope,
    split_units,
)


@pytest.fixture(scope="module")
def setup(li_escrow: ModuleType) -> Any:
    return li_escrow.setup("production")


def reopen(li_escrow: ModuleType, setup: Any, database: Any) -> EscrowV2:
    """A ledger over `database` with the fixture defaults: verify() runs on construction."""
    ledger: EscrowV2 = li_escrow.ledger(setup, database)
    return ledger


# --- CRD-16: test genesis ------------------------------------------------------------------------

GENESIS_FAULTS: dict[str, tuple[dict[str, Any], str]] = {
    "signed_by_a_stranger": ({"signer": STRANGER}, "ORIGIN_AUTHORITY"),
    "made_for_another_run": ({"claimed_run": H}, "ORIGIN_AUTHORITY"),
    "allocation_hash_is_not_the_pinned_one": ({"pinned_hash": "ff" * 32}, "GENESIS_ALLOCATION"),
    "origin_id_outside_the_genesis_domain": ({"origin_ids": {0: H}}, "GENESIS_ORIGIN_DOMAIN"),
}


@pytest.mark.parametrize("fault", GENESIS_FAULTS)
def test_a_genesis_that_is_not_the_pinned_one_is_refused_with_its_code(
    li_escrow: ModuleType, fault: str
) -> None:
    """CRD-16: the test genesis is signed by the reward authority for this run, carries the
    allocation hash the policy pins and origin ids of the genesis domain; anything else is
    refused and the ledger stays empty."""
    escrow = reopen(li_escrow, li_escrow.setup("test"), memory_db())
    changes, code = GENESIS_FAULTS[fault]
    genesis = make_genesis(li_escrow, escrow.run_id, **changes)
    assert refusal(lambda: escrow.test_genesis(genesis)) == code
    assert event_count(escrow) == 0 and escrow.balances().issued == 0


def test_another_allocation_than_the_pinned_one_is_not_a_genesis(li_escrow: ModuleType) -> None:
    """CRD-16: a self-consistent genesis of units the policy did not pin is refused."""
    escrow = reopen(li_escrow, li_escrow.setup("test"), memory_db())
    other = [(li_escrow.COLD.ss58, 20001, 1), (li_escrow.OTHER.ss58, 9999, 1)]
    genesis = make_genesis(li_escrow, escrow.run_id, allocation=other)
    assert refusal(lambda: escrow.test_genesis(genesis)) == "GENESIS_ALLOCATION"
    assert escrow.balances().issued == 0


def test_genesis_is_test_mode_only_and_only_ever_the_first_event(li_escrow: ModuleType) -> None:
    """CRD-16: a production ledger never accepts a test genesis, nor does a test ledger once any
    event, here a reward, exists."""
    production = reopen(li_escrow, li_escrow.setup("production"), memory_db())
    assert (
        refusal(lambda: production.test_genesis(make_genesis(li_escrow, production.run_id)))
        == "TEST_GENESIS_FORBIDDEN"
    )
    test_setup = li_escrow.setup("test")
    plan = build_plan(li_escrow, test_setup, [miner(0)])
    reward, evidence = plan.build()
    escrow = ledger_for(li_escrow, test_setup, plan)
    escrow.reward_finalize(reward, evidence)
    rows = ledger_rows(escrow)
    genesis = make_genesis(li_escrow, escrow.run_id)
    assert refusal(lambda: escrow.test_genesis(genesis)) == "TEST_GENESIS_FORBIDDEN"
    assert ledger_rows(escrow) == rows and production.balances().issued == 0


def test_genesis_replays_conflicts_on_another_body_and_its_units_are_never_payable(
    li_escrow: ModuleType,
) -> None:
    """CRD-16: the same genesis again returns the first receipt, another body under the run's
    genesis operation is OPERATION_CONFLICT, and the matured genesis units are free to move but
    are never emission: payable() lists none of them."""
    escrow = reopen(li_escrow, li_escrow.setup("test"), memory_db())
    genesis = make_genesis(li_escrow, escrow.run_id)
    first = escrow.test_genesis(genesis)
    assert escrow.test_genesis(make_genesis(li_escrow, escrow.run_id)) == first
    changed = make_genesis(
        li_escrow,
        escrow.run_id,
        allocation=[(li_escrow.COLD.ss58, 20000, 2), (li_escrow.OTHER.ss58, 10000, 1)],
    )
    rows = ledger_rows(escrow)
    assert refusal(lambda: escrow.test_genesis(changed)) == "OPERATION_CONFLICT"
    assert ledger_rows(escrow) == rows
    escrow.mature(H, tuple(o.origin_id for o in genesis.origins), 1)
    assert escrow.balances().available == 30000 and escrow.balances().conserved()
    assert escrow.payable(10**9) == []


def test_genesis_may_issue_up_to_the_pinned_cap_and_not_one_unit_more(
    li_escrow: ModuleType,
) -> None:
    """CRD-11 CRD-16: a genesis of exactly max_total_issuance is accepted, one unit over is
    refused and issues nothing."""
    for cap, accepted in ((30_000, True), (29_999, False)):
        escrow = reopen(
            li_escrow, repin(li_escrow.setup("test"), max_total_issuance=cap), memory_db()
        )
        genesis = make_genesis(li_escrow, escrow.run_id)
        if accepted:
            escrow.test_genesis(genesis)
            assert escrow.balances().issued == cap
        else:
            assert refusal(partial(escrow.test_genesis, genesis)) == "GENESIS_ALLOCATION"
            assert escrow.balances().issued == 0


# --- CRD-17: production bootstrap ----------------------------------------------------------------

BOOT_OP = "44" * 32


def bootstrap_allocation(li_escrow: ModuleType) -> list[tuple[str, int, int]]:
    return [(li_escrow.COLD.ss58, 20000, 1), (li_escrow.OTHER.ss58, 10000, 1)]


def bootstrap_setup(li_escrow: ModuleType, **policy: Any) -> Any:
    pin = genesis_allocation_hash(bootstrap_allocation(li_escrow))
    return repin(li_escrow.setup("production"), bootstrap_allocation_hash=pin, **policy)


BOOTSTRAP_FAULTS: dict[str, tuple[dict[str, Any], str]] = {
    "signed_by_a_stranger": ({"signer": STRANGER}, "ORIGIN_AUTHORITY"),
    "made_for_another_run": ({"claimed_run": H}, "ORIGIN_AUTHORITY"),
    "allocation_hash_is_not_the_pinned_one": ({"pinned_hash": "ff" * 32}, "BOOTSTRAP_ALLOCATION"),
    "allocation_is_consistent_but_not_the_pinned_one": (
        {"allocation": lambda li: [(li.COLD.ss58, 30000, 1)]},
        "BOOTSTRAP_ALLOCATION",
    ),
    "origin_id_outside_the_bootstrap_domain": (
        {"origin_ids": {1: H}},
        "BOOTSTRAP_ORIGIN_DOMAIN",
    ),
}


@pytest.mark.parametrize("fault", BOOTSTRAP_FAULTS)
def test_a_bootstrap_that_is_not_the_pinned_one_is_refused_with_its_code(
    li_escrow: ModuleType, fault: str
) -> None:
    """CRD-17: the bootstrap is signed by the reward authority for this run, carries the pinned
    allocation hash and origin ids of the bootstrap domain; anything else issues nothing."""
    escrow = reopen(li_escrow, bootstrap_setup(li_escrow), memory_db())
    changes, code = dict(BOOTSTRAP_FAULTS[fault][0]), BOOTSTRAP_FAULTS[fault][1]
    allocation = changes.pop("allocation", bootstrap_allocation)(li_escrow)
    bootstrap = make_bootstrap(li_escrow, escrow.run_id, allocation, operation=BOOT_OP, **changes)
    assert refusal(lambda: escrow.production_bootstrap(bootstrap)) == code
    assert event_count(escrow) == 0 and escrow.balances().issued == 0


@pytest.mark.parametrize("fault", ["unpinned_policy", "test_mode", "above_the_cap", "second_one"])
def test_a_bootstrap_outside_its_conditions_is_refused_and_issues_nothing_more(
    li_escrow: ModuleType, fault: str
) -> None:
    """CRD-17 CRD-11: production mode with a pin, once per run, within the cap."""
    allocation = bootstrap_allocation(li_escrow)
    setup = {
        "unpinned_policy": lambda: li_escrow.setup("production"),
        "test_mode": lambda: li_escrow.setup("test"),
        "above_the_cap": lambda: bootstrap_setup(li_escrow, max_total_issuance=29_999),
        "second_one": lambda: bootstrap_setup(li_escrow),
    }[fault]()
    escrow = reopen(li_escrow, setup, memory_db())
    if fault == "second_one":
        escrow.production_bootstrap(
            make_bootstrap(li_escrow, escrow.run_id, allocation, operation=BOOT_OP)
        )
    rows = ledger_rows(escrow)
    attempt = make_bootstrap(
        li_escrow,
        escrow.run_id,
        allocation,
        operation="55" * 32 if fault == "second_one" else BOOT_OP,
    )
    expected = (
        "TOTAL_ISSUANCE_BUDGET" if fault == "above_the_cap" else "PRODUCTION_BOOTSTRAP_FORBIDDEN"
    )
    assert refusal(lambda: escrow.production_bootstrap(attempt)) == expected
    assert ledger_rows(escrow) == rows


@pytest.mark.parametrize("kind", ["genesis", "bootstrap"])
def test_origins_that_do_not_hash_to_the_pinned_allocation_are_refused(
    li_escrow: ModuleType, kind: str
) -> None:
    """CRD-16 CRD-17: an issuance can name the pinned allocation hash, and origin ids derived from
    it, and still carry other units. The ledger hashes the origins it is given, so that issuance
    is refused and issues nothing."""
    other = [(li_escrow.COLD.ss58, 30000, 1)]
    if kind == "genesis":
        escrow = reopen(li_escrow, li_escrow.setup("test"), memory_db())
        pin = escrow.policy.genesis_allocation_hash
        assert pin is not None
        forged = make_genesis(
            li_escrow,
            escrow.run_id,
            allocation=other,
            pinned_hash=pin,
            origin_ids={0: issued_id(li_escrow, genesis_origin_id, escrow.run_id, pin, 0)},
        )
        issue: Callable[[], object] = partial(escrow.test_genesis, forged)
        code = "GENESIS_ALLOCATION"
    else:
        escrow = reopen(li_escrow, bootstrap_setup(li_escrow), memory_db())
        pin = escrow.policy.bootstrap_allocation_hash
        assert pin is not None
        forged_bootstrap = make_bootstrap(
            li_escrow,
            escrow.run_id,
            other,
            operation=BOOT_OP,
            pinned_hash=pin,
            origin_ids={0: issued_id(li_escrow, bootstrap_origin_id, escrow.run_id, pin, 0)},
        )
        issue = partial(escrow.production_bootstrap, forged_bootstrap)
        code = "BOOTSTRAP_ALLOCATION"
    assert refusal(issue) == code
    assert event_count(escrow) == 0 and escrow.balances().issued == 0


# --- CRD-18 / CRD-19: shadow origins -------------------------------------------------------------


def shadow_ledger(li_escrow: ModuleType, setup: Any, plan: Any) -> EscrowV2:
    """A ledger that serves shadow rewards: the shadow settlement callback is mandatory."""
    return ledger_for(
        li_escrow,
        setup,
        plan,
        settlement=lambda _: match_settlement(digest(plan.finalize())),
    )


def test_shadow_ordinals_are_consecutive_unique_bounded_and_reserved_one_at_a_time(
    li_escrow: ModuleType, setup: Any
) -> None:
    """CRD-18: the k-th accepted shadow reward holds ordinal k with its own origin, a second
    reservation waits for the pending one, a replay returns the first receipt, and a thirteenth
    ordinal does not exist."""
    first = build_plan(li_escrow, setup, [miner(0)], w=0)
    escrow = shadow_ledger(li_escrow, setup, first)
    origins = []
    for k in range(12):
        plan = build_plan(li_escrow, setup, [miner(0)], w=k)
        binding, shadow, evidence = reserve_shadow_round(li_escrow, escrow, plan, f"{k + 1:064x}")
        assert binding.shadow_ordinal == k
        if k == 0:
            waiting = build_plan(li_escrow, setup, [miner(0)], w=1)
            blocked = refusal(
                partial(reserve_shadow_round, li_escrow, escrow, waiting, f"{99:064x}")
            )
            assert blocked == "SHADOW_RESERVATION_PENDING"
        receipt = escrow.reward_finalize(shadow, evidence)
        assert escrow.reward_finalize(shadow, evidence) == receipt
        origins.append(shadow.origin_ids[0])
    assert len(set(origins)) == 12
    assert rows_of(escrow, "SELECT ordinal FROM escrow_shadow_finalized ORDER BY ordinal") == [
        (k,) for k in range(12)
    ]
    rows = ledger_rows(escrow)
    last = build_plan(li_escrow, setup, [miner(0)], w=12)
    assert refusal(lambda: reserve_shadow_round(li_escrow, escrow, last, f"{13:064x}")) == (
        "BOOTSTRAP_FINISHED"
    )
    assert ledger_rows(escrow) == rows
    assert escrow.balances().issued == 12 * BUDGET and reward_origins(escrow) == []
    escrow.verify()


def rows_of(escrow: EscrowV2, sql: str) -> list[tuple[Any, ...]]:
    return [tuple(row) for row in escrow.db.execute(sql)]


def test_shadow_and_live_origins_of_one_round_coexist_but_only_live_units_are_emission(
    li_escrow: ModuleType, setup: Any
) -> None:
    """CRD-19 (condition: a reader of reward_round must tell the two apart). A shadow trial of
    epoch 0 and the live round 0 of the same miner both hold an origin with reward_round 0.
    The naive count of that column sees two, the disambiguated count one; after maturity
    payable() lists the live origin only."""
    plan = build_plan(li_escrow, setup, [miner(0)], w=0)
    escrow = shadow_ledger(li_escrow, setup, plan)
    _, shadow, shadow_evidence = reserve_shadow_round(li_escrow, escrow, plan, f"{1:064x}")
    live, live_evidence = plan.build()
    escrow.reward_finalize(shadow, shadow_evidence)
    escrow.reward_finalize(live, live_evidence)
    assert set(shadow.origin_ids).isdisjoint(live.origin_ids)
    hot = miner(0).ss58
    naive = rows_of(
        escrow, f"SELECT COUNT(*) FROM escrow_origins WHERE reward_round=0 AND hotkey='{hot}'"
    )
    assert naive == [(2,)]
    assert reward_origins(escrow) == [(0, hot)]
    escrow.mature("66" * 32, tuple(shadow.origin_ids + live.origin_ids), 20)
    assert escrow.balances().available == 2 * BUDGET
    assert [(row["origin"], row["units"]) for row in escrow.payable(10**9)] == [
        (live.origin_ids[0], BUDGET)
    ]


# --- CRD-20: the settlement a maturity needs -----------------------------------------------------

SETTLEMENT_VIEWS: dict[str, tuple[Callable[[str], Settlement | None], str]] = {
    "none_recorded": (lambda finality: None, "UNACCEPTED_SETTLEMENT"),
    "unresolved": (
        lambda finality: match_settlement(finality).model_copy(update={"unresolved": True}),
        "DISPUTED_OR_UNVESTED",
    ),
    "fraud": (
        lambda finality: match_settlement(finality).model_copy(update={"outcome": "FRAUD"}),
        "DISPUTED_OR_UNVESTED",
    ),
    "release_beacon_not_reached": (
        lambda finality: match_settlement(finality, release_beacon=21),
        "DISPUTED_OR_UNVESTED",
    ),
    "another_finality_hash": (lambda finality: match_settlement(H), "DISPUTED_OR_UNVESTED"),
}


@pytest.mark.parametrize("view", [*SETTLEMENT_VIEWS, "control"])
def test_a_reward_origin_matures_only_on_an_accepted_resolved_clean_vested_settlement(
    li_escrow: ModuleType, setup: Any, view: str
) -> None:
    """CRD-20: the ledger releases reward units only against a settlement the service accepted
    for this origin, not disputed, not fraudulent, vested at the beacon and bound to the
    finalize hash recorded for the round; each other answer leaves the units pending."""
    plan = build_plan(li_escrow, setup, HOTKEYS)
    reward, evidence = plan.build()
    make, code = SETTLEMENT_VIEWS.get(view, (lambda finality: match_settlement(finality), ""))
    table = {
        origin: settlement
        for origin in reward.origin_ids
        if (settlement := make(reward.finalize_hash)) is not None
    }
    escrow = ledger_for(li_escrow, setup, plan, settlement=registered_settlements(table))
    escrow.reward_finalize(reward, evidence)
    rows = ledger_rows(escrow)
    origins = tuple(reward.origin_ids)
    if view == "control":
        escrow.mature("77" * 32, origins, 20)
        assert (escrow.balances().available, escrow.balances().reward_pending) == (BUDGET, 0)
        return
    assert refusal(lambda: escrow.mature("77" * 32, origins, 20)) == code
    assert ledger_rows(escrow) == rows


def test_one_origin_in_fraud_keeps_every_origin_of_the_call_pending(
    li_escrow: ModuleType, setup: Any
) -> None:
    """CRD-20: a maturity is all or nothing; a clean origin does not pass with a fraudulent one."""
    plan = build_plan(li_escrow, setup, HOTKEYS)
    reward, evidence = plan.build()
    table = {origin: match_settlement(reward.finalize_hash) for origin in reward.origin_ids}
    table[reward.origin_ids[1]] = table[reward.origin_ids[1]].model_copy(
        update={"outcome": "FRAUD"}
    )
    escrow = ledger_for(li_escrow, setup, plan, settlement=registered_settlements(table))
    escrow.reward_finalize(reward, evidence)
    rows = ledger_rows(escrow)
    code = refusal(lambda: escrow.mature("77" * 32, tuple(reward.origin_ids), 20))
    assert code == "DISPUTED_OR_UNVESTED" and ledger_rows(escrow) == rows
    escrow.mature("78" * 32, (reward.origin_ids[0],), 20)
    assert escrow.balances().available == BUDGET // 2


def test_an_origin_is_not_matured_before_its_own_maturity_round(
    li_escrow: ModuleType, setup: Any
) -> None:
    """CRD-20: even with a vested settlement the beacon must have reached the origin's mature_at."""
    plan = build_plan(li_escrow, setup, [miner(0)])
    reward, evidence = plan.build()
    table = {origin: match_settlement(reward.finalize_hash, 1) for origin in reward.origin_ids}
    escrow = ledger_for(li_escrow, setup, plan, settlement=registered_settlements(table))
    escrow.reward_finalize(reward, evidence)
    rows = ledger_rows(escrow)
    code = refusal(lambda: escrow.mature("77" * 32, tuple(reward.origin_ids), 19))
    assert code == "IMMATURE_OR_UNKNOWN_ORIGIN" and ledger_rows(escrow) == rows


def test_the_service_settlement_is_the_record_the_service_wrote_and_nothing_else(
    li_escrow: ModuleType, setup: Any, tmp_path: Path
) -> None:
    """CRD-20: with the store's own callback an origin whose finalize left no settlement record
    answers UNACCEPTED_SETTLEMENT, as does any reference nobody recorded, before and after the
    service writes the records of its reward; the records of the reward then let its maturity
    succeed."""
    plan = build_plan(li_escrow, setup, HOTKEYS)
    reward, evidence = plan.build()
    with hosted_reward(tmp_path, li_escrow, (setup, evidence, reward), real_settlement=True) as h:
        h.escrow.replay_finality = plan.replay
        h.escrow.owner_of = registered_owners(plan.owners)
        with h.store._tx():
            h.escrow.reward_finalize(reward, evidence)  # a ledger credit without a record
        origins = tuple(reward.origin_ids)
        assert refusal(lambda: h.escrow.mature("77" * 32, origins, 20)) == "UNACCEPTED_SETTLEMENT"
        assert refusal(lambda: h.store._settlement_v2(h.run_id, H)) == "UNACCEPTED_SETTLEMENT"
        assert h.escrow.balances().reward_pending == BUDGET
        h.finalize()
        assert refusal(lambda: h.store._settlement_v2(h.run_id, H)) == "UNACCEPTED_SETTLEMENT"
        h.escrow.mature("77" * 32, origins, 20)
        assert h.escrow.balances().available == BUDGET and h.escrow.balances().conserved()


# --- CRD-24: conservation ------------------------------------------------------------------------

OPERATIONS = st.one_of(
    st.tuples(st.sampled_from(["send", "back", "lock", "pay"]), st.integers(1, 12000)),
    st.sampled_from([("release", 0), ("restart", 0)]),
)


@given(operations=st.lists(OPERATIONS, max_size=12))
@example(
    operations=[
        ("send", 12000),
        ("back", 12000),
        ("lock", 9000),
        ("lock", 9000),
        ("release", 0),
        ("pay", 20000),
        ("restart", 0),
    ]
)
@example(operations=[("lock", 1), ("lock", 99), ("lock", 100), ("lock", 19900), ("lock", 1)])
def test_units_are_conserved_by_every_sequence_of_ledger_operations(
    li_escrow: ModuleType, operations: list[tuple[str, int]]
) -> None:
    """CRD-24: transfers, locks, releases, pays and restarts, accepted or refused (for want of
    funds, or as dust for a lock below the minimum), keep issued equal to the sum of the buckets
    and to what each owner holds, and every restart re-verifies the event chain."""
    setup = li_escrow.setup("test")
    database = memory_db()
    escrow = reopen(li_escrow, setup, database)
    cold_origin, _ = li_escrow.fund(escrow)
    cold, other = li_escrow.COLD.ss58, li_escrow.OTHER.ss58
    free, abroad, locks, paid = 20000, 0, [], 0
    for step, (kind, amount) in enumerate(operations):
        identity = sha256_hex(f"credit-operation|{step}".encode())
        fields: dict[str, Any] = {
            "operation_id": identity,
            "units": amount,
            "origin_ids": [cold_origin],
            "admission_id": None,
            "dispute_id": None,
        }
        affordable = amount <= (abroad if kind == "back" else free)
        # a new admission lock below a tenth of the funding floor is dust and is refused before
        # anything is checked against the funds (escrow-relay-fixes, ADR 0014)
        dust = kind == "lock" and amount < max(1, escrow.policy.S_min_units // 10)
        call: Callable[[], object] | None = None
        if kind == "send":
            op = EscrowTransfer(owner=cold, recipient=other, **fields)
            call = partial(escrow.transfer, op, signer=cold)
        elif kind == "back":
            op = EscrowTransfer(owner=other, recipient=cold, **fields)
            call = partial(escrow.transfer, op, signer=other)
        elif kind == "lock":
            lock = li_escrow.lock(cold_origin, identity, amount)
            call = partial(escrow.lock, lock, signer=cold)
        elif kind == "pay":
            pay = EscrowOperation(owner=cold, **fields)
            call = partial(escrow.pay, pay, signer=cold)
        elif kind == "release" and locks:
            lock_id, units = locks.pop(0)
            release = EscrowRelease(
                **{**fields, "units": units, "admission_id": H},
                owner=cold,
                lock_id=lock_id,
                finality_hash=H,
                closed_dispute_root=H,
            )
            escrow.release(release, signer=cold, now_beacon=10)
            free += units
        elif kind == "restart":
            escrow = reopen(li_escrow, setup, database)
        if dust:
            assert refusal(call) == "LOCK_BELOW_MINIMUM"
        elif call is not None and affordable:
            call()
            if kind == "send":
                free, abroad = free - amount, abroad + amount
            elif kind == "back":
                free, abroad = free + amount, abroad - amount
            elif kind == "lock":
                free -= amount
                locks.append((identity, amount))
            else:
                free, paid = free - amount, paid + amount
        elif call is not None:
            assert refusal(call) == "UNFUNDED_OR_UNKNOWN_ORIGIN"
        balances = escrow.balances()
        assert balances.conserved() and balances.issued == 30000
        assert (balances.paid, balances.admission_locked) == (paid, sum(u for _, u in locks))
        assert escrow.balances(cold).available == free
        assert escrow.balances(other).available == 10000 + abroad
    reopen(li_escrow, setup, database)


TAMPERING = {
    "an_owner_is_swapped": (
        "UPDATE escrow_units SET owner='{other}' WHERE bucket='reward_pending'",
        "PERSISTED_STATE_MISMATCH",
    ),
    "a_balance_is_inflated": (
        "UPDATE escrow_units SET units=units+1 WHERE bucket='reward_pending'",
        "PERSISTED_STATE_MISMATCH",
    ),
    "an_origin_is_forgotten": ("DELETE FROM escrow_origins", "PERSISTED_STATE_MISMATCH"),
    "a_finalized_round_is_forgotten": ("DELETE FROM escrow_finalized", "PERSISTED_STATE_MISMATCH"),
    "an_event_is_edited": (
        "UPDATE escrow_events SET request='{{}}' WHERE kind='REWARD_FINALIZE'",
        "EVENT_CHAIN",
    ),
    "a_snapshot_is_removed": ("DELETE FROM escrow_snapshots", "MISSING_EVENT_SNAPSHOT"),
}


@pytest.mark.parametrize("tamper", TAMPERING)
def test_a_ledger_whose_rows_were_edited_is_refused_when_reopened(
    li_escrow: ModuleType, setup: Any, tmp_path: Path, tamper: str
) -> None:
    """CRD-24 CRD-01: ownership, balances, the origin list, the finalized-round list, the event
    chain and its snapshots are all verified on restart; an edit that would let a round be
    credited again or move units is refused."""
    plan = build_plan(li_escrow, setup, [miner(0)])
    reward, evidence = plan.build()
    path = tmp_path / "ledger.db"
    escrow = ledger_for(li_escrow, setup, plan, path)
    escrow.reward_finalize(reward, evidence)
    statement, code = TAMPERING[tamper]
    escrow.db.execute(statement.format(other=owner(7).ss58))
    escrow.db.close()
    assert refusal(lambda: ledger_for(li_escrow, setup, plan, path)) == code


# --- CRD-14: the share of one coldkey -------------------------------------------------------------


@given(holders=st.lists(st.integers(0, 2), min_size=2, max_size=7))
@example(holders=[0, 1, 0, 2, 1])
@example(holders=[0, 0, 0])
def test_a_coldkey_is_credited_one_share_per_included_hotkey_and_no_more(
    li_escrow: ModuleType, setup: Any, holders: list[int]
) -> None:
    """CRD-14 CRD-12: the ledger has no per-coldkey cap, so a coldkey receives exactly the shares
    of the included hotkeys it owns (floor share plus at most one unit each), credited to that
    coldkey; the bound on how many hotkeys a coldkey may run is the admission stake."""
    keys = [miner(i) for i in range(len(holders))]
    owners = {key.ss58: owner(c).ss58 for key, c in zip(keys, holders, strict=True)}
    plan = build_plan(li_escrow, setup, keys, owners=owners)
    reward, evidence = plan.build()
    escrow = ledger_for(li_escrow, setup, plan)
    escrow.reward_finalize(reward, evidence)
    shares = split_units(BUDGET, [key.ss58 for key in keys])
    ceiling = -(-BUDGET // len(keys))
    for c in set(holders):
        mine = [key.ss58 for key, held in zip(keys, holders, strict=True) if held == c]
        credited = escrow.balances(owner(c).ss58).reward_pending
        assert credited == sum(shares[hotkey] for hotkey in mine) <= len(mine) * ceiling
    assert sum(escrow.balances(owner(c).ss58).reward_pending for c in set(holders)) == BUDGET


# --- the store: owner binding, funding per admission, atomic credit ------------------------------


def service_keys() -> tuple[list[Any], list[Any], Any]:
    service = load_fixture_module(*SERVICE_FIXTURE)
    return service.HOT, service.COLD, service.REFEREE


def join_request(world: Any, hot: Any, cold: Any, tag: str) -> Any:
    network = world.network
    request = sign_join(
        hot,
        cold,
        run_id=world.run_id,
        request_id=sha256_hex(tag.encode()),
        expires_beacon=10000,
        policy_hash=network.manifest.network.admission_policy_hash,
        hardware_hint=HardwareHint(device_name="cpu", device_count=999, driver="advisory"),
        env=honest_env(),
    )
    return network.client.post(network.url + "/join", content=canonicalize(request.body()))


def test_a_rotation_moves_the_admission_but_never_the_coldkey_that_is_credited(
    tmp_path: Path,
) -> None:
    """CRD-12: the coldkey bound at join is the owner of every credit of the admission. Only that
    coldkey can rotate it, the new hotkey inherits the same owner, the old hotkey is no longer an
    identity, and neither a rejoin nor a second coldkey can take a hotkey that has a history."""
    hot, cold, referee = service_keys()
    with service_world(tmp_path) as world:
        network, store, run = world.network, world.store, world.run_id
        assert join_request(world, hot[0], cold[0], "first").status_code == 200
        assert store._owner_v2(run, hot[0].ss58) == cold[0].ss58

        def rotation(signer: Any, operation: str) -> Any:
            return network.client.post(
                network.url + "/admission/" + hot[0].ss58 + "/rotate",
                json=rotation_envelope(run, signer, hot[0].ss58, referee, operation, 1000),
            )

        before = world.db_digest()
        stranger = rotation(cold[1], "de" * 32)
        assert (stranger.status_code, stranger.json()["detail"]) == (
            409,
            "signer is not authorized for message role",
        )
        assert world.db_digest() == before
        assert rotation(cold[0], "df" * 32).status_code == 200
        assert store._owner_v2(run, referee.ss58) == cold[0].ss58
        with pytest.raises(ChallengeError) as retired:
            store._owner_v2(run, hot[0].ss58)
        assert retired.value.status == 403
        rows = store._db.execute("SELECT hotkey,coldkey FROM admissions_v2").fetchall()
        assert [tuple(row) for row in rows] == [(referee.ss58, cold[0].ss58)]
        history = store._db.execute("SELECT hotkey,coldkey FROM admission_history").fetchall()
        assert {tuple(row) for row in history} == {
            (hot[0].ss58, cold[0].ss58),
            (referee.ss58, cold[0].ss58),
        }
        after = world.db_digest()
        for hotkey, coldkey, tag in ((hot[0], cold[1], "again"), (referee, cold[2], "takeover")):
            refused = join_request(world, hotkey, coldkey, tag)
            assert (refused.status_code, refused.json()["detail"]) == (409, "HOTKEY_HISTORY_EXISTS")
        assert world.db_digest() == after


def test_each_admission_of_one_coldkey_needs_its_own_funded_lock_to_be_eligible(
    tmp_path: Path,
) -> None:
    """CRD-14 (condition): the ledger does not cap a coldkey, the stake does. Two active
    admissions of one coldkey are eligible only through the lock named after each of them."""
    hot, cold, _ = service_keys()
    with service_world(tmp_path) as world:
        network, store = world.network, world.store
        admissions = []
        for index in range(2):
            joined = join_request(world, hot[index], cold[0], f"admission-{index}")
            assert joined.status_code == 200, joined.text
            admissions.append(joined.json()["admission_id"])
            with store._tx():
                store._db.execute(
                    "UPDATE admissions_v2 SET state='ACTIVE',clean_count=12,screen_until=? "
                    "WHERE admission_id=?",
                    (10**9, admissions[-1]),
                )
        _, admission, _ = store._services(world.run_id)

        def eligible() -> list[bool]:
            return [admission.status(h.ss58, now=network.now).eligible for h in hot[:2]]

        assert eligible() == [False, False]
        lock = EscrowLock(
            operation_id="a1" * 32,
            owner=cold[0].ss58,
            units=2000,
            origin_ids=[network.origin_ids[0]],
            admission_id=admissions[0],
            dispute_id=None,
            kind="LOCK_ADMISSION",
        )
        response = network.client.post(
            network.url + "/escrow/lock", json=network.signed(cold[0], "EscrowLock", lock)
        )
        assert response.status_code == 200, response.text
        assert eligible() == [True, False]


class Crash(Exception):
    """A failure injected after a store transaction has already written."""


def test_a_failure_after_the_ledger_wrote_leaves_the_store_exactly_as_it_was(
    li_escrow: ModuleType, setup: Any, tmp_path: Path
) -> None:
    """CRD-05 (condition holds: the services exist before the transaction). The credit, its
    settlement records and the service's own bookkeeping are one transaction: a failure after the
    ledger write undoes all of it, and the same work then commits whole."""
    plan = build_plan(li_escrow, setup, HOTKEYS)
    reward, evidence = plan.build()
    with hosted_reward(tmp_path, li_escrow, (setup, evidence, reward), real_settlement=True) as h:
        h.escrow.replay_finality = plan.replay
        h.escrow.owner_of = registered_owners(plan.owners)
        before = database_digest(h.store)
        with pytest.raises(Crash), h.store._tx():
            h.escrow.reward_finalize(reward, evidence)
            h.store._put_record_v2(h.run_id, "finalize-result", "0", {"status": "SETTLED"})
            raise Crash
        assert database_digest(h.store) == before and event_count(h.escrow) == 0
        assert not h.store._db.in_transaction
        h.finalize()
        assert event_count(h.escrow, "REWARD_FINALIZE") == 1
        assert h.escrow.balances().issued == BUDGET
        settled = h.store._db.execute(
            "SELECT COUNT(*) FROM records_v2 WHERE kind='settlement'"
        ).fetchone()[0]
        assert settled == len(reward.origin_ids)


def test_building_the_services_inside_a_transaction_never_commits_it_half_way(
    tmp_path: Path,
) -> None:
    """CRD-05 refuted-by-test: the Lens reads that the service constructors (executescript) commit
    an open store transaction on the first call after a restart, so ROLLBACK is lost and
    a half-finalized round stays. Whatever building the services does inside a transaction
    (raise or build), the rows the surrounding transaction wrote are rolled back with it."""
    with service_world(tmp_path) as world:
        store = world.store
        store._services_v2.clear()  # a restarted master: nothing is built yet
        with pytest.raises((sqlite3.OperationalError, Crash)), store._tx():
            store._db.execute("INSERT OR REPLACE INTO meta VALUES('credit-marker','1')")
            store._services(world.run_id)
            raise Crash
        marker = store._db.execute("SELECT COUNT(*) FROM meta WHERE key=?", ("credit-marker",))
        assert marker.fetchone()[0] == 0
        assert not store._db.in_transaction


# --- CRD-08: when a round may settle ---------------------------------------------------------

OPEN_ROUND = RoundStatus(H, 0, True, "PENDING", 0, 0, False, H, None)
ROUND_FINALIZE = Finalize(w=0, final_theta_hash_w1=H, included=[miner(0).ss58], entitlements_root=H)

CANNOT_SETTLE = {
    "an_audit_is_unresolved": {"unresolved_audits": 1},
    "a_dispute_is_unresolved": {"unresolved_disputes": 1},
    "the_round_was_excluded": {"disposition": "EXCLUDED"},
    "the_update_was_never_applied": {"applied": False},
    "the_applied_state_is_another": {"state_hash": "22" * 32},
    "the_round_number_is_another": {"w": 1},
}


@pytest.mark.parametrize("fault", CANNOT_SETTLE)
def test_a_round_settles_only_when_applied_audited_undisputed_and_not_excluded(
    fault: str,
) -> None:
    """CRD-08: finality is accepted for a round only once its update is applied, every audit and
    dispute is closed and it was not excluded, and the Finalize names the applied state."""
    status = replace(OPEN_ROUND, **CANNOT_SETTLE[fault])
    with pytest.raises(FinalityError, match="round cannot settle"):
        settle(status, ROUND_FINALIZE, H)


def test_a_round_settles_with_one_finality_hash_for_ever() -> None:
    """CRD-08 CRD-02: settling twice with the same finality is idempotent; a different finality
    hash for a round that already settled is a conflict, and the settled status stays closed."""
    settled = settle(OPEN_ROUND, ROUND_FINALIZE, H)
    assert (settled.disposition, settled.resolution_hash, settled.closed) == ("SETTLED", H, True)
    assert settle(settled, ROUND_FINALIZE, H) == settled
    with pytest.raises(FinalityError, match="conflicting finality"):
        settle(settled, ROUND_FINALIZE, "22" * 32)
    assert OPEN_ROUND.disposition == "PENDING" and OPEN_ROUND.resolution_hash is None
