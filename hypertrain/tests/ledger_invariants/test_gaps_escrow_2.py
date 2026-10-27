"""Gaps of the escrow group, ledger half: slash, release, replay, rollback and stake attribution.

What the first wave left open in T-ESC-12, 18, 22, 23, 24 and 26. Each test states a defence the
ledger must hold ("refuses X and every unit row and event is unchanged", "answers a replay from
its own events"), and its control is the same operation made valid.
"""

from __future__ import annotations

import sqlite3
from pathlib import Path
from types import ModuleType
from typing import Any

import pytest

from hypertrain.ledger.escrow_v2 import EscrowError, Settlement
from hypertrain.protocol.envelope_v2 import seal
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages_v2 import EscrowRelease

from .support import (
    Real,
    bare_escrow,
    error_code,
    match_settlement,
    registered_settlements,
)
from .support_escrow_laws import (
    COLD_UNITS,
    OTHER_UNITS,
    fingerprint,
    funded,
    lock_op,
    release_op,
    settled,
    transfer_op,
)
from .support_gaps_escrow import (
    CONTEST_UNITS,
    abort_snapshot_insert,
    admission_snapshot,
    code,
    contested_bond,
    ledger_state,
)
from .support_locks import STAKE, Lab
from .support_opids import DefenceMissing, fresh_id

UNFUNDED = "UNFUNDED_OR_UNKNOWN_ORIGIN"

# --- T-ESC-12: a slash refuses an empty or self referee instead of refunding the owner ---------


@pytest.mark.parametrize(
    "referee", ["", "owner"], ids=["an_empty_referee", "the_slashed_owner_as_referee"]
)
@pytest.mark.xfail(
    strict=True,
    raises=DefenceMissing,
    reason="ESC-15+ESC-D10: a slash that names an empty referee, or the slashed owner, is refused "
    "and moves nothing, but both pass the referee funding check and the referee cost flows back "
    "to the owner",
)
def test_a_slash_naming_an_empty_or_self_referee_is_refused_and_changes_nothing(
    li_escrow: ModuleType, referee: str
) -> None:
    # Given: a first FRAUD bond slashed with a funded, distinct referee (the control), and a
    # second bond of the same owner.
    fx, _, request = contested_bond(li_escrow, settled("FRAUD"))
    escrow, cold, other = fx.escrow, li_escrow.COLD.ss58, li_escrow.OTHER.ss58
    authority = li_escrow.COORD.ss58

    def slash(request: EscrowRelease, named: str) -> None:
        escrow.slash(
            request,
            authority=authority,
            now_beacon=10,
            referee=named,
            referee_cost=100,
            max_referee_cost=100,
        )

    slash(request, other)
    assert escrow.balances(other).available == OTHER_UNITS + 100
    second = lock_op(
        li_escrow, fx.cold, operation="25" * 32, units=CONTEST_UNITS, kind="LOCK_CONTEST"
    )
    escrow.lock(second, signer=cold)
    again = release_op(second, operation="38" * 32)
    before, burned = fingerprint(escrow), escrow.balances().burned

    # When: the second bond is slashed naming the owner itself, or nobody, as the referee.
    try:
        slash(again, cold if referee == "owner" else referee)
    except EscrowError:
        pass
    else:
        paid_back = escrow.balances(cold).available - (COLD_UNITS - 2 * CONTEST_UNITS)
        raise DefenceMissing(f"the slash was accepted and {paid_back} units went to the owner")

    # Then: it is refused (any stable code), nothing moved, and the bond is still slashable
    # with a real referee.
    assert fingerprint(escrow) == before
    assert escrow.balances(cold).dispute_locked == CONTEST_UNITS
    assert escrow.balances().burned == burned
    slash(again, other)
    assert escrow.balances().burned == burned + CONTEST_UNITS - 100
    assert escrow.balances().conserved()


# --- T-ESC-18: a release names exactly the origins of its lock ----------------------------------


@pytest.mark.parametrize("names", ["another_owners_origin", "own_origin_and_another_owners"])
def test_a_release_naming_an_origin_the_lock_does_not_hold_is_refused(
    li_escrow: ModuleType, names: str
) -> None:
    # Given: COLD's stake in one origin, and a release of it that also (or only) names the
    # origin of another owner.
    fx = funded(li_escrow)
    escrow, cold = fx.escrow, li_escrow.COLD.ss58
    stake = lock_op(li_escrow, fx.cold)
    escrow.lock(stake, signer=cold)
    origins = [fx.other] if names == "another_owners_origin" else sorted([fx.cold, fx.other])
    request = EscrowRelease.model_validate(
        {**release_op(stake).model_dump(), "origin_ids": origins}
    )
    before = fingerprint(escrow)
    # When / Then: refused, the stake is untouched, and the other owner's units never moved.
    assert error_code(lambda: escrow.release(request, signer=cold, now_beacon=10)) == UNFUNDED
    assert fingerprint(escrow) == before
    assert escrow.balances(cold).admission_locked == stake.units
    assert escrow.balances(li_escrow.OTHER.ss58).available == OTHER_UNITS
    # Control: the release naming exactly the lock's origins returns the stake.
    escrow.release(release_op(stake), signer=cold, now_beacon=10)
    assert escrow.balances(cold).admission_locked == 0
    assert escrow.balances().conserved()


# --- T-ESC-22: a live reward matures only against the finalized round on record ------------------


def test_a_reward_whose_finalized_round_is_not_on_record_does_not_mature(
    li_escrow: ModuleType, real_reward: Real
) -> None:
    # Given: the real round-0 reward, issued, with a settlement that allows its maturity; then
    # the stored finalize row of round 0 is lost (tampering: a reopen refuses such a ledger).
    setup, evidence, reward = real_reward
    origin = reward.origin_ids[0]
    escrow = bare_escrow(
        li_escrow, setup, settlement={origin: match_settlement(reward.finalize_hash, 20)}
    )
    escrow.reward_finalize(reward, evidence)
    escrow.db.execute("DELETE FROM escrow_finalized WHERE w=?", (reward.w,))
    before = fingerprint(escrow)
    # When / Then: no maturity without the finalize it is bound to; the units stay pending.
    assert error_code(lambda: escrow.mature("55" * 32, (origin,), 20)) == "DISPUTED_OR_UNVESTED"
    assert fingerprint(escrow) == before
    assert escrow.balances().reward_pending == reward.budget_units
    # Control: with the row back, the same maturity vests the whole budget once.
    escrow.db.execute("INSERT INTO escrow_finalized VALUES(?,?)", (reward.w, reward.finalize_hash))
    escrow.mature("55" * 32, (origin,), 20)
    assert (escrow.balances().available, escrow.balances().reward_pending) == (
        reward.budget_units,
        0,
    )
    assert escrow.balances().conserved()


# --- T-ESC-23: a replay returns the stored receipt without asking the settlement ----------------


def test_a_replayed_release_or_slash_is_answered_from_the_ledger_not_from_the_settlement(
    li_escrow: ModuleType,
) -> None:
    # Given: a ledger whose settlement the test controls and counts: an accepted MATCH of the
    # stake lock, then an accepted FRAUD of the contest bond; both applied once.
    asked: list[str] = []
    answer: dict[str, Any] = {"now": settled("MATCH")}

    def settlement(reference: str) -> Settlement:
        asked.append(reference)
        if isinstance(answer["now"], Exception):
            raise answer["now"]
        return answer["now"]  # type: ignore[no-any-return]

    fx = funded(li_escrow, settlement=settlement)
    escrow, cold, authority = fx.escrow, li_escrow.COLD.ss58, li_escrow.COORD.ss58
    stake = lock_op(li_escrow, fx.cold, operation="22" * 32)
    bond = lock_op(li_escrow, fx.cold, operation="24" * 32, kind="LOCK_CONTEST", units=500)
    escrow.lock(stake, signer=cold)
    escrow.lock(bond, signer=cold)
    release, slash = release_op(stake), release_op(bond, operation="45" * 32)
    released = escrow.release(release, signer=cold, now_beacon=10)
    answer["now"] = settled("FRAUD")
    slashed = escrow.slash(slash, authority=authority, now_beacon=10)
    assert len(asked) == 2

    # When: the settlement stops answering (unresolved, then unaccepted) and both requests come
    # again, identically.
    after, state = fingerprint(escrow), ledger_state(escrow)
    for later in (settled("MATCH", unresolved=True), EscrowError("UNACCEPTED_SETTLEMENT")):
        answer["now"] = later
        asked.clear()
        # Then: each answers its first receipt, asks nothing and moves nothing.
        assert escrow.release(release, signer=cold, now_beacon=10) == released
        assert escrow.slash(slash, authority=authority, now_beacon=10) == slashed
        assert asked == []
        assert fingerprint(escrow) == after and ledger_state(escrow) == state
    assert escrow.balances().conserved()


@pytest.mark.parametrize(
    "change",
    [
        {"referee": "owner"},
        {"referee_cost": 50},
        {"max_referee_cost": 200},
    ],
    ids=["another_referee", "another_referee_cost", "another_cost_cap"],
)
def test_a_slash_operation_id_names_one_request_and_one_burn_even_after_a_restart(
    li_escrow: ModuleType, tmp_path: Path, change: dict[str, object]
) -> None:
    # Given: a bond slashed with a referee share of 100, on a ledger file.
    setup, path = li_escrow.setup(), tmp_path / "ledger.db"
    escrow = li_escrow.ledger(setup, path, settlement=lambda _reference: settled("FRAUD"))
    origin, _ = li_escrow.fund(escrow)
    cold, other = li_escrow.COLD.ss58, li_escrow.OTHER.ss58
    bond = lock_op(li_escrow, origin, units=CONTEST_UNITS, kind="LOCK_CONTEST", operation="22" * 32)
    escrow.lock(bond, signer=cold)
    request = release_op(bond, operation="33" * 32)
    terms: dict[str, Any] = {"referee": other, "referee_cost": 100, "max_referee_cost": 100}
    authority = li_escrow.COORD.ss58
    receipt = escrow.slash(request, authority=authority, now_beacon=10, **terms)
    before = ledger_state(escrow)

    # When / Then: the same id with another referee, cost or cap is a conflict, nothing moves.
    other_terms = {**terms, **{k: cold if v == "owner" else v for k, v in change.items()}}
    assert (
        error_code(lambda: escrow.slash(request, authority=authority, now_beacon=10, **other_terms))
        == "OPERATION_CONFLICT"
    )
    assert ledger_state(escrow) == before
    # and the same terms replay the first receipt, on this engine and on a reopened one.
    assert escrow.slash(request, authority=authority, now_beacon=10, **terms) == receipt
    reopened = li_escrow.ledger(setup, path, settlement=lambda _reference: settled("FRAUD"))
    assert reopened.slash(request, authority=authority, now_beacon=10, **terms) == receipt
    assert ledger_state(reopened) == before
    assert reopened.balances().burned == CONTEST_UNITS - 100
    assert reopened.balances(other).available == OTHER_UNITS + 100


def test_a_transfer_operation_id_names_one_recipient(li_escrow: ModuleType) -> None:
    # Given: a transfer to OTHER under an id.
    fx = funded(li_escrow)
    escrow, cold = fx.escrow, li_escrow.COLD.ss58
    sent = transfer_op(li_escrow, fx.cold)
    receipt = escrow.transfer(sent, signer=cold)
    before = fingerprint(escrow)
    # When / Then: the same id and amount to another recipient is a conflict and moves nothing;
    # the original body replays its receipt.
    elsewhere = transfer_op(li_escrow, fx.cold, recipient=li_escrow.COORD.ss58)
    assert error_code(lambda: escrow.transfer(elsewhere, signer=cold)) == "OPERATION_CONFLICT"
    assert fingerprint(escrow) == before
    assert escrow.transfer(sent, signer=cold) == receipt
    assert escrow.balances(li_escrow.OTHER.ss58).available == OTHER_UNITS + sent.units
    assert escrow.balances(li_escrow.COORD.ss58).available == 0


# --- T-ESC-24: a failure after the movement rolls back a release and a two-move slash -----------


def test_a_release_and_a_slash_with_a_referee_cost_that_fail_late_change_nothing(
    li_escrow: ModuleType, tmp_path: Path
) -> None:
    # Given: an admission stake settled MATCH and a contest bond settled FRAUD on a ledger file.
    setup, path = li_escrow.setup(), tmp_path / "ledger.db"
    stake_id, bond_id = fresh_id("stake"), fresh_id("bond")
    table = {stake_id: settled("MATCH"), bond_id: settled("FRAUD")}
    escrow = li_escrow.ledger(setup, path, settlement=registered_settlements(table))
    origin, _ = li_escrow.fund(escrow)
    cold, other, authority = li_escrow.COLD.ss58, li_escrow.OTHER.ss58, li_escrow.COORD.ss58
    stake = lock_op(li_escrow, origin, operation=stake_id)
    bond = lock_op(li_escrow, origin, operation=bond_id, kind="LOCK_CONTEST")
    escrow.lock(stake, signer=cold)
    escrow.lock(bond, signer=cold)
    release = release_op(stake, operation=fresh_id("release"))
    slash = release_op(bond, operation=fresh_id("slash"))
    terms: dict[str, Any] = {"referee": other, "referee_cost": 100, "max_referee_cost": 100}
    before = ledger_state(escrow)

    # When: the storage fails at the end of every operation.
    abort_snapshot_insert(escrow)
    with pytest.raises(sqlite3.IntegrityError, match="storage failure"):
        escrow.release(release, signer=cold, now_beacon=10)
    with pytest.raises(sqlite3.IntegrityError, match="storage failure"):
        escrow.slash(slash, authority=authority, now_beacon=10, **terms)

    # Then: no unit moved, in particular no referee share and no burn from the first leg of the
    # slash, no event was written, and the next operation starts clean.
    assert ledger_state(escrow) == before
    assert not escrow.db.in_transaction
    # And once the storage works, each applies exactly once, replays, and survives a reopen.
    escrow.db.execute("DROP TRIGGER li_storage_failure")
    released = escrow.release(release, signer=cold, now_beacon=10)
    slashed = escrow.slash(slash, authority=authority, now_beacon=10, **terms)
    assert escrow.release(release, signer=cold, now_beacon=10) == released
    assert escrow.slash(slash, authority=authority, now_beacon=10, **terms) == slashed
    reopened = li_escrow.ledger(setup, path, settlement=registered_settlements(table))
    assert (reopened.balances().admission_locked, reopened.balances().dispute_locked) == (0, 0)
    assert reopened.balances().burned == bond.units - 100
    assert reopened.balances(other).available == OTHER_UNITS + 100
    assert len(ledger_state(reopened)[1]) == len(before[1]) + 2


# --- T-ESC-26: a stake belongs to exactly one admission and one coldkey --------------------------


def test_the_admission_service_takes_no_contest_lock_and_writes_nothing_for_it(
    tmp_path: Path,
) -> None:
    # Given: a registered admission and a contest bond its coldkey signed (the lock the dispute
    # route takes, not the admission route).
    lab = Lab(tmp_path)
    lab.join()
    bond = lab.lock_op(lab.A.H, 100, "22" * 32).model_copy(
        update={"kind": "LOCK_CONTEST", "admission_id": None, "dispute_id": lab.A.H}
    )
    raw = canonicalize(seal(lab.A.COLD, "EscrowLock", lab.a.escrow.run_id, bond, 500))
    before = (ledger_state(lab.a.escrow), lab.a.escrow.balances())
    # When / Then: the admission route refuses it for want of an admission, and no lock, event or
    # outbox row exists afterwards.
    assert code(lambda: lab.a.lock(raw, now=1)) in ("UNKNOWN_ADMISSION", "LOCK_OWNER_OR_KIND")
    assert (ledger_state(lab.a.escrow), lab.a.escrow.balances()) == before
    assert lab.a.escrow.balances().dispute_locked == 0
    assert lab.a.store.db.execute("SELECT COUNT(*) FROM admission_outbox").fetchone()[0] == 0


def test_the_stake_of_one_admission_of_a_coldkey_never_funds_another_one_of_the_same_coldkey(
    tmp_path: Path,
) -> None:
    # Given: one coldkey with two ACTIVE admissions; only the first is staked, twice the required.
    lab = Lab(tmp_path)
    first = lab.join()
    lab.live(first)
    second = lab.join(hot=lab.A.OTHER, cold=lab.A.COLD, request="33" * 32)
    lab.live(second)
    lab.stake(first, 2 * STAKE, "22" * 32)

    def standing(key: Keypair) -> tuple[int, bool]:
        status = lab.a.status(key.ss58, now=5)
        return status.funding.locked_units, status.eligible

    # Then: the stake counts for the admission it names and for no other of that coldkey.
    assert standing(lab.A.HOT) == (2 * STAKE, True)
    assert standing(lab.A.OTHER) == (0, False)

    # When: the coldkey puts every other unit it owns behind the first admission, then asks to
    # stake the second one from units that are already spoken for.
    lab.stake(first, COLD_UNITS - 2 * STAKE, "23" * 32)
    before = (ledger_state(lab.a.escrow), admission_snapshot(lab))

    def stake_second() -> None:
        with lab.a.escrow.tx():  # as the service does for every request
            lab.stake(second, STAKE, "24" * 32)

    # Then: refused, nothing written, and the standing of both admissions is as before.
    assert error_code(stake_second) == UNFUNDED
    assert (ledger_state(lab.a.escrow), admission_snapshot(lab)) == before
    assert standing(lab.A.HOT) == (COLD_UNITS, True)
    assert standing(lab.A.OTHER) == (0, False)
    assert lab.a.escrow.balances().conserved()
