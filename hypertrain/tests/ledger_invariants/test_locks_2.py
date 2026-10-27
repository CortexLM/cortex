"""Admission standing at the service API: eligibility, graduation, rotation, recovery, join.

The package `locks`, second half. Everything here drives `Admission` on a bare ledger, as
tests/challenge/test_admission_v2.py does; the route-level and settlement tests are in test_locks.
"""

from __future__ import annotations

from collections.abc import Callable
from dataclasses import replace
from pathlib import Path

import pytest

from hypertrain.challenge.admission_store import AdmissionError
from hypertrain.ledger.escrow_v2 import digest
from hypertrain.miner.admission import sign_join, sign_rotation
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages_v2 import (
    EconomicsPolicyV2,
    HardwareHint,
    RotateRequest,
    RunManifestV2,
)

from .support_locks import STAKE, Lab


def code(call: Callable[[], object]) -> str:
    """The code of the AdmissionError `call` raises; any other outcome fails the test."""
    try:
        call()
    except AdmissionError as error:
        return error.code
    except Exception as error:
        raise AssertionError(f"expected an AdmissionError, got {type(error).__name__}") from error
    raise AssertionError("expected an AdmissionError, nothing was raised")


def snapshot(lab: Lab) -> tuple[tuple[object, ...], ...]:
    """Every admission table as one comparable value."""
    db = lab.a.store.db
    return tuple(
        tuple(tuple(row) for row in db.execute(f"SELECT * FROM {table} ORDER BY 1,2"))  # noqa: S608
        for table in (
            "admissions_v2",
            "admission_history",
            "admission_transitions",
            "admission_pending",
            "admission_outbox",
        )
    )


# --- LCK-01, LCK-14: eligibility, the gate every reward, roster entry and accept reads --------


@pytest.mark.parametrize(
    ("fault", "eligible"),
    [
        pytest.param(None, True, id="funded-active"),
        pytest.param("probation-four-clean", True, id="funded-live-probation"),
        pytest.param("no-stake", False, id="no-stake"),
        pytest.param("short-stake", False, id="one-unit-short"),
        pytest.param("pending-dispute", False, id="pending-dispute"),
        pytest.param("screen-lapsed", False, id="screen-lapsed"),
        pytest.param("probation-three-clean", False, id="probation-below-four"),
        pytest.param("applied", False, id="applied"),
        pytest.param("suspended", False, id="suspended"),
        pytest.param("banned", False, id="banned"),
        pytest.param("lock-in-flight", False, id="lock-not-yet-applied"),
        pytest.param("bound-not-conservative", False, id="bound-not-conservative"),
    ],
)
def test_eligibility_needs_current_stake_standing_and_screen(
    tmp_path: Path, fault: str | None, eligible: bool
) -> None:
    # Given: an ACTIVE admission holding exactly the required stake, and one thing changed
    lab = Lab(tmp_path)
    admission = lab.join()
    if fault != "no-stake":
        lab.stake(admission, STAKE - 1 if fault == "short-stake" else STAKE)
    lab.live(admission)
    edits: dict[str, dict[str, object]] = {
        "probation-four-clean": {"state": "PROBATION", "clean_count": 4},
        "probation-three-clean": {"state": "PROBATION", "clean_count": 3},
        "pending-dispute": {"pending_dispute": 1},
        "screen-lapsed": {"screen_until": 4},
        "applied": {"state": "APPLIED"},
        "suspended": {"state": "SUSPENDED"},
        "banned": {"state": "BANNED"},
    }
    if fault in edits:
        lab.set(admission, **edits[fault])
    if fault == "lock-in-flight":
        lab.enqueue(admission, 1, "23" * 32)
    if fault == "bound-not-conservative":
        lab.a.conservative_bound = lambda: False
    # When
    status = lab.a.status(lab.A.HOT.ss58, now=5)
    # Then
    assert (status.eligible, status.shadow_only) == (eligible, not eligible)
    assert status.funding.locked_units == {"no-stake": 0, "short-stake": STAKE - 1}.get(
        fault, STAKE
    )


@pytest.mark.parametrize("role", ["coordinator", "auditor", "referee", "run-owner"])
def test_no_role_key_is_exempt_from_the_stake(tmp_path: Path, role: str) -> None:
    # Given: an ACTIVE admission on a key the run itself pins to a role
    lab = Lab(tmp_path)
    key = {
        "coordinator": lab.A.COORD,
        "auditor": lab.A.fixtures.AUDITOR,
        "referee": Keypair(b"\x71" * 32),
        "run-owner": Keypair(b"\x77" * 32),
    }[role]
    admission = lab.join(hot=key)
    lab.live(admission)

    def standing() -> tuple[bool, int]:
        status = lab.a.status(key.ss58, now=5)
        return status.eligible, status.funding.locked_units

    # When / Then: it is as ineligible as anyone with no stake or too little, and eligible at
    # exactly the required stake
    assert standing() == (False, 0)
    lab.stake(admission, STAKE - 1)
    assert standing() == (False, STAKE - 1)
    lab.stake(admission, 1, operation="23" * 32)
    assert standing() == (True, STAKE)


def test_the_stake_gate_is_the_policy_a_run_that_requires_nothing_gates_nothing(
    tmp_path: Path,
) -> None:
    # Given: the same admission under the fixture policy and under a stand-in policy, owned by
    # this test, that sets the three stake terms to zero (the condition of LCK-01 broken)
    strict = Lab(tmp_path / "strict")
    policy = EconomicsPolicyV2.model_validate(
        {
            **strict.s.policy.model_dump(),
            "S_min_units": 0,
            "R_collectible_units": 0,
            "G_max_units": 0,
        }
    )
    body = strict.s.manifest.body()
    body["network"]["economics_policy_hash"] = digest(policy)
    free = Lab(
        tmp_path / "free",
        setup=replace(strict.s, manifest=RunManifestV2.model_validate(body), policy=policy),
    )
    eligible = {}
    for name, lab in (("strict", strict), ("free", free)):
        lab.live(lab.join())
        eligible[name] = lab.a.status(lab.A.HOT.ss58, now=5).eligible
    # When / Then: only the policy decides whether an unstaked ACTIVE admission is eligible
    assert eligible == {"strict": False, "free": True}


# --- LCK-03: ACTIVE only through graduate(), funded, screened and canary-complete -------------


@pytest.mark.parametrize(
    "fault",
    [
        pytest.param(None, id="all-conditions-held"),
        pytest.param("stake-one-short", id="stake-one-short"),
        pytest.param("eleven-clean-finalizations", id="eleven-clean-finalizations"),
        pytest.param("canary-block-missing", id="canary-block-missing"),
        pytest.param("not-in-probation", id="not-in-probation"),
        pytest.param("screen-lapsed", id="screen-lapsed"),
    ],
)
def test_graduation_needs_funding_clean_finalizations_and_every_canary_block(
    tmp_path: Path, fault: str | None
) -> None:
    # Given: a PROBATION admission one call away from ACTIVE, and one condition broken
    lab = Lab(tmp_path)
    admission = lab.join()
    lab.stake(admission, STAKE - 1 if fault == "stake-one-short" else STAKE)
    lab.set(
        admission,
        state="APPLIED" if fault == "not-in-probation" else "PROBATION",
        clean_count=11 if fault == "eleven-clean-finalizations" else 12,
        canary_blocks="1,2" if fault == "canary-block-missing" else "1,2,3",
        screen_until=4 if fault == "screen-lapsed" else 1_000_000,
    )
    # When
    status = lab.a.graduate(lab.A.HOT.ss58, now=5)
    # Then: ACTIVE with its transition on record, or nothing at all
    graduated = [
        tuple(row)
        for row in lab.a.store.db.execute(
            "SELECT state,reason FROM admission_transitions WHERE admission_id=?", (admission,)
        )
    ]
    if fault is None:
        assert (status.record.state, status.eligible) == ("ACTIVE", True)
        assert graduated == [("ACTIVE", "GRADUATED_FULL_REPLAY")]
    else:
        assert status.record.state == ("APPLIED" if fault == "not-in-probation" else "PROBATION")
        assert graduated == []


# --- LCK-16, LCK-23: rotation carries the standing, the history keeps both keys ---------------


def rotate(lab: Lab, old: Keypair, new: Keypair, operation: str) -> None:
    request = RotateRequest(hotkey=old.ss58, new_hotkey=new.ss58, operation_id=operation)
    lab.a.rotate(canonicalize(sign_rotation(lab.A.COLD, lab.a.escrow.run_id, request, 200)), now=2)


def test_rotation_keeps_the_standing_and_the_history_of_both_keys(tmp_path: Path) -> None:
    # Given: a suspended admission with a stake, two strikes and a pending reference
    lab = Lab(tmp_path)
    first, second, third = lab.A.HOT, lab.A.OTHER, Keypair(b"\x0a" * 32)
    admission = lab.join()
    lab.stake(admission, STAKE)
    lab.a.suspend(
        first.ss58,
        reason="pending",
        now=2,
        pending_dispute=True,
        dispute_id="33" * 32,
        evidence_hash="44" * 32,
    )
    lab.set(admission, strikes=2, clean_count=5)
    before = lab.a.store.record(first.ss58)
    # When: the coldkey rotates the admission twice
    rotate(lab, first, second, "77" * 32)
    rotate(lab, second, third, "78" * 32)
    # Then: it is the same admission with the same standing under the newest key
    after = lab.a.store.record(third.ss58)
    assert (after.admission_id, after.coldkey, after.state, after.reason) == (
        before.admission_id,
        before.coldkey,
        before.state,
        before.reason,
    )
    assert (after.clean_count, after.strikes, after.pending_dispute) == (5, 2, True)
    assert lab.a.status(third.ss58, now=2).funding.locked_units == STAKE
    pending = lab.a.store.db.execute("SELECT dispute_id,resolved FROM admission_pending").fetchall()
    assert [tuple(row) for row in pending] == [("33" * 32, 0)]
    # every key it ever held stays in the history, so none can apply or be rotated to again
    held = {
        row[0]: row[1]
        for row in lab.a.store.db.execute("SELECT hotkey,admission_id FROM admission_history")
    }
    assert held == {key.ss58: admission for key in (first, second, third)}
    assert code(lambda: rotate(lab, third, first, "79" * 32)) == "NEW_HOTKEY_HISTORY"
    assert code(lambda: lab.join(hot=second, cold=lab.A.OTHER, request="55" * 32)) == (
        "HOTKEY_HISTORY_EXISTS"
    )
    assert lab.a.store.record(third.ss58).admission_id == admission


def test_a_second_application_is_refused_for_a_known_hotkey_or_an_unsettled_coldkey(
    tmp_path: Path,
) -> None:
    # Given: one admission, still APPLIED
    lab = Lab(tmp_path)
    admission = lab.join()
    other = lab.A.OTHER

    def applications() -> int:
        return int(lab.a.store.db.execute("SELECT COUNT(*) FROM admissions_v2").fetchone()[0])

    # When / Then: its hotkey cannot apply again under another coldkey
    before = snapshot(lab)
    assert code(lambda: lab.join(hot=lab.A.HOT, cold=other, request="22" * 32)) == (
        "HOTKEY_HISTORY_EXISTS"
    )
    # and its coldkey cannot hold a second admission while this one is not ACTIVE
    assert code(lambda: lab.join(hot=other, cold=lab.A.COLD, request="33" * 32)) == (
        "COLDKEY_PROBATION_EXISTS"
    )
    assert snapshot(lab) == before and applications() == 1
    # once it is ACTIVE the coldkey may admit a further hotkey
    lab.live(admission)
    lab.join(hot=other, cold=lab.A.COLD, request="33" * 32)
    assert applications() == 2


def test_a_join_signed_for_another_policy_is_refused_and_writes_nothing(tmp_path: Path) -> None:
    # Given: an admission policy the join request does not name
    lab = Lab(tmp_path)
    signed = sign_join(
        lab.A.HOT,
        lab.A.COLD,
        run_id=lab.a.escrow.run_id,
        request_id=lab.A.H,
        expires_beacon=10000,
        policy_hash="55" * 32,
        hardware_hint=HardwareHint(device_name="advisory", device_count=1, driver="cpu-test"),
    )
    before = snapshot(lab)
    # When / Then
    refusal = code(lambda: lab.a.join(canonicalize(signed.body()), now=1, ip_prefix="loopback"))
    assert refusal == "POLICY_CHANGED" and snapshot(lab) == before


# --- LCK-22, AD-08, DIB-09: recovery from a pending dispute, once, from a nonfraud record -----

PENDING, EVIDENCE = "33" * 32, "44" * 32


def pending_miner(lab: Lab, **settlement: object) -> str:
    """A suspended admission with the pending reference PENDING, and the settlement record for
    it that `settlement` overrides on top of an accepted, linked, nonfraud MATCH at round 2."""
    admission = lab.join()
    lab.a.suspend(
        lab.A.HOT.ss58,
        reason="pending",
        now=2,
        pending_dispute=True,
        dispute_id=PENDING,
        evidence_hash=EVIDENCE,
    )
    lab.set(admission, strikes=2)
    fields: dict[str, object] = {
        "admission_id": admission,
        "coldkey": lab.A.COLD.ss58,
        "dispute_id": PENDING,
        "evidence_hash": EVIDENCE,
        "release_beacon": 2,
    }
    lab.settle(**{**fields, **settlement})
    return admission


@pytest.mark.parametrize(
    ("record", "recovers"),
    [
        pytest.param({}, True, id="match-at-its-release-round"),
        pytest.param({"outcome": "INFRASTRUCTURE"}, True, id="infrastructure-closure"),
        pytest.param({"outcome": "FRAUD"}, False, id="fraud"),
        pytest.param({"unresolved": True}, False, id="unresolved"),
        pytest.param({"release_beacon": 3}, False, id="before-the-release-round"),
    ],
)
def test_recovery_needs_a_resolved_nonfraud_record_at_or_past_its_release_round(
    tmp_path: Path, record: dict[str, object], recovers: bool
) -> None:
    # Given
    lab = Lab(tmp_path)
    admission = pending_miner(lab, **record)
    hotkey = lab.A.HOT.ss58
    before = snapshot(lab)
    # When / Then: the record decides, and a refusal changes nothing
    if not recovers:
        assert code(lambda: lab.a.resolve_pending(hotkey, PENDING, now=2)) == "PENDING_SETTLEMENT"
        assert snapshot(lab) == before
        assert lab.a.store.record(hotkey).pending_dispute
        return
    lab.a.resolve_pending(hotkey, PENDING, now=2)
    lifted = lab.a.store.record(hotkey)
    assert (lifted.state, lifted.pending_dispute, lifted.clean_count, lifted.strikes) == (
        "PROBATION",
        False,
        0,
        2,
    )
    assert lifted.admission_id == admission
    # and the same incident never lifts it twice
    after = snapshot(lab)
    assert code(lambda: lab.a.resolve_pending(hotkey, PENDING, now=2)) == (
        "PENDING_SETTLEMENT_REFERENCE"
    )
    assert snapshot(lab) == after


# --- AD-09 (neighbour), DIB-09: a BANNED admission has no way back ------------------------------


def test_a_banned_admission_cannot_be_resumed_suspended_challenged_or_graduated_or_reapplied(
    tmp_path: Path,
) -> None:
    # Given: an admission BANNED for proven fraud (the state the missing wiring would write)
    lab = Lab(tmp_path)
    admission = lab.join()
    lab.stake(admission, STAKE)
    lab.set(admission, state="BANNED", reason="FINALIZED_TRAINING_FRAUD", clean_count=12)
    hotkey = lab.A.HOT.ss58
    before = snapshot(lab)
    # When / Then: every door the state machine has refuses it and the row is untouched
    assert code(lambda: lab.a.resume(hotkey, now=3)) == "RESUME_STATE"
    assert code(lambda: lab.a.suspend(hotkey, reason="again", now=3)) == "TERMINAL_BANNED"
    assert code(lambda: lab.a.challenge(admission, now=3)) == "CHALLENGE_STATE"
    assert code(lambda: lab.a.resolve_pending(hotkey, PENDING, now=3)) == (
        "PENDING_SETTLEMENT_REFERENCE"
    )
    assert lab.a.graduate(hotkey, now=3).record.state == "BANNED"
    assert not lab.a.status(hotkey, now=3).eligible
    # the coldkey cannot start over with another key, nor the key under another coldkey
    assert code(lambda: lab.join(hot=lab.A.OTHER, cold=lab.A.COLD, request="22" * 32)) == (
        "COLDKEY_PROBATION_EXISTS"
    )
    assert code(lambda: lab.join(hot=lab.A.HOT, cold=lab.A.OTHER, request="23" * 32)) == (
        "HOTKEY_HISTORY_EXISTS"
    )
    assert snapshot(lab) == before
