"""Ledger and escrow defences of the audit-cheaters lot: double credit, empty or unaudited epochs,
races around finalize, and the admission-lock rows of the red-team table.

Everything runs in-process with test keys. Each docstring states the defence the test asserts;
an `xfail(strict=True)` test is a hole whose body is the minimal reproducer asserting the DESIRED
defence (it flips to a failure the day the hole is closed, so the marker must then be removed).
"""

from __future__ import annotations

import importlib.util
import itertools
import json
import sys
import threading
from collections.abc import Callable, Iterator
from contextlib import contextmanager
from dataclasses import dataclass, replace
from pathlib import Path
from typing import Any

import pytest
from ac_precondition import precondition
from hypothesis import example, given, settings
from hypothesis import strategies as st

from hypertrain.challenge.store import ChallengeStore
from hypertrain.data.store import LocalFSStore
from hypertrain.ledger import FULL_SHARE_MASS, Params
from hypertrain.ledger.escrow_v2 import (
    EscrowError,
    EscrowV2,
    FinalityEvidence,
    Settlement,
    authority_message,
    digest,
    reward_origin_id,
)
from hypertrain.protocol.envelope_v2 import EnvelopeV2, seal
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages import Finalize
from hypertrain.protocol.messages_v2 import (
    EscrowLock,
    EscrowRelease,
    EscrowTransfer,
    OriginAllocation,
    RewardFinalize,
)

TESTS = Path(__file__).parents[1]


def load(name: str, relative: str) -> Any:
    """Import a sibling test module by path, as the service tests do (basenames are not unique)."""
    spec = importlib.util.spec_from_file_location(name, TESTS / relative)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


E = load("ac_ledger_escrow_fixture", "ledger/test_escrow_v2.py")
C = load("ac_ledger_challenge_conftest", "challenge/conftest.py")
D = load("ac_ledger_disputes_fixture", "challenge/test_disputes_v2.py")
# Module-scoped real signed w=0 reward (two executions of the tiny CPU model), as the escrow tests.
actual_reward = E.actual_reward

H = E.H
HOT, COLD, OTHER = E.HOT.ss58, E.COLD.ss58, E.OTHER.ss58


@dataclass
class Hosted:
    """A ChallengeStore hosting one run's EscrowV2 behind the service settlement callback."""

    store: ChallengeStore
    escrow: EscrowV2
    reward: RewardFinalize
    evidence: FinalityEvidence
    run_id: str


@contextmanager
def hosting(
    root: Path,
    real: tuple[Any, FinalityEvidence, RewardFinalize],
    settlement: Callable[[str], Settlement] | None = None,
) -> Iterator[Hosted]:
    s, evidence, reward = real
    run_id = s.manifest.run_id()
    store = ChallengeStore(
        root / "state",
        # v1 genesis deliberately differs from the drand genesis the maturity round uses.
        Params("hypertrain", s.manifest.training.beacon.genesis_time - 600, 300, 1, 1),
        None,
        None,
        lambda payload: None,
        LocalFSStore(root / "objects"),
    )
    escrow = E.ledger(
        s,
        store._db,
        replay=E.verified_shadow,
        settlement=settlement or (lambda ref: store._settlement_v2(run_id, ref)),
    )
    escrow.mutex = store._lock
    store._db.execute(
        "INSERT INTO runs VALUES(?,?,?,'created',NULL)",
        (run_id, s.manifest.model_dump_json(), "{}"),
    )
    store._services = lambda _: (escrow, None, None)
    try:
        yield Hosted(store, escrow, reward, evidence, run_id)
    finally:
        store.close()


@pytest.fixture
def hosted(tmp_path: Path, actual_reward: Any) -> Iterator[Hosted]:
    with hosting(tmp_path, actual_reward) as h:
        yield h


def finalize(h: Hosted) -> Any:
    """What Store.finalize_v2 does after verification: reward_finalize plus the accepted
    settlement record of every origin, in one transaction (challenge/store.py:8383)."""
    with h.store._tx():
        receipt = h.escrow.reward_finalize(h.reward, h.evidence)
        for origin in h.reward.origin_ids:
            h.store._put_record_v2(
                h.run_id,
                "settlement",
                origin,
                Settlement(
                    finality_hash=h.reward.finalize_hash,
                    closed_dispute_root=sha256_hex(canonicalize([])),
                    release_beacon=h.reward.mature_at,
                    unresolved=False,
                    outcome="MATCH",
                ).body(),
            )
    return receipt


def at(escrow: EscrowV2, rnd: int) -> int:
    """Chain time of drand round rnd for the hosted manifest."""
    beacon = escrow.manifest.training.beacon
    return beacon.genesis_time + (rnd - 1) * beacon.period


def mature(h: Hosted) -> None:
    h.escrow.mature(H, tuple(h.reward.origin_ids), 20)


def answer(h: Hosted, epoch: int, rnd: int = 20) -> dict[str, Any]:
    return json.loads(h.store.weights(epoch, at(h.escrow, rnd), 1))


def pays(escrow: EscrowV2) -> int:
    return escrow.db.execute("SELECT COUNT(*) FROM escrow_events WHERE kind='PAY'").fetchone()[0]


def signed_variant(
    real: tuple[Any, FinalityEvidence, RewardFinalize], **final_changes: Any
) -> tuple[RewardFinalize, FinalityEvidence]:
    """A coordinator-signed Finalize/RewardFinalize pair differing from the fixture's.

    The origin split is recomputed for the fixture's single audited worker, so a variant that is
    refused is refused for its Finalize content, not for a malformed allocation."""
    s, evidence, reward = real
    run_id = s.manifest.run_id()
    final = Finalize.model_validate({**evidence.finalize.body, **final_changes})
    envelope = EnvelopeV2.model_validate(seal(E.COORD, "Finalize", run_id, final, 200))
    origin = OriginAllocation(
        origin_id=reward_origin_id(run_id, 0, HOT, digest(final)),
        owner=COLD,
        units=reward.budget_units,
        mature_at=reward.mature_at,
    )
    unsigned = reward.model_copy(
        update={
            "finalize_hash": digest(final),
            "origin_ids": [origin.origin_id],
            "allocation_hash": sha256_hex(canonicalize([origin.body()])),
        }
    )
    signed = unsigned.model_copy(
        update={"authority_sig": E.COORD.sign(authority_message(unsigned)).hex()}
    )
    return signed, replace(evidence, finalize=envelope)


def file_ledger(real: tuple[Any, FinalityEvidence, RewardFinalize], path: Path) -> EscrowV2:
    return E.ledger(real[0], path, replay=E.verified_shadow)


def reward_rows(escrow: EscrowV2) -> tuple[int, int, int]:
    """(origins, REWARD_FINALIZE events, finalized rounds) as persisted."""
    count = escrow.db.execute
    return (
        count("SELECT COUNT(*) FROM escrow_origins").fetchone()[0],
        count("SELECT COUNT(*) FROM escrow_events WHERE kind='REWARD_FINALIZE'").fetchone()[0],
        count("SELECT COUNT(*) FROM escrow_finalized").fetchone()[0],
    )


# --- A. double credit ----------------------------------------------------------------------


def test_replayed_finalize_issues_nothing_twice(tmp_path: Path, actual_reward: Any) -> None:
    """Defence: the same signed finalize, replayed on the same engine or after a restart, returns
    the original receipt and creates no second origin, event or finalized round."""
    # Given: a finalized live round.
    _, evidence, reward = actual_reward
    path = tmp_path / "ledger.db"
    e = file_ledger(actual_reward, path)
    receipt = e.reward_finalize(reward, evidence)
    assert reward_rows(e) == (1, 1, 1) and e.balances().issued == 1_000_000
    # When: the identical finalize arrives again, then once more on a restarted engine.
    again = e.reward_finalize(reward, evidence)
    restarted = file_ledger(actual_reward, path).reward_finalize(reward, evidence)
    # Then: the receipt is the original and nothing was issued.
    assert again == receipt and restarted == receipt
    assert reward_rows(e) == (1, 1, 1)
    assert e.balances().issued == 1_000_000 and e.balances().reward_pending == 1_000_000


def test_second_finalize_of_a_round_with_other_content_is_refused(
    tmp_path: Path, actual_reward: Any
) -> None:
    """Defence: a round finalizes once. A second, validly signed finalize of the same round with
    different content (here another entitlements_root, hence other origin ids) is refused and
    issues no second origin set."""
    # Given: round 0 finalized.
    _, evidence, reward = actual_reward
    e = file_ledger(actual_reward, tmp_path / "ledger.db")
    e.reward_finalize(reward, evidence)
    other_reward, other_evidence = signed_variant(actual_reward, entitlements_root="22" * 32)
    assert other_reward.origin_ids != reward.origin_ids
    # When / Then
    with pytest.raises(EscrowError, match="OPERATION_CONFLICT"):
        e.reward_finalize(other_reward, other_evidence)
    assert reward_rows(e) == (1, 1, 1) and e.balances().issued == 1_000_000
    assert e.db.execute("SELECT finalize_hash FROM escrow_finalized").fetchone()[0] == (
        reward.finalize_hash
    )


@pytest.mark.parametrize(
    "stale_set,code",
    [
        # The finalize lists a hotkey that has no audited work (the slice of an excluded miner).
        ("lists_unaudited_hotkey", "NO_VERIFIED_WORK_OR_EXCLUSIONS"),
        # The finalize drops a hotkey whose audited work is presented.
        ("omits_audited_hotkey", "UNVERIFIED_REWARD_WORK"),
    ],
)
def test_finalize_refuses_an_included_set_that_differs_from_the_audited_work(
    tmp_path: Path, actual_reward: Any, stale_set: str, code: str
) -> None:
    """Defence: the reward is minted only when Finalize.included equals the set of hotkeys with a
    MATCH audit presented as work; a stale or edited included set mints nothing."""
    # Given: a signed finalize whose included set disagrees with the audited work.
    included = [HOT, OTHER] if stale_set == "lists_unaudited_hotkey" else []
    reward, evidence = signed_variant(actual_reward, included=included)
    e = file_ledger(actual_reward, tmp_path / "ledger.db")
    # When / Then
    with pytest.raises(EscrowError, match=code):
        e.reward_finalize(reward, evidence)
    assert e.balances().issued == 0 and reward_rows(e) == (0, 0, 0)


# --- B. empty, unaudited and disputed epochs -----------------------------------------------


def http_weights(client: Any, epoch: int, epoch_at: int | None = None) -> Any:
    query = f"epoch={epoch}" + ("" if epoch_at is None else f"&epoch_at={epoch_at}")
    headers = {**C.bearer(C.INTERNAL), "x-platform-challenge-slug": C.SLUG}
    return client.get(f"/internal/v1/get_weights?{query}", headers=headers)


@pytest.fixture
def http(tmp_path: Path) -> Iterator[Any]:
    secrets = tmp_path / "secrets"
    secrets.mkdir()
    for name, value in (
        ("internal.token", C.INTERNAL),
        ("admin.token", C.ADMIN),
        ("worker.token", C.WORKER),
        ("coord.key", C.COORD_SEED.hex()),
    ):
        (secrets / name).write_text(value + "\n")
    with C.make_client(tmp_path / "state", secrets, C.Master(), [1_800_000_000.0]) as client:
        yield client


@pytest.mark.parametrize("epoch,epoch_at", [(0, 0), (3, None), (9, 1_800_000_000)])
def test_epoch_without_any_reward_answers_200_and_burns_the_whole_mass(
    http: Any, epoch: int, epoch_at: int | None
) -> None:
    """Defence: an epoch with no origin and no round (before genesis, default time, wall-clock
    time) is a 200 with an empty weight map and the full share mass burned to UID0, never an
    error and never a payment."""
    # When
    response = http_weights(http, epoch, epoch_at)
    # Then
    assert response.status_code == 200, response.text
    body = response.json()
    meta = body["metadata"]
    assert body["weights"] == {} and meta["reason"] == "ok"
    assert meta["units_paid"] == 0 and meta["units_burned_this_epoch"] == FULL_SHARE_MASS
    assert body["full_share_mass"] == FULL_SHARE_MASS
    assert http_weights(http, epoch, epoch_at).content == response.content


@pytest.mark.parametrize(
    "case", ["unresolved_dispute", "fraud", "inside_dispute_window", "settlement_never_accepted"]
)
def test_reward_of_a_disputed_or_unaudited_round_never_matures_nor_pays(
    hosted: Hosted, case: str
) -> None:
    """Defence: a finalized round whose settlement is open, fraudulent, still inside its release
    window or was never accepted by the service keeps its units reward_pending; get_weights for an
    epoch past mature_at pays nothing and burns the whole mass."""
    # Given: round 0 is finalized; the service's settlement view is one of the closed doors.
    h = hosted
    if case == "settlement_never_accepted":
        with h.store._tx():
            h.escrow.reward_finalize(h.reward, h.evidence)  # no accepted settlement record
    else:
        finalize(h)
        accepted = h.escrow.settlement(h.reward.origin_ids[0])
        view = {
            "unresolved_dispute": {"unresolved": True},
            "fraud": {"outcome": "FRAUD"},
            "inside_dispute_window": {"release_beacon": 21},
        }[case]
        h.escrow.settlement = lambda ref: accepted.model_copy(update=view)
    # When: maturity is attempted at the epoch's round, then the epoch is asked.
    with pytest.raises(EscrowError) as refused:
        mature(h)
    result = answer(h, 0)
    # Then
    expected = (
        "UNACCEPTED_SETTLEMENT" if case == "settlement_never_accepted" else ("DISPUTED_OR_UNVESTED")
    )
    assert refused.value.code == expected
    assert result["weights"] == {} and result["metadata"]["units_burned_this_epoch"] == 10**6
    b = h.escrow.balances()
    assert (b.reward_pending, b.available, b.paid) == (1_000_000, 0, 0) and pays(h.escrow) == 0


# --- C. races around finalize --------------------------------------------------------------


def run_threads(targets: list[Callable[[], None]]) -> None:
    """Run targets in real threads released together by a barrier."""
    barrier = threading.Barrier(len(targets))
    failures: list[BaseException] = []

    def body(target: Callable[[], None]) -> None:
        barrier.wait(timeout=10)
        try:
            target()
        except BaseException as error:  # reported to the test thread, never swallowed
            failures.append(error)

    threads = [threading.Thread(target=body, args=(t,)) for t in targets]
    for t in threads:
        t.start()
    for t in threads:
        t.join(timeout=60)
        assert not t.is_alive()
    assert not failures, failures


def test_concurrent_finalizes_of_one_round_issue_one_origin_set(
    tmp_path: Path, actual_reward: Any
) -> None:
    """Defence: two engines on separate connections finalizing the same round at the same instant
    serialize on BEGIN IMMEDIATE; one issues, the other replays the same receipt."""
    # Given: a barrier and independent connections, no timing luck.
    _, evidence, reward = actual_reward
    path = tmp_path / "ledger.db"
    file_ledger(actual_reward, path)
    engines = [file_ledger(actual_reward, path) for _ in range(3)]
    receipts: list[Any] = []

    def finalize_on(engine: EscrowV2) -> Callable[[], None]:
        return lambda: receipts.append(engine.reward_finalize(reward, evidence))

    # When
    run_threads([finalize_on(engine) for engine in engines])
    # Then: every caller holds the one receipt; one origin set exists.
    assert len(receipts) == 3 and len({r.event_hash for r in receipts}) == 1
    assert reward_rows(engines[0]) == (1, 1, 1)
    assert engines[0].balances().issued == 1_000_000 and engines[0].balances().conserved()


def test_concurrent_conflicting_finalizes_of_one_round_let_exactly_one_win(
    tmp_path: Path, actual_reward: Any
) -> None:
    """Defence: two different signed finalizes of the same round racing each other produce
    exactly one winner; the loser is refused and the persisted origins are the winner's."""
    # Given
    other_reward, other_evidence = signed_variant(actual_reward, entitlements_root="22" * 32)
    _, evidence, reward = actual_reward
    path = tmp_path / "ledger.db"
    file_ledger(actual_reward, path)
    engines = [file_ledger(actual_reward, path) for _ in range(2)]
    outcomes: list[str] = []

    def attempt(engine: EscrowV2, r: RewardFinalize, ev: FinalityEvidence) -> Callable[[], None]:
        def go() -> None:
            try:
                engine.reward_finalize(r, ev)
                outcomes.append(r.finalize_hash)
            except EscrowError as error:
                outcomes.append(error.code)

        return go

    # When
    run_threads(
        [
            attempt(engines[0], reward, evidence),
            attempt(engines[1], other_reward, other_evidence),
        ]
    )
    # Then: whichever was first won; the other saw OPERATION_CONFLICT.
    winner = engines[0].db.execute("SELECT finalize_hash FROM escrow_finalized").fetchone()[0]
    assert winner in (reward.finalize_hash, other_reward.finalize_hash)
    assert sorted(outcomes) == sorted([winner, "OPERATION_CONFLICT"])
    won = reward if winner == reward.finalize_hash else other_reward
    assert [r[0] for r in engines[0].db.execute("SELECT origin FROM escrow_origins")] == (
        won.origin_ids
    )
    assert reward_rows(engines[0]) == (1, 1, 1) and engines[0].balances().conserved()


def test_concurrent_get_weights_for_one_epoch_persist_and_pay_once(hosted: Hosted) -> None:
    """Defence: callers racing on one epoch are serialized by the answer's re-check inside the
    second transaction; every caller receives the first persisted answer and the epoch pays once.

    Four callers are parked in service resolution, between the two transactions of
    Store.weights, so each has already seen 'no stored answer' before any of them writes. They
    are parked one at a time because Store.weights reads the shared connection outside the store
    lock before that point (store.py:2064-2074, LED-1, MINEUR): the lock audit that pins that
    hole is test_requests_use_the_shared_connection_only_under_the_store_lock[get_weights] in
    test_finalize_and_epoch_service.py."""
    # Given: a matured unit and 4 callers parked between the two transactions.
    h = hosted
    finalize(h)
    mature(h)
    parked, resume = threading.Semaphore(0), threading.Event()
    services = h.store._services

    def parked_services(run_id: str) -> Any:
        parked.release()
        assert resume.wait(timeout=10)
        return services(run_id)

    h.store._services = parked_services
    answers: list[bytes] = []
    failures: list[BaseException] = []

    def call() -> None:
        try:
            answers.append(h.store.weights(0, at(h.escrow, 20), 1))
        except BaseException as error:  # reported to the test thread, never swallowed
            failures.append(error)

    threads = [threading.Thread(target=call) for _ in range(4)]
    for t in threads:
        t.start()
        assert parked.acquire(timeout=10)
    # When: all four resume together.
    resume.set()
    for t in threads:
        t.join(timeout=30)
        assert not t.is_alive()
    # Then
    assert not failures, failures
    assert len(answers) == 4 and len(set(answers)) == 1
    assert json.loads(answers[0])["weights"] == {HOT: 1_000_000.0}
    assert h.store._db.execute("SELECT COUNT(*) FROM weights_runs").fetchone()[0] == 1
    assert pays(h.escrow) == 1 and h.escrow.balances().paid == 1_000_000


@pytest.mark.parametrize(
    "order",
    list(itertools.permutations(["finalize", "mature", "ask"])),
    ids="-".join,
)
def test_finalize_maturity_and_weights_in_any_order_pay_each_unit_once(
    hosted: Hosted, order: tuple[str, ...]
) -> None:
    """Defence: finalize, maturity and get_weights never pay an unmatured unit and never pay a
    unit twice, in any of their six orders: the epoch answered in between pays what had matured by
    then, and the units left are paid once by the next epoch.

    Finalize and maturity are each one atomic transaction; get_weights is two, and the window
    between them is raced by the two tests around this one. The six serial orders are what the
    actors can produce when they do not interleave inside a get_weights call."""
    # Given
    h = hosted
    outcome: dict[str, Any] = {}

    def mature_when_possible() -> None:
        try:
            mature(h)
            outcome["matured"] = True
        except EscrowError as error:
            outcome["matured"] = error.code

    actors = {
        "finalize": lambda: finalize(h),
        "mature": mature_when_possible,
        "ask": lambda: outcome.update(first=answer(h, 0)),
    }
    # When
    for name in order:
        actors[name]()
    if outcome["matured"] is not True:
        assert outcome["matured"] == "IMMATURE_OR_UNKNOWN_ORIGIN"
        mature(h)
    second = answer(h, 1, 21)
    # Then: units are paid exactly once, in the epoch that followed their maturity.
    first = outcome["first"]
    paid_first = first["metadata"]["units_paid"]
    assert paid_first == (1_000_000 if order == ("finalize", "mature", "ask") else 0)
    assert paid_first + second["metadata"]["units_paid"] == 1_000_000
    assert first["metadata"]["units_burned_this_epoch"] == 10**6 - paid_first
    b = h.escrow.balances()
    assert (b.paid, b.available, b.reward_pending) == (1_000_000, 0, 0) and b.conserved()
    assert pays(h.escrow) == 1 and reward_rows(h.escrow) == (1, 1, 1)


def test_maturity_landing_between_the_two_transactions_of_get_weights_pays_once(
    hosted: Hosted,
) -> None:
    """Defence: get_weights holds no store lock while it resolves its services and reads the payable
    units inside the transaction that pays and persists the epoch, so a maturity that lands in
    that window is neither lost for that epoch nor paid twice (the same end state as the order
    finalize, mature, ask above, reached through a call that was already under way)."""
    # Given: a finalized, not yet matured round and a get_weights call parked between its
    # transactions.
    h = hosted
    finalize(h)
    parked, resume = threading.Event(), threading.Event()
    services = h.store._services

    def gated(run_id: str) -> Any:
        parked.set()
        assert resume.wait(timeout=10)
        return services(run_id)

    h.store._services = gated
    result: list[bytes] = []
    failures: list[BaseException] = []

    def call() -> None:
        try:
            result.append(h.store.weights(0, at(h.escrow, 20), 1))
        except BaseException as error:  # reported to the test thread, never swallowed
            failures.append(error)

    caller = threading.Thread(target=call)
    caller.start()
    assert parked.wait(timeout=10)
    # When: the maturity lands in the window, then the call resumes.
    mature(h)
    resume.set()
    caller.join(timeout=30)
    assert not caller.is_alive()
    # Then
    assert not failures, failures
    assert json.loads(result[0])["weights"] == {HOT: 1_000_000.0}
    later = json.loads(h.store.weights(1, at(h.escrow, 21), 2))
    assert later["weights"] == {} and pays(h.escrow) == 1 and h.escrow.balances().conserved()


# --- A (continued). randomized conservation ---------------------------------------------------

OPERATIONS = st.one_of(
    st.just(("mature", 0)),
    st.tuples(st.just("lock"), st.integers(1, 600_000)),
    st.tuples(st.just("send"), st.integers(1, 600_000)),
    st.tuples(st.just("back"), st.integers(1, 600_000)),
    st.just(("release", 0)),
    st.just(("weights", 0)),
    st.just(("again", 0)),
)


def op_id(counter: int) -> str:
    return sha256_hex(f"ac-ledger-op|{counter}".encode())


# derandomize: CI explores the same 40 sequences on every run, and the explicit examples pin the
# orderings the invariants exist for (units sent away or locked before a pay, a release, a replay),
# so a mutation of those paths is caught by every run and not by the luck of a seed.
@settings(max_examples=40, deadline=None, derandomize=True)
@example(
    operations=[("mature", 0), ("send", 400_000), ("weights", 0), ("back", 400_000), ("weights", 0)]
)
@example(
    operations=[
        ("mature", 0),
        ("lock", 300_000),
        ("weights", 0),
        ("release", 0),
        ("weights", 0),
        ("again", 0),
    ]
)
@example(operations=[("weights", 0), ("mature", 0), ("lock", 600_000), ("lock", 600_000)])
@given(operations=st.lists(OPERATIONS, max_size=14))
def test_random_operations_conserve_units_and_never_pay_a_unit_twice(
    tmp_path_factory: pytest.TempPathFactory, actual_reward: Any, operations: list[tuple[str, int]]
) -> None:
    """Defence: whatever the order of maturity, admission locks and releases, transfers away and
    back, and get_weights calls, units are conserved, every unit paid as weight leaves the
    earning owner's free balance exactly once, units held by another owner or locked are never
    paid, and an answered epoch is final."""
    _, evidence, reward = actual_reward
    settled = Settlement(
        finality_hash=reward.finalize_hash,
        closed_dispute_root=H,
        release_beacon=20,
        unresolved=False,
        outcome="MATCH",
    )
    with hosting(tmp_path_factory.mktemp("random"), actual_reward, lambda ref: settled) as h:
        h.escrow.reward_finalize(reward, evidence)
        origin = reward.origin_ids[0]
        matured, free, away, locks, paid, epoch = False, 0, 0, [], 0, 0
        answers: dict[int, bytes] = {}
        counter = 0
        for kind, units in operations:
            counter += 1
            if kind == "mature":
                if not matured:
                    mature(h)
                    matured, free = True, 1_000_000
            elif kind == "lock":
                lock = EscrowLock(
                    operation_id=op_id(counter),
                    owner=COLD,
                    units=units,
                    origin_ids=[origin],
                    admission_id=H,
                    dispute_id=None,
                    kind="LOCK_ADMISSION",
                )
                if units <= free:
                    h.escrow.lock(lock, signer=COLD)
                    free -= units
                    locks.append((lock.operation_id, units))
                else:
                    with pytest.raises(EscrowError):
                        h.escrow.lock(lock, signer=COLD)
            elif kind in ("send", "back"):
                sender, receiver = (COLD, OTHER) if kind == "send" else (OTHER, COLD)
                have = free if kind == "send" else away
                move = EscrowTransfer(
                    operation_id=op_id(counter),
                    owner=sender,
                    units=units,
                    origin_ids=[origin],
                    admission_id=None,
                    dispute_id=None,
                    recipient=receiver,
                )
                if units <= have:
                    h.escrow.transfer(move, signer=sender)
                    free, away = (
                        (free - units, away + units)
                        if kind == "send"
                        else (
                            free + units,
                            away - units,
                        )
                    )
                else:
                    with pytest.raises(EscrowError):
                        h.escrow.transfer(move, signer=sender)
            elif kind == "release" and locks:
                lock_id, units = locks.pop(0)
                h.escrow.release(
                    EscrowRelease(
                        operation_id=op_id(counter),
                        owner=COLD,
                        units=units,
                        origin_ids=[origin],
                        admission_id=H,
                        dispute_id=None,
                        lock_id=lock_id,
                        finality_hash=reward.finalize_hash,
                        closed_dispute_root=H,
                    ),
                    signer=COLD,
                    now_beacon=20,
                )
                free += units
            elif kind == "weights":
                text = h.store.weights(epoch, at(h.escrow, 20) + 300 * epoch, 1)
                answers[epoch] = text
                result = json.loads(text)
                weights = result["weights"]
                assert weights == ({HOT: float(free)} if free else {})
                assert result["metadata"]["units_burned_this_epoch"] == 10**6 - free
                paid, free, epoch = paid + free, 0, epoch + 1
            elif kind == "again" and answers:
                old = min(answers)
                assert h.store.weights(old, at(h.escrow, 20) + 300 * old, 2) == answers[old]
            b = h.escrow.balances()
            assert b.conserved() and b.issued == 1_000_000
            assert b.paid == paid
            assert b.available == free + away
            assert b.admission_locked == sum(units for _, units in locks)
            assert h.escrow.balances(OTHER).available == away
            h.escrow.verify()
        assert pays(h.escrow) == sum(1 for text in answers.values() if json.loads(text)["weights"])


# --- D. admission lock rows of the red-team table ------------------------------------------


def test_admission_lock_cannot_be_released_after_the_miner_was_rewarded(hosted: Hosted) -> None:
    """Defence: a miner that was included in a finalized round and holds matured reward units
    cannot release its admission lock early; the service has no accepted settlement for the lock.

    Today the refusal is because the release path is inert, not because of a policy decision:
    nothing writes a settlement record for an admission lock (RTA-06). The test pins the refusal
    so that whoever makes the path live must write the settlement from evidence."""
    # Given: round 0 finalized and matured, 400k of the miner's free units locked for admission.
    h = hosted
    finalize(h)
    mature(h)
    lock = E.lock(h.reward.origin_ids[0], "22" * 32, 400_000)
    h.escrow.lock(lock, signer=COLD)
    release = EscrowRelease(
        operation_id="33" * 32,
        owner=COLD,
        units=400_000,
        origin_ids=h.reward.origin_ids,
        admission_id=H,
        dispute_id=None,
        lock_id=lock.operation_id,
        finality_hash=h.reward.finalize_hash,
        closed_dispute_root=sha256_hex(canonicalize([])),
    )
    # When / Then: the release is refused and the stake stays locked, unpayable.
    with pytest.raises(EscrowError, match="UNACCEPTED_SETTLEMENT"):
        h.escrow.release(release, signer=COLD, now_beacon=10**6)
    assert h.escrow.balances().admission_locked == 400_000 and h.escrow.balances().conserved()


FAR = 2**62


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="RTA-09 (MINEUR, needs the internal bearer): epoch_at is bounded only by 2**63. One "
    "call with a far-future epoch_at is journaled as the newest answer time (ledger.py:313, "
    "368-371), so every later honest epoch answers out_of_order (all mass burned) for good: no "
    "wall-clock epoch_at is ever accepted again",
)
def test_far_future_epoch_at_does_not_freeze_later_honest_epochs(http: Any) -> None:
    """Desired defence: a far-future epoch_at is refused, or at least does not turn every later
    honest epoch into an out_of_order answer."""
    # When: one call with an absurd epoch_at, then the next epoch at wall-clock time.
    far = http_weights(http, 1, FAR)
    precondition(far.status_code < 500, f"far call crashed: {far.text}")
    honest = http_weights(http, 2, 1_800_000_000)
    # Then
    assert 400 <= far.status_code < 500 or honest.json()["metadata"]["reason"] == "ok"


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="RTA-09 (MINEUR, needs the internal bearer): epoch is bounded only by 2**63; one call "
    "with a far epoch is journaled as the newest epoch (ledger.py:313, 368-371), so every later "
    "honest epoch answers out_of_order for good",
)
def test_far_future_epoch_does_not_freeze_later_honest_epochs(http: Any) -> None:
    """Desired defence: a far-future epoch number is refused, or at least does not turn every later
    honest epoch into an out_of_order answer."""
    # When: one call with an absurd epoch, then the next epoch at wall-clock time.
    far = http_weights(http, FAR, 1_800_000_000)
    precondition(far.status_code < 500, f"far call crashed: {far.text}")
    honest = http_weights(http, 2, 1_800_000_000)
    # Then
    assert 400 <= far.status_code < 500 or honest.json()["metadata"]["reason"] == "ok"
