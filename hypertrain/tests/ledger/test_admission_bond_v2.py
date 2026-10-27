"""The ledger settles an admission lock through its own resolver, chosen by the lock kind."""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

import pytest

from hypertrain.ledger.escrow_v2 import EscrowError, Settlement
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


def test_contest_lock_keeps_the_generic_resolver(tmp_path: Path) -> None:
    # Given: both resolvers installed, the admission one refusing everything
    seen = []

    def generic(reference: str) -> Settlement:
        seen.append(reference)
        return verdict("MATCH")

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
    # When / Then: a contest lock is settled by the generic callback, as before
    e.release(release_of(contest, origin), signer=f.COLD.ss58, now_beacon=10)
    assert seen == [contest.operation_id] and e.balances().dispute_locked == 0


def unvested_reward(e, origin: str = "aa" * 32, units: int = 500) -> None:
    """A reward of COLD that has not vested: units in reward_pending of an issued origin."""
    e.db.execute("INSERT INTO escrow_origins VALUES(?,?,?,?,?)", (origin, units, 99, 0, f.HOT.ss58))
    e.db.execute(
        "INSERT INTO escrow_units VALUES(?,?,?,?,?)",
        (origin, f.COLD.ss58, "reward_pending", "", units),
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
