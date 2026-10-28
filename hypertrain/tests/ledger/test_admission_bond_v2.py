"""The ledger settles an admission lock through its own resolver, chosen by the lock kind."""

from __future__ import annotations

import hmac
import importlib.util
import sys
from pathlib import Path

import pytest

from hypertrain.ledger.escrow_v2 import SERVER_SCOPE, EscrowError, Settlement
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.messages_v2 import EscrowLock, EscrowRelease

_spec = importlib.util.spec_from_file_location(
    "bond_escrow_fixture", Path(__file__).parent / "test_escrow_v2.py"
)
assert _spec is not None and _spec.loader is not None
f = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = f
_spec.loader.exec_module(f)


def verdict(outcome: str, *, unresolved: bool = False, beacon: int = 10) -> Settlement:
    return Settlement(
        finality_hash=f.H,
        closed_dispute_root=f.H,
        release_beacon=beacon,
        unresolved=unresolved,
        outcome=outcome,  # type: ignore[arg-type]
    )


def release_of(lock: EscrowLock, origin: str, operation: str = "33" * 32, units: int = 1000):
    return EscrowRelease(
        operation_id=operation,
        owner=f.COLD.ss58,
        units=units,
        origin_ids=[origin],
        admission_id=lock.admission_id,
        dispute_id=lock.dispute_id,
        lock_id=lock.operation_id,
        finality_hash=f.H,
        closed_dispute_root=f.H,
    )


def test_admission_lock_settles_only_through_its_own_resolver(tmp_path: Path) -> None:
    # Given: a settlement callback that says MATCH for every id, as one that resolves a lock id
    # borrowed from a dispute or a reward origin would, and an admission resolver that holds.
    e = f.ledger(f.setup(), tmp_path / "ledger.db")
    origin, _ = f.fund(e)
    bond = f.lock(origin)
    e.lock(bond, signer=f.COLD.ss58)
    e.admission_settlement = lambda _: verdict("MATCH", unresolved=True)
    # When / Then: the bond is not released on the strength of the generic callback
    with pytest.raises(EscrowError, match="DISPUTED_OR_UNVESTED"):
        e.release(release_of(bond, origin), signer=f.COLD.ss58, now_beacon=10)
    assert e.balances().admission_locked == 1000
    # and it is released once its own resolver says so
    e.admission_settlement = lambda _: verdict("MATCH")
    e.release(release_of(bond, origin), signer=f.COLD.ss58, now_beacon=10)
    assert e.balances().admission_locked == 0 and e.balances().conserved()


def test_admission_lock_is_slashed_on_its_own_resolver_not_the_generic_one(tmp_path: Path) -> None:
    def generic(reference: str) -> Settlement:
        raise AssertionError("the generic callback must not be asked about an admission lock")

    e = f.ledger(f.setup(), tmp_path / "ledger.db", settlement=generic)
    origin, _ = f.fund(e)
    bond = f.lock(origin)
    e.lock(bond, signer=f.COLD.ss58)
    e.admission_settlement = lambda _: verdict("FRAUD")
    e.slash(release_of(bond, origin), authority=f.COORD.ss58, now_beacon=10)
    assert e.balances().burned == 1000 and e.balances().admission_locked == 0


def test_contest_lock_is_settled_by_the_dispute_that_took_it(tmp_path: Path) -> None:
    # Given: a generic callback and an admission resolver that must not be asked about a contest
    # lock, and the contest resolver the disputes service installs.
    def generic(reference: str) -> Settlement:
        raise AssertionError("the generic callback must not be asked about a lock")

    e = f.ledger(f.setup(), tmp_path / "ledger.db", settlement=generic)
    origin, _ = f.fund(e)
    e.admission_settlement = lambda _: verdict("FRAUD", unresolved=True)
    contest = EscrowLock(
        operation_id="44" * 32,
        owner=f.COLD.ss58,
        units=1000,
        origin_ids=[origin],
        admission_id=None,
        dispute_id=f.H,
        kind="LOCK_CONTEST",
    )
    e.lock(contest, signer=f.COLD.ss58)
    seen: list[str] = []

    def contest_settlement(lock: EscrowLock) -> Settlement:
        seen.append(lock.operation_id)
        return verdict("MATCH").model_copy(update={"dispute_id": lock.dispute_id})

    e.contest_settlement = contest_settlement
    # When / Then: the contest resolver settles it, by the lock itself
    e.release(release_of(contest, origin), signer=f.COLD.ss58, now_beacon=10)
    assert seen == [contest.operation_id] and e.balances().dispute_locked == 0


def unvested_reward(e, origin: str = "aa" * 32, units: int = 500, owner: str = f.COLD.ss58) -> None:
    """A reward of `owner` (COLD) that has not vested: reward_pending units of an issued origin."""
    e.db.execute("INSERT INTO escrow_origins VALUES(?,?,?,?,?)", (origin, units, 99, 0, f.HOT.ss58))
    e.db.execute(
        "INSERT INTO escrow_units VALUES(?,?,?,?,?)", (origin, owner, "reward_pending", "", units)
    )


@pytest.mark.parametrize("burn_pending", [True, False])
def test_admission_slash_burns_unvested_rewards_unless_told_otherwise(
    tmp_path: Path, burn_pending: bool
) -> None:
    e = f.ledger(f.setup(), tmp_path / "ledger.db")
    origin, _ = f.fund(e)
    bond = f.lock(origin)
    e.lock(bond, signer=f.COLD.ss58)
    with e.tx():
        unvested_reward(e)
    e.admission_settlement = lambda _: verdict("FRAUD")
    op = release_of(bond, origin)
    first = e.slash(op, authority=f.COORD.ss58, now_beacon=10, burn_pending=burn_pending)
    # the bond always burns; the coldkey's unvested rewards only when the sanction says so
    assert e.balances().admission_locked == 0
    assert e.balances().reward_pending == (0 if burn_pending else 500)
    assert e.balances().burned == (1500 if burn_pending else 1000)
    assert e.balances().conserved()
    # a retry is the same receipt and burns nothing more
    assert e.slash(op, authority=f.COORD.ss58, now_beacon=10, burn_pending=burn_pending) == first
    assert e.balances().burned == (1500 if burn_pending else 1000)
    e.verify()


def held(e, origin: str, owner: str) -> dict[str, int]:
    """The units `owner` holds of one origin, bucket by bucket (an emptied row shows as 0)."""
    return {
        row["bucket"]: row["units"]
        for row in e.db.execute(
            "SELECT bucket,units FROM escrow_units WHERE origin=? AND owner=?", (origin, owner)
        )
    }


def burn_events(e) -> int:
    query = "SELECT COUNT(*) FROM escrow_events WHERE kind='BURN_PENDING'"
    return e.db.execute(query).fetchone()[0]


def test_burn_pending_burns_the_named_origin_of_its_owner_and_nothing_else(tmp_path: Path) -> None:
    # Given: COLD holds two unvested origins, OTHER one; the genesis origins have vested
    e = f.ledger(f.setup(), tmp_path / "ledger.db")
    vested, _ = f.fund(e)
    named, kept, others = "aa" * 32, "bb" * 32, "cc" * 32
    with e.tx():
        unvested_reward(e, named, 500)
        unvested_reward(e, kept, 300)
        unvested_reward(e, others, 400, owner=f.OTHER.ss58)
    genesis = held(e, vested, f.COLD.ss58)
    assert e.pending_origins(f.COLD.ss58) == [named, kept]
    # When: the reward authority burns one origin of COLD
    receipt = e.burn_pending(named, owner=f.COLD.ss58, authority=f.COORD.ss58)
    # Then: that origin's pending units are burned, the row stays (it marks a live reward origin),
    # and no other origin, owner or vested unit moved
    assert held(e, named, f.COLD.ss58) == {"reward_pending": 0, "burned": 500}
    assert held(e, kept, f.COLD.ss58) == {"reward_pending": 300}
    assert held(e, others, f.OTHER.ss58) == {"reward_pending": 400}
    assert held(e, vested, f.COLD.ss58) == genesis
    assert e.pending_origins(f.COLD.ss58) == [kept]
    assert e.balances(f.COLD.ss58).burned == 500 and e.balances().conserved()
    e.verify()
    # The event is a server event: srv: + HMAC(id key, sha256 of the origin's own preimage)
    raw = sha256_hex(f"ht-burn-pending/1|{e.run_id}|{named}".encode())
    (key,) = e.db.execute("SELECT operation_id FROM escrow_events WHERE kind='BURN_PENDING'")
    assert key[0] == SERVER_SCOPE + hmac.digest(f.ID_KEY, raw.encode(), "sha256").hex()
    assert receipt.operation_id == raw
    # and a replay is the same receipt, burning nothing more
    assert e.burn_pending(named, owner=f.COLD.ss58, authority=f.COORD.ss58) == receipt
    assert burn_events(e) == 1 and e.balances(f.COLD.ss58).burned == 500


def test_burn_pending_burns_nothing_that_is_not_the_named_owners_pending_units(
    tmp_path: Path,
) -> None:
    # Given: COLD's genesis origin has vested, and OTHER holds a pending origin
    e = f.ledger(f.setup(), tmp_path / "ledger.db")
    vested, _ = f.fund(e)
    others = "cc" * 32
    with e.tx():
        unvested_reward(e, others, 400, owner=f.OTHER.ss58)
    before = (held(e, vested, f.COLD.ss58), held(e, others, f.OTHER.ss58))
    assert before[0]["available"] == 20000
    # When / Then: a vested origin, an unknown origin and another owner's origin burn nothing and
    # write no event
    for origin in (vested, "dd" * 32, others):
        assert e.burn_pending(origin, owner=f.COLD.ss58, authority=f.COORD.ss58) is None
    assert (held(e, vested, f.COLD.ss58), held(e, others, f.OTHER.ss58)) == before
    assert e.balances().burned == 0 and burn_events(e) == 0


@pytest.mark.parametrize("fault", ["authority", "id_key"])
def test_burn_pending_needs_the_reward_authority_and_the_id_key(tmp_path: Path, fault: str) -> None:
    e = f.ledger(f.setup(), tmp_path / "ledger.db")
    f.fund(e)
    with e.tx():
        unvested_reward(e)
    if fault == "id_key":
        e.id_key = None
    code = "BURN_AUTHORITY" if fault == "authority" else "ID_KEY_REQUIRED"
    signer = f.COLD.ss58 if fault == "authority" else f.COORD.ss58
    with pytest.raises(EscrowError, match=code):
        e.burn_pending("aa" * 32, owner=f.COLD.ss58, authority=signer)
    assert held(e, "aa" * 32, f.COLD.ss58) == {"reward_pending": 500}
    assert e.balances().burned == 0 and burn_events(e) == 0


def test_a_burn_of_an_origin_for_another_owner_is_a_conflict_not_a_replay(tmp_path: Path) -> None:
    e = f.ledger(f.setup(), tmp_path / "ledger.db")
    f.fund(e)
    with e.tx():
        unvested_reward(e)
    e.burn_pending("aa" * 32, owner=f.COLD.ss58, authority=f.COORD.ss58)
    with pytest.raises(EscrowError, match="OPERATION_CONFLICT"):
        e.burn_pending("aa" * 32, owner=f.OTHER.ss58, authority=f.COORD.ss58)
    assert e.balances().burned == 500 and burn_events(e) == 1
