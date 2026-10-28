"""get_weights: what an epoch answer may pay, and that it pays it once (WGT-01 .. WGT-23).

Every test states a defence of the weights path: it reads only units that were finalized, settled
and matured, never pays a unit twice, keeps its answer for ever and fails closed. The unit under
test is `ChallengeStore.weights`. Two hosts feed it: the real signed w=0 reward (a PRODUCTION
escrow with the store's own settlement records) and a test-mode escrow whose live origins are
written the way issuance and maturity leave them, which lets a property quantify over origins.
The HTTP boundary (bearer, metagraph, clock) and the merge of several runs are in
test_weights_2.py.
"""

from __future__ import annotations

import sqlite3
import tempfile
from collections.abc import Iterator
from contextlib import suppress
from pathlib import Path
from types import ModuleType

import pytest
from hypothesis import given
from hypothesis import strategies as st

from hypertrain.challenge.store import ChallengeError
from hypertrain.ledger import FULL_SHARE_MASS
from hypertrain.ledger.escrow_v2 import EscrowError, Settlement
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages_v2 import (
    EscrowLock,
    EscrowOperation,
    EscrowRelease,
    EscrowTransfer,
)

from .support import Hosted, Real, at, error_code, hosted_reward, mature, pays
from .support_weights import (
    assert_answer_shape,
    events,
    fail_persist,
    mode_store,
    open_store,
    parse,
    pay_events,
    pay_id,
    recover_persist,
    reward_store,
    stored_bytes,
)

ISSUED = 1_000_000
HOLD_AT = 20  # the drand round the real reward matures at


@pytest.fixture
def finalized(tmp_path: Path, li_escrow: ModuleType, real_reward: Real) -> Iterator[Hosted]:
    """The real reward accepted by finalize with its settlement record, still reward_pending."""
    with hosted_reward(tmp_path, li_escrow, real_reward, real_settlement=True) as hosted:
        hosted.finalize()
        yield hosted


def ask(host: Hosted, epoch: int, rnd: int = HOLD_AT, computed_at: int = 1) -> dict:
    return parse(host.store.weights(epoch, at(host.escrow, rnd), computed_at))


def weights_rows(db: sqlite3.Connection) -> int:
    return int(db.execute("SELECT COUNT(*) FROM weights_runs").fetchone()[0])


def settlement_record(host: Hosted, **changes: object) -> None:
    """Replace the service's settlement record of the reward origin (test-owned stand-in)."""
    record = Settlement(
        finality_hash=host.reward.finalize_hash,
        closed_dispute_root=sha256_hex(canonicalize([])),
        release_beacon=host.reward.mature_at,
        unresolved=False,
        outcome="MATCH",
    ).model_copy(update=changes)
    host.store._put_record_v2(host.run_id, "settlement", host.reward.origin_ids[0], record.body())


# --- WGT-01: only live reward origins are ever paid ----------------------------------------------


def test_genesis_collateral_matured_next_to_a_live_origin_is_never_emitted(
    li_escrow: ModuleType,
) -> None:
    """WGT-01: free, matured genesis units (no hotkey) are collateral; only the live origin pays."""
    # Given: 30000 genesis units matured and free, and one live origin of 300k.
    with mode_store(li_escrow) as host:
        genesis = li_escrow.fund(host.escrow)
        host.origin("aa" * 32, "live-hotkey", 300_000, 5)
        assert host.escrow.balances().available == 330_000
        # When
        first = host.answer(0, HOLD_AT)
        second = host.answer(1, HOLD_AT + 1)
        # Then
        assert first["weights"] == {"live-hotkey": 300_000.0}
        assert_answer_shape(first)
        assert second["weights"] == {} and second["metadata"]["units_paid"] == 0
        assert [(e["origin"], e["units"]) for e in pay_events(host.escrow)] == [
            ("aa" * 32, 300_000)
        ]
        for origin, units in zip(genesis, (20_000, 10_000), strict=True):
            row = host.escrow.db.execute(
                "SELECT bucket,units FROM escrow_units WHERE origin=? AND units>0", (origin,)
            ).fetchall()
            assert [tuple(r) for r in row] == [("available", units)]


# --- WGT-02: a round that is not closed mints no payable weight ----------------------------------


@pytest.mark.parametrize(
    ("gate", "code"),
    [
        ("unaccepted", "UNACCEPTED_SETTLEMENT"),
        ("unresolved", "DISPUTED_OR_UNVESTED"),
        ("fraud", "DISPUTED_OR_UNVESTED"),
        ("inside_window", "DISPUTED_OR_UNVESTED"),
        ("other_finality", "DISPUTED_OR_UNVESTED"),
    ],
)
def test_a_reward_whose_settlement_is_not_closed_stays_pending_and_pays_nothing(
    tmp_path: Path, li_escrow: ModuleType, real_reward: Real, gate: str, code: str
) -> None:
    """WGT-02 (condition): an unaccepted, disputed, fraudulent or early settlement holds the reward.

    The condition is the settlement the service records at finalize. Broken in a test-owned record,
    maturity is refused with its own code and the epoch burns; closed again, the same units pay.
    """
    with hosted_reward(tmp_path, li_escrow, real_reward, real_settlement=True) as host:
        # Given: the round is finalized; the settlement the service would trust is one of the gates.
        if gate == "unaccepted":
            with host.store._tx():
                host.escrow.reward_finalize(host.reward, host.evidence)
        else:
            host.finalize()
            changes: dict[str, object] = {
                "unresolved": {"unresolved": True},
                "fraud": {"outcome": "FRAUD"},
                "inside_window": {"release_beacon": HOLD_AT + 5},
                "other_finality": {"finality_hash": "22" * 32},
            }[gate]
            settlement_record(host, **changes)
        before = host.escrow.balances()
        # When: maturity is tried at the epoch's round and the epoch is asked.
        assert error_code(lambda: mature(host.escrow, host.reward)) == code
        answer = ask(host, 0)
        # Then: nothing matured, nothing paid, the whole mass burns.
        assert answer["weights"] == {} and answer["metadata"]["units_burned_this_epoch"] == ISSUED
        assert host.escrow.balances() == before and before.reward_pending == ISSUED
        assert pays(host.escrow) == 0
        if gate in ("fraud", "other_finality"):
            return
        # And: once the settlement is closed and the window passed, the same units pay.
        settlement_record(host, release_beacon=HOLD_AT)
        mature(host.escrow, host.reward, operation_id="56" * 32)
        later = ask(host, 1, HOLD_AT + 1)
        assert later["weights"] == {li_escrow.HOT.ss58: float(ISSUED)}
        assert pays(host.escrow) == 1


# --- WGT-03: maturity comes from the pushed beacon, never from the caller's epoch_at -------------


def test_a_unit_vests_at_its_maturity_round_not_before(
    finalized: Hosted, li_escrow: ModuleType
) -> None:
    """WGT-03: vesting one round early is refused with its code and moves nothing."""
    host = finalized
    origins = tuple(host.reward.origin_ids)
    events_before, balances_before = events(host.escrow), host.escrow.balances()
    # When: vesting is asked one round before mature_at, then the epoch far past it.
    code = error_code(lambda: host.escrow.mature("55" * 32, origins, host.reward.mature_at - 1))
    answer = ask(host, 0, rnd=10_000)
    # Then
    assert code == "IMMATURE_OR_UNKNOWN_ORIGIN"
    assert answer["weights"] == {} and answer["metadata"]["units_burned_this_epoch"] == ISSUED
    assert events(host.escrow) == events_before and host.escrow.balances() == balances_before
    # And at its round it vests and the next epoch pays it.
    host.escrow.mature("55" * 32, origins, host.reward.mature_at)
    assert ask(host, 1, 10_001)["weights"] == {li_escrow.HOT.ss58: float(ISSUED)}


@given(
    matured=st.booleans(),
    asks=st.lists(
        st.tuples(st.one_of(st.integers(1, 40), st.integers(10**6, 10**9)), st.integers(0, 2)),
        min_size=1,
        max_size=4,
    ),
)
def test_no_unit_pays_before_the_pushed_beacon_matured_it_whatever_the_epoch_at(
    li_escrow: ModuleType, real_reward: Real, matured: bool, asks: list[tuple[int, int]]
) -> None:
    """WGT-03 (property): over any increasing epoch_at, a pending unit never pays; a vested unit
    pays in the first epoch whose drand round reached its maturity, and only once."""
    # Given: a finalized reward, vested or not.
    beacon = real_reward[0].manifest.training.beacon
    with (
        tempfile.TemporaryDirectory(prefix="li-weights-") as scratch,
        hosted_reward(Path(scratch), li_escrow, real_reward, real_settlement=True) as host,
    ):
        host.finalize()
        if matured:
            mature(host.escrow, host.reward)
        paid_so_far = 0
        for epoch, (rnd, offset) in enumerate(sorted(asks)):
            epoch_at = beacon.genesis_time + (rnd - 1) * beacon.period + offset
            # When
            answer = parse(host.store.weights(epoch, epoch_at, 1))
            # Then: exactly the vested, due and not yet paid units.
            due = ISSUED if matured and rnd >= HOLD_AT and not paid_so_far else 0
            assert answer["metadata"]["units_paid"] == due
            assert answer["metadata"]["units_burned_this_epoch"] == FULL_SHARE_MASS - due
            paid_so_far += due
        balances = host.escrow.balances()
        assert (balances.paid, pays(host.escrow)) == (paid_so_far, int(paid_so_far > 0))
        assert balances.reward_pending == (0 if matured else ISSUED)


# --- WGT-05: an earner whose settlement turned bad after vesting is still paid -------------------

FROZEN_FRAUD = pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="WGT-05+WGT-D3: units whose origin settlement became FRAUD after vesting are not "
    "emitted (payable() consults no settlement; ADR 0010 part 6 burns unvested rewards only) "
    "[candidate] [fix-lot: decision (product choice pending)]",
)
FROZEN_DISPUTE = pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="WGT-05+WGT-D3: units whose origin settlement is disputed again after vesting are "
    "not emitted (docs/network-v2.md lists disputed units among those never paid) [candidate] "
    "[fix-lot: decision (product choice pending)]",
)


@pytest.mark.parametrize(
    "view",
    [
        "closed",
        pytest.param("fraud", marks=FROZEN_FRAUD),
        pytest.param("unresolved", marks=FROZEN_DISPUTE),
    ],
)
def test_vested_units_of_a_fraudulent_or_disputed_origin_are_not_emitted(
    finalized: Hosted, li_escrow: ModuleType, view: str
) -> None:
    # Given: the reward vested under a closed MATCH settlement; the control keeps it closed.
    host = finalized
    mature(host.escrow, host.reward)
    if view == "fraud":
        settlement_record(host, outcome="FRAUD")
    elif view == "unresolved":
        settlement_record(host, unresolved=True)
    # When
    answer = ask(host, 0)
    # Then: only the closed settlement pays.
    expected = {li_escrow.HOT.ss58: float(ISSUED)} if view == "closed" else {}
    assert answer["weights"] == expected


# --- WGT-06: one final, byte-identical answer per epoch, across restarts -------------------------


def test_an_epoch_answer_is_final_for_any_later_epoch_at_computed_at_and_restart(
    tmp_path: Path, li_escrow: ModuleType, real_reward: Real
) -> None:
    with hosted_reward(tmp_path, li_escrow, real_reward, real_settlement=True) as host:
        host.finalize()
        mature(host.escrow, host.reward)
        # When: epoch 0 is asked, then again at later and earlier chain times and other clocks.
        first = host.store.weights(0, at(host.escrow, HOLD_AT), 1)
        later = host.store.weights(0, at(host.escrow, HOLD_AT + 20), 99)
        earlier = host.store.weights(0, at(host.escrow, 3), 5)
        # Then: the first bytes stand and the units were paid once.
        assert parse(first)["weights"] == {li_escrow.HOT.ss58: float(ISSUED)}
        assert parse(first)["computed_at"] == 1
        assert later == first and earlier == first
        assert pays(host.escrow) == 1 and weights_rows(host.store._db) == 1
        balances = host.escrow.balances()
        # And: a restart of the service answers the same bytes and pays nothing again.
        with reward_store(tmp_path, li_escrow, real_reward, host) as again:
            assert again.store.weights(0, at(again.escrow, HOLD_AT + 40), 7) == first
            assert again.escrow.balances() == balances and pays(again.escrow) == 1
            nxt = parse(again.store.weights(1, at(again.escrow, HOLD_AT + 41), 8))
            assert nxt["weights"] == {} and nxt["metadata"]["units_burned_this_epoch"] == ISSUED


# --- WGT-07: pay and persist are one transaction (condition: the same epoch is retried) ---------


def test_a_failed_persist_pays_nothing_and_the_retry_of_the_same_epoch_pays_once(
    finalized: Hosted, li_escrow: ModuleType
) -> None:
    """WGT-07 (condition kept): the crash between pay and persist rolls the pay back."""
    host = finalized
    mature(host.escrow, host.reward)
    balances, written = host.escrow.balances(), events(host.escrow)
    fail_persist(host.store._db)
    # When: the persist fails twice.
    for attempt in (1, 2):
        with pytest.raises(sqlite3.IntegrityError):
            host.store.weights(0, at(host.escrow, HOLD_AT), attempt)
        # Then: no unit moved, no event was written, no answer exists.
        assert host.escrow.balances() == balances and events(host.escrow) == written
        assert weights_rows(host.store._db) == 0
    # And: the retry of the same epoch pays the unit once; its answer is the one the first attempt
    # journaled (the retry's own computed_at is not in it), and it is final.
    recover_persist(host.store._db)
    retry = ask(host, 0, computed_at=9)
    assert retry["weights"] == {li_escrow.HOT.ss58: float(ISSUED)}
    assert retry["computed_at"] == 1
    assert host.escrow.balances().paid == ISSUED and pays(host.escrow) == 1
    assert host.store.weights(0, at(host.escrow, HOLD_AT), 9) == host.store.weights(
        0, at(host.escrow, HOLD_AT + 3), 10
    )


# --- WGT-08: an out-of-order epoch is answered empty and final ----------------------------------


def test_an_epoch_below_the_newest_answered_one_is_empty_final_and_pays_later(
    li_escrow: ModuleType,
) -> None:
    with mode_store(li_escrow) as host:
        host.origin("aa" * 32, "hk", ISSUED, 5)
        host.answer(5, 4)
        # When: epoch 4 is asked after epoch 5, with an epoch_at that would be payable.
        late = host.store.weights(4, at(host.escrow, 30), 2)
        # Then: empty, full burn, nothing paid, and the units are still free.
        answer = parse(late)
        assert answer["metadata"]["reason"] == "out_of_order"
        assert answer["weights"] == {} and answer["metadata"]["units_paid"] == 0
        assert answer["metadata"]["units_burned_this_epoch"] == FULL_SHARE_MASS
        assert pays(host.escrow) == 0 and host.escrow.balances().available == ISSUED
        # And the answer is final: epoch 4 never gets completed later.
        assert host.store.weights(4, at(host.escrow, 40), 3) == late
        # And the next in-order epoch pays the units.
        assert host.answer(6, 31)["weights"] == {"hk": float(ISSUED)}


def test_a_higher_epoch_with_an_earlier_epoch_at_is_out_of_order(li_escrow: ModuleType) -> None:
    with mode_store(li_escrow) as host:
        host.origin("aa" * 32, "hk0", 600_000, 5)
        host.origin("bb" * 32, "hk1", 900_000, 6)
        assert host.answer(3, HOLD_AT)["metadata"]["units_paid"] == FULL_SHARE_MASS
        # When: epoch 4 is asked with an epoch_at before the one epoch 3 was answered at.
        late = host.store.weights(4, at(host.escrow, 10), 2)
        # Then
        answer = parse(late)
        assert answer["metadata"]["reason"] == "out_of_order" and answer["weights"] == {}
        assert [e["units"] for e in pay_events(host.escrow)] == [600_000, 400_000]
        assert host.store.weights(4, at(host.escrow, 50), 3) == late
        # And the units it did not pay stay owed: the next in-order epoch pays them.
        assert host.answer(5, HOLD_AT + 1)["weights"] == {"hk1": 500_000.0}


def test_epochs_asked_at_the_same_chain_time_are_both_in_order(li_escrow: ModuleType) -> None:
    """WGT-08: only an epoch_at before the newest answered one is out of order, not an equal one."""
    with mode_store(li_escrow) as host:
        host.origin("aa" * 32, "hk", 1_500_000, 5)
        # When: two consecutive epochs are asked at the same chain time.
        first, second = host.answer(0, HOLD_AT), host.answer(1, HOLD_AT)
        # Then: both are answered in order and the second pays what the first left owed.
        assert (first["metadata"]["reason"], second["metadata"]["reason"]) == ("ok", "ok")
        assert (first["weights"], second["weights"]) == ({"hk": 1_000_000.0}, {"hk": 500_000.0})


# --- WGT-10: an epoch without payable units is a defined full burn -------------------------------


@pytest.mark.parametrize("case", ["pending", "early"])
def test_a_round_without_payable_units_answers_a_defined_full_burn(
    finalized: Hosted, case: str
) -> None:
    """WGT-10: finalized work that has not vested (or is not due) is an explicit empty answer."""
    host = finalized
    if case == "early":
        mature(host.escrow, host.reward)
    rnd = HOLD_AT if case == "pending" else HOLD_AT - 1
    balances = host.escrow.balances()
    # When
    first = host.store.weights(0, at(host.escrow, rnd), 1)
    # Then
    answer = parse(first)
    assert_answer_shape(answer)
    assert answer["weights"] == {} and answer["metadata"]["reason"] == "ok"
    assert answer["metadata"]["units_paid"] == 0
    assert answer["metadata"]["units_burned_this_epoch"] == FULL_SHARE_MASS
    assert host.escrow.balances() == balances and pays(host.escrow) == 0
    assert host.store.weights(0, at(host.escrow, rnd + 5), 9) == first


def test_a_partly_vested_epoch_pays_the_vested_part_and_burns_the_rest(
    li_escrow: ModuleType,
) -> None:
    with mode_store(li_escrow) as host:
        host.origin("aa" * 32, "vested", 200_000, 5)
        host.origin("bb" * 32, "pending-until-30", 500_000, 30)
        first = host.answer(0, HOLD_AT)
        assert first["weights"] == {"vested": 200_000.0}
        assert first["metadata"]["units_burned_this_epoch"] == 800_000
        assert_answer_shape(first)
        assert host.answer(1, 30)["weights"] == {"pending-until-30": 500_000.0}


@pytest.mark.parametrize("hosted_run", [True, False])
def test_a_store_that_finalized_no_reward_answers_a_defined_full_burn(
    tmp_path: Path, li_escrow: ModuleType, hosted_run: bool
) -> None:
    """WGT-10: units that sit in an escrow without a finalized live reward are not emitted, and a
    store that hosts no run at all answers the same explicit burn."""
    if hosted_run:
        with mode_store(li_escrow, gate=False) as host:
            host.origin("aa" * 32, "hk", 300_000, 5)
            answer = host.answer(0, HOLD_AT)
            assert host.escrow.balances().available == 300_000 and pays(host.escrow) == 0
    else:
        store = open_store(tmp_path, 1_900_000_000)
        try:
            answer = parse(store.weights(0, None, 1))
        finally:
            store.close()
    assert_answer_shape(answer)
    assert answer["weights"] == {} and answer["metadata"]["reason"] == "ok"
    assert answer["metadata"]["units_burned_this_epoch"] == FULL_SHARE_MASS


def test_a_failing_service_build_refuses_the_new_epoch_before_anything_is_journaled(
    finalized: Hosted,
) -> None:
    """WGT-10 (condition): without its escrow the store refuses; it never answers a bare burn."""
    host = finalized
    mature(host.escrow, host.reward)
    healthy = host.store._services
    journal = len(host.store.ledger.journal.records)

    def refuse(run_id: str) -> tuple:
        raise ChallengeError(403, "backend qualification authority differs")

    host.store._services = refuse
    # When
    with pytest.raises(ChallengeError) as refused:
        host.store.weights(0, at(host.escrow, HOLD_AT), 1)
    # Then: the refusal is the service error; no journal record, no stored answer, no unit moved.
    assert refused.value.status == 403
    assert len(host.store.ledger.journal.records) == journal
    assert weights_rows(host.store._db) == 0 and pays(host.escrow) == 0
    # And once the service builds again the same epoch answers and pays.
    host.store._services = healthy
    assert ask(host, 0)["metadata"]["units_paid"] == ISSUED


# --- WGT-11, WGT-22: mass conservation, vector shape and the order units are paid in ------------


def oldest_first(books: dict[str, list]) -> list[str]:
    """The documented payment order: oldest maturity first, then origin id."""
    return sorted(books, key=lambda origin: (books[origin][1], origin))


def expected_epoch(books: dict[str, list], rnd: int) -> dict[str, int]:
    """What the documentation says an epoch at drand round `rnd` pays, and consume it.

    books maps origin -> [units left, mature_at, hotkey]; one epoch pays at most the full mass.
    """
    budget, paid = FULL_SHARE_MASS, {}
    for origin in oldest_first(books):
        left, due, hotkey = books[origin]
        take = min(budget, left) if due <= rnd else 0
        if take:
            books[origin][0] -= take
            paid[hotkey] = paid.get(hotkey, 0) + take
            budget -= take
    return paid


@given(
    origins=st.lists(
        st.tuples(st.integers(0, 3), st.integers(1, 1_500_000), st.integers(1, 30)),
        max_size=6,
    ),
    rounds=st.lists(st.integers(1, 40), min_size=1, max_size=4),
)
def test_every_stored_answer_conserves_the_mass_and_pays_what_the_documentation_says(
    li_escrow: ModuleType, origins: list[tuple[int, int, int]], rounds: list[int]
) -> None:
    """WGT-11 (property): weights are exact non-negative integers that add up to units_paid, paid
    plus burned is the full mass, and each epoch pays the oldest vested units first."""
    with mode_store(li_escrow) as host:
        books: dict[str, list] = {}
        for index, (hotkey, units, due) in enumerate(origins):
            books[f"{index:02x}" * 32] = [units, due, f"hk{hotkey}"]
            host.origin(f"{index:02x}" * 32, f"hk{hotkey}", units, due)
        delivered: dict[str, int] = {}
        for epoch, rnd in enumerate(sorted(rounds)):
            # When
            answer = host.answer(epoch, rnd)
            # Then
            assert_answer_shape(answer)
            paid = expected_epoch(books, rnd)
            assert answer["weights"] == {hotkey: float(paid[hotkey]) for hotkey in sorted(paid)}
            for hotkey, units in paid.items():
                delivered[hotkey] = delivered.get(hotkey, 0) + units
        balances = host.escrow.balances()
        assert balances.conserved() and balances.paid == sum(delivered.values())
        assert sum(e["units"] for e in pay_events(host.escrow)) == balances.paid


def test_the_answer_envelope_has_exactly_the_documented_fields(li_escrow: ModuleType) -> None:
    """WGT-11/WGT-18: a pure-v2 store still answers in the ledger's envelope.

    docs/challenge-routes.md names the top-level fields and docs/mechanism.md the counters; the
    metadata keys beyond those are the ones the ledger emits today, so a new key is a decision.
    """
    with mode_store(li_escrow) as host:
        host.origin("aa" * 32, "hk", 1_000, 5)
        answer = host.answer(0, HOLD_AT)
    assert set(answer) == {
        "challenge_slug",
        "epoch",
        "weights",
        "full_share_mass",
        "metadata",
        "computed_at",
    }
    assert set(answer["metadata"]) == {
        "epoch_at",
        "round_at",
        "reason",
        "units_paid",
        "units_burned_this_epoch",
        "hotkeys_capped",
        "ledger_minted",
        "ledger_burned",
        "ledger_paid",
        "ledger_pending",
    }
    assert (answer["challenge_slug"], answer["epoch"], answer["computed_at"]) == (
        "hypertrain",
        0,
        1,
    )


@given(order=st.permutations(range(5)))
def test_payment_order_is_oldest_maturity_then_origin_id_whatever_the_insertion_order(
    li_escrow: ModuleType, order: list[int]
) -> None:
    """WGT-22: a short budget pays a deterministic prefix; the rest stays owed for later epochs."""
    shape = [(700_000, 9), (700_000, 9), (700_000, 4), (700_000, 4), (700_000, 12)]
    with mode_store(li_escrow) as host:
        books = {}
        for index in order:
            units, due = shape[index]
            books[f"{index + 1:02x}" * 32] = [units, due, f"hk{index}"]
            host.origin(f"{index + 1:02x}" * 32, f"hk{index}", units, due)
        for epoch, rnd in enumerate((HOLD_AT, HOLD_AT + 1, HOLD_AT + 2, HOLD_AT + 3)):
            paid = expected_epoch(books, rnd)
            assert host.answer(epoch, rnd)["weights"] == {
                hotkey: float(paid[hotkey]) for hotkey in sorted(paid)
            }
        assert host.escrow.balances().available == 0


# --- WGT-12, WGT-13: each unit pays at most once; pay ids are deterministic ----------------------


def test_an_origin_is_never_paid_beyond_what_it_issued_across_epochs(li_escrow: ModuleType) -> None:
    with mode_store(li_escrow) as host:
        origin = "aa" * 32
        host.origin(origin, "hk", 1_500_000, 5)
        # When: three consecutive epochs.
        answers = [host.answer(epoch, HOLD_AT + epoch) for epoch in range(3)]
        # Then: the budget splits the origin over two epochs and the third has nothing to pay.
        assert [a["metadata"]["units_paid"] for a in answers] == [1_000_000, 500_000, 0]
        paid = pay_events(host.escrow)
        assert [e["units"] for e in paid] == [1_000_000, 500_000]
        assert len({e["id"] for e in paid}) == 2
        balances = host.escrow.balances()
        assert (balances.paid, balances.available, balances.issued) == (1_500_000, 0, 1_500_000)
        assert balances.conserved()
        # And the escrow refuses one more unit for the exhausted origin, whoever asks.
        extra = EscrowOperation(
            operation_id="77" * 32,
            owner=li_escrow.COLD.ss58,
            units=1,
            origin_ids=[origin],
            admission_id=None,
            dispute_id=None,
        )
        written = events(host.escrow)
        code = error_code(lambda: host.escrow.pay(extra, signer=li_escrow.COLD.ss58))
        assert code == "UNFUNDED_OR_UNKNOWN_ORIGIN"
        assert events(host.escrow) == written and host.escrow.balances() == balances
        # And a re-asked epoch is the stored answer and pays nothing.
        assert parse(host.store.weights(1, at(host.escrow, 99), 5)) == answers[1]
        assert pays(host.escrow) == 2


def test_pay_ids_depend_only_on_run_epoch_origin_and_owner(li_escrow: ModuleType) -> None:
    """WGT-13: the same payout asked at another chain time and clock gets the same id."""
    shape = (("aa" * 32, "hk0", 1_500_000, 5), ("bb" * 32, "hk1", 400_000, 6))
    seen: list[list[str]] = []
    for rnd, computed_at in ((HOLD_AT, 1), (HOLD_AT + 40, 999)):
        with mode_store(li_escrow) as host:
            for origin in shape:
                host.origin(*origin)
            host.answer(0, rnd, computed_at)
            host.answer(1, rnd + 1, computed_at)
            seen.append([e["id"] for e in pay_events(host.escrow)])
    assert seen[0] == seen[1] and len(seen[0]) == 3
    assert len(set(seen[0])) == 3


def test_each_stored_answer_is_the_sum_of_the_pay_events_that_carry_its_epoch(
    li_escrow: ModuleType,
) -> None:
    """WGT-19: every unit of every stored answer is one PAY event of its epoch, and the reverse."""
    shape = (
        ("aa" * 32, "hk0", 900_000, 5),
        ("bb" * 32, "hk1", 800_000, 6),
        ("cc" * 32, "hk0", 500_000, 6),
    )
    owner = li_escrow.COLD.ss58
    with mode_store(li_escrow) as host:
        for origin in shape:
            host.origin(*origin)
        answers = {epoch: host.answer(epoch, HOLD_AT + epoch) for epoch in range(4)}
        by_id = {e["id"]: e["units"] for e in pay_events(host.escrow)}
        run_id = host.escrow.run_id
        accounted: set[str] = set()
        for epoch, answer in answers.items():
            rebuilt: dict[str, float] = {}
            for origin, hotkey, _, _ in shape:
                event = pay_id(run_id, epoch, origin, owner)
                if event in by_id:
                    accounted.add(event)
                    rebuilt[hotkey] = rebuilt.get(hotkey, 0.0) + by_id[event]
            assert answer["weights"] == dict(sorted(rebuilt.items()))
        assert accounted == set(by_id) and sum(by_id.values()) == 2_200_000


@pytest.mark.parametrize("preempt", [False, True])
def test_a_client_event_carrying_the_pay_id_never_blocks_the_payout(
    finalized: Hosted, li_escrow: ModuleType, preempt: bool
) -> None:
    host = finalized
    mature(host.escrow, host.reward)
    origin, cold, other = host.reward.origin_ids[0], li_escrow.COLD.ss58, li_escrow.OTHER.ss58

    def transfer(operation: str, owner: str, units: int) -> EscrowTransfer:
        return EscrowTransfer(
            operation_id=operation,
            owner=owner,
            units=units,
            origin_ids=[origin],
            admission_id=None,
            dispute_id=None,
            recipient=other,
        )

    # Given: another owner holds one free unit, received by an ordinary owner-signed transfer.
    host.escrow.transfer(transfer("66" * 32, cold, 1), signer=cold)
    # When: that owner signs an event whose id is the one the service derives for the payout of
    # epoch 0 (the control signs nothing).
    if preempt:
        # The same event under an ordinary id is accepted, so a refusal below is about its id.
        host.escrow.transfer(transfer("67" * 32, other, 1), signer=other)
        # A ledger may refuse this event or keep it apart from the key of the payout.
        with suppress(EscrowError):
            host.escrow.transfer(
                transfer(pay_id(host.run_id, 0, origin, cold), other, 1), signer=other
            )
    # Then: either way the epoch pays the earner its free units, in one PAY event.
    answer = ask(host, 0)
    assert answer["weights"] == {li_escrow.HOT.ss58: float(ISSUED - 1)}
    assert pays(host.escrow) == 1


# --- WGT-14: one entry per hotkey, origins and owners summed -------------------------------------


def test_origins_of_one_hotkey_are_summed_into_one_entry_whatever_their_owner(
    li_escrow: ModuleType,
) -> None:
    with mode_store(li_escrow) as host:
        cold, other = li_escrow.COLD.ss58, li_escrow.OTHER.ss58
        host.origin("aa" * 32, "hk-a", 100_000, 5, owner=cold)
        host.origin("bb" * 32, "hk-a", 200_000, 6, owner=other)
        host.origin("cc" * 32, "hk-a", 50_000, 7, owner=cold)
        host.origin("dd" * 32, "hk-b", 300_000, 7, owner=cold)
        # When
        answer = host.answer(0, HOLD_AT)
        # Then: parse() already refused a hotkey listed twice; the sums are exact.
        assert answer["weights"] == {"hk-a": 350_000.0, "hk-b": 300_000.0}
        assert_answer_shape(answer)
        assert len(pay_events(host.escrow)) == 4


# --- WGT-17: a stored answer does not move when the escrow changes afterwards --------------------


def test_a_stored_answer_does_not_move_when_units_are_locked_or_burned_later(
    tmp_path: Path, li_escrow: ModuleType, real_reward: Real
) -> None:
    with hosted_reward(tmp_path, li_escrow, real_reward, real_settlement=False) as host:
        host.finalize()
        mature(host.escrow, host.reward)
        origin, cold = host.reward.origin_ids[0], li_escrow.COLD.ss58
        lock = EscrowLock(
            operation_id="33" * 32,
            owner=cold,
            units=400_000,
            origin_ids=[origin],
            admission_id=li_escrow.H,
            dispute_id=None,
            kind="LOCK_ADMISSION",
        )
        host.escrow.lock(lock, signer=cold)
        first = host.store.weights(0, at(host.escrow, HOLD_AT), 1)
        assert parse(first)["weights"] == {li_escrow.HOT.ss58: 600_000.0}
        stored = host.store._db.execute("SELECT answer FROM weights_runs").fetchall()
        # When: the lock is slashed after the answer, and the epoch is asked again.
        host.fraud.add(lock.operation_id)
        slash = EscrowRelease(
            operation_id="44" * 32,
            owner=cold,
            units=400_000,
            origin_ids=[origin],
            admission_id=li_escrow.H,
            dispute_id=None,
            lock_id=lock.operation_id,
            finality_hash=host.reward.finalize_hash,
            closed_dispute_root=li_escrow.H,
        )
        host.escrow.slash(slash, authority=li_escrow.COORD.ss58, now_beacon=HOLD_AT)
        # Then: the burn took the locked units only; the stored bytes are the first bytes.
        balances = host.escrow.balances()
        assert (balances.paid, balances.burned, balances.available) == (600_000, 400_000, 0)
        assert host.store.weights(0, at(host.escrow, HOLD_AT + 9), 7) == first
        assert host.store._db.execute("SELECT answer FROM weights_runs").fetchall() == stored
        assert ask(host, 1, HOLD_AT + 1)["weights"] == {}
        assert pays(host.escrow) == 1


# --- WGT-21: the cost of paying must not grow with the history -----------------------------------


def bytes_stored_by_one_payout(li_escrow: ModuleType, origins: int) -> int:
    """Bytes an epoch answer adds to the store when it pays one of `origins` origins."""
    with mode_store(li_escrow) as host:
        host.origin("00" * 32, "hk0", 1_000, 5)
        for index in range(1, origins):
            host.origin(f"{index:02x}" * 32, f"hk{index}", 1_000, 30)  # not due at HOLD_AT
        before = stored_bytes(host.store._db)
        answer = host.answer(0, HOLD_AT)
        assert answer["weights"] == {"hk0": 1_000.0} and len(pay_events(host.escrow)) == 1
        return stored_bytes(host.store._db) - before


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="WGT-21+WGT-D4: the bytes one payout adds to the store do not grow with the number of "
    "origins the escrow holds (every escrow event stores a full snapshot; ADR 0014 part 4.6 "
    "defers the cure to a maintainer decision) [fix-lot: escrow-relay-fixes ADR0014-P4]",
)
def test_the_storage_cost_of_one_payout_does_not_grow_with_the_number_of_origins(
    li_escrow: ModuleType,
) -> None:
    small, large = (
        bytes_stored_by_one_payout(li_escrow, 8),
        bytes_stored_by_one_payout(li_escrow, 32),
    )
    assert 0 < small and large < 2 * small


# --- WGT-18: the legacy journal that still orders every epoch fails closed -----------------------


def test_a_journal_that_answers_one_epoch_twice_is_refused_at_open(tmp_path: Path) -> None:
    from hypertrain.ledger import Ledger, Params
    from hypertrain.ledger.ledger import RebuildMismatch

    params = Params("hypertrain", 1_000, 300, 1, 1)
    ledger = Ledger(tmp_path, params)
    ledger.get_weights(0, 1_300, 5)
    first = ledger.journal.records[-1]
    # Given: a well-chained journal in which epoch 0 is answered a second time.
    ledger.journal.append("weights", {"answer": first["answer"], "payments": first["payments"]})
    # When / Then: opening it again refuses to rebuild rather than pick one of the answers.
    with pytest.raises(RebuildMismatch, match="second answer for an epoch"):
        Ledger(tmp_path, params)
