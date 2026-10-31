"""ChallengeStore.weights pays matured live v2 reward origins from EscrowV2, once per origin."""

from __future__ import annotations

import importlib.util
import json
import sqlite3
import sys
from dataclasses import replace
from pathlib import Path

import pytest

from hypertrain.challenge.admission_store import AdmissionStore
from hypertrain.challenge.store import ChallengeStore
from hypertrain.data.store import LocalFSStore
from hypertrain.ledger import FULL_SHARE_MASS, Params
from hypertrain.ledger.escrow_v2 import Settlement
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages_v2 import EscrowLock, EscrowRelease, EscrowTransfer

spec = importlib.util.spec_from_file_location(
    "v2_weights_pay_escrow", Path(__file__).parents[1] / "ledger/test_escrow_v2.py"
)
assert spec is not None and spec.loader is not None
E = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = E
spec.loader.exec_module(E)
actual_reward = E.actual_reward
spec = importlib.util.spec_from_file_location(
    "v2_weights_pay_bootstrap", Path(__file__).parents[1] / "ledger/test_production_bootstrap_v2.py"
)
assert spec is not None and spec.loader is not None
B = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = B
spec.loader.exec_module(B)


@pytest.fixture
def paying(tmp_path, actual_reward, monkeypatch):
    """A store hosting the real signed live w=0 reward (1e6 units, HOT -> COLD), matured at 20."""
    s, evidence, reward = actual_reward
    store = ChallengeStore(
        tmp_path / "state",
        # v1 genesis deliberately differs from the drand genesis the maturity round uses.
        Params("hypertrain", s.manifest.training.beacon.genesis_time - 600, 300, 1, 1),
        None,
        None,
        lambda payload: None,
        LocalFSStore(tmp_path / "objects"),
    )
    fraud: set[str] = set()
    escrow = E.ledger(
        s,
        store._db,
        replay=E.verified_shadow,
        settlement=lambda ref: Settlement(
            finality_hash=reward.finalize_hash,
            closed_dispute_root=E.H,
            release_beacon=20,
            unresolved=False,
            outcome="FRAUD" if ref in fraud else "MATCH",
        ),
    )
    escrow.mutex = store._lock
    store._db.execute(
        "INSERT INTO runs VALUES(?,?,?,'created',NULL)",
        (s.manifest.run_id(), s.manifest.model_dump_json(), "{}"),
    )
    monkeypatch.setattr(store, "_services", lambda run_id: (escrow, None, None))
    AdmissionStore(escrow)  # the tables the real services create; the tests below fill them
    escrow.reward_finalize(reward, evidence)
    yield store, escrow, reward, evidence, fraud
    store.close()


# A transfer goes to a key the ledger knows (ADR 0014 Part 5.2) and the live reward issues to COLD
# only, so the run of `paying_beside_other` also pins a bootstrap that makes OTHER an owner.
OTHER_BOOTSTRAP = [(E.OTHER.ss58, 10_000, 1)]


@pytest.fixture(scope="module")
def bootstrapped_reward(tmp_path_factory):
    """The real signed live w=0 reward of a run whose policy also pins OTHER_BOOTSTRAP."""
    original = E.setup
    try:
        E.setup = lambda mode="production": B.pinned(original(mode), OTHER_BOOTSTRAP)
        return E.actual_reward.__wrapped__(tmp_path_factory)
    finally:
        E.setup = original


@pytest.fixture
def paying_beside_other(tmp_path, bootstrapped_reward, monkeypatch):
    """`paying` for the run of `bootstrapped_reward`; its bootstrap is not issued yet."""
    yield from paying.__wrapped__(tmp_path, bootstrapped_reward, monkeypatch)


def at(escrow, rnd: int) -> int:
    """Chain time of drand round rnd for the hosted manifest."""
    beacon = escrow.manifest.training.beacon
    return beacon.genesis_time + (rnd - 1) * beacon.period


def mature(escrow, reward) -> None:
    escrow.mature(E.H, tuple(reward.origin_ids), 20)


def mass(answer: dict) -> int:
    return int(sum(answer["weights"].values())) + answer["metadata"]["units_burned_this_epoch"]


def pays(escrow) -> int:
    return escrow.db.execute("SELECT COUNT(*) FROM escrow_events WHERE kind='PAY'").fetchone()[0]


def test_matured_reward_paid_once_when_epoch_asked_twice(paying) -> None:
    # Given: one matured live origin of 1e6 units.
    store, escrow, reward, _, _ = paying
    mature(escrow, reward)
    # When: Cortex asks the same epoch twice, then the next epoch.
    first = store.weights(0, at(escrow, 20), 1)
    second = store.weights(0, at(escrow, 20), 2)
    later = json.loads(store.weights(1, at(escrow, 21), 3))
    # Then: paid as weight to the origin hotkey exactly once; replay returns the same bytes.
    answer = json.loads(first)
    assert answer["weights"] == {E.HOT.ss58: 1_000_000.0}
    assert answer["metadata"]["units_paid"] == 1_000_000 and mass(answer) == FULL_SHARE_MASS
    assert second == first
    assert later["weights"] == {} and mass(later) == FULL_SHARE_MASS
    assert escrow.balances().paid == 1_000_000 and escrow.balances().available == 0
    assert pays(escrow) == 1 and escrow.balances().conserved()


def test_immature_round_not_paid_until_epoch_reaches_mature_at(paying) -> None:
    # Given: the origin is available, mature_at=20.
    store, escrow, reward, _, _ = paying
    mature(escrow, reward)
    # When: an epoch at round 19, then one at round 20.
    early = json.loads(store.weights(0, at(escrow, 19), 1))
    held = escrow.balances().available
    due = json.loads(store.weights(1, at(escrow, 20), 2))
    # Then: nothing before maturity, everything once the epoch's round reaches it.
    assert early["weights"] == {} and early["metadata"]["units_burned_this_epoch"] == 10**6
    assert held == 1_000_000
    assert due["weights"] == {E.HOT.ss58: 1_000_000.0} and pays(escrow) == 1


def test_dispute_locked_and_burned_units_never_paid(paying) -> None:
    # Given: 400k locked for a contest, 300k admission-locked then slashed (burned), 300k free.
    store, escrow, reward, _, fraud = paying
    mature(escrow, reward)
    origin, cold = reward.origin_ids[0], E.COLD.ss58
    contest = EscrowLock(
        operation_id="22" * 32,
        owner=cold,
        units=400_000,
        origin_ids=[origin],
        admission_id=None,
        dispute_id=E.H,
        kind="LOCK_CONTEST",
    )
    escrow.lock(contest, signer=cold)
    admission = EscrowLock(
        operation_id="33" * 32,
        owner=cold,
        units=300_000,
        origin_ids=[origin],
        admission_id=E.H,
        dispute_id=None,
        kind="LOCK_ADMISSION",
    )
    escrow.lock(admission, signer=cold)
    fraud.add(admission.operation_id)
    slash = EscrowRelease(
        operation_id="44" * 32,
        owner=cold,
        units=300_000,
        origin_ids=[origin],
        admission_id=E.H,
        dispute_id=None,
        lock_id=admission.operation_id,
        finality_hash=reward.finalize_hash,
        closed_dispute_root=E.H,
    )
    escrow.slash(slash, authority=E.COORD.ss58, now_beacon=20)
    # When
    answer = json.loads(store.weights(0, at(escrow, 20), 1))
    # Then: only the free 300k is paid; the rest of the epoch mass burns.
    assert answer["weights"] == {E.HOT.ss58: 300_000.0}
    assert answer["metadata"]["units_burned_this_epoch"] == 700_000
    b = escrow.balances()
    assert (b.paid, b.dispute_locked, b.burned, b.available) == (300_000, 400_000, 300_000, 0)


def test_v1_and_v2_share_one_full_share_mass(paying) -> None:
    # Given: a v1 entitlement of 500k for "a" (b's FAULT slice burns) and 1e6 matured v2 units.
    store, escrow, reward, _, _ = paying
    mature(escrow, reward)
    g = store.params.genesis_unix
    for hotkey in ("a", "b"):
        store.ledger.commit(0, hotkey, g + 10)
    store.ledger.verdict(0, "a", "MATCH", 1000, 10**6, g + 20)
    store.ledger.verdict(0, "b", "FAULT", 1000, 10**6, g + 20)
    store.ledger.finalize(0, g + 30)
    # When: two consecutive epochs once v1 vesting has passed.
    first = json.loads(store.weights(3, at(escrow, 20), 1))
    second = json.loads(store.weights(4, at(escrow, 20) + 300, 2))
    # Then: v2 fills only the mass v1 left unpaid, and the remainder pays next epoch.
    assert first["weights"] == {"a": 500_000.0, E.HOT.ss58: 500_000.0}
    assert first["metadata"]["units_burned_this_epoch"] == 0 and mass(first) == FULL_SHARE_MASS
    assert second["weights"] == {E.HOT.ss58: 500_000.0} and mass(second) == FULL_SHARE_MASS
    assert escrow.balances().paid == 1_000_000 and escrow.balances().available == 0


def test_pay_rolls_back_when_answer_not_persisted(paying) -> None:
    # Given: persisting the epoch answer fails (a crash between pay and persistence).
    store, escrow, reward, _, _ = paying
    mature(escrow, reward)
    store._db.execute(
        "CREATE TRIGGER fail_persist BEFORE INSERT ON weights_runs "
        "BEGIN SELECT RAISE(ABORT, 'crash'); END"
    )
    with pytest.raises(sqlite3.IntegrityError):
        store.weights(0, at(escrow, 20), 1)
    # Then: nothing was paid.
    assert escrow.balances().available == 1_000_000 and pays(escrow) == 0
    # When: the retry (the v1 journal already holds this epoch's answer) persists.
    store._db.execute("DROP TRIGGER fail_persist")
    retry = json.loads(store.weights(0, at(escrow, 20), 2))
    # Then: the epoch pays exactly once, on the retry.
    assert retry["weights"] == {E.HOT.ss58: 1_000_000.0}
    assert escrow.balances().paid == 1_000_000 and pays(escrow) == 1


def test_held_units_never_paid(paying_beside_other) -> None:
    # Given: the epoch's round reached mature_at, but the origin has not matured (reward_pending),
    # and OTHER holds the run's bootstrap, unvested: collateral that frees and pays no unit.
    store, escrow, reward, _, _ = paying_beside_other
    escrow.production_bootstrap(B.bootstrap(escrow.run_id, OTHER_BOOTSTRAP))
    pending = json.loads(store.weights(0, at(escrow, 20), 1))
    # And then: it matures, 400k is admission-locked and 500k moves to another owner.
    mature(escrow, reward)
    origin, cold = reward.origin_ids[0], E.COLD.ss58
    lock = E.lock(origin, units=400_000)
    escrow.lock(lock, signer=cold)
    gift = EscrowTransfer(
        operation_id="66" * 32,
        owner=cold,
        units=500_000,
        origin_ids=[origin],
        admission_id=None,
        dispute_id=None,
        recipient=E.OTHER.ss58,
    )
    escrow.transfer(gift, signer=cold)
    # When
    held = json.loads(store.weights(1, at(escrow, 21), 2))
    # Then: only the 100k the earning owner still holds free pays to the origin hotkey.
    assert pending["weights"] == {}
    assert held["weights"] == {E.HOT.ss58: 100_000.0} and mass(held) == FULL_SHARE_MASS
    b = escrow.balances()
    assert (b.paid, b.admission_locked, b.available) == (100_000, 400_000, 500_000)
    assert escrow.balances(E.OTHER.ss58).available == 500_000


def test_shadow_bootstrap_reward_not_paid(paying) -> None:
    # Given: a matured shadow (bootstrap) origin next to the matured live origin, same hotkey.
    store, escrow, reward, evidence, _ = paying
    shadow = E.sign_actual_shadow(reward, E.reserve_actual_shadow(escrow, evidence))
    escrow.reward_finalize(shadow, replace(evidence, shadow=True))
    escrow.mature("55" * 32, tuple(shadow.origin_ids), 20)
    mature(escrow, reward)
    # When
    answer = json.loads(store.weights(0, at(escrow, 20), 1))
    # Then: only the live origin is paid; the shadow units stay available as collateral.
    assert answer["weights"] == {E.HOT.ss58: 1_000_000.0}
    held = dict(
        escrow.db.execute(
            "SELECT origin,bucket FROM escrow_units WHERE units>0 AND origin IN (?,?)",
            (reward.origin_ids[0], shadow.origin_ids[0]),
        ).fetchall()
    )
    assert held == {reward.origin_ids[0]: "paid", shadow.origin_ids[0]: "available"}


def test_legacy_epoch_after_upgrade_gap_never_repaid(paying) -> None:
    # Given: before the upgrade the v1 journal answered epochs 0 and 2, skipping epoch 1.
    store, escrow, reward, _, _ = paying
    mature(escrow, reward)
    served = {e: store.ledger.get_weights(e, at(escrow, 20) + e * 300, 1) for e in (0, 2)}
    # When: the first post-upgrade call asks for the gap epoch, then for the answered ones.
    gap = json.loads(store.weights(1, at(escrow, 20) + 300, 2))
    again = {e: store.weights(e, at(escrow, 20) + e * 300, 3) for e in (2, 0)}
    fresh = json.loads(store.weights(3, at(escrow, 20) + 900, 4))
    # Then: already-served epochs keep their bytes; the out-of-order gap pays nothing;
    # v2 pays on the first epoch past the journal.
    assert again == served
    assert gap["metadata"]["reason"] == "out_of_order" and gap["weights"] == {}
    assert fresh["weights"] == {E.HOT.ss58: 1_000_000.0} and pays(escrow) == 1


def origin(escrow, origin_id: str, hotkey: str, units: int, mature_at: int) -> None:
    """A matured live origin held by its issued owner, as reward_finalize + mature leave it."""
    escrow.db.execute(
        "INSERT INTO escrow_origins VALUES(?,?,?,0,?)", (origin_id, units, mature_at, hotkey)
    )
    for bucket, held in (("reward_pending", 0), ("available", units)):
        escrow.db.execute(
            "INSERT INTO escrow_units VALUES(?,?,?,'',?)", (origin_id, E.COLD.ss58, bucket, held)
        )


def test_oldest_maturity_paid_first_and_same_hotkey_accumulates(paying) -> None:
    # Given: the fixture origin (HOT, matures 20, 1e6) and two older ones whose ids sort after it.
    store, escrow, reward, _, _ = paying
    mature(escrow, reward)
    origin(escrow, "ff" * 32, E.HOT.ss58, 300_000, 10)
    origin(escrow, "ee" * 32, "other", 500_000, 15)
    # When: one epoch with the whole 1e6 mass unpaid by v1.
    answer = json.loads(store.weights(0, at(escrow, 20), 1))
    # Then: 300k (round 10) + 500k (round 15) + 200k of the round-20 origin, HOT summed.
    assert answer["weights"] == {E.HOT.ss58: 500_000.0, "other": 500_000.0}
    assert answer["metadata"]["units_burned_this_epoch"] == 0
    left = escrow.db.execute(
        "SELECT units FROM escrow_units WHERE origin=? AND bucket='available'",
        (reward.origin_ids[0],),
    ).fetchone()[0]
    assert left == 800_000 and pays(escrow) == 3


def test_hotkey_past_entry_cap_keeps_units_for_later(paying, monkeypatch) -> None:
    # Given: a one-entry weight cap and a second hotkey matured between two HOT origins.
    store, escrow, reward, _, _ = paying
    mature(escrow, reward)
    origin(escrow, "ff" * 32, E.HOT.ss58, 300_000, 10)
    origin(escrow, "ee" * 32, "other", 500_000, 15)
    monkeypatch.setattr("hypertrain.ledger.MAX_ENTRIES", 1)
    # When
    answer = json.loads(store.weights(0, at(escrow, 20), 1))
    # Then: the capped hotkey is skipped, HOT already listed still fills the mass.
    assert answer["weights"] == {E.HOT.ss58: 1_000_000.0}
    assert answer["metadata"]["units_burned_this_epoch"] == 0
    kept = escrow.db.execute(
        "SELECT units FROM escrow_units WHERE origin=? AND bucket='available'", ("ee" * 32,)
    ).fetchone()[0]
    assert kept == 500_000 and pays(escrow) == 2


def test_no_v2_pay_before_any_live_reward_finalized(paying) -> None:
    # Given: matured units, but no live reward was ever finalized for the run.
    store, escrow, reward, _, _ = paying
    mature(escrow, reward)
    escrow.db.execute("DELETE FROM escrow_finalized")
    # When
    answer = json.loads(store.weights(0, at(escrow, 20), 1))
    # Then: the epoch is the plain v1 answer.
    assert answer["weights"] == {} and answer["metadata"]["units_burned_this_epoch"] == 10**6
    assert pays(escrow) == 0


def test_out_of_order_epoch_not_completed_with_v2(paying) -> None:
    # Given: epoch 1 already answered, then Cortex asks for the older epoch 0.
    store, escrow, reward, _, _ = paying
    store.weights(1, at(escrow, 19), 1)
    mature(escrow, reward)
    # When
    late = json.loads(store.weights(0, at(escrow, 20), 2))
    # Then: v1 refuses it as out_of_order and v2 does not fill its burned mass.
    assert late["metadata"]["reason"] == "out_of_order" and late["weights"] == {}
    assert pays(escrow) == 0 and escrow.balances().available == 1_000_000


NEW_HOT = Keypair(b"\x0b" * 32)


def admit(escrow, hotkeys: list[Keypair], state: str = "ACTIVE") -> str:
    """One admission of E.COLD that held `hotkeys` in order; the last one is the current hotkey."""
    admission = "ad" * 32
    escrow.db.execute(
        "INSERT INTO admissions_v2(admission_id, run_id, hotkey, coldkey, state) VALUES(?,?,?,?,?)",
        (admission, escrow.run_id, hotkeys[-1].ss58, E.COLD.ss58, state),
    )
    for key in hotkeys:
        escrow.db.execute(
            "INSERT INTO admission_history VALUES(?,?,?)", (key.ss58, admission, E.COLD.ss58)
        )
    return admission


def test_a_reward_follows_its_coldkey_to_the_current_hotkey(paying) -> None:
    # Given: the reward was earned by HOT, then the coldkey moved the admission to NEW_HOT.
    store, escrow, reward, _, _ = paying
    admit(escrow, [E.HOT, NEW_HOT])
    mature(escrow, reward)
    # When
    answer = json.loads(store.weights(0, at(escrow, 20), 1))
    # Then: the mass goes to the hotkey the coldkey has now, none to the retired one, once.
    assert answer["weights"] == {NEW_HOT.ss58: 1_000_000.0}
    assert answer["metadata"]["units_paid"] == 1_000_000 and mass(answer) == FULL_SHARE_MASS
    assert escrow.balances().paid == 1_000_000 and escrow.balances().conserved()
    assert pays(escrow) == 1


def test_a_reward_of_a_hotkey_that_never_rotated_is_paid_to_it(paying) -> None:
    store, escrow, reward, _, _ = paying
    admit(escrow, [E.HOT])
    mature(escrow, reward)
    answer = json.loads(store.weights(0, at(escrow, 20), 1))
    assert answer["weights"] == {E.HOT.ss58: 1_000_000.0}


@pytest.mark.parametrize("rotated", [False, True])
def test_a_banned_coldkey_is_not_paid_whatever_hotkey_it_holds(paying, rotated: bool) -> None:
    # Given: the admission was banned for fraud; its coldkey may or may not have rotated since.
    store, escrow, reward, _, _ = paying
    admit(escrow, [E.HOT, NEW_HOT] if rotated else [E.HOT], state="BANNED")
    mature(escrow, reward)
    # When
    answer = json.loads(store.weights(0, at(escrow, 20), 1))
    # Then: nothing is paid, the epoch mass burns, the units stay where they are.
    assert answer["weights"] == {}
    assert answer["metadata"]["units_paid"] == 0
    assert answer["metadata"]["units_burned_this_epoch"] == 1_000_000
    assert (escrow.balances().paid, escrow.balances().available) == (0, 1_000_000)
    assert pays(escrow) == 0 and escrow.balances().conserved()


def test_a_coldkey_banned_through_another_admission_is_not_paid(paying) -> None:
    # Given: the earning admission is live, but the same coldkey was banned through another one.
    store, escrow, reward, _, _ = paying
    admit(escrow, [E.HOT])
    escrow.db.execute(
        "INSERT INTO admissions_v2(admission_id, run_id, hotkey, coldkey, state) VALUES(?,?,?,?,?)",
        ("ae" * 32, escrow.run_id, NEW_HOT.ss58, E.COLD.ss58, "BANNED"),
    )
    mature(escrow, reward)
    # When / Then: the ban is the coldkey's.
    answer = json.loads(store.weights(0, at(escrow, 20), 1))
    assert answer["weights"] == {} and escrow.balances().paid == 0
