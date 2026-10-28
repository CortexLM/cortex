"""Shadow-reward credit at the ledger boundary (SHD-01, 02, 13, 14, 15, 16, 17, 18).

Shadow rewards are collateral: one origin of one round budget per reservation, never emission.
Every test here states a defence the ledger must hold and asserts, for a refusal, the exact
code and that nothing moved. The work is the real w=0 execution of `real_reward`; the ledger is
the production EscrowV2 on a private database, so a refusal comes from the ledger and from nothing
a stand-in decided. What only the store guards (the first-trial rule of a reservation) is tested
in test_shadow_2.py.
"""

from __future__ import annotations

import sqlite3
from dataclasses import replace
from pathlib import Path
from types import ModuleType
from typing import Any

import pytest
from hypothesis import given
from hypothesis import strategies as st

from hypertrain.ledger.escrow_v2 import EscrowError, EscrowReceipt, Settlement
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages_v2 import OriginAllocation
from hypertrain.protocol.messages_v2 import (
    ShadowReservationRequestV1 as ReservationRequest,
)
from hypertrain.protocol.messages_v2 import (
    ShadowReservationV1 as Reservation,
)

from .support import H, Real, mature
from .support_shadow import (
    MATURE_AT,
    UNIT,
    Engine,
    as_live_class,
    engine,
    file_engine,
    refusal_code,
    resign_shadow,
)


def issued_origins(built: Engine) -> list[sqlite3.Row]:
    rows: list[sqlite3.Row] = built.escrow.db.execute(
        "SELECT * FROM escrow_origins ORDER BY origin"
    ).fetchall()
    return rows


# --- SHD-01: credited at most once per reservation ------------------------------------------


def test_a_shadow_reward_is_credited_once_however_often_it_is_delivered(
    li_escrow: ModuleType, real_reward: Real
) -> None:
    # Given: a reserved ordinal and its accepted reward
    built = engine(li_escrow, real_reward)
    signed = built.reward(built.reserve())
    receipt = built.escrow.reward_finalize(signed, built.facts)
    assert built.escrow.balances().reward_pending == UNIT

    # When: it is delivered again as it was, and again with a fresh signature (sr25519 signatures
    # are randomised, so a retry never repeats the first one byte for byte)
    renewed = resign_shadow(li_escrow, signed)
    assert renewed.authority_sig != signed.authority_sig
    redelivered = [built.escrow.reward_finalize(r, built.facts) for r in (signed, renewed)]

    # Then: the stored receipt comes back and nothing is issued twice
    assert redelivered == [receipt, receipt]
    assert built.events("SHADOW_REWARD_FINALIZE") == 1
    assert built.escrow.balances().issued == UNIT
    assert len(issued_origins(built)) == 1

    # And: another content under the same reservation is a conflict, not a second credit
    before = built.marker()
    other = resign_shadow(li_escrow, signed, tape_hash=H)
    assert refusal_code(lambda: built.escrow.reward_finalize(other, built.facts)) == (
        "OPERATION_CONFLICT"
    )
    assert built.marker() == before


def test_the_shadow_table_itself_refuses_a_second_row_for_an_ordinal_a_reservation_or_an_origin(
    li_escrow: ModuleType, real_reward: Real
) -> None:
    # Given: one credited shadow reward
    built = engine(li_escrow, real_reward)
    built.accept(built.reserve())
    db = built.escrow.db
    row = db.execute("SELECT * FROM escrow_shadow_finalized").fetchone()
    other = "cd" * 32

    # When / Then: a second row sharing the ordinal, the reservation or the origin is refused by
    # the schema, whatever the application checked before it
    clashes = {
        "ordinal": (0, other, other),
        "reservation_hash": (1, row["reservation_hash"], other),
        "origin": (1, other, row["origin"]),
    }
    for column, (ordinal, reservation_hash, origin) in clashes.items():
        with pytest.raises(sqlite3.IntegrityError, match=f"UNIQUE.*{column}"):
            db.execute(
                "INSERT INTO escrow_shadow_finalized VALUES(?,?,?,?,?)",
                (ordinal, row["trial_epoch"], row["finalize_hash"], reservation_hash, origin),
            )
    assert db.execute("SELECT COUNT(*) FROM escrow_shadow_finalized").fetchone()[0] == 1
    assert built.escrow.balances().conserved()


# --- SHD-01, 02, 15, 17: random operation sequences ------------------------------------------

# Dense on purpose: every step is meaningful in most states, so a short sequence reaches
# "reserved, credited, matured, then reserved again" often.
STEPS = st.lists(
    st.sampled_from(
        [("reserve", 0), ("reserve", 1), ("reserve", 2)]
        + [("accept", 0), ("accept", 0), ("mature", 0), ("live", 0)]
    ),
    min_size=8,
    max_size=16,
)


def check_every_law(
    built: Engine, reserved: dict[int, Reservation], receipts: dict[int, EscrowReceipt]
) -> None:
    escrow = built.escrow
    balances = escrow.balances()
    assert balances.conserved()
    # One pending slot, and it is exactly the reserved-but-not-yet-credited reservation.
    assert built.pending() == len(reserved) - len(receipts) <= 1
    # Reserved-but-unissued rounds count against the cap exactly like issued ones.
    assert balances.issued + built.pending() * UNIT <= escrow.policy.max_total_issuance
    rows = escrow.db.execute("SELECT * FROM escrow_shadow_finalized ORDER BY ordinal").fetchall()
    assert [row["ordinal"] for row in rows] == list(range(len(rows)))
    assert built.events("SHADOW_REWARD_FINALIZE") == len(rows) == len(receipts)
    assert len({row["origin"] for row in rows}) == len(rows)
    assert balances.issued == (len(rows) + built.events("REWARD_FINALIZE")) * UNIT
    for row in rows:
        issued = escrow.db.execute(
            "SELECT issued FROM escrow_origins WHERE origin=?", (row["origin"],)
        ).fetchone()
        assert issued[0] == UNIT
    # Collateral, never emission: no matured shadow origin is ever payable.
    assert not {row["origin"] for row in rows} & {r["origin"] for r in escrow.payable(MATURE_AT)}


def apply_step(
    built: Engine,
    kind: str,
    index: int,
    reserved: dict[int, Reservation],
    receipts: dict[int, EscrowReceipt],
) -> None:
    if kind == "reserve":
        pending_before = built.pending()
        binding = built.reserve(index)
        # A new reservation never appears while another one is pending.
        assert index in reserved or pending_before == 0
        assert reserved.setdefault(index, binding) == binding
    elif kind == "accept":
        # The pending reservation if there is one, else a redelivery of the oldest credit.
        targets = [i for i in reserved if i not in receipts] or list(receipts)
        if targets:
            receipt = built.accept(reserved[targets[0]])
            assert receipts.setdefault(targets[0], receipt) == receipt
    elif kind == "mature":
        for i in list(receipts):
            origin = built.origin(reserved[i])
            built.escrow.mature(format(0x40 + i, "02x") * 32, (origin,), MATURE_AT)
    else:
        built.escrow.reward_finalize(built.live, built.evidence)


@given(STEPS)
def test_random_shadow_operations_keep_every_ledger_law(
    li_escrow: ModuleType, real_reward: Real, steps: list[tuple[str, int]]
) -> None:
    # Given: a ledger with room for two rounds and three competing admissions
    built = engine(li_escrow, real_reward, awards=2)
    cap = built.escrow.policy.max_total_issuance
    reserved: dict[int, Reservation] = {}
    receipts: dict[int, EscrowReceipt] = {}
    for kind, index in steps:
        before = built.marker()
        pending, issued = built.pending(), built.escrow.balances().issued
        try:
            apply_step(built, kind, index, reserved, receipts)
        except EscrowError as error:
            # Then: a refusal has a reason the ledger laws give, and changes nothing
            if error.code == "SHADOW_RESERVATION_PENDING":
                assert kind == "reserve" and pending == 1
            elif error.code == "TOTAL_ISSUANCE_BUDGET":
                held = pending * UNIT if kind == "live" else 0
                assert kind in ("reserve", "live") and issued + held + UNIT > cap
                assert kind == "live" or pending == 0
            else:
                raise
            assert built.marker() == before
        check_every_law(built, reserved, receipts)
    built.escrow.verify()


# --- SHD-02: one pending slot, contiguous ordinals 0..11 -------------------------------------


def test_reservations_take_one_pending_slot_and_the_ordinals_zero_to_eleven_in_order(
    li_escrow: ModuleType, real_reward: Real
) -> None:
    # Given: room for thirteen rounds, so only the bootstrap bound can stop a thirteenth
    built = engine(li_escrow, real_reward, awards=13)
    first = built.reserve(0)
    assert first.shadow_ordinal == 0
    assert built.reserve(0) == first

    # When: the slot is taken and another admission asks, or the same key asks differently
    before = built.marker()
    assert refusal_code(lambda: built.reserve(1)) == "SHADOW_RESERVATION_PENDING"
    request = ReservationRequest.model_validate(
        {k: v for k, v in first.body().items() if k != "shadow_ordinal"}
    )
    custody = li_escrow.original_trial_custody(built.evidence)
    different = request.model_copy(update={"custody_hash": "33" * 32})
    assert refusal_code(
        lambda: built.escrow.reserve_shadow(different, replace(custody, custody_hash="33" * 32))
    ) == ("SHADOW_RESERVATION_CONFLICT")
    # Then: nothing was reserved
    assert built.marker() == before

    # When: each reservation is accepted before the next one is taken
    ordinals = [first.shadow_ordinal]
    built.accept(first)
    assert built.reserve(0) == first, "an accepted reservation is still the answer to its key"
    for index in range(1, 12):
        binding = built.reserve(index)
        ordinals.append(binding.shadow_ordinal)
        built.accept(binding)

    # Then: ordinals are contiguous and the thirteenth reservation is refused for the bound
    assert ordinals == list(range(12))
    before = built.marker()
    assert refusal_code(lambda: built.reserve(12)) == "BOOTSTRAP_FINISHED"
    assert built.marker() == before
    assert built.escrow.balances().issued == 12 * UNIT
    built.escrow.verify()


# --- SHD-14: amount, owner and maturity are derived by the ledger ----------------------------

BODY_FAULTS = {
    "round budget": ({"budget_units": UNIT + 1}, "ROUND_BUDGET"),
    "origin ids": ({"origin_ids": [H]}, "REWARD_ALLOCATION"),
    "allocation hash": ({"allocation_hash": H}, "REWARD_ALLOCATION"),
    "earlier maturity": ({"mature_at": MATURE_AT - 1}, "SHADOW_RESERVATION_CONFLICT"),
    "later maturity": ({"mature_at": MATURE_AT + 1}, "SHADOW_RESERVATION_CONFLICT"),
    "other ordinal": ({"shadow_ordinal": 1}, "SHADOW_RESERVATION_REQUIRED"),
    "other reservation": ({"reservation_hash": "22" * 32}, "SHADOW_RESERVATION_CONFLICT"),
    "other epoch": ({"w": 1}, "SHADOW_RESERVATION_CONFLICT"),
    "verdict root": ({"verdict_root": H}, "VERDICT_ROOT"),
    "tape hash": ({"tape_hash": H}, "FINALITY_BINDING_OR_VESTING"),
}


@pytest.mark.parametrize("fault", BODY_FAULTS)
def test_a_signed_shadow_reward_that_differs_from_what_the_ledger_derives_is_refused(
    li_escrow: ModuleType, real_reward: Real, fault: str
) -> None:
    # Given: a reservation and a reward re-signed by the reward authority with one field changed
    built = engine(li_escrow, real_reward)
    reward = built.reward(built.reserve())
    changes, code = BODY_FAULTS[fault]
    before = built.marker()

    # When / Then: the ledger names the field it refused and moves nothing
    assert refusal_code(
        lambda: built.escrow.reward_finalize(
            resign_shadow(li_escrow, reward, **changes), built.facts
        )
    ) == (code)
    assert built.marker() == before


def test_an_over_budget_reward_is_refused_even_when_its_allocation_agrees_with_it(
    li_escrow: ModuleType, real_reward: Real
) -> None:
    # Given: a reward whose budget, origin and allocation hash are all consistent with U + 1
    built = engine(li_escrow, real_reward)
    binding = built.reserve()
    reward = built.reward(binding)
    origin = OriginAllocation(
        origin_id=built.origin(binding),
        owner=binding.coldkey,
        units=UNIT + 1,
        mature_at=binding.mature_at,
    )
    oversized = resign_shadow(
        li_escrow,
        reward,
        budget_units=UNIT + 1,
        allocation_hash=sha256_hex(canonicalize([origin.body()])),
    )
    before = built.marker()

    # When / Then: only the round budget stops it
    assert refusal_code(lambda: built.escrow.reward_finalize(oversized, built.facts)) == (
        "ROUND_BUDGET"
    )
    assert built.marker() == before


def evidence_faults(built: Engine) -> dict[str, Any]:
    fixture = built.fixture
    facts = built.facts
    foreign = replace(facts.works[0], owner=fixture.OTHER.ss58)
    return {
        "foreign owner": (lambda: None, replace(facts, works=(foreign,)), "UNVERIFIED_REWARD_WORK"),
        "another assignment": (
            lambda: setattr(built.escrow, "shadow_assignment_of", lambda _binding: (1,)),
            facts,
            "UNVERIFIED_REWARD_WORK",
        ),
        "owner changed after the reservation": (
            lambda: setattr(built.escrow, "owner_of", lambda _hotkey: fixture.OTHER.ss58),
            replace(facts, works=(foreign,)),
            "SHADOW_WORK_BINDING",
        ),
    }


@pytest.mark.parametrize(
    "fault", ["foreign owner", "another assignment", "owner changed after the reservation"]
)
def test_shadow_work_is_credited_to_the_reserved_owner_and_assignment_only(
    li_escrow: ModuleType, real_reward: Real, fault: str
) -> None:
    # Given: a reservation, and evidence that differs from it in one way
    built = engine(li_escrow, real_reward)
    reward = built.reward(built.reserve())
    prepare, facts, code = evidence_faults(built)[fault]
    prepare()
    before = built.marker()

    # When / Then
    assert refusal_code(lambda: built.escrow.reward_finalize(reward, facts)) == code
    assert built.marker() == before


@pytest.mark.parametrize("signature", ["zeros", "live reward class name"])
def test_a_shadow_reward_needs_its_own_authority_signature(
    li_escrow: ModuleType, real_reward: Real, signature: str
) -> None:
    # Given: a shadow reward with no authority signature, or with the signature the reward
    # authority would make over this very body for a live reward (the class name is the only
    # difference between the two messages)
    built = engine(li_escrow, real_reward)
    reward = built.reward(built.reserve())
    forged = (
        reward.model_copy(update={"authority_sig": "0" * 128})
        if signature == "zeros"
        else as_live_class(li_escrow, reward)
    )
    before = built.marker()

    # When / Then
    assert refusal_code(lambda: built.escrow.reward_finalize(forged, built.facts)) == (
        "ORIGIN_AUTHORITY"
    )
    assert built.marker() == before


def test_an_accepted_shadow_reward_is_one_origin_of_one_round_budget_owned_by_the_reserved_coldkey(
    li_escrow: ModuleType, real_reward: Real
) -> None:
    # Given / When
    built = engine(li_escrow, real_reward)
    binding = built.reserve()
    built.accept(binding)

    # Then: exactly the origin the ledger derived, pending until the reserved maturity
    origins = issued_origins(built)
    assert [tuple(row) for row in origins] == [
        (built.origin(binding), UNIT, binding.mature_at, binding.trial_epoch, binding.hotkey)
    ]
    units = built.escrow.db.execute(
        "SELECT origin,owner,bucket,ref,units FROM escrow_units"
    ).fetchall()
    assert [tuple(row) for row in units] == [
        (built.origin(binding), binding.coldkey, "reward_pending", "", UNIT)
    ]
    assert binding.coldkey == li_escrow.COLD.ss58


# --- SHD-13: a reopened ledger verifies every shadow row -------------------------------------

PERSISTED_FAULTS = {
    "origin of another reservation": (
        "UPDATE escrow_shadow_finalized SET origin=?",
        ("ee" * 32,),
        "SHADOW_PERSISTED_BINDING",
    ),
    "finalize hash": (
        "UPDATE escrow_shadow_finalized SET finalize_hash=?",
        ("ee" * 32,),
        "SHADOW_PERSISTED_BINDING",
    ),
    "reservation hash": (
        "UPDATE escrow_shadow_finalized SET reservation_hash=?",
        ("ee" * 32,),
        "SHADOW_PERSISTED_BINDING",
    ),
    "trial epoch": (
        "UPDATE escrow_shadow_finalized SET trial_epoch=?",
        (5,),
        "SHADOW_PERSISTED_BINDING",
    ),
    "ordinal gap": ("UPDATE escrow_shadow_finalized SET ordinal=?", (1,), "SHADOW_FINALIZE_ORDER"),
    "reservation not accepted": (
        "UPDATE records_v2 SET data=json_set(data,'$.accepted',json('null')) "
        "WHERE kind='shadow-reservation'",
        (),
        "SHADOW_PERSISTED_BINDING",
    ),
    "stored receipt": (
        "UPDATE records_v2 SET data=json_set(data,'$.accepted.event_hash',?) "
        "WHERE kind='shadow-reservation'",
        ("ee" * 32,),
        "SHADOW_PERSISTED_RECEIPT",
    ),
    "reservation missing": (
        "DELETE FROM records_v2 WHERE kind='shadow-reservation'",
        (),
        "SHADOW_RESERVATION_REQUIRED",
    ),
}


def test_a_reopened_ledger_returns_the_stored_receipt_and_does_not_issue_again(
    li_escrow: ModuleType, real_reward: Real, tmp_path: Path
) -> None:
    # Given: an accepted reward whose reply was lost, and a process restart
    path = tmp_path / "ledger.db"
    built = file_engine(li_escrow, real_reward, path)
    signed = built.reward(built.reserve())
    receipt = built.escrow.reward_finalize(signed, built.facts)
    built.escrow.db.close()

    # When: the ledger is rebuilt on the same file and the reward is delivered again
    reopened = file_engine(li_escrow, real_reward, path)
    again = reopened.escrow.reward_finalize(resign_shadow(li_escrow, signed), reopened.facts)

    # Then
    assert again == receipt
    assert reopened.events("SHADOW_REWARD_FINALIZE") == 1
    assert reopened.escrow.balances().issued == UNIT
    reopened.escrow.verify()


@pytest.mark.parametrize("fault", PERSISTED_FAULTS)
def test_reopening_a_ledger_checks_every_credited_shadow_row_against_its_reservation(
    li_escrow: ModuleType, real_reward: Real, tmp_path: Path, fault: str
) -> None:
    # Given: a credited reward, and one stored row that no longer agrees with the rest
    path = tmp_path / "ledger.db"
    built = file_engine(li_escrow, real_reward, path)
    built.accept(built.reserve())
    statement, arguments, code = PERSISTED_FAULTS[fault]
    built.escrow.db.execute(statement, arguments)
    built.escrow.db.close()

    # When / Then: the next process refuses to start on it, with the code of that row
    assert refusal_code(lambda: file_engine(li_escrow, real_reward, path)) == code


# --- SHD-15: the issuance cap counts reserved-but-unissued rounds ----------------------------


def test_the_cap_holds_pending_reservations_against_live_issuance_and_new_reservations(
    li_escrow: ModuleType, real_reward: Real
) -> None:
    # Given: room for two rounds; one shadow round credited, the next one reserved
    built = engine(li_escrow, real_reward, awards=2)
    built.accept(built.reserve(0))
    pending = built.reserve(1)
    assert built.escrow.balances().issued == UNIT

    # When / Then: the live round may not take the round the reservation holds
    before = built.marker()
    assert refusal_code(lambda: built.escrow.reward_finalize(built.live, built.evidence)) == (
        "TOTAL_ISSUANCE_BUDGET"
    )
    assert built.marker() == before

    # When: the reservation is credited, the cap is full for everyone
    built.accept(pending)
    assert built.escrow.balances().issued == 2 * UNIT
    assert refusal_code(lambda: built.escrow.reward_finalize(built.live, built.evidence)) == (
        "TOTAL_ISSUANCE_BUDGET"
    )
    assert refusal_code(lambda: built.reserve(2)) == "TOTAL_ISSUANCE_BUDGET"
    assert built.escrow.balances().issued == 2 * UNIT and built.escrow.balances().conserved()


# --- SHD-16: shadow and live are separate domains --------------------------------------------


def test_a_reward_is_refused_with_the_other_domains_evidence_whichever_way_round(
    li_escrow: ModuleType, real_reward: Real
) -> None:
    # Given: one reservation, and the live reward of the same Finalize
    built = engine(li_escrow, real_reward)
    shadow = built.reward(built.reserve())
    before = built.marker()

    # When / Then: shadow evidence cannot back the live reward, live evidence cannot back the
    # shadow one, and neither attempt moves anything
    assert refusal_code(lambda: built.escrow.reward_finalize(built.live, built.facts)) == (
        "REWARD_DOMAIN"
    )
    assert refusal_code(lambda: built.escrow.reward_finalize(shadow, built.evidence)) == (
        "REWARD_DOMAIN"
    )
    assert built.marker() == before


# --- SHD-17: shadow origins are collateral, never emission -----------------------------------


def test_a_matured_shadow_origin_is_never_payable_while_a_live_origin_of_the_same_hotkey_is(
    li_escrow: ModuleType, real_reward: Real
) -> None:
    # Given: a shadow and a live reward of one hotkey, both matured and spendable
    built = engine(li_escrow, real_reward)
    binding = built.reserve()
    built.accept(binding)
    built.escrow.reward_finalize(built.live, built.evidence)
    built.escrow.mature("44" * 32, (built.origin(binding),), MATURE_AT)
    mature(built.escrow, built.live)
    assert built.escrow.balances().available == 2 * UNIT

    # When
    payable = built.escrow.payable(MATURE_AT)

    # Then: only the live origin is emission; the shadow units stay collateral
    assert [(row["origin"], row["hotkey"], row["units"]) for row in payable] == [
        (built.live.origin_ids[0], li_escrow.HOT.ss58, UNIT)
    ]


# --- SHD-18: maturity of a shadow origin is gated by its settlement --------------------------


def clean(**changes: Any) -> Settlement:
    base: dict[str, Any] = {
        "finality_hash": "",
        "closed_dispute_root": H,
        "release_beacon": MATURE_AT,
        "unresolved": False,
        "outcome": "MATCH",
    }
    return Settlement.model_validate({**base, **changes})


SETTLEMENT_FAULTS = {
    "unresolved dispute": ({"unresolved": True}, MATURE_AT, "DISPUTED_OR_UNVESTED"),
    "fraud outcome": ({"outcome": "FRAUD"}, MATURE_AT, "DISPUTED_OR_UNVESTED"),
    "another finality": ({"finality_hash": "ee" * 32}, MATURE_AT, "DISPUTED_OR_UNVESTED"),
    "release after now": ({"release_beacon": MATURE_AT + 1}, MATURE_AT, "DISPUTED_OR_UNVESTED"),
    "before the reserved maturity": ({}, MATURE_AT - 1, "IMMATURE_OR_UNKNOWN_ORIGIN"),
}


@pytest.mark.parametrize("fault", SETTLEMENT_FAULTS)
def test_shadow_units_stay_pending_unless_the_shadow_settlement_is_clean_and_due(
    li_escrow: ModuleType, real_reward: Real, fault: str
) -> None:
    # Given: a credited shadow origin and a settlement that is wrong in one way
    built = engine(li_escrow, real_reward)
    binding = built.reserve()
    built.accept(binding)
    changes, now, code = SETTLEMENT_FAULTS[fault]
    finality = built.live.finalize_hash
    built.escrow.shadow_settlement = lambda _origin: clean(**{"finality_hash": finality, **changes})
    origin = built.origin(binding)
    before = built.marker()

    # When / Then: the units are neither spendable nor burned
    assert refusal_code(lambda: built.escrow.mature("44" * 32, (origin,), now)) == code
    assert built.marker() == before
    assert built.escrow.balances().reward_pending == UNIT

    # And: the same origin matures once the settlement is clean (the refusals were the reason)
    built.escrow.shadow_settlement = lambda _origin: clean(finality_hash=finality)
    built.escrow.mature("55" * 32, (origin,), MATURE_AT)
    balances = built.escrow.balances()
    assert (balances.available, balances.reward_pending) == (UNIT, 0) and balances.conserved()


def test_a_ledger_without_a_shadow_settlement_matures_nothing(
    li_escrow: ModuleType, real_reward: Real
) -> None:
    # Given: a credited shadow origin and a ledger built without the shadow settlement
    built = engine(li_escrow, real_reward)
    binding = built.reserve()
    built.accept(binding)
    built.escrow.shadow_settlement = None
    before = built.marker()

    # When / Then: the live settlement is never a substitute
    assert refusal_code(
        lambda: built.escrow.mature("44" * 32, (built.origin(binding),), MATURE_AT)
    ) == ("SHADOW_AUTHORITY_UNAVAILABLE")
    assert built.marker() == before
