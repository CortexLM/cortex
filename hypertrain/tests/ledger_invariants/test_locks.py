"""Admission stake, its settlement and the standing that eligibility reads (package `locks`).

Defences the service must hold, each stated as "the system refuses X and its state is unchanged":
a reward only to a currently funded identity, a stake that only a settlement bound to it can
move, a lost fraud contest that costs the admission and not just the contest bond, a suspended
miner with a route back. Holes are strict xfails whose body asserts the desired defence; a
sibling test is the control that proves the scenario reaches the point under test. Everything
runs in-process on the v2 `network` fixture and the real `ChallengeStore._settlement_v2`.
"""

from __future__ import annotations

from collections.abc import Iterator
from pathlib import Path

import httpx
import pytest

from hypertrain.challenge.store import ChallengeError
from hypertrain.ledger.escrow_v2 import Settlement
from hypertrain.protocol.messages import Finalize
from hypertrain.protocol.messages_v2 import EscrowLock

from .support import H, service_world
from .support_admission_http import precondition
from .support_locks import (
    BOND,
    STAKE,
    Contest,
    Miner,
    Rig,
    event_count,
    ident,
    seed_live_origin,
    units_of,
    vested,
)


@pytest.fixture
def rig(tmp_path: Path) -> Iterator[Rig]:
    with service_world(tmp_path) as world:
        yield Rig(world)


def close_without_settling(rig: Rig, dispute_id: str, *, at: int) -> None:
    """The dispute is closed without a loser at round `at`; its bond lock is not yet settled."""
    with rig.world.store._tx():
        rig.disputes.pause(dispute_id)
        rig.disputes.exclude(dispute_id, now=at)


# --- the contest bond settles through the real callback: the control of the stake tests ------


def test_the_owner_releases_a_contest_bond_only_with_the_terms_of_its_own_closed_dispute(
    rig: Rig,
) -> None:
    # Given: a funded contest, its bond held in dispute_locked
    miner = rig.join(0)
    contest = rig.open_contest(miner)
    before = units_of(rig.escrow, miner.coldkey)
    assert before["dispute_locked"] == BOND
    opened = rig.disputes.settlement(contest.dispute_id)

    def refused(finality: str, closed: str, attempt: str) -> None:
        digest = rig.world.db_digest()
        response = rig.release(miner, contest.bond, finality, closed, attempt=attempt)
        assert (response.status_code, response.json()["detail"]) == (409, "DISPUTED_OR_UNVESTED")
        assert rig.world.db_digest() == digest

    # When / Then: no release while the dispute is open, and none before its release round
    refused(opened.finality_hash, opened.closed_dispute_root, "open")
    close_without_settling(rig, contest.dispute_id, at=rig.net.now + 5)
    closed = rig.disputes.settlement(contest.dispute_id)
    assert (closed.unresolved, closed.outcome, closed.release_beacon) == (False, "MATCH", 6)
    refused(closed.finality_hash, closed.closed_dispute_root, "early")
    rig.net.push(closed.release_beacon)
    # and none with the terms of anything but this dispute
    refused(H, H, "foreign")
    refused(closed.finality_hash, H, "root")
    refused(H, closed.closed_dispute_root, "finality")
    response = rig.release(miner, contest.bond, closed.finality_hash, closed.closed_dispute_root)
    assert response.status_code == 200, response.text
    after = units_of(rig.escrow, miner.coldkey)
    assert (after["dispute_locked"], after["available"]) == (0, before["available"] + BOND)
    assert rig.escrow.balances().conserved()


# --- ESC-10, ES-05, LCK-04: a release is bound to the lock it names ---------------------------


def refused_release(rig: Rig, miner: Miner, stake: EscrowLock, finality: str, closed: str) -> None:
    """The owner's release of `stake` with these terms is refused, nothing changes and the stake
    still backs the live admission."""
    digest, locked = rig.world.db_digest(), units_of(rig.escrow, miner.coldkey)
    assert rig.status(miner).eligible
    response = rig.release(miner, stake, finality, closed)
    assert (response.status_code, response.json()["detail"]) == (409, "DISPUTED_OR_UNVESTED")
    assert rig.world.db_digest() == digest
    assert units_of(rig.escrow, miner.coldkey) == locked
    assert rig.status(miner).eligible


@pytest.mark.parametrize("named_after", ["a closed dispute", "a settled reward origin"])
def test_a_live_admission_keeps_its_stake_when_the_lock_is_named_after_a_settled_object(
    rig: Rig, named_after: str
) -> None:
    # Given: an object the service settled as MATCH, with the beacon past its release round
    miner = rig.join(0)
    settled: Settlement
    if named_after == "a closed dispute":
        contest = rig.open_contest(miner)
        rig.net.push(rig.disputes.get(contest.dispute_id).absolute_deadline + 1)
        object_id, settled = contest.dispute_id, rig.disputes.settlement(contest.dispute_id)
    else:
        object_id = seed_live_origin(rig, miner)
        rig.net.push(5)
        settled = rig.world.store._settlement_v2(rig.run_id, object_id)
        assert vested(rig, object_id) == 500
    assert (settled.unresolved, settled.outcome) == (False, "MATCH")
    assert rig.net.now >= settled.release_beacon
    # and an ACTIVE admission whose only stake is a lock the owner named with that object's id
    rig.activate(miner)
    stake = rig.lock_stake(miner, operation=object_id)
    # When / Then: the terms of that object do not release it
    refused_release(rig, miner, stake, settled.finality_hash, settled.closed_dispute_root)


def test_the_stake_of_a_live_admission_is_released_only_once_it_is_no_longer_live(
    rig: Rig,
) -> None:
    # Given: an ACTIVE, funded admission and the terms the service derives for its stake
    miner = rig.join(0)
    stake = rig.lock_stake(miner)
    rig.activate(miner)
    live = rig.bond_entry(miner, stake.operation_id)
    # When / Then: the service does not offer the release, and refuses it with its own terms
    assert live["releasable"] is False
    refused_release(rig, miner, stake, live["finality_hash"], live["closed_dispute_root"])
    # and once its work screen has lapsed the same stake is the owner's to take back
    with rig.world.store._tx():
        rig.world.store._db.execute(
            "UPDATE admissions_v2 SET screen_until=? WHERE admission_id=?",
            (rig.net.now - 1, miner.admission_id),
        )
    quiet, before = rig.bond_entry(miner, stake.operation_id), units_of(rig.escrow, miner.coldkey)
    assert quiet["releasable"] is True
    response = rig.release(miner, stake, quiet["finality_hash"], quiet["closed_dispute_root"])
    assert response.status_code == 200, response.text
    after = units_of(rig.escrow, miner.coldkey)
    assert (after["admission_locked"], after["available"]) == (0, before["available"] + STAKE)
    assert rig.escrow.balances().conserved()


# --- LCK-06, DIB-08, DIA-23, AD-09, BND-19, ES-04: proven fraud costs the admission -----------
# The ban and the burn of the stake of a miner who lost its contest are pinned over HTTP by
# tests/challenge/test_admission_hardening_v2.py (a referee verdict against the miner).


def lose_the_contest(rig: Rig) -> tuple[Miner, EscrowLock, Contest]:
    """A staked miner opens a contest and the referee rules FRAUD against it; the contest bond
    is burned by the real settle_lock."""
    miner = rig.join(0)
    stake = rig.lock_stake(miner)
    contest = rig.open_contest(miner)
    response = rig.refereed(contest.dispute_id, "FRAUD", miner.hotkey)
    assert response.status_code == 200, response.text
    assert rig.disputes.settlement(contest.dispute_id).outcome == "FRAUD"
    held = units_of(rig.escrow, miner.coldkey)
    assert held["dispute_locked"] == 0 and held["burned"] >= BOND
    return miner, stake, contest


# --- LCK-09, DIB-07, CRD-22: a reward does not vest under a dispute ---------------------------


@pytest.mark.parametrize(
    ("record", "vests_at", "never"),
    [
        pytest.param({}, 5, False, id="match-at-its-round"),
        pytest.param({"release_beacon": 9}, 9, False, id="match-held-to-the-release-round"),
        pytest.param({"unresolved": True}, 0, True, id="unresolved"),
        pytest.param({"outcome": "FRAUD"}, 0, True, id="fraud"),
        pytest.param({"finality_hash": H}, 0, True, id="another-finalization"),
    ],
)
def test_a_reward_origin_vests_only_as_its_settlement_record_allows(
    rig: Rig, record: dict[str, object], vests_at: int, never: bool
) -> None:
    # Given: an origin issued by round 0, maturing at round 5, with the record under test
    miner = rig.join(0)
    origin = seed_live_origin(rig, miner, **record)
    # When / Then: the beacon route vests it at the first round the record allows, not before
    rig.net.push(vests_at - 1 if vests_at else 50)
    assert vested(rig, origin) == 0
    if never:
        rig.net.push(500)
        assert vested(rig, origin) == 0
    else:
        rig.net.push(vests_at)
        assert vested(rig, origin) == 500
    assert rig.escrow.balances().conserved()


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="LCK-09+DIB-07: a reward origin does not vest while a dispute naming its miner and "
    "round is unresolved, but the origin's settlement is a record written at finalize that no "
    "dispute ever touches [fix-lot: none (no design yet)]",
)
def test_a_reward_origin_does_not_vest_while_a_dispute_on_its_round_is_open(rig: Rig) -> None:
    # Given: round 0 issued the staked miner an origin maturing at round 5, then the miner's
    # contest of round 0 is opened and stays unresolved
    miner = rig.join(0)
    rig.lock_stake(miner)
    origin = seed_live_origin(rig, miner)
    contest = rig.open_contest(miner)
    # When: the beacon passes the origin's release round, inside the dispute's horizon
    rig.net.push(10)
    assert rig.disputes.get(contest.dispute_id).resolution is None
    # Then: the units are still pending
    assert vested(rig, origin) == 0


def test_fraud_proven_against_a_staked_miner_burns_the_reward_origin_it_never_vested(
    rig: Rig,
) -> None:
    # Given: round 0 issued the staked miner an origin maturing at round 5, then the miner's
    # contest of round 0 is lost: the referee rules FRAUD against it
    miner = rig.join(0)
    rig.lock_stake(miner)
    origin = seed_live_origin(rig, miner)
    contest = rig.open_contest(miner)
    assert rig.refereed(contest.dispute_id, "FRAUD", miner.hotkey).status_code == 200
    # When: the beacon passes the origin's release round, inside the dispute's horizon
    rig.net.push(10)
    # Then: the origin never vested; its units burned with the contest bond and the stake
    assert vested(rig, origin) == 0
    held = units_of(rig.escrow, miner.coldkey)
    assert (held["reward_pending"], held["burned"]) == (0, BOND + STAKE + 500)
    assert rig.escrow.balances().conserved()


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="CRD-22+LCK-09: fraud proven against a miner burns the reward origin it never vested, "
    "but a conviction that follows a bond-only seizure finds no admission lock to slash, and the "
    "burn of the unvested reward is part of slashing a lock (ADR 0010 Decisions for Forge 4): "
    "the miner is banned and its origin still vests. Closing it needs a ledger operation that "
    "burns reward_pending without a lock; adding it or accepting that the reward survives is a "
    "founder / Principal call [fix-lot: decision (product choice pending)]",
)
def test_fraud_proven_after_the_bond_was_seized_still_burns_the_unvested_reward(rig: Rig) -> None:
    # Given: a staked miner with a reward origin pending, whose bond the service burned for a
    # confirmed audit mismatch
    miner = rig.join(0)
    rig.lock_stake(miner)
    origin = seed_live_origin(rig, miner)
    seized = rig.admission.seize_bond(
        miner.admission_id, reason="AUDIT_MISMATCH", evidence_hash=ident("mismatch"), now=1
    )
    held = units_of(rig.escrow, miner.coldkey)
    precondition(
        (seized, held["admission_locked"], held["reward_pending"]) == (STAKE, 0, 500),
        f"the bond was not burned alone: {seized} {held}",
    )
    # and the miner then loses its contest of that round: the referee rules FRAUD against it
    contest = rig.open_contest(miner)
    ruled = rig.refereed(contest.dispute_id, "FRAUD", miner.hotkey)
    precondition(ruled.status_code == 200, f"the resolution was refused: {ruled.text}")
    precondition(rig.admission.store.record(miner.hotkey).state == "BANNED", "no conviction")
    # When: the beacon passes the release round of the origin
    rig.net.push(10)
    # Then: the origin never vested
    assert vested(rig, origin) == 0


# --- AD-06, AD-08, DIB-09: the route back from a suspension, and its limits -------------------


def recover(rig: Rig, miner: Miner, dispute_id: str) -> httpx.Response:
    """The miner's signed recovery request for the pending dispute `dispute_id`."""
    body = {"w": 0, "commit_hash": dispute_id, "received_round": rig.net.now}
    return rig.net.client.post(
        rig.net.url + "/admission/recover", json=rig.net.signed(miner.hot, "Receipt", body)
    )


def test_a_suspended_miner_recovers_once_from_a_closed_dispute_and_not_before_its_release_round(
    rig: Rig,
) -> None:
    # Given: a contest closed without a loser at round 6; the miner carries two strikes
    miner = rig.join(0)
    contest = rig.open_contest(miner)
    with rig.world.store._tx():
        rig.world.store._db.execute(
            "UPDATE admissions_v2 SET strikes=2 WHERE admission_id=?", (miner.admission_id,)
        )
    close_without_settling(rig, contest.dispute_id, at=rig.net.now + 5)
    suspended = rig.admission.store.record(miner.hotkey)
    assert (suspended.state, suspended.pending_dispute) == ("SUSPENDED", True)
    # When / Then: another reference than the pending one lifts nothing
    digest = rig.world.db_digest()
    elsewhere = recover(rig, miner, ident("another incident"))
    assert (elsewhere.status_code, rig.world.db_digest()) == (403, digest)
    # and the pending one is refused one round early, the miner still suspended
    early = recover(rig, miner, contest.dispute_id)
    assert (early.status_code, early.json()["detail"]) == (409, "PENDING_SETTLEMENT")
    assert rig.world.db_digest() == digest
    # accepted at the release round: a fresh probation that keeps the strikes
    rig.net.push(6)
    response = recover(rig, miner, contest.dispute_id)
    assert response.status_code == 200, response.text
    lifted = rig.admission.store.record(miner.hotkey)
    assert (lifted.state, lifted.pending_dispute, lifted.clean_count, lifted.strikes) == (
        "PROBATION",
        False,
        0,
        2,
    )
    # and never twice for the same incident
    digest = rig.world.db_digest()
    again = recover(rig, miner, contest.dispute_id)
    assert (again.status_code, rig.world.db_digest()) == (403, digest)


def test_a_miner_who_lost_a_fraud_contest_has_no_route_back_and_no_new_application(
    rig: Rig,
) -> None:
    # Given: the referee ruled FRAUD against the miner
    miner, _, contest = lose_the_contest(rig)
    rig.net.push(rig.net.now + 50)
    digest = rig.world.db_digest()
    # When / Then: the recovery route refuses the incident, whatever round it is asked at
    refused = recover(rig, miner, contest.dispute_id)
    assert refused.status_code in (403, 409), refused.text
    # and neither key applies again, the coldkey with another hotkey or the hotkey itself
    other_hotkey = rig.join_request(0, hot=rig.service.HOT[1])
    same_hotkey = rig.join_request(0, hot=miner.hot, request="again")
    assert (other_hotkey.status_code, other_hotkey.json()["detail"]) == (
        409,
        "COLDKEY_PROBATION_EXISTS",
    )
    assert (same_hotkey.status_code, same_hotkey.json()["detail"]) == (
        409,
        "HOTKEY_HISTORY_EXISTS",
    )
    # nothing changed, and the miner is out of every live state
    assert rig.world.db_digest() == digest
    assert rig.admission.store.record(miner.hotkey).state in ("SUSPENDED", "BANNED")
    assert not rig.status(miner).eligible


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="AD-06: a miner suspended by a replay mismatch has a signed route back to PROBATION "
    "(or the docs name that suspension terminal: a pending product decision), but the pending "
    "dispute of a trial mismatch is an id no dispute row or settlement record carries, so the "
    "recovery route can only answer UNACCEPTED_SETTLEMENT "
    "[fix-lot: decision (product choice pending)]",
)
def test_a_miner_suspended_by_a_replay_mismatch_has_a_signed_route_back(rig: Rig) -> None:
    # Given: the state Admission.proof leaves on a trial whose proof differs from the reference:
    # SUSPENDED, a strike and a pending reference derived from the trial, not from a contest
    miner = rig.join(0)
    pending = ident("replay-mismatch pending reference")
    rig.admission.suspend(
        miner.hotkey,
        now=rig.net.now,
        reason="PENDING_REPLAY_MISMATCH",
        pending_dispute=True,
        dispute_id=pending,
        evidence_hash=ident("replay-mismatch evidence"),
    )
    suspended = rig.admission.store.record(miner.hotkey)
    assert (suspended.state, suspended.pending_dispute) == ("SUSPENDED", True)
    rig.net.push(300)
    # When: the miner asks to be let back
    response = recover(rig, miner, pending)
    # Then
    assert response.status_code == 200, response.text
    assert rig.admission.store.record(miner.hotkey).state == "PROBATION"


# --- LCK-01, LCK-02, LCK-14: roster, accept and finalize need current funded eligibility ------

NOT_FUNDED = "roster lacks current funded admission authority"


def cohort(rig: Rig, last: str) -> list[Miner]:
    """Four ACTIVE miners, the first three funded; the last one is as `last` says."""
    miners = [rig.admit(i) for i in range(3)]
    match last:
        case "funded":
            miners.append(rig.admit(3))
        case "underfunded":
            miners.append(rig.admit(3, stake=STAKE - 1))
        case "unfunded":
            miners.append(rig.admit(3, stake=0))
        case "referee-unfunded":
            miners.append(rig.admit(3, stake=0, hot=rig.service.REFEREE))
    return miners


@pytest.mark.parametrize("last", ["underfunded", "unfunded", "referee-unfunded"])
def test_a_round_opens_only_when_every_roster_member_is_currently_funded(
    rig: Rig, last: str
) -> None:
    # Given: four ACTIVE miners; the last one is short of the stake, or has none, or holds the
    # pinned referee key (no identity is exempt)
    miners = cohort(rig, last)
    digest = rig.world.db_digest()
    # When / Then: the opening is refused and nothing is written
    refused = rig.open_round(rig.opening(miners))
    assert (refused.status_code, refused.json()["detail"]) == (403, NOT_FUNDED)
    assert rig.world.db_digest() == digest
    # the member tops its stake up (by a whole lock: a ledger may refuse a lock below its floor)
    # and the very same opening is accepted
    rig.lock_stake(miners[3], units=STAKE, operation=ident("top-up"))
    opened = rig.open_round(rig.opening(miners))
    assert opened.status_code == 200, opened.text


@pytest.mark.parametrize(
    ("field", "forged", "status", "detail"),
    [
        pytest.param("admission_id", H, 403, NOT_FUNDED, id="admission-id"),
        pytest.param("coldkey_group", "5" + "Gs" * 24, 403, NOT_FUNDED, id="coldkey-group"),
        pytest.param("state", "PROBATION", 403, NOT_FUNDED, id="state"),
        pytest.param("q_i", "00000000", 403, NOT_FUNDED, id="quality"),
        pytest.param(
            "eligible_weight", 4194303, 422, "live eligible ceiling must be Q/4", id="weight-below"
        ),
        pytest.param(
            "eligible_weight", 4194305, 422, "live eligible ceiling must be Q/4", id="weight-above"
        ),
    ],
)
def test_a_roster_entry_must_equal_the_admission_it_names(
    rig: Rig, field: str, forged: object, status: int, detail: str
) -> None:
    # Given: four funded ACTIVE miners and an opening whose first entry differs in one field
    miners = cohort(rig, "funded")
    digest = rig.world.db_digest()
    # When / Then: the opening is refused whatever the field, and the honest one is accepted
    refused = rig.open_round(rig.opening(miners, **{field: forged}))
    assert (refused.status_code, refused.json()["detail"]) == (status, detail)
    assert rig.world.db_digest() == digest
    assert rig.open_round(rig.opening(miners)).status_code == 200


@pytest.mark.parametrize("lapse", ["screen", "pending-dispute"])
def test_an_accept_is_refused_when_the_miner_lost_its_standing_after_the_opening(
    rig: Rig, lapse: str
) -> None:
    # Given: an opened round and an assignment window
    miners = cohort(rig, "funded")
    opening = rig.opening(miners)
    assert rig.open_round(opening).status_code == 200
    rig.net.push(opening.d_assign)
    hashes = {m.hotkey: rig.assignment_hash(m) for m in miners}
    # control: a miner in good standing is accepted
    assert rig.accept(miners[1], hashes[miners[1].hotkey]).status_code == 200
    # When: another loses its screen, or is suspended under a dispute
    lost = miners[0]
    if lapse == "screen":
        with rig.world.store._tx():
            rig.world.store._db.execute(
                "UPDATE admissions_v2 SET screen_until=1 WHERE admission_id=?",
                (lost.admission_id,),
            )
    else:
        rig.admission.suspend(
            lost.hotkey,
            now=rig.net.now,
            reason="PENDING_DISPUTE",
            pending_dispute=True,
            dispute_id=ident("pending"),
            evidence_hash=ident("evidence"),
        )
    assert not rig.status(lost).eligible
    digest = rig.world.db_digest()
    refused = rig.accept(lost, hashes[lost.hotkey])
    # Then
    assert (refused.status_code, refused.json()["detail"]) == (
        403,
        "screen or funded admission stale",
    )
    assert rig.world.db_digest() == digest


def test_a_finalize_input_is_refused_for_a_member_who_lost_standing_after_the_opening(
    rig: Rig,
) -> None:
    # Given: an opened round with four funded members and nothing accepted yet
    miners = cohort(rig, "funded")
    assert rig.open_round(rig.opening(miners)).status_code == 200
    store = rig.world.store
    # control: with every member in good standing the check passes and the call stops later,
    # at the first member's missing commit
    with pytest.raises(ChallengeError) as stopped:
        store.verified_inputs_v2(rig.run_id, 0)
    assert "eligible" not in str(stopped.value)
    # When: the first member is suspended under a dispute
    rig.admission.suspend(
        miners[0].hotkey,
        now=rig.net.now,
        reason="PENDING_DISPUTE",
        pending_dispute=True,
        dispute_id=ident("pending"),
        evidence_hash=ident("evidence"),
    )
    # Then: no reward input is built from it
    with pytest.raises(ChallengeError) as refused:
        store.verified_inputs_v2(rig.run_id, 0)
    assert (refused.value.status, str(refused.value)) == (
        403,
        "current admission funding/identity no longer eligible",
    )


def close_round(rig: Rig, w: int) -> None:
    """Round `w` is settled with nothing open, as the finality record of a finalized round says
    (a real settlement needs a whole audited round)."""
    store = rig.world.store
    settled = {
        **store._record_v2(rig.run_id, "finality", str(w)),
        "applied": True,
        "disposition": "SETTLED",
        "unresolved_audits": 0,
        "unresolved_disputes": 0,
    }
    with store._tx():
        store._put_record_v2(rig.run_id, "finality", str(w), settled)


def test_the_stake_of_a_rostered_member_is_held_until_its_round_closes(rig: Rig) -> None:
    # Given: an opened round of four funded members, and the work screen of the first one lapsed
    # afterwards, so that only the open round still holds its stake
    miners = cohort(rig, "funded")
    assert rig.open_round(rig.opening(miners)).status_code == 200
    member, stake = miners[0], rig.stake_lock(miners[0])
    with rig.world.store._tx():
        rig.world.store._db.execute(
            "UPDATE admissions_v2 SET screen_until=0 WHERE admission_id=?", (member.admission_id,)
        )
    held = rig.bond_entry(member, stake.operation_id)
    # When / Then: the service does not offer the release, and refuses it with its own terms
    assert held["releasable"] is False
    digest, locked = rig.world.db_digest(), units_of(rig.escrow, member.coldkey)
    refused = rig.release(member, stake, held["finality_hash"], held["closed_dispute_root"])
    assert (refused.status_code, refused.json()["detail"]) == (409, "DISPUTED_OR_UNVESTED")
    assert rig.world.db_digest() == digest
    assert units_of(rig.escrow, member.coldkey) == locked
    # and once the round is closed the same stake is the owner's to take back
    close_round(rig, 0)
    closed = rig.bond_entry(member, stake.operation_id)
    assert closed["releasable"] is True
    response = rig.release(member, stake, closed["finality_hash"], closed["closed_dispute_root"])
    assert response.status_code == 200, response.text
    assert units_of(rig.escrow, member.coldkey)["admission_locked"] == 0
    assert rig.escrow.balances().conserved()


def test_a_lapsed_challenge_window_suspends_nobody_and_frees_the_trial_place(rig: Rig) -> None:
    # Given: an application whose trial was opened at round 2 and never proved
    miner = rig.join(0)
    rig.net.push(2)
    assert rig.open_trial(miner).status_code == 200
    # When: the beacon passes the deadline of that trial (round 102)
    rig.net.push(103)
    # Then: the place is free and the miner is where it was: no suspension to find a way back from
    record = rig.admission.store.record(miner.hotkey)
    assert (record.state, record.strikes, record.pending_dispute) == ("APPLIED", 0, False)
    trials = rig.admission.store.db.execute("SELECT outcome FROM admission_trial_results")
    assert [row[0] for row in trials] == ["EXPIRED"]
    transitions = rig.admission.store.db.execute("SELECT state FROM admission_transitions")
    assert [row[0] for row in transitions] == []
    again = rig.open_trial(miner)
    assert (again.status_code, again.json()["detail"]) == (409, "CHALLENGE_EXPIRED")


# --- LCK-07: one finalization per round; its replay writes nothing and a rival is refused ------


def test_a_finalized_round_answers_a_replay_with_its_stored_result_and_refuses_another_body(
    rig: Rig,
) -> None:
    # Given: round 0 opened and finalized: the coordinator's Finalize and the result the
    # finalization stored (the writes of a real finalize need a whole audited round)
    miners = cohort(rig, "funded")
    assert rig.open_round(rig.opening(miners)).status_code == 200
    included = sorted(m.hotkey for m in miners)
    final = Finalize(
        w=0, final_theta_hash_w1=ident("theta"), included=included, entitlements_root=ident("a")
    )
    envelope = rig.net.signed(rig.service.COORD, "Finalize", final)
    stored = {"status": "SETTLED", "reward_receipt": {"seq": 7}, "theta_hash": ident("theta")}
    with rig.world.store._tx():
        rig.world.store._put_record_v2(rig.run_id, "finalize", "0", envelope)
        rig.world.store._put_record_v2(rig.run_id, "finalize-result", "0", stored)
    route = rig.net.url + "/admin/rounds/0/finalize"

    def post(body: object) -> httpx.Response:
        return rig.net.client.post(route, json=body, headers=rig.world.admin)

    # When: the same Finalize is posted twice
    first, second = post(envelope), post(envelope)
    settled = rig.world.count("records_v2", "kind='settlement'")
    events, digest = event_count(rig.escrow), rig.world.db_digest()
    # Then: both answer with the stored result and the second wrote nothing
    assert (first.status_code, first.json()) == (200, stored)
    assert (second.status_code, second.json()) == (200, stored)
    assert post(envelope).json() == stored
    assert rig.world.db_digest() == digest
    # a Finalize with another body for the round is refused and changes nothing
    rival = Finalize(
        w=0, final_theta_hash_w1=ident("theta"), included=included, entitlements_root=ident("b")
    )
    refused = post(rig.net.signed(rig.service.COORD, "Finalize", rival))
    assert refused.status_code == 409 and "conflicting" in refused.json()["detail"]
    assert rig.world.db_digest() == digest
    assert (rig.world.count("records_v2", "kind='settlement'"), event_count(rig.escrow)) == (
        settled,
        events,
    )
