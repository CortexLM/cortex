"""Gap tests of the admission-lock group, second half: a roster member that loses its standing
after its audit (T-LOCK-16, rotation before the aggregation and the finalization leg).

One world is built once per module (five real members, audited, not yet aggregated) and every
test starts from a byte-identical copy of its database, as `support_finality.Stage` does.
"""

from __future__ import annotations

import sqlite3
from collections.abc import Iterator
from dataclasses import dataclass
from typing import Any

import pytest

from . import support_exit_roster as X
from .support_locks import rotation_envelope


@dataclass
class Audited:
    """Round 0 of five members whose audits all matched, frozen before its aggregation."""

    rnd: X.Round
    snapshot: sqlite3.Connection
    now: int
    clock: Any

    def reset(self) -> X.Round:
        """The database, the beacon clock and the service cache back as frozen."""
        store = self.rnd.store
        with store._lock:
            self.snapshot.backup(store._db)
            store._services_v2.clear()
            self.rnd.network.now = self.now
            store.clock = self.clock
            store._services(self.rnd.run_id)
        return self.rnd


@pytest.fixture(scope="module")
def audited(tmp_path_factory: pytest.TempPathFactory) -> Iterator[Audited]:
    with X.activated_network(tmp_path_factory.mktemp("lock16")) as network:
        rnd = X.open_round(network, 0, range(5))
        X.train(rnd, rnd.members)
        response = X.schedule(rnd)
        X.control(response.status_code == 200, response.text)
        X.audit_queue(rnd, len(rnd.members))
        snapshot = sqlite3.connect(":memory:", check_same_thread=False)
        network.store._db.backup(snapshot)
        try:
            yield Audited(rnd, snapshot, network.now, network.store.clock)
        finally:
            snapshot.close()


@pytest.fixture
def fresh(audited: Audited) -> X.Round:
    return audited.reset()


LOST_STANDING = pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="LCK-18+LCK-17+CL-06+CL-28 (T-LOCK-16): a roster member that loses its screen or its "
    "hotkey after its audit is excluded with a recorded cause and the round is aggregated and "
    "finalized for the others, but verified_inputs_v2, which aggregate_v2 and finalize_v2 "
    "share, raises for it (403 'no longer eligible', 409 UNKNOWN_HOTKEY) and every retry fails "
    "[fix-lot: audit-integrity-2 ADR0011-P8 decisions 21-22]",
)
CAUSES = {"none": None, "screen-lapse": LOST_STANDING, "rotation": LOST_STANDING}


def lose_standing(rnd: X.Round, member: int, cause: str) -> bool:
    """Apply `cause` to `member`; whether the member really lost its place in the round."""
    if cause == "screen-lapse":
        with rnd.store._tx():
            rnd.store._db.execute(
                "UPDATE admissions_v2 SET screen_until=1 WHERE hotkey=?", (rnd.hotkey(member),)
            )
        X.control(X.eligible(rnd, member) is False, "its work screen lapsed")
        return True
    if cause == "rotation":
        envelope = rotation_envelope(
            rnd.run_id, X.cold(member), rnd.hotkey(member), X.hot(X.SPARE), "df" * 32
        )
        rotated = rnd.network.client.post(
            f"{rnd.network.url}/admission/{rnd.hotkey(member)}/rotate", json=envelope
        )
        X.control(
            rotated.status_code == 200 or 400 <= rotated.status_code < 500,
            f"the rotation was neither accepted nor refused: {rotated.status_code}",
        )
        return rotated.status_code == 200  # a service that refuses it keeps the member in
    return False


@pytest.mark.parametrize(
    "cause", [pytest.param(name, marks=mark) if mark else name for name, mark in CAUSES.items()]
)
def test_a_member_that_loses_its_standing_after_its_audit_never_freezes_the_round(
    fresh: X.Round, cause: str
) -> None:
    # Given: five audited members, nothing applied, and one member losing its place in the round
    rnd = fresh
    member = rnd.members[2]
    X.control(
        len(rnd.store.verified_inputs_v2(rnd.run_id, 0)) == 5 and not rnd.finality()["applied"],
        "all five members are verified before the cause",
    )
    lost = lose_standing(rnd, member, cause)
    # When: the operator aggregates the round
    aggregated = X.aggregate(rnd)
    # Then: the round is aggregated, the member that lost its place is excluded with a cause
    assert aggregated.status_code == 200, aggregated.text
    assert (rnd.hotkey(member) in rnd.excluded()) is lost
    # and the coordinator's Finalize of exactly the members the master verified settles it
    kept = [i for i in rnd.members if not (lost and i == member)]
    finalized = rnd.network.client.post(
        **X.finalize_request(rnd, kept, str(aggregated.json()["theta_hash"]))
    )
    assert finalized.status_code == 200, finalized.text
    assert rnd.finality()["disposition"] == "SETTLED"
    with rnd.store._lock:
        minted = {
            row[0]
            for row in rnd.store._db.execute(
                "SELECT hotkey FROM escrow_origins WHERE reward_round=0"
            )
        }
    assert minted == {rnd.hotkey(i) for i in kept}


def test_a_finalize_after_the_aggregation_mints_nothing_for_a_member_whose_stake_fell(
    fresh: X.Round,
) -> None:
    # Given: a round aggregated for five funded members, then the stake of one falls a unit short
    rnd = fresh
    member = rnd.members[2]
    aggregated = X.aggregate(rnd)
    X.control(aggregated.status_code == 200, aggregated.text)
    escrow, cold = rnd.store._services(rnd.run_id)[0], X.cold(member).ss58
    origin = rnd.network.origin_ids[member]
    with rnd.store._tx():
        for bucket, delta in (("admission_locked", -1), ("available", 1)):
            moved = escrow.db.execute(
                "UPDATE escrow_units SET units=units+? WHERE origin=? AND owner=? AND bucket=?",
                (delta, origin, cold, bucket),
            )
            X.control(moved.rowcount == 1, f"no {bucket} row to edit")
    X.control(X.eligible(rnd, member) is False, "a member one unit short is still eligible")
    X.control(escrow.balances().conserved(), "the ledger edit broke conservation")
    world = X.world_of(rnd.network)
    before, owned = world.db_digest(), escrow.balances(cold)
    # When: the coordinator's Finalize of the five members is posted
    refused = rnd.network.client.post(
        **X.finalize_request(rnd, rnd.members, str(aggregated.json()["theta_hash"]))
    )
    # Then: it is refused (for the member's funding today; a service that excludes the member
    # refuses a Finalize that still names it), nothing is written, and the ledger holds no origin
    # of round 0 for that member
    assert refused.status_code in (403, 409, 422), refused.text
    assert world.db_digest() == before
    assert world.count("escrow_origins", "hotkey=? AND reward_round=0", (rnd.hotkey(member),)) == 0
    assert escrow.balances(cold) == owned
