"""Operation ids: a client event never takes an id the service derives for its own events.

A client may sign any 64-hex id, and every id the service derives for itself (reward finalize,
shadow finalize, maturity, payout, dispute settlement) is a function of values that are public or
shared with the miner. The escrow stores such a service event under a key no client id can name,
so each test takes the id from the receipt of the control run of the service step, lets a funded
bystander sign an event with exactly that id, and requires the step to succeed with the effect it
has without the bystander. The same module pins the lookups that must not confuse a lock id, a
dispute id and a reward origin id, and the escrow engine's own id, atomicity and replay laws.
"""

from __future__ import annotations

import dataclasses
import inspect
import json
import sqlite3
from pathlib import Path
from types import ModuleType

import pytest
from hypothesis import example, given
from hypothesis import strategies as st

from hypertrain.challenge.disputes_v2 import DisputeError
from hypertrain.ledger.escrow_v2 import EscrowError, EscrowReceipt, EscrowV2
from hypertrain.protocol.envelope_v2 import seal
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages_v2 import (
    EscrowLock,
    EscrowOperation,
    EscrowRelease,
    EscrowTransfer,
    ResolutionV2,
)

from .support import (
    Real,
    bare_escrow,
    error_code,
    hosted_reward,
    load_fixture_module,
    match_settlement,
    mature,
    memory_db,
)
from .support_opids import (
    RECIPIENT,
    DefenceMissing,
    client_event,
    collateral_escrow,
    dispute_code,
    dispute_world,
    fresh_id,
    must_refuse,
    refused,
    seed_reward_origin,
    service_step,
    transfer_to_recipient,
)


def reward_effect(escrow) -> tuple[int, int]:
    balances = escrow.balances()
    return balances.issued, balances.reward_pending


@pytest.mark.parametrize("kind", ["transfer", "lock"])
def test_a_client_event_never_blocks_the_reward_finalize_id(
    li_escrow: ModuleType, real_reward: Real, kind: str
) -> None:
    setup, evidence, reward = real_reward
    # Given: the control, a ledger without the bystander, shows the id and the effect of the step.
    control, _ = collateral_escrow(li_escrow, setup)
    served = control.reward_finalize(reward, evidence)
    finalize_id = served.operation_id
    effect = reward_effect(control)
    assert effect == (1_001_000, 1_000_000)

    # When: a funded bystander signs an event with exactly that id before the round is finalized.
    escrow, origin = collateral_escrow(li_escrow, setup)
    key = li_escrow.OTHER
    client_event(escrow, kind, key, fresh_id("neutral"), origin)  # control: the bystander can sign
    refused(lambda: client_event(escrow, kind, key, finalize_id, origin))

    # Then: the finalize of the round still succeeds, with the same effect and the same id.
    receipt = service_step(lambda: escrow.reward_finalize(reward, evidence))
    assert receipt.operation_id == finalize_id
    assert reward_effect(escrow) == effect


@pytest.mark.parametrize("kind", ["transfer", "lock"])
def test_a_client_event_never_blocks_the_shadow_finalize_id(
    li_escrow: ModuleType, real_reward: Real, kind: str
) -> None:
    setup, evidence, reward = real_reward
    registry = match_settlement(reward.finalize_hash)

    def shadow_ledger():
        escrow, origin = collateral_escrow(li_escrow, setup, settlement=lambda _: registry)
        binding = li_escrow.reserve_actual_shadow(escrow, evidence)
        return escrow, origin, li_escrow.sign_actual_shadow(reward, binding)

    # Given: the control run of the shadow finalize.
    control, _, signed = shadow_ledger()
    shadow_evidence = dataclasses.replace(evidence, shadow=True)
    finalize_id = control.reward_finalize(signed, shadow_evidence).operation_id
    effect = reward_effect(control)
    assert effect == (1_001_000, 1_000_000)

    # When: a funded bystander signs an event with that id after the reservation.
    escrow, origin, signed = shadow_ledger()
    client_event(escrow, kind, li_escrow.OTHER, fresh_id("neutral"), origin)
    refused(lambda: client_event(escrow, kind, li_escrow.OTHER, finalize_id, origin))

    # Then: the reservation is finalized as without the bystander.
    receipt = service_step(lambda: escrow.reward_finalize(signed, shadow_evidence))
    assert receipt.operation_id == finalize_id
    assert reward_effect(escrow) == effect


def named_ids(escrow, kind: str) -> list[str]:
    """The ids the service named for its events of `kind`, as their receipts carry them. The key
    an event is stored under is a keyed derivative that no client can sign, so it is not the id a
    bystander would take."""
    rows = escrow.db.execute("SELECT receipt FROM escrow_events WHERE kind=? ORDER BY seq", (kind,))
    return [json.loads(row[0])["operation_id"] for row in rows]


def pay_ids(escrow) -> list[str]:
    return named_ids(escrow, "PAY")


def release_ids(escrow) -> list[str]:
    return named_ids(escrow, "RELEASE")


def resolved_and_settled(world, turn, settle=lambda step: step()):
    """What the resolution route does: the referee's resolution and the release of the bond in
    one transaction, so a refused release also takes the resolution back. A control calls it
    as it is: a refusal there fails the test. The run under test passes `service_step`."""
    with world.escrow.tx():
        closed = world.resolve_match(turn, now=3)
        settle(lambda: world.disputes.settle_lock(turn.dispute_id, now=3))
    return closed


def matured_with_a_bystander(hosted, fixture) -> str:
    """The reward is finalized and matured; its owner has sent two units to OTHER, a funded key
    that is not the earning owner. Returns the reward origin."""
    hosted.finalize()
    mature(hosted.escrow, hosted.reward)
    origin = hosted.reward.origin_ids[0]
    hosted.escrow.transfer(
        EscrowTransfer(
            operation_id=fresh_id("fund-bystander"),
            owner=fixture.COLD.ss58,
            recipient=fixture.OTHER.ss58,
            units=2,
            origin_ids=[origin],
            admission_id=None,
            dispute_id=None,
        ),
        signer=fixture.COLD.ss58,
    )
    return origin


@pytest.mark.parametrize("kind", ["transfer", "lock"])
def test_a_client_event_never_blocks_the_weights_payout_id(
    li_escrow: ModuleType, real_reward: Real, tmp_path: Path, kind: str
) -> None:
    # Given: the control run of get_weights pays the earning owner and shows the payout id.
    with hosted_reward(tmp_path / "control", li_escrow, real_reward, real_settlement=True) as run:
        matured_with_a_bystander(run, li_escrow)
        served = run.answer(0)
        payout_ids = pay_ids(run.escrow)
    assert served["weights"] == {li_escrow.HOT.ss58: 999_998.0} and len(payout_ids) == 1

    # When: a funded bystander signs an event with exactly that id before the epoch is asked.
    with hosted_reward(tmp_path / "contended", li_escrow, real_reward, real_settlement=True) as run:
        origin = matured_with_a_bystander(run, li_escrow)
        key = li_escrow.OTHER
        client_event(run.escrow, kind, key, fresh_id("neutral"), origin)
        assert run.escrow.balances(key.ss58).available >= 1  # a unit is left to sign with
        refused(lambda: client_event(run.escrow, kind, key, payout_ids[0], origin))

        # Then: the epoch is answered with the same payment, once.
        answer = service_step(lambda: run.answer(0))
        assert answer["weights"] == served["weights"]
        assert answer["metadata"]["units_paid"] == served["metadata"]["units_paid"]
        assert pay_ids(run.escrow) == payout_ids


@pytest.mark.parametrize("kind", ["transfer", "lock"])
def test_a_client_event_never_blocks_the_dispute_settlement_id(
    li_escrow: ModuleType, tmp_path: Path, kind: str
) -> None:
    verdict, bond = "11" * 32, "44" * 32

    def opened(world):
        dispute_id = world.add_contest(verdict, li_escrow.HOT)
        world.bond(li_escrow.COLD, bond, dispute_id, world.origins[0])
        return world.open(li_escrow.HOT, verdict, bond, now=1)

    # Given: the control run settles a resolved dispute and shows the id of its release.
    with dispute_world(tmp_path / "control", li_escrow) as world:
        resolved_and_settled(world, opened(world))
        settle_ids = release_ids(world.escrow)
        effect = world.escrow.balances(li_escrow.COLD.ss58)
    assert len(settle_ids) == 1 and effect.dispute_locked == 0 and effect.available == 20000

    # When: a funded bystander signs an event with that id while the dispute is open.
    with dispute_world(tmp_path / "contended", li_escrow) as world:
        turn = opened(world)
        key, origin = li_escrow.OTHER, world.origins[1]
        client_event(world.escrow, kind, key, fresh_id("neutral"), origin)
        refused(lambda: client_event(world.escrow, kind, key, settle_ids[0], origin))

        # Then: the referee's resolution and the release of the bond go through together, as
        # without the bystander.
        resolved_and_settled(world, turn, settle=service_step)
        assert world.disputes.get(turn.dispute_id).resolution is not None
        assert world.escrow.balances(li_escrow.COLD.ss58) == effect
        assert release_ids(world.escrow) == settle_ids


FIRST_VERDICT, SECOND_VERDICT, THIRD_VERDICT = "11" * 32, "55" * 32, "66" * 32


def borrowing_dispute(world, li_escrow):
    """A first dispute (COLD's miner) resolved MATCH and settled, then a second dispute (OTHER's
    miner) whose bond lock carries the dispute id of the first one.

    Returns (the closed first turn, the second dispute id, the second bond lock, the opened second
    turn). The lock and the turn are None when the id was refused before the dispute opened."""
    first_id = world.add_contest(FIRST_VERDICT, li_escrow.HOT)
    first_bond = world.bond(li_escrow.COLD, "44" * 32, first_id, world.origins[0])
    first = world.resolve_match(
        world.open(li_escrow.HOT, FIRST_VERDICT, first_bond.operation_id, now=1), now=3
    )
    world.disputes.settle_lock(first_id, now=3)
    assert world.escrow.balances(li_escrow.COLD.ss58).dispute_locked == 0  # its own life works

    # Control: the second miner's own bond and dispute are accepted, resolved and settled under an
    # id of their own, so a refusal below is the refusal of the borrowed id and of nothing else.
    own_id = world.add_contest(THIRD_VERDICT, world.miner_two)
    world.bond(li_escrow.OTHER, "77" * 32, own_id, world.origins[1])
    world.resolve_match(world.open(world.miner_two, THIRD_VERDICT, "77" * 32, now=3), now=3)
    world.disputes.settle_lock(own_id, now=3)
    assert world.escrow.balances(li_escrow.OTHER.ss58).dispute_locked == 0

    second_id = world.add_contest(SECOND_VERDICT, world.miner_two)
    bond = second = None
    try:
        bond = world.bond(li_escrow.OTHER, first_id, second_id, world.origins[1])
        second = world.open(world.miner_two, SECOND_VERDICT, first_id, now=4)
    except (EscrowError, DisputeError):
        pass  # the id is refused at creation, so no bond borrows it
    return first, second_id, bond, second


def test_a_bond_borrowing_a_closed_disputes_id_cannot_be_released_while_its_dispute_is_open(
    li_escrow: ModuleType, tmp_path: Path
) -> None:
    with dispute_world(tmp_path, li_escrow) as world:
        first, _, bond, opened = borrowing_dispute(world, li_escrow)
        if opened is None:
            return
        other = li_escrow.OTHER
        release = world.release_of(other, bond, first, fresh_id("early-release"), world.origins[1])

        # When / Then: the owner quotes the first dispute's resolution to take its bond back.
        must_refuse(
            lambda: world.escrow.release(release, signer=other.ss58, now_beacon=5),
            "the bond of an unresolved dispute was released",
        )
        assert world.escrow.balances(other.ss58).dispute_locked == 100


def test_a_bystander_event_under_a_plain_id_of_the_lock_does_not_keep_the_owner_from_its_bond(
    li_escrow: ModuleType, tmp_path: Path
) -> None:
    other, bystander = li_escrow.OTHER, li_escrow.COLD
    with dispute_world(tmp_path, li_escrow, admission=True) as world:
        assert world.admission is not None
        admission_id = world.join(world.miner_two, other, fresh_id("join"))
        lock = EscrowLock(
            operation_id=fresh_id("bond"),
            owner=other.ss58,
            units=1000,
            origin_ids=[world.origins[1]],
            admission_id=admission_id,
            dispute_id=None,
            kind="LOCK_ADMISSION",
        )
        world.admission.lock(
            canonicalize(seal(other, "EscrowLock", world.run_id, lock, 500)), now=2
        )

        def event_of(operation_id: str) -> EscrowTransfer:
            return EscrowTransfer(
                operation_id=operation_id,
                owner=bystander.ss58,
                recipient=other.ss58,
                units=1,
                origin_ids=[world.origins[0]],
                admission_id=None,
                dispute_id=None,
            )

        # Given: a funded bystander can sign an event (control).
        world.escrow.transfer(event_of(fresh_id("neutral")), signer=bystander.ss58)

        # When: it signs events under the ids a plain derivation from the lock id would give.
        for word in ("settle", "release"):
            plain = sha256_hex(canonicalize([lock.operation_id, word]))
            refused(
                lambda plain=plain: world.escrow.transfer(event_of(plain), signer=bystander.ss58)
            )

        # Then: the owner still takes its bond back, once.
        receipts = service_step(lambda: world.admission.release_bond(admission_id, now=5))
        assert len(receipts) == 1
        assert world.escrow.balances(other.ss58).admission_locked == 0
        assert world.admission.release_bond(admission_id, now=6) == []


def test_a_dispute_whose_bond_borrows_a_closed_disputes_id_can_still_be_settled(
    li_escrow: ModuleType, tmp_path: Path
) -> None:
    with dispute_world(tmp_path, li_escrow) as world:
        _, second_id, _, opened = borrowing_dispute(world, li_escrow)
        if opened is None:
            return
        world.resolve_match(opened, now=6)

        # When / Then: the referee has resolved it, so its bond goes back to its owner.
        service_step(lambda: world.disputes.settle_lock(second_id, now=6))
        assert world.escrow.balances(li_escrow.OTHER.ss58).dispute_locked == 0


@pytest.mark.parametrize("bond_first", [False, True], ids=["origin-first", "bond-first"])
def test_a_bond_borrowing_a_reward_origins_id_cannot_keep_that_origin_from_vesting(
    li_escrow: ModuleType, tmp_path: Path, bond_first: bool
) -> None:
    reward_origin, finalize_hash = fresh_id("reward-origin"), fresh_id("finalize")
    cold = li_escrow.COLD.ss58

    # Given: the control, where the origin of round 0 vests at round 5.
    with dispute_world(tmp_path / "control", li_escrow) as world:
        seed_reward_origin(world, reward_origin, cold, 500, 5, finalize_hash)
        world.escrow.mature(fresh_id("mature"), (reward_origin,), 5)
        effect = world.escrow.balances(cold)
    assert (effect.available, effect.reward_pending) == (20500, 0)

    # When: OTHER's miner opens a dispute whose bond lock carries the origin id, once the origin
    # exists (the dispute is then refused) or before it exists (the name is then free).
    with dispute_world(tmp_path / "contended", li_escrow) as world:
        dispute_id = world.add_contest(SECOND_VERDICT, world.miner_two)

        def borrow() -> None:
            world.bond(li_escrow.OTHER, reward_origin, dispute_id, world.origins[1])
            world.open(world.miner_two, SECOND_VERDICT, reward_origin, now=1)

        if bond_first:
            borrow()
            seed_reward_origin(world, reward_origin, cold, 500, 5, finalize_hash)
        else:
            seed_reward_origin(world, reward_origin, cold, 500, 5, finalize_hash)
            refused(borrow)

        # Then: the origin vests at round 5 as in the control.
        service_step(lambda: world.escrow.mature(fresh_id("mature"), (reward_origin,), 5))
        assert world.escrow.balances(cold) == effect


# --- ESC-19: an operation is committed or rolled back by its own outcome -----------------------


@pytest.mark.xfail(
    strict=True,
    raises=DefenceMissing,
    reason="ESC-19: an escrow operation that completes while its caller is handling an unrelated "
    "exception is committed, not rolled back behind a receipt [candidate] "
    "[fix-lot: none (no design yet)]",
)
def test_an_operation_completed_inside_an_except_block_is_committed(
    li_escrow: ModuleType, tmp_path: Path
) -> None:
    setup, path = li_escrow.setup(), tmp_path / "ledger.db"
    escrow = li_escrow.ledger(setup, path)
    origin, _ = li_escrow.fund(escrow)
    cold = li_escrow.COLD.ss58

    # Given: a lock outside any handler is committed (control).
    escrow.lock(li_escrow.lock(origin, fresh_id("outside"), 500), signer=cold)
    assert li_escrow.ledger(setup, path).balances(cold).admission_locked == 500

    # When: the same kind of lock completes while the caller handles an unrelated error.
    try:
        raise ValueError("unrelated failure of the caller")
    except ValueError:
        receipt = escrow.lock(li_escrow.lock(origin, fresh_id("inside"), 700), signer=cold)

    # Then: the receipt and the persisted ledger agree.
    assert receipt.admission_locked_units == 1200
    persisted = li_escrow.ledger(setup, path).balances(cold).admission_locked
    if persisted != 1200:
        raise DefenceMissing(
            f"the receipt says 1200 units are locked, the ledger holds {persisted}"
        )


# --- ESC-18: a failed operation leaves no partial event or movement ----------------------------


def ledger_state(escrow) -> tuple[list, list, list]:
    """Every row an escrow operation may write: unit rows, events and their snapshots."""

    def rows(sql: str) -> list:
        return [tuple(row) for row in escrow.db.execute(sql)]

    return (
        rows("SELECT * FROM escrow_units ORDER BY origin, owner, bucket, ref"),
        rows("SELECT seq, operation_id, event_hash FROM escrow_events ORDER BY seq"),
        rows("SELECT operation_id FROM escrow_snapshots ORDER BY operation_id"),
    )


def abort_snapshot_insert(escrow, operation_id: str | None = None) -> None:
    """A storage failure after the movement and the event row, before the snapshot: the last
    statement of an operation, so everything it did before is what the rollback must undo."""
    condition = f"WHEN NEW.operation_id = '{operation_id}' " if operation_id else ""
    escrow.db.execute(
        f"CREATE TRIGGER li_storage_failure BEFORE INSERT ON escrow_snapshots {condition}"
        "BEGIN SELECT RAISE(ABORT, 'storage failure'); END"
    )


def test_an_operation_that_fails_before_its_event_commits_changes_nothing(
    li_escrow: ModuleType, tmp_path: Path
) -> None:
    setup, path = li_escrow.setup(), tmp_path / "ledger.db"
    escrow = li_escrow.ledger(setup, path)
    origin, _ = li_escrow.fund(escrow)
    cold, other = li_escrow.COLD.ss58, li_escrow.OTHER.ss58
    lock = li_escrow.lock(origin, fresh_id("lock"), 1000)
    send = transfer_to_recipient(cold, fresh_id("send"), origin, 40)
    before = ledger_state(escrow)

    # Given: the storage fails at the end of every operation.
    abort_snapshot_insert(escrow)

    # When / Then: each operation is refused and the ledger is exactly as before, outside any
    # transaction, so the next operation starts clean.
    with pytest.raises(sqlite3.IntegrityError, match="storage failure"):
        escrow.lock(lock, signer=cold)
    with pytest.raises(sqlite3.IntegrityError, match="storage failure"):
        escrow.transfer(send, signer=cold)
    assert ledger_state(escrow) == before
    assert not escrow.db.in_transaction

    # And once the storage works, the same operations apply exactly once.
    escrow.db.execute("DROP TRIGGER li_storage_failure")
    receipt = escrow.lock(lock, signer=cold)
    escrow.transfer(send, signer=cold)
    assert escrow.lock(lock, signer=cold) == receipt
    reopened = li_escrow.ledger(setup, path)
    assert reopened.balances(cold).admission_locked == 1000
    assert reopened.balances(cold).available == 20000 - 1000 - 40
    assert reopened.balances(RECIPIENT.ss58).available == 40
    assert reopened.balances(other).available == 10000
    assert len(ledger_state(reopened)[1]) == len(before[1]) + 2


def test_a_failed_operation_inside_a_callers_transaction_is_undone_alone(
    li_escrow: ModuleType, tmp_path: Path
) -> None:
    setup, path = li_escrow.setup(), tmp_path / "ledger.db"
    escrow = li_escrow.ledger(setup, path)
    origin, _ = li_escrow.fund(escrow)
    cold, other = li_escrow.COLD.ss58, li_escrow.OTHER.ss58
    sent = EscrowTransfer(
        operation_id=fresh_id("sent"),
        owner=cold,
        recipient=other,
        units=300,
        origin_ids=[origin],
        admission_id=None,
        dispute_id=None,
    )
    failing = li_escrow.lock(origin, fresh_id("failing"), 900)
    abort_snapshot_insert(escrow, failing.operation_id)

    # When: the caller's transaction applies one operation, then one that fails, and goes on.
    with escrow.tx():
        escrow.transfer(sent, signer=cold)
        with pytest.raises(sqlite3.IntegrityError, match="storage failure"):
            escrow.lock(failing, signer=cold)

    # Then: the first operation is committed with the caller's transaction, the second left
    # neither its movement nor its event.
    reopened = li_escrow.ledger(setup, path)
    assert reopened.balances(cold).admission_locked == 0
    assert reopened.balances(cold).available == 20000 - 300
    assert reopened.balances(other).available == 10000 + 300
    events = [row[1] for row in ledger_state(reopened)[1]]
    assert sent.operation_id in events and failing.operation_id not in events


def test_an_operation_in_progress_keeps_another_connection_from_starting_one(
    li_escrow: ModuleType, tmp_path: Path
) -> None:
    setup, path = li_escrow.setup(), tmp_path / "ledger.db"
    first = li_escrow.ledger(setup, path)
    origin, _ = li_escrow.fund(first)
    second = li_escrow.ledger(setup, path)
    second.db.execute("PRAGMA busy_timeout=0")
    cold = li_escrow.COLD.ss58
    mine = li_escrow.lock(origin, fresh_id("mine"), 15000)
    rival = li_escrow.lock(origin, fresh_id("rival"), 15000)
    outcome: dict[str, str | None] = {}

    # Given: a second connection to the same file asks for the same funds at the instant the
    # first operation reads the ledger (no thread: the interleaving is the authorizer's).
    def rival_arrives(action: int, *_: object) -> int:
        if action == sqlite3.SQLITE_READ and "rival" not in outcome:
            outcome["rival"] = None
            try:
                second.lock(rival, signer=cold)
                outcome["rival"] = "ran to completion"
            except sqlite3.OperationalError as error:
                outcome["rival"] = str(error)
        return sqlite3.SQLITE_OK

    # When: the first operation runs.
    first.db.set_authorizer(rival_arrives)
    try:
        outcome["first"] = first.lock(mine, signer=cold).operation_id
    except EscrowError as error:
        outcome["first"] = error.code
    finally:
        first.db.set_authorizer(None)

    # Then: the write lock was already the first operation's, so the rival could not start and the
    # funds went to the first operation alone.
    assert outcome == {"rival": "database is locked", "first": mine.operation_id}
    assert first.balances(cold).admission_locked == 15000


def test_the_engine_refuses_to_be_built_inside_an_open_transaction(li_escrow: ModuleType) -> None:
    # Given: a caller with a pending write on the connection it hands to the engine.
    db = memory_db()
    db.execute("CREATE TABLE note(x)")
    db.execute("BEGIN IMMEDIATE")
    db.execute("INSERT INTO note VALUES(1)")

    # When / Then: construction is refused (SQLite will not change the durability level inside a
    # transaction), and the caller's transaction is still its own.
    with pytest.raises(sqlite3.OperationalError):
        li_escrow.ledger(li_escrow.setup(), db)
    assert db.in_transaction
    db.execute("ROLLBACK")
    assert db.execute("SELECT COUNT(*) FROM note").fetchone()[0] == 0


# --- ESC-03: one operation id, one body, one movement ------------------------------------------

POOL = [fresh_id(f"pool-{index}") for index in range(3)]
KINDS = ("lock", "transfer", "pay")
# A ledger that takes an id key keeps the ids a server derives (`pay` is one) in a key space no
# client id can name (ADR 0014, part 1); before it, every operation shares one space.
SERVER_IDS_ARE_KEYED = "id_key" in inspect.signature(EscrowV2).parameters


def id_space(kind: str) -> str:
    return "server" if SERVER_IDS_ARE_KEYED and kind == "pay" else "client"


def operation_of(fixture: ModuleType, kind: str, operation_id: str, units: int, origin: str):
    """The request of `kind` owned by COLD that consumes `units` of its available units."""
    if kind == "lock":
        return fixture.lock(origin, operation_id, units)
    if kind == "transfer":
        return EscrowTransfer(
            operation_id=operation_id,
            owner=fixture.COLD.ss58,
            recipient=fixture.OTHER.ss58,
            units=units,
            origin_ids=[origin],
            admission_id=None,
            dispute_id=None,
        )
    return EscrowOperation(
        operation_id=operation_id,
        owner=fixture.COLD.ss58,
        units=units,
        origin_ids=[origin],
        admission_id=None,
        dispute_id=None,
    )


def apply(escrow: EscrowV2, kind: str, operation) -> EscrowReceipt:
    cold = operation.owner
    if kind == "lock":
        return escrow.lock(operation, signer=cold)
    if kind == "transfer":
        return escrow.transfer(operation, signer=cold)
    return escrow.pay(operation, signer=cold)


@pytest.fixture(scope="module")
def escrow_setup(li_escrow: ModuleType):
    return li_escrow.setup()


@given(
    steps=st.lists(
        st.tuples(st.sampled_from(KINDS), st.sampled_from(POOL), st.integers(1, 3)),
        min_size=1,
        max_size=12,
    )
)
@example(steps=[("lock", POOL[0], 1), ("lock", POOL[0], 2)])
@example(steps=[("lock", POOL[0], 1), ("transfer", POOL[0], 1)])
@example(steps=[("transfer", POOL[1], 2), ("pay", POOL[1], 2), ("transfer", POOL[1], 2)])
def test_an_operation_id_names_one_body_and_one_movement(
    li_escrow: ModuleType, escrow_setup, steps: list[tuple[str, str, int]]
) -> None:
    escrow = bare_escrow(li_escrow, escrow_setup)
    origin, _ = li_escrow.fund(escrow)
    cold, other = li_escrow.COLD.ss58, li_escrow.OTHER.ss58
    genesis_events = len(ledger_state(escrow)[1])
    unit = max(1, escrow_setup.policy.S_min_units // 10)  # a smaller admission lock is dust
    stored: dict[tuple[str, str], tuple[str, int, EscrowReceipt]] = {}
    moved = {"lock": 0, "transfer": 0, "pay": 0}

    def check(engine: EscrowV2) -> None:
        balances = engine.balances(cold)
        assert balances.admission_locked == moved["lock"]
        assert balances.paid == moved["pay"]
        assert balances.available == 20000 - sum(moved.values())
        assert engine.balances(other).available == 10000 + moved["transfer"]
        assert len(ledger_state(engine)[1]) == genesis_events + len(stored)
        assert engine.balances().conserved()

    # When: random operations over three ids, so ids collide with other bodies and other kinds.
    for kind, operation_id, count in steps:
        amount = count * unit
        operation = operation_of(li_escrow, kind, operation_id, amount, origin)
        known = stored.get((id_space(kind), operation_id))
        if known is None:
            receipt = apply(escrow, kind, operation)
            assert receipt.operation_id == operation_id
            stored[(id_space(kind), operation_id)] = (kind, amount, receipt)
            moved[kind] += amount
        elif known[:2] == (kind, amount):
            assert apply(escrow, kind, operation) == known[2]  # same body: same receipt
        else:
            code = error_code(lambda kind=kind, operation=operation: apply(escrow, kind, operation))
            assert code == "OPERATION_CONFLICT"
        check(escrow)

    # Then: a restarted engine on the same ledger answers every id as the first one did.
    restarted = li_escrow.ledger(escrow_setup, escrow.db)
    for (_, operation_id), (kind, amount, receipt) in stored.items():
        operation = operation_of(li_escrow, kind, operation_id, amount, origin)
        assert apply(restarted, kind, operation) == receipt
    check(restarted)


def test_a_release_and_a_maturity_replay_with_their_first_receipt_after_a_restart(
    li_escrow: ModuleType, tmp_path: Path
) -> None:
    setup, path = li_escrow.setup(), tmp_path / "ledger.db"
    escrow = li_escrow.ledger(setup, path)
    origin, other_origin = li_escrow.fund(escrow)
    cold = li_escrow.COLD.ss58
    lock = li_escrow.lock(origin, fresh_id("lock"), 1000)
    escrow.lock(lock, signer=cold)
    release = EscrowRelease(
        operation_id=fresh_id("release"),
        owner=cold,
        units=1000,
        origin_ids=[origin],
        admission_id=lock.admission_id,
        dispute_id=None,
        lock_id=lock.operation_id,
        finality_hash=li_escrow.H,
        closed_dispute_root=li_escrow.H,
    )
    receipt = escrow.release(release, signer=cold, now_beacon=10)
    matured = escrow.mature(li_escrow.H, (origin, other_origin), 1)  # fund() matured them once
    assert matured.operation_id == li_escrow.H
    before = ledger_state(escrow)

    # When: the identical requests come again, to the same engine and to a restarted one.
    restarted = li_escrow.ledger(setup, path)
    for engine in (escrow, restarted):
        assert engine.release(release, signer=cold, now_beacon=10) == receipt
        assert engine.mature(li_escrow.H, (origin, other_origin), 1) == matured

    # Then: nothing moved and no event was added; a different body under a used id is refused.
    assert ledger_state(restarted) == before
    taller = release.model_copy(update={"units": 999})
    assert error_code(lambda: restarted.release(taller, signer=cold, now_beacon=10)) == (
        "OPERATION_CONFLICT"
    )
    assert error_code(lambda: restarted.mature(li_escrow.H, (origin,), 1)) == "OPERATION_CONFLICT"
    assert ledger_state(restarted) == before


# --- DIA-08: exactly one terminal transition applies to a dispute -------------------------------

DISPUTES_FIXTURE = ("li_disputes_fixture", "challenge/test_disputes_v2.py")


@pytest.mark.parametrize("paused", [True, False], ids=["paused", "running"])
@pytest.mark.parametrize("first", ["resolution", "exclusion"])
def test_exactly_one_terminal_transition_applies_to_a_dispute(
    li_escrow: ModuleType, tmp_path: Path, first: str, paused: bool
) -> None:
    helpers = load_fixture_module(*DISPUTES_FIXTURE)
    with dispute_world(tmp_path, li_escrow) as world:
        disputes, referee = world.disputes, world.referee
        dispute_id = world.add_contest(FIRST_VERDICT, li_escrow.HOT)
        bond = world.bond(li_escrow.COLD, "44" * 32, dispute_id, world.origins[0])
        opened = world.open(li_escrow.HOT, FIRST_VERDICT, bond.operation_id, now=1)
        if paused:  # the guard of a reassignment is then the resolution and nothing else
            disputes.pause(dispute_id)

        # Given: the first terminal transition, then the settlement of the bond. A running
        # dispute is excluded only at its absolute deadline.
        closing = opened.absolute_deadline if first == "exclusion" and not paused else 3
        if first == "resolution":
            closed = world.resolve_match(disputes.get(dispute_id), now=closing)
        else:
            closed = disputes.exclude(dispute_id, now=closing)
        assert closed.resolution is not None and closed.resolution.reason == (
            "MATCH" if first == "resolution" else "INFRASTRUCTURE"
        )
        disputes.settle_lock(dispute_id, now=closing)
        frozen = (disputes.get(dispute_id), ledger_state(world.escrow))

        # When: every other terminal transition, or any further move, is attempted later.
        stored = closed.resolution
        different = ResolutionV2(
            dispute_id=dispute_id,
            transcript_hash=closed.transcript_hash,
            reason="FRAUD",
            loser=li_escrow.HOT.ss58,
            evidence_hash=fresh_id("evidence nobody registered"),
        )
        # Each of the next two agrees with the stored resolution on one field that identifies it,
        # so a comparison that looks at that field alone would take it for the stored one.
        other_evidence = stored.model_copy(update={"evidence_hash": fresh_id("other evidence")})
        other_outcome = different.model_copy(update={"evidence_hash": stored.evidence_hash})
        late = closed.turn_deadline + 1
        proofs = helpers.availability(disputes, closed, (li_escrow.COORD, referee), late)

        def resolve(body: ResolutionV2):
            raw = canonicalize(seal(referee, "ResolutionV2", world.run_id, body, 100000))
            return lambda: disputes.resolution(raw, now=4)

        refusals = {
            "a different resolution": ("RESOLUTION_CONFLICT", resolve(different)),
            "the same outcome on other evidence": ("RESOLUTION_CONFLICT", resolve(other_evidence)),
            "another outcome on the stored evidence": (
                "RESOLUTION_CONFLICT",
                resolve(other_outcome),
            ),
            "a timeout": (
                "TIMEOUT_NOT_DUE",
                lambda: disputes.timeout(dispute_id, proofs, now=late),
            ),
            "a bisection": (
                "CLOSED_PAUSED_OR_EXPIRED",
                lambda: helpers.reply(disputes, li_escrow.HOT, ["11" * 32] * 3, now=4),
            ),
            "a reassignment": (
                "REFEREE_REASSIGNMENT_BOUND",
                lambda: disputes.reassign(dispute_id, world.next_referee.ss58, now=4),
            ),
            "a pause": ("CLOSED_DISPUTE", lambda: disputes.pause(dispute_id)),
        }

        # Then: each is refused with its own code, an exclusion or an equal resolution returns the
        # stored turn, a second settlement adds no event, and the dispute is exactly as it was.
        assert {name: dispute_code(call) for name, (_, call) in refusals.items()} == {
            name: code for name, (code, _) in refusals.items()
        }
        assert disputes.exclude(dispute_id, now=late) == closed
        if first == "resolution":
            assert world.resolve_match(closed, now=4) == closed
        disputes.settle_lock(dispute_id, now=late)
        assert (disputes.get(dispute_id), ledger_state(world.escrow)) == frozen
