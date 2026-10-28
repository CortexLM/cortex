"""Escrow laws of ledger/escrow_v2.py stated as properties of a bare ledger.

Each test names a defence the ledger must hold: a refusal leaves every unit row and every event
exactly as they were, an accepted operation moves exactly the units it names, and the control of
each refusal is the same operation made valid. Findings: ESC-01 .. ESC-24, LCK-05/10/11/24,
DIA-01/10 and the C2 ids ES-01/02/03/07 (the HTTP half is in test_escrow_laws_2.py).
"""

from __future__ import annotations

import shutil
import sqlite3
from collections.abc import Callable
from pathlib import Path
from types import ModuleType

import pytest
from pydantic import ValidationError

from hypertrain.ledger.escrow_v2 import Balances, Settlement, authority_message
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages_v2 import (
    EscrowLock,
    EscrowOperation,
    EscrowRelease,
    EscrowTransfer,
)

from .support import H, bare_escrow, error_code
from .support_escrow_laws import (
    COLD_UNITS,
    OTHER_UNITS,
    Funded,
    events,
    fingerprint,
    funded,
    lock_floor,
    lock_op,
    pay_op,
    release_op,
    settled,
    signed_genesis,
    transfer_op,
)

UNFUNDED = "UNFUNDED_OR_UNKNOWN_ORIGIN"


# ---------------------------------------------------------------- owner signature (ESC-02)


@pytest.mark.parametrize("operation", ["lock", "transfer", "release", "pay"])
def test_units_move_only_under_the_signature_of_their_owner(
    li_escrow: ModuleType, operation: str
) -> None:
    """ESC-02, LCK-05, ES-01: a key that is not the owner moves nothing, whatever it names."""
    # Given: COLD holds a 1000-unit stake and plenty of free units; OTHER is a foreign key.
    fx = funded(li_escrow)
    escrow, cold, foreign = fx.escrow, li_escrow.COLD.ss58, li_escrow.OTHER.ss58
    stake = lock_op(li_escrow, fx.cold)
    escrow.lock(stake, signer=cold)
    call: Callable[[str], object] = {
        "lock": lambda who: escrow.lock(
            lock_op(li_escrow, fx.cold, operation="23" * 32, units=500), signer=who
        ),
        "transfer": lambda who: escrow.transfer(
            transfer_op(li_escrow, fx.cold, recipient=foreign), signer=who
        ),
        "release": lambda who: escrow.release(release_op(stake), signer=who, now_beacon=10),
        "pay": lambda who: escrow.pay(pay_op(li_escrow, fx.cold), signer=who),
    }[operation]
    before = fingerprint(escrow)
    # When: the foreign key signs an operation that names COLD as owner.
    # Then: it is refused with the ownership code, even though COLD could afford it.
    assert error_code(lambda: call(foreign)) == "OWNER_AUTHORITY"
    assert fingerprint(escrow) == before
    # Control: the owner's own signature performs the very same operation.
    call(cold)
    assert fingerprint(escrow) != before
    owner, bucket, expected = {
        "lock": (cold, "admission_locked", 1500),
        "transfer": (foreign, "available", OTHER_UNITS + 500),
        "release": (cold, "admission_locked", 0),
        "pay": (cold, "paid", 500),
    }[operation]
    assert getattr(escrow.balances(owner), bucket) == expected
    assert escrow.balances().conserved()
    # A foreign replay of the recorded operation gets no receipt either.
    assert error_code(lambda: call(foreign)) == "OWNER_AUTHORITY"


# ------------------------------------------------------- lock funding (ESC-01, LCK-05, ES-02)

LOCK_FAULTS = [
    "asks_for_more_than_it_holds",
    "names_another_owners_origin",
    "names_an_origin_never_issued",
    "lists_an_origin_it_holds_nothing_of",
    "a_foreign_key_names_the_victims_origin",
    "units_are_still_reward_pending",
]


@pytest.mark.parametrize("fault", LOCK_FAULTS)
def test_a_lock_is_funded_only_by_the_owners_available_units_of_the_listed_origins(
    li_escrow: ModuleType, fault: str
) -> None:
    """ESC-01, ES-02 (T-ES-04): units enter a lock only from the signer's own available bucket."""
    # Given: genesis units, matured except in the last case.
    pending = fault == "units_are_still_reward_pending"
    fx = funded(li_escrow, matured=not pending)
    escrow, cold = fx.escrow, li_escrow.COLD.ss58
    thief = li_escrow.OTHER.ss58
    refused = {
        "asks_for_more_than_it_holds": lock_op(li_escrow, fx.cold, units=COLD_UNITS + 1),
        "names_another_owners_origin": lock_op(li_escrow, fx.other),
        "names_an_origin_never_issued": lock_op(li_escrow, "ff" * 32),
        "lists_an_origin_it_holds_nothing_of": lock_op(li_escrow, [fx.cold, fx.other], units=100),
        "a_foreign_key_names_the_victims_origin": lock_op(li_escrow, fx.cold, owner=thief),
        "units_are_still_reward_pending": lock_op(li_escrow, fx.cold),
    }[fault]
    before, issued = fingerprint(escrow), escrow.balances().issued
    # When / Then: the lock is refused, nothing is recorded, nothing is created.
    assert error_code(lambda: escrow.lock(refused, signer=refused.owner)) == UNFUNDED
    assert fingerprint(escrow) == before
    assert escrow.balances().issued == issued
    # Control: the same shape of lock, valid, is accepted (after vesting in the pending case).
    if pending:
        escrow.mature(H, (fx.cold,), 1)
    escrow.lock(lock_op(li_escrow, fx.cold, operation="24" * 32), signer=cold)
    assert escrow.balances(cold).admission_locked == 1000
    assert escrow.balances().conserved()


def test_no_lock_and_no_transfer_creates_units_on_an_empty_ledger(li_escrow: ModuleType) -> None:
    """ES-02 (T-ES-04): units exist only after authority-signed issuance."""
    # Given: a ledger that was never issued anything.
    escrow = bare_escrow(li_escrow, li_escrow.setup())
    cold = li_escrow.COLD.ss58
    attempts = {
        "admission lock": lambda: escrow.lock(lock_op(li_escrow, H), signer=cold),
        "contest lock": lambda: escrow.lock(
            lock_op(li_escrow, H, kind="LOCK_CONTEST"), signer=cold
        ),
        "transfer": lambda: escrow.transfer(transfer_op(li_escrow, H), signer=cold),
        "pay": lambda: escrow.pay(pay_op(li_escrow, H), signer=cold),
    }
    # When / Then: each finds no unit to move; nothing is issued, nothing is recorded.
    assert {name: error_code(call) for name, call in attempts.items()} == dict.fromkeys(
        attempts, UNFUNDED
    )
    assert (escrow.balances().issued, events(escrow)) == (0, 0)


@pytest.mark.parametrize("operation", ["transfer", "pay"])
def test_transfer_and_pay_spend_only_the_signers_own_available_units(
    li_escrow: ModuleType, operation: str
) -> None:
    """ESC-01, ESC-07: the boundary of a spend is the owner's available bucket, to the unit."""
    # Given: COLD holds exactly 20000 available units of its origin; OTHER holds another origin.
    fx = funded(li_escrow)
    escrow, cold = fx.escrow, li_escrow.COLD.ss58
    build = {"transfer": transfer_op, "pay": pay_op}[operation]
    spend = {"transfer": escrow.transfer, "pay": escrow.pay}[operation]
    refused = {
        "one unit too many": build(li_escrow, fx.cold, operation="71" * 32, units=COLD_UNITS + 1),
        "another owner's origin": build(li_escrow, fx.other, operation="72" * 32, units=1),
        "an origin never issued": build(li_escrow, "ff" * 32, operation="73" * 32, units=1),
    }
    before = fingerprint(escrow)
    # When / Then
    codes = {name: error_code(lambda op=op: spend(op, signer=cold)) for name, op in refused.items()}
    assert codes == dict.fromkeys(refused, UNFUNDED)
    assert fingerprint(escrow) == before
    # Control: the exact amount goes through and leaves nothing behind.
    spend(build(li_escrow, fx.cold, operation="74" * 32, units=COLD_UNITS), signer=cold)
    assert escrow.balances(cold).available == 0
    assert escrow.balances().conserved()


# ------------------------------------------------------------ closed exit set (ESC-07)

BUCKETS = ["admission_locked", "dispute_locked", "paid", "burned", "reward_pending"]


@pytest.mark.parametrize("bucket", BUCKETS)
def test_no_operation_draws_on_a_locked_pending_paid_or_burned_bucket(
    li_escrow: ModuleType, bucket: str
) -> None:
    """ESC-07: lock, transfer and pay can source only from available."""
    # Given: all 20000 units of COLD's origin sit in `bucket`.
    verdicts: dict[str, Settlement] = {}
    fx = funded(li_escrow, matured=bucket != "reward_pending", settlement=verdicts)
    escrow, cold = fx.escrow, li_escrow.COLD.ss58
    if bucket == "admission_locked":
        escrow.lock(lock_op(li_escrow, fx.cold, units=COLD_UNITS), signer=cold)
    elif bucket in ("dispute_locked", "burned"):
        bond = lock_op(li_escrow, fx.cold, units=COLD_UNITS, kind="LOCK_CONTEST")
        escrow.lock(bond, signer=cold)
        if bucket == "burned":
            verdicts[bond.operation_id] = settled("FRAUD")
            escrow.slash(release_op(bond), authority=li_escrow.COORD.ss58, now_beacon=10)
    elif bucket == "paid":
        escrow.pay(pay_op(li_escrow, fx.cold, units=COLD_UNITS), signer=cold)
    assert getattr(escrow.balances(cold), bucket) == COLD_UNITS
    attempts = {
        "lock_admission": lambda: escrow.lock(
            lock_op(li_escrow, fx.cold, operation="61" * 32, units=lock_floor(escrow)),
            signer=cold,
        ),
        "lock_contest": lambda: escrow.lock(
            lock_op(li_escrow, fx.cold, operation="62" * 32, units=1, kind="LOCK_CONTEST"),
            signer=cold,
        ),
        "transfer": lambda: escrow.transfer(
            transfer_op(li_escrow, fx.cold, operation="63" * 32, units=1), signer=cold
        ),
        "pay": lambda: escrow.pay(
            pay_op(li_escrow, fx.cold, operation="64" * 32, units=1), signer=cold
        ),
    }
    before = fingerprint(escrow)
    # When / Then: each attempt finds nothing to spend and changes nothing.
    assert {name: error_code(call) for name, call in attempts.items()} == dict.fromkeys(
        attempts, UNFUNDED
    )
    assert fingerprint(escrow) == before
    assert getattr(escrow.balances(cold), bucket) == COLD_UNITS


def test_collateral_issued_at_genesis_is_never_payable_as_weight(li_escrow: ModuleType) -> None:
    """ESC-16: only live reward origins, which carry a hotkey, can ever be paid."""
    # Given: matured genesis units, free to be locked, transferred or paid by their owners.
    fx = funded(li_escrow)
    # When / Then: no beacon, however late, makes any of them payable.
    assert fx.escrow.balances().available == COLD_UNITS + OTHER_UNITS
    assert fx.escrow.payable(10**9) == []


# ------------------------------------------------------------- release laws (ESC-09, ESC-11)

RELEASE_FAULTS = {
    "foreign_signer": "OWNER_AUTHORITY",
    "names_a_transfer_not_a_lock": "UNKNOWN_LOCK",
    "names_no_event_at_all": "UNKNOWN_LOCK",
    "other_admission_reference": "LOCK_REFERENCE",
    "other_owner_reference": "LOCK_REFERENCE",
    "contest_reference_on_an_admission_lock": "LOCK_REFERENCE",
    "other_dispute_reference": "LOCK_REFERENCE",
}


@pytest.mark.parametrize("fault", RELEASE_FAULTS)
def test_a_release_is_the_owners_for_the_owners_own_lock_with_matching_references(
    li_escrow: ModuleType, fault: str
) -> None:
    """ESC-09: release needs the owner's signature, an existing lock and the lock's references."""
    # Given: COLD's 1000-unit admission stake (a contest bond for the dispute case), a recorded
    # transfer, and a settlement that allows the release.
    fx = funded(li_escrow)
    escrow, cold, other = fx.escrow, li_escrow.COLD.ss58, li_escrow.OTHER.ss58
    contest = fault == "other_dispute_reference"
    bucket = "dispute_locked" if contest else "admission_locked"
    stake = lock_op(li_escrow, fx.cold, kind="LOCK_CONTEST" if contest else "LOCK_ADMISSION")
    escrow.lock(stake, signer=cold)
    transfer = transfer_op(li_escrow, fx.cold, operation="33" * 32)
    escrow.transfer(transfer, signer=cold)
    request, signer = release_op(stake), cold
    if fault == "foreign_signer":
        signer = other
    elif fault == "names_a_transfer_not_a_lock":
        request = release_op(stake, lock_id=transfer.operation_id)
    elif fault == "names_no_event_at_all":
        request = release_op(stake, lock_id="ee" * 32)
    elif fault == "other_admission_reference":
        request = release_op(stake, admission_id="ab" * 32)
    elif fault == "other_owner_reference":
        request, signer = release_op(stake, owner=other), other
    elif fault == "contest_reference_on_an_admission_lock":
        request = EscrowRelease.model_validate(
            {**release_op(stake).model_dump(), "admission_id": None, "dispute_id": H}
        )
    elif fault == "other_dispute_reference":
        request = EscrowRelease.model_validate(
            {**release_op(stake).model_dump(), "dispute_id": "ab" * 32}
        )
    before = fingerprint(escrow)
    # When / Then: refused with the code of the broken reference; the stake is untouched.
    assert (
        error_code(lambda: escrow.release(request, signer=signer, now_beacon=10))
        == RELEASE_FAULTS[fault]
    )
    assert fingerprint(escrow) == before
    assert getattr(escrow.balances(cold), bucket) == 1000
    # Control: the matching release by the owner returns exactly the locked units to the owner.
    escrow.release(release_op(stake), signer=cold, now_beacon=10)
    assert getattr(escrow.balances(cold), bucket) == 0
    assert escrow.balances(cold).available == COLD_UNITS - 500
    assert escrow.balances().conserved()


def test_a_lock_releases_at_most_what_it_locked_and_only_through_its_own_units(
    li_escrow: ModuleType,
) -> None:
    """ESC-11: no release exceeds its lock, a drained lock stays drained, replays are idempotent."""
    # Given: two locks of one owner on one origin, 1000 and 500 units.
    fx = funded(li_escrow)
    escrow, cold = fx.escrow, li_escrow.COLD.ss58
    first = lock_op(li_escrow, fx.cold, operation="22" * 32, units=1000)
    second = lock_op(li_escrow, fx.cold, operation="23" * 32, units=500)
    escrow.lock(first, signer=cold)
    escrow.lock(second, signer=cold)
    before = fingerprint(escrow)
    # When / Then: more than the first lock holds is refused, though both locks together cover it.
    for units in (1001, 1400):
        over = release_op(first, operation="45" * 32, units=units)
        assert (
            error_code(lambda over=over: escrow.release(over, signer=cold, now_beacon=10))
            == UNFUNDED
        )
    assert fingerprint(escrow) == before
    # Partial releases are legal and exact; a replay returns the stored receipt and moves nothing.
    part = release_op(first, operation="44" * 32, units=600)
    receipt = escrow.release(part, signer=cold, now_beacon=10)
    escrow.release(release_op(first, operation="46" * 32, units=400), signer=cold, now_beacon=10)
    settled_state = fingerprint(escrow)
    assert escrow.release(part, signer=cold, now_beacon=10) == receipt
    assert fingerprint(escrow) == settled_state
    # The drained lock refuses one more unit; the second lock was never touched.
    drained = release_op(first, operation="47" * 32, units=1)
    assert error_code(lambda: escrow.release(drained, signer=cold, now_beacon=10)) == UNFUNDED
    assert fingerprint(escrow) == settled_state
    assert escrow.balances(cold).admission_locked == 500
    assert escrow.balances(cold).available == COLD_UNITS - 500
    assert escrow.locked(H, cold)[0] == 500
    assert escrow.balances().conserved()


RELEASE_SETTLEMENTS = {
    "not_yet_resolved": settled(unresolved=True),
    "proven_fraud": settled("FRAUD"),
    "before_the_release_beacon": settled(release_beacon=11),
    "another_finality": settled(finality_hash="ef" * 32),
    "another_closed_dispute": settled(closed_dispute_root="cd" * 32),
}


@pytest.mark.parametrize("name", RELEASE_SETTLEMENTS)
def test_a_release_waits_for_a_resolved_vested_non_fraud_settlement_bound_to_its_hashes(
    li_escrow: ModuleType, name: str
) -> None:
    """ESC-09, ESC-14: an open dispute, a FRAUD outcome or a mismatched pair never frees a bond."""
    # Given: a 1000-unit contest bond whose settlement is not a final MATCH for this request.
    fx, bond, request = contested(li_escrow, RELEASE_SETTLEMENTS[name])
    escrow, cold = fx.escrow, li_escrow.COLD.ss58
    before = fingerprint(escrow)
    # When / Then: refused, and the bond stays locked.
    assert (
        error_code(lambda: escrow.release(request, signer=cold, now_beacon=10))
        == "DISPUTED_OR_UNVESTED"
    )
    assert fingerprint(escrow) == before
    assert escrow.balances(cold).dispute_locked == CONTEST_UNITS
    # Control: the settled MATCH of the same request frees it, to its owner and nobody else.
    fx, bond, request = contested(li_escrow, settled("MATCH"))
    fx.escrow.release(request, signer=cold, now_beacon=10)
    assert fx.escrow.balances(cold).available == COLD_UNITS
    assert fx.escrow.balances().dispute_locked == 0


# -------------------------------------------------------------- slash laws (ESC-13/14/15)

CONTEST_UNITS = 1000


def contested(
    li_escrow: ModuleType, verdict: Settlement
) -> tuple[Funded, EscrowLock, EscrowRelease]:
    """A ledger with COLD's 1000-unit contest bond and the slash request for all of it."""
    fx = funded(li_escrow, settlement=lambda _reference: verdict)
    bond = lock_op(
        li_escrow, fx.cold, units=CONTEST_UNITS, kind="LOCK_CONTEST", operation="22" * 32
    )
    fx.escrow.lock(bond, signer=li_escrow.COLD.ss58)
    return fx, bond, release_op(bond, operation="33" * 32)


SLASH_REFUSALS = {
    "authority_is_not_the_reward_authority": ("OTHER", 0, 100),
    "negative_referee_cost": ("COORD", -1, 100),
    "cost_above_the_cap": ("COORD", 101, 100),
    "negative_cap": ("COORD", 0, -1),
}


@pytest.mark.parametrize("fault", SLASH_REFUSALS)
def test_a_slash_needs_the_reward_authority_and_a_bounded_referee_cost(
    li_escrow: ModuleType, fault: str
) -> None:
    """ESC-13: authority and cost bounds are checked before anything moves."""
    # Given: a bond whose settlement is a resolved, vested FRAUD.
    fx, bond, request = contested(li_escrow, settled("FRAUD"))
    key, cost, cap = SLASH_REFUSALS[fault]
    authority = getattr(li_escrow, key).ss58
    before = fingerprint(fx.escrow)
    # When / Then
    assert (
        error_code(
            lambda: fx.escrow.slash(
                request,
                authority=authority,
                now_beacon=10,
                referee=li_escrow.OTHER.ss58,
                referee_cost=cost,
                max_referee_cost=cap,
            )
        )
        == "SLASH_AUTHORITY_OR_COST"
    )
    assert fingerprint(fx.escrow) == before
    # Control: the pinned authority with a cost inside the bound burns the bond.
    fx.escrow.slash(request, authority=li_escrow.COORD.ss58, now_beacon=10)
    assert fx.escrow.balances().burned == CONTEST_UNITS


def test_a_referee_cost_never_exceeds_the_units_slashed(li_escrow: ModuleType) -> None:
    """ESC-13, ESC-15: the cost is bounded by min(cap, units), whatever the cap says."""
    fx, bond, _ = contested(li_escrow, settled("FRAUD"))
    partial = release_op(bond, operation="34" * 32, units=10)
    before = fingerprint(fx.escrow)
    assert (
        error_code(
            lambda: fx.escrow.slash(
                partial,
                authority=li_escrow.COORD.ss58,
                now_beacon=10,
                referee=li_escrow.OTHER.ss58,
                referee_cost=11,
                max_referee_cost=1000,
            )
        )
        == "SLASH_AUTHORITY_OR_COST"
    )
    assert fingerprint(fx.escrow) == before


SLASH_SETTLEMENTS = {
    "match": settled("MATCH"),
    "infrastructure": settled("INFRASTRUCTURE"),
    "fraud_not_yet_resolved": settled("FRAUD", unresolved=True),
    "fraud_before_the_release_beacon": settled("FRAUD", release_beacon=11),
    "fraud_of_another_closed_dispute": settled("FRAUD", closed_dispute_root="cd" * 32),
    "fraud_of_another_finality": settled("FRAUD", finality_hash="ef" * 32),
}


@pytest.mark.parametrize("name", SLASH_SETTLEMENTS)
def test_a_slash_needs_a_resolved_vested_fraud_settlement_bound_to_the_request(
    li_escrow: ModuleType, name: str
) -> None:
    """ESC-14: only finalized, hash-bound FRAUD evidence burns a bond."""
    # Given: a bond and a settlement that is anything but a final FRAUD for this request.
    fx, bond, request = contested(li_escrow, SLASH_SETTLEMENTS[name])
    before = fingerprint(fx.escrow)
    # When / Then
    assert (
        error_code(lambda: fx.escrow.slash(request, authority=li_escrow.COORD.ss58, now_beacon=10))
        == "UNSETTLED_SLASH"
    )
    assert fingerprint(fx.escrow) == before
    assert fx.escrow.balances().dispute_locked == CONTEST_UNITS


SLASH_REFERENCES = {
    "other_dispute_reference": "LOCK_REFERENCE",
    "other_owner_reference": "LOCK_REFERENCE",
    "admission_reference_on_a_contest_bond": "LOCK_REFERENCE",
    "names_a_transfer_not_a_lock": "UNKNOWN_LOCK",
    "names_no_event_at_all": "UNKNOWN_LOCK",
}


@pytest.mark.parametrize("fault", SLASH_REFERENCES)
def test_a_slash_burns_only_the_lock_it_names_with_matching_references(
    li_escrow: ModuleType, fault: str
) -> None:
    """ESC-14: a slash names an existing lock and the references that lock was made with."""
    # Given: a bond settled FRAUD for every reference, and a recorded transfer.
    fx, bond, request = contested(li_escrow, settled("FRAUD"))
    escrow, cold = fx.escrow, li_escrow.COLD.ss58
    transfer = transfer_op(li_escrow, fx.cold, operation="34" * 32)
    escrow.transfer(transfer, signer=cold)
    body = request.model_dump()
    refused = {
        "other_dispute_reference": EscrowRelease.model_validate({**body, "dispute_id": "ab" * 32}),
        "other_owner_reference": EscrowRelease.model_validate(
            {**body, "owner": li_escrow.OTHER.ss58}
        ),
        "admission_reference_on_a_contest_bond": EscrowRelease.model_validate(
            {**body, "admission_id": H, "dispute_id": None}
        ),
        "names_a_transfer_not_a_lock": release_op(
            bond, operation="33" * 32, lock_id=transfer.operation_id
        ),
        "names_no_event_at_all": release_op(bond, operation="33" * 32, lock_id="ee" * 32),
    }[fault]
    authority = li_escrow.COORD.ss58
    before = fingerprint(escrow)
    # When / Then: refused with the code of the broken reference; the bond is untouched.
    assert (
        error_code(lambda: escrow.slash(refused, authority=authority, now_beacon=10))
        == SLASH_REFERENCES[fault]
    )
    assert fingerprint(escrow) == before
    assert escrow.balances(cold).dispute_locked == CONTEST_UNITS
    # Control: the request that matches its lock burns the bond.
    escrow.slash(request, authority=authority, now_beacon=10)
    assert escrow.balances().burned == CONTEST_UNITS


def test_a_proven_fraud_burns_the_bond_once_and_closes_the_release_path(
    li_escrow: ModuleType,
) -> None:
    """ESC-14, ESC-15: slash is at-most-once, moves only the bond, and nothing releases it after."""
    # Given: a resolved, vested FRAUD settlement of a 1000-unit bond.
    fx, bond, request = contested(li_escrow, settled("FRAUD"))
    escrow, cold = fx.escrow, li_escrow.COLD.ss58
    authority = li_escrow.COORD.ss58
    # When
    receipt = escrow.slash(request, authority=authority, now_beacon=10)
    after = fingerprint(escrow)
    # Then: the whole bond burned, nothing else moved, the same request replays to its receipt.
    assert escrow.balances().burned == CONTEST_UNITS
    assert escrow.balances(cold).available == COLD_UNITS - CONTEST_UNITS
    assert escrow.balances(li_escrow.OTHER.ss58).available == OTHER_UNITS
    assert escrow.slash(request, authority=authority, now_beacon=10) == receipt
    assert fingerprint(escrow) == after
    # A second slash under a new operation id finds no units; a release finds a FRAUD outcome.
    again = release_op(bond, operation="35" * 32)
    assert error_code(lambda: escrow.slash(again, authority=authority, now_beacon=10)) == UNFUNDED
    back = release_op(bond, operation="36" * 32)
    assert (
        error_code(lambda: escrow.release(back, signer=cold, now_beacon=10))
        == "DISPUTED_OR_UNVESTED"
    )
    assert fingerprint(escrow) == after
    assert escrow.balances().conserved()


def test_a_slash_pays_the_referee_cost_then_burns_exactly_the_rest(li_escrow: ModuleType) -> None:
    """ESC-15: the destination is the burned bucket and the named funded referee, nothing else."""
    # Given
    fx, bond, request = contested(li_escrow, settled("FRAUD"))
    escrow, referee = fx.escrow, li_escrow.OTHER.ss58
    # When
    escrow.slash(
        request,
        authority=li_escrow.COORD.ss58,
        now_beacon=10,
        referee=referee,
        referee_cost=100,
        max_referee_cost=100,
    )
    # Then: 100 to the referee, 900 burned, the bond is empty and the owner lost exactly the bond.
    assert escrow.balances(referee).available == OTHER_UNITS + 100
    assert (escrow.balances().burned, escrow.balances().dispute_locked) == (900, 0)
    assert escrow.balances(li_escrow.COLD.ss58).available == COLD_UNITS - CONTEST_UNITS
    assert escrow.balances().conserved()


@pytest.mark.parametrize("fault", ["referee_is_not_funded", "referee_is_missing", "stake_lock"])
def test_a_referee_cost_needs_a_funded_referee_and_a_contest_bond(
    li_escrow: ModuleType, fault: str
) -> None:
    """ESC-15: a cost is paid only out of a contest bond and only to a referee that holds units."""
    # Given: a FRAUD settlement of the bond (or of an admission stake in the last case).
    fx, bond, request = contested(li_escrow, settled("FRAUD"))
    escrow = fx.escrow
    if fault == "stake_lock":
        stake = lock_op(li_escrow, fx.cold, operation="24" * 32, units=CONTEST_UNITS)
        escrow.lock(stake, signer=li_escrow.COLD.ss58)
        request = release_op(stake, operation="37" * 32)
    referee = {
        "referee_is_not_funded": Keypair(b"\x42" * 32).ss58,
        "referee_is_missing": None,
        "stake_lock": li_escrow.OTHER.ss58,
    }[fault]
    before = fingerprint(escrow)
    # When / Then
    assert (
        error_code(
            lambda: escrow.slash(
                request,
                authority=li_escrow.COORD.ss58,
                now_beacon=10,
                referee=referee,
                referee_cost=100,
                max_referee_cost=100,
            )
        )
        == "UNFUNDED_REFEREE"
    )
    assert fingerprint(escrow) == before


# ------------------------------------------------------------------ issuance (ESC-20)


def test_units_are_issued_only_under_the_pinned_authority_signature_bound_to_the_run(
    li_escrow: ModuleType,
) -> None:
    """ESC-20: genesis needs the reward authority's signature over this run, and happens once."""
    # Given: an empty test-mode ledger.
    escrow = bare_escrow(li_escrow, li_escrow.setup())
    forged = {
        "signed by another key": signed_genesis(li_escrow, escrow, key=li_escrow.OTHER),
        "signed for another run": signed_genesis(li_escrow, escrow, run_id=H),
    }
    # When / Then: neither issues a unit, neither records an event.
    for name, genesis in forged.items():
        assert (
            error_code(lambda genesis=genesis: escrow.test_genesis(genesis)) == "ORIGIN_AUTHORITY"
        )
        assert (escrow.balances().issued, events(escrow)) == (0, 0), name
    # Control: the authority's signature issues the pinned allocation.
    genesis = signed_genesis(li_escrow, escrow)
    receipt = escrow.test_genesis(genesis)
    assert escrow.balances().issued == COLD_UNITS + OTHER_UNITS
    # One-shot: the same genesis replays to its receipt; a conflicting body never adds units.
    assert escrow.test_genesis(genesis) == receipt
    changed = [
        o.model_copy(update={"units": o.units + d})
        for o, d in zip(genesis.origins, (1, -1), strict=True)
    ]
    other_split = signed_genesis(li_escrow, escrow).model_copy(update={"origins": changed})
    other_split = other_split.model_copy(
        update={"authority_sig": li_escrow.COORD.sign(authority_message(other_split)).hex()}
    )
    before = fingerprint(escrow)
    assert error_code(lambda: escrow.test_genesis(other_split)) == "OPERATION_CONFLICT"
    assert fingerprint(escrow) == before
    assert (escrow.balances().issued, events(escrow)) == (COLD_UNITS + OTHER_UNITS, 1)
    assert escrow.balances().conserved()


# ----------------------------------------------------------- restart verification (ESC-22)


def closed_ledger(li_escrow: ModuleType, path: Path, *, with_events: bool = True) -> None:
    """A file ledger holding genesis, maturity, a lock, a transfer and a pay, then closed."""
    escrow = li_escrow.ledger(li_escrow.setup(), path)
    if with_events:
        origin, _ = li_escrow.fund(escrow)
        cold = li_escrow.COLD.ss58
        escrow.lock(lock_op(li_escrow, origin), signer=cold)
        escrow.transfer(transfer_op(li_escrow, origin), signer=cold)
        escrow.pay(pay_op(li_escrow, origin), signer=cold)
    escrow.db.close()


Statements = list[tuple[str, tuple[str, ...]]]
TAMPERED: dict[str, tuple[Callable[[ModuleType], Statements], str]] = {
    "an_event_request_rewritten": (
        lambda fx: [("UPDATE escrow_events SET request=request||' ' WHERE seq=2", ())],
        "EVENT_CHAIN",
    ),
    "an_event_hash_rewritten": (
        lambda fx: [
            (
                "UPDATE escrow_events SET event_hash=substr(event_hash,2)||'0' "
                "WHERE seq=(SELECT MAX(seq) FROM escrow_events)",
                (),
            )
        ],
        "EVENT_CHAIN",
    ),
    "a_receipt_hash_rewritten": (
        lambda fx: [
            (
                "UPDATE escrow_events SET receipt=replace(receipt,event_hash,?) "
                "WHERE seq=(SELECT MAX(seq) FROM escrow_events)",
                ("f" * 64,),
            )
        ],
        "EVENT_CHAIN",
    ),
    "a_link_to_the_previous_event_rewritten": (
        lambda fx: [("UPDATE escrow_events SET prev_hash=? WHERE seq=3", ("f" * 64,))],
        "EVENT_CHAIN",
    ),
    "a_sequence_number_rewritten": (
        lambda fx: [
            (
                "UPDATE escrow_events SET seq=seq+1000 "
                "WHERE seq=(SELECT MAX(seq) FROM escrow_events)",
                (),
            )
        ],
        "EVENT_CHAIN",
    ),
    "a_receipt_moved_to_another_operation": (
        lambda fx: [
            (
                "UPDATE escrow_events SET receipt=replace(receipt,"
                "json_extract(receipt,'$.operation_id'),?) "
                "WHERE seq=(SELECT MAX(seq) FROM escrow_events)",
                ("f" * 64,),
            )
        ],
        "EVENT_CHAIN",
    ),
    "a_receipt_of_another_ledger_mode": (
        lambda fx: [
            (
                "UPDATE escrow_events SET receipt=replace(receipt,?,?) "
                "WHERE seq=(SELECT MAX(seq) FROM escrow_events)",
                ('"ledger_mode":"test"', '"ledger_mode":"production"'),
            )
        ],
        "EVENT_CHAIN",
    ),
    "a_snapshot_deleted": (
        lambda fx: [
            (
                "DELETE FROM escrow_snapshots WHERE operation_id="
                "(SELECT operation_id FROM escrow_events WHERE seq=2)",
                (),
            )
        ],
        "MISSING_EVENT_SNAPSHOT",
    ),
    "units_moved_to_another_owner": (
        lambda fx: [("UPDATE escrow_units SET owner=? WHERE owner=?", (fx.HOT.ss58, fx.COLD.ss58))],
        "PERSISTED_STATE_MISMATCH",
    ),
    "units_added_without_issuance": (
        lambda fx: [
            (
                "UPDATE escrow_units SET units=units+1 WHERE owner=? AND bucket='available'",
                (fx.COLD.ss58,),
            )
        ],
        "PERSISTED_STATE_MISMATCH",
    ),
    "the_last_event_dropped_with_its_snapshot": (
        lambda fx: [
            (
                "DELETE FROM escrow_snapshots WHERE operation_id="
                "(SELECT operation_id FROM escrow_events ORDER BY seq DESC LIMIT 1)",
                (),
            ),
            ("DELETE FROM escrow_events WHERE seq=(SELECT MAX(seq) FROM escrow_events)", ()),
        ],
        "PERSISTED_STATE_MISMATCH",
    ),
}
BARE_TAMPERED = {
    "an_origin_issued_without_units": (
        ("INSERT INTO escrow_origins VALUES(?,100,1,NULL,NULL)", ("ab" * 32,)),
        "ORIGIN_CONSERVATION",
    ),
    "units_of_an_origin_never_issued": (
        ("INSERT INTO escrow_units VALUES(?,?,'available','',100)", ("cd" * 32, "5" * 48)),
        "CONSERVATION",
    ),
}


def reopened_balances(li_escrow: ModuleType, setup: object, path: Path) -> Balances:
    escrow = li_escrow.ledger(setup, path)
    try:
        return escrow.balances()
    finally:
        escrow.db.close()


def edit(path: Path, statements: Statements) -> None:
    db = sqlite3.connect(path, isolation_level=None)
    for statement, parameters in statements:
        db.execute(statement, parameters)
    db.close()


@pytest.mark.parametrize("case", TAMPERED)
def test_a_ledger_file_edited_or_cut_in_place_is_refused_at_reopen(
    li_escrow: ModuleType, tmp_path: Path, case: str
) -> None:
    """ESC-22 (condition: the file is damaged, not re-forged): verify() runs at every open."""
    # Given: a closed file ledger and a private copy of it.
    original = tmp_path / "ledger.db"
    closed_ledger(li_escrow, original)
    copy = tmp_path / "copy.db"
    shutil.copy(original, copy)
    setup = li_escrow.setup()
    # Control: an untouched copy reopens with the same balances as the original.
    assert reopened_balances(li_escrow, setup, copy) == reopened_balances(
        li_escrow, setup, original
    )
    # When: one stored row is changed in the copy.
    statements, code = TAMPERED[case]
    edit(copy, statements(li_escrow))
    # Then: opening it fails with the check that owns that kind of damage; the original opens.
    assert error_code(lambda: li_escrow.ledger(setup, copy)) == code
    assert reopened_balances(li_escrow, setup, original).conserved()


@pytest.mark.parametrize("case", BARE_TAMPERED)
def test_units_that_issuance_does_not_explain_are_refused_when_no_event_exists(
    li_escrow: ModuleType, tmp_path: Path, case: str
) -> None:
    """ESC-22: with no event to compare against, per-origin and global conservation still hold."""
    # Given: a closed ledger without a single event, then a stray row.
    path = tmp_path / "ledger.db"
    closed_ledger(li_escrow, path, with_events=False)
    statement, code = BARE_TAMPERED[case]
    edit(path, [statement])
    # When / Then
    assert error_code(lambda: li_escrow.ledger(li_escrow.setup(), path)) == code


# ----------------------------------------------------------------- conservation (ESC-06)


def test_a_unit_row_that_issuance_does_not_explain_stops_the_next_event(
    li_escrow: ModuleType,
) -> None:
    """ESC-06: conservation is re-checked inside every event, before it is recorded."""
    # Given: a ledger that accepted a transfer, then gains one unit outside any operation.
    fx = funded(li_escrow)
    escrow, cold = fx.escrow, li_escrow.COLD.ss58
    escrow.transfer(transfer_op(li_escrow, fx.cold, operation="31" * 32), signer=cold)
    escrow.db.execute(
        "UPDATE escrow_units SET units=units+1 WHERE owner=? AND bucket='available'", (cold,)
    )
    before = fingerprint(escrow)
    # When / Then: the next legitimate event is refused and records nothing.
    assert (
        error_code(
            lambda: escrow.transfer(
                transfer_op(li_escrow, fx.cold, operation="32" * 32), signer=cold
            )
        )
        == "ORIGIN_CONSERVATION"
    )
    assert fingerprint(escrow) == before


@pytest.mark.parametrize("units", [0, -1, 1.5, "10", True, None, 2**53])
def test_escrow_amounts_are_strict_positive_integers_within_the_wire_ceiling(
    li_escrow: ModuleType, units: object
) -> None:
    """ESC-06: no float, string, boolean, zero, negative or oversized amount reaches the ledger."""
    cold, other = li_escrow.COLD.ss58, li_escrow.OTHER.ss58
    common = {
        "operation_id": "aa" * 32,
        "owner": cold,
        "origin_ids": ["bb" * 32],
        "admission_id": None,
        "dispute_id": None,
    }
    builders: dict[str, Callable[[object], object]] = {
        "operation": lambda u: EscrowOperation(**common, units=u),
        "transfer": lambda u: EscrowTransfer(**common, units=u, recipient=other),
        "release": lambda u: EscrowRelease(
            **common, units=u, lock_id="cc" * 32, finality_hash=H, closed_dispute_root=H
        ),
        "lock": lambda u: EscrowLock(**{**common, "dispute_id": H}, units=u, kind="LOCK_CONTEST"),
    }
    for build in builders.values():
        build(1)
        build(2**53 - 1)  # the control: both ends of the accepted range
    for build in builders.values():
        with pytest.raises(ValidationError, match="units"):
            build(units)


# ----------------------------------------------------------- stake of an admission (LCK-10/11)


def test_stake_counts_only_locks_that_name_its_admission_and_owner(li_escrow: ModuleType) -> None:
    """LCK-10: locked(admission, owner) sums the live units of locks naming exactly that pair."""
    # Given: locks of two admissions by COLD, one by OTHER, and a contest bond (not stake).
    fx = funded(li_escrow)
    escrow, cold, other = fx.escrow, li_escrow.COLD.ss58, li_escrow.OTHER.ss58
    first, second = "a1" * 32, "b2" * 32
    plan = [
        lock_op(li_escrow, fx.cold, operation="c1" * 32, units=600, reference=first),
        lock_op(li_escrow, fx.cold, operation="c2" * 32, units=300, reference=first),
        lock_op(li_escrow, fx.cold, operation="c3" * 32, units=100, reference=second),
        lock_op(li_escrow, fx.other, operation="c4" * 32, units=170, reference=first, owner=other),
        lock_op(
            li_escrow, fx.cold, operation="c5" * 32, units=50, reference=first, kind="LOCK_CONTEST"
        ),
    ]
    for op in plan:
        escrow.lock(op, signer=op.owner)
    # When / Then
    stakes = {
        ("first", "COLD"): escrow.locked(first, cold),
        ("second", "COLD"): escrow.locked(second, cold),
        ("first", "OTHER"): escrow.locked(first, other),
    }
    assert {pair: units for pair, (units, _) in stakes.items()} == {
        ("first", "COLD"): 900,
        ("second", "COLD"): 100,
        ("first", "OTHER"): 170,
    }
    assert len({receipt for _, receipt in stakes.values()}) == 3
    assert escrow.locked(second, other) == (0, "0" * 64)
    assert escrow.locked("cc" * 32, cold) == (0, "0" * 64)
    # A released lock is no stake any more, and the receipt hash follows the live locks.
    escrow.release(release_op(plan[1], operation="44" * 32), signer=cold, now_beacon=10)
    after = escrow.locked(first, cold)
    assert after[0] == 600 and after[1] != stakes[("first", "COLD")][1]


def test_the_same_units_cannot_back_two_locks_or_two_admissions(li_escrow: ModuleType) -> None:
    """LCK-11: a unit sits in one lock; stake moves between admissions only through a release."""
    # Given: all of COLD's units locked for the first admission.
    fx = funded(li_escrow)
    escrow, cold = fx.escrow, li_escrow.COLD.ss58
    first, second = "a1" * 32, "b2" * 32
    everything = lock_op(li_escrow, fx.cold, operation="c1" * 32, units=COLD_UNITS, reference=first)
    escrow.lock(everything, signer=cold)
    before = fingerprint(escrow)
    # When / Then: one more unit for the second admission, or the same lock id re-pointed, fails.
    one_more = lock_op(
        li_escrow, fx.cold, operation="c2" * 32, units=lock_floor(escrow), reference=second
    )
    assert error_code(lambda: escrow.lock(one_more, signer=cold)) == UNFUNDED
    repointed = lock_op(li_escrow, fx.cold, operation="c1" * 32, units=COLD_UNITS, reference=second)
    assert error_code(lambda: escrow.lock(repointed, signer=cold)) == "OPERATION_CONFLICT"
    assert fingerprint(escrow) == before
    assert (escrow.locked(first, cold)[0], escrow.locked(second, cold)[0]) == (COLD_UNITS, 0)
    # The stake follows a release: afterwards the second admission holds it and the first none.
    escrow.release(release_op(everything, operation="44" * 32), signer=cold, now_beacon=10)
    escrow.lock(
        lock_op(li_escrow, fx.cold, operation="c3" * 32, units=COLD_UNITS, reference=second),
        signer=cold,
    )
    assert (escrow.locked(first, cold)[0], escrow.locked(second, cold)[0]) == (0, COLD_UNITS)
    assert escrow.balances().admission_locked == COLD_UNITS and escrow.balances().conserved()
