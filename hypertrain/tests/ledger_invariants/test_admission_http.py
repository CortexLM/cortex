"""Admission defences over the signed HTTP routes of the v2 service.

Lens C1 LCK-15, LCK-17, LCK-20, LCK-21, SHD-04 and C2 SG-06..08, GT-01, GT-03, AD-01, AD-02,
AD-05, OR-01, RL-01 (GT-02 is pinned by tests/challenge/test_admission_v2.py, see
mutants_admission_http.py). Every test states what the service must refuse or keep, and that a
refusal leaves the admission tables and the escrow log as they were. A hole is an xfail(strict)
test whose body asserts the desired defence; the steps that make its scenario meaningful raise
PreconditionError, which is not an AssertionError, so a broken scenario fails loudly instead of
hiding behind the expected failure.

Rotation and challenge requests are built in one place (`Gate.rotation_request`,
`trial_challenge_headers`): when ADR 0010 Parts 1 and 3 land, that is where the new hotkey's
consent and the owner's challenge signature are added.
"""

from __future__ import annotations

import json

import pytest

from hypertrain.protocol.envelope_v2 import join_signing_message
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.messages_v2 import EscrowTransfer

from .support import SERVICE_FIXTURE, load_fixture_module, service_world
from .support_admission_http import (
    COLDS,
    HOTS,
    STRANGER,
    Gate,
    Subnet,
    detail,
    precondition,
)

service = load_fixture_module(*SERVICE_FIXTURE)
# The module-scoped real shadow trial of the service tests: one genuine reference/miner pair,
# cloned per test. Its fixtures are bound here so pytest finds them.
shadow_reservation_work = service.shadow_reservation_work
reserved_trial = service.reserved_trial

OTHER_RUN = "ee" * 32
STALE_POLICY = "cd" * 32


@pytest.fixture
def gate(tmp_path, monkeypatch):
    """The v2 service; the fixture miners, the role keys and 40 spare hotkeys are on the subnet."""
    registered = [
        *(key.ss58 for key in service.HOT),
        service.REFEREE.ss58,
        *(key.ss58 for key in service.AUDITORS),
        service.COORD.ss58,
        *(key.ss58 for key in HOTS),
    ]
    subnet = Subnet(registered)
    monkeypatch.setattr(service, "subnet", subnet.transport)
    with service_world(tmp_path) as world:
        yield Gate(world, subnet)


def fill_pending_places(gate: Gate) -> None:
    for i in range(gate.policy.max_pending):
        admitted = gate.join(HOTS[i], COLDS[i], f"unfunded-{i}")
        precondition(admitted.status_code == 200, f"application {i} refused: {admitted.text}")


# SG-06: a join needs two valid signatures, the run, a live expiry and the current policy.


def forged_join(gate: Gate, fault: str) -> tuple[dict, str]:
    """A join request with exactly one defect, and the refusal it is owed."""
    hot, cold = service.HOT[0], service.COLD[0]
    request = gate.join_request(hot, cold)
    body = request.body()
    message = join_signing_message(request)
    code = "DUAL_SIGNATURE_RUN_OR_EXPIRY"
    if fault == "hot_signature_of_another_key":
        body["hot_sig"] = STRANGER.sign(message).hex()
    elif fault == "cold_signature_of_another_key":
        body["cold_sig"] = STRANGER.sign(message).hex()
    elif fault == "unsigned":
        body["hot_sig"] = body["cold_sig"] = "0" * 128
    elif fault == "signed_for_another_run":
        body = gate.join_request(hot, cold, run_id=OTHER_RUN).body()
    elif fault == "expired_one_round_ago":
        body = gate.join_request(hot, cold, expires=9).body()
    else:
        assert fault == "stale_policy"
        body = gate.join_request(hot, cold, policy_hash=STALE_POLICY).body()
        code = "POLICY_CHANGED"
    return body, code


@pytest.mark.parametrize(
    "fault",
    [
        "hot_signature_of_another_key",
        "cold_signature_of_another_key",
        "unsigned",
        "signed_for_another_run",
        "expired_one_round_ago",
        "stale_policy",
    ],
)
def test_sg06_a_join_with_one_defect_is_refused_and_leaves_no_trace(gate: Gate, fault: str) -> None:
    # Given: round 10 and a request differing from a valid one in exactly one respect
    hot, cold = service.HOT[0], service.COLD[0]
    gate.push(10)
    body, code = forged_join(gate, fault)
    before = gate.world.db_digest()
    # When / Then
    refused = gate.post_join(body)
    assert (refused.status_code, detail(refused)) == (409, code)
    assert gate.world.db_digest() == before
    # And: the refusal reserved neither the identity nor the request id, so the valid request,
    # live on its very last round, is admitted
    admitted = gate.join(hot, cold, expires=10)
    assert admitted.status_code == 200, admitted.text
    assert admitted.json()["state"] == "APPLIED"


def test_sg06_repeating_a_join_returns_the_same_admission_and_spends_no_quota(gate: Gate) -> None:
    # Given: an admitted application, and a later beacon (a client retries after a restart)
    hot, cold = service.HOT[0], service.COLD[0]
    first = gate.join(hot, cold)
    assert first.status_code == 200, first.text
    gate.push(5)
    held = gate.snapshot()
    precondition(
        len(held["admission_quota"]) == 3,
        f"quota rows of the hotkey, the coldkey and the prefix: {held['admission_quota']}",
    )
    # When: the exact request arrives again, as many times as the client retries
    repeats = [gate.join(hot, cold) for _ in range(3)]
    # Then: the stored answer, one identity, nothing charged and no admission row touched
    assert [r.json() for r in repeats] == [first.json()] * 3
    assert gate.count("admissions_v2") == 1
    assert gate.snapshot() == held
    # And: another body under the same (coldkey, request id) is a conflict, not a second identity
    before = gate.world.db_digest()
    conflicting = gate.join(service.HOT[1], cold)
    assert (conflicting.status_code, detail(conflicting)) == (409, "REPLAY_CONFLICT")
    assert gate.world.db_digest() == before


# OR-01: what a refusal says does not depend on what the service holds about someone else.


def test_or01_a_forged_join_gets_the_same_refusal_whoever_is_already_admitted(gate: Gate) -> None:
    # Given: an admitted participant; the checks that would reveal it come after the signatures
    taken_hot, taken_cold = service.HOT[0], service.COLD[0]
    precondition(gate.join(taken_hot, taken_cold).status_code == 200, "setup join")
    applicants = {
        "hotkey already admitted": (taken_hot, COLDS[1]),
        "coldkey already holding an admission": (HOTS[1], taken_cold),
        "unknown identity": (HOTS[2], COLDS[2]),
    }
    answers = {}
    for name, (hot, cold) in applicants.items():
        body = gate.join_request(hot, cold, name).body()
        body["cold_sig"] = STRANGER.sign(b"not the join message").hex()
        before = gate.world.db_digest()
        refused = gate.post_join(body)
        answers[name] = (refused.status_code, detail(refused))
        assert gate.world.db_digest() == before
    # Then: one answer for all of them
    assert set(answers.values()) == {(409, "DUAL_SIGNATURE_RUN_OR_EXPIRY")}, answers


def test_or01_recover_refuses_alike_whatever_the_admission_holds(
    gate: Gate,
) -> None:
    # Given: an admission without a pending dispute, a registered hotkey without any admission,
    # and an admission whose pending dispute is a different one than the receipt cites
    gate.join(service.HOT[0], service.COLD[0])
    gate.join(service.HOT[2], service.COLD[2])
    gate.admission.suspend(
        service.HOT[2].ss58,
        now=gate.network.now,
        reason="PENDING_DISPUTE",
        pending_dispute=True,
        dispute_id="ea" * 32,
        evidence_hash="eb" * 32,
    )
    senders = {
        "no admission": service.HOT[1],
        "no pending row": service.HOT[0],
        "other pending dispute": service.HOT[2],
    }
    before = gate.world.db_digest()
    # When
    answers = {name: gate.recover(key, "ec" * 32) for name, key in senders.items()}
    # Then
    assert {name: (r.status_code, detail(r)) for name, r in answers.items()} == {
        name: (403, "recovery reference differs from exact current pending dispute")
        for name in senders
    }
    assert gate.world.db_digest() == before


# SG-07: the two keys of one application are two keys.


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="SG-07+SIG-13: a join whose hotkey and coldkey are the same key is accepted [candidate]",
)
def test_sg07_a_join_cannot_use_one_key_as_hotkey_and_coldkey(gate: Gate) -> None:
    # Given: the route admits an ordinary application
    ordinary = gate.join(HOTS[1], COLDS[1])
    precondition(ordinary.status_code == 200, f"control join refused: {ordinary.text}")
    # When
    same = gate.join(HOTS[0], HOTS[0], "one key")
    # Then
    assert same.status_code in (403, 409), same.text
    assert gate.count("admission_history", "hotkey=?", (HOTS[0].ss58,)) == 0


# SG-08, LCK-15, LCK-17: who may rotate an admission, and onto which hotkey.


def test_sg08_only_the_coldkey_of_the_admission_can_rotate_it(gate: Gate) -> None:
    # Given: an admission of COLD[0]; COLD[1] is the coldkey of another admission, so a signer
    # that merely owns some admission is not enough
    hot, cold = service.HOT[0], service.COLD[0]
    precondition(gate.join(hot, cold).status_code == 200, "setup join")
    precondition(gate.join(service.HOT[1], service.COLD[1]).status_code == 200, "other join")
    before = gate.world.db_digest()
    # When / Then: a coldkey that does not own it is refused
    foreign = gate.rotate(service.COLD[1], hot, HOTS[0], "foreign")
    assert (foreign.status_code, detail(foreign)) == (
        409,
        "signer is not authorized for message role",
    )
    assert gate.world.db_digest() == before
    # And: the hotkey itself does not stand in for its coldkey
    by_hotkey = gate.rotate(hot, hot, HOTS[0], "by hotkey")
    assert by_hotkey.status_code == 409
    assert gate.world.db_digest() == before
    # And: the owner's own rotation moves the same admission
    moved = gate.rotate(cold, hot, HOTS[0], "owner")
    assert moved.status_code == 200, moved.text
    assert moved.json()["hotkey"] == HOTS[0].ss58


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="LCK-17+SG-08+RTA-01: a rotation signed by the coldkey alone binds an admission to a "
    "registered hotkey that never agreed and spends that hotkey for good",
)
def test_lck17_a_rotation_onto_a_hotkey_that_never_agreed_is_refused(gate: Gate) -> None:
    # Given: an admission that rotates once onto a hotkey of its owner (the control), and a
    # registered hotkey of another participant that never applied
    owner, mine, other, other_cold = service.COLD[0], HOTS[0], HOTS[1], COLDS[1]
    precondition(gate.join(service.HOT[0], owner).status_code == 200, "setup join")
    own = gate.rotate(owner, service.HOT[0], mine, "own hotkey")
    precondition(own.status_code == 200, f"the owner's own rotation was refused: {own.text}")
    precondition(other.ss58 in gate.subnet.registered, "the other hotkey is on the subnet")
    # When: the coldkey alone names that hotkey, whose key it does not hold
    taken = gate.rotate(owner, mine, other.ss58, "without consent")
    # Then: refused by the admission rules, not by routing, and its holder can still apply
    assert taken.status_code in (403, 409), taken.text
    assert "not registered on the subnet" not in taken.text
    assert gate.count("admission_history", "hotkey=?", (other.ss58,)) == 0
    applied = gate.join(other, other_cold, "holder applies")
    assert applied.status_code == 200, applied.text


ROLE_KEYS = {
    "referee": service.REFEREE,
    "auditor": service.AUDITORS[1],
    "coordinator": service.COORD,
}


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="LCK-15+SG-08+RTA-01: a rotation can move a miner admission onto a pinned auditor, "
    "referee or coordinator hotkey [candidate]",
)
@pytest.mark.parametrize("role", sorted(ROLE_KEYS))
def test_lck15_a_rotation_onto_a_pinned_protocol_role_hotkey_is_refused(
    gate: Gate, role: str
) -> None:
    # Given: an admission that rotated onto an ordinary hotkey of its owner (the control)
    owner, mine, pinned = service.COLD[0], HOTS[0], ROLE_KEYS[role]
    precondition(gate.join(service.HOT[0], owner).status_code == 200, "setup join")
    own = gate.rotate(owner, service.HOT[0], mine, "own hotkey")
    precondition(own.status_code == 200, f"the owner's own rotation was refused: {own.text}")
    precondition(pinned.ss58 in gate.subnet.registered, "the role key is on the subnet")
    # When / Then
    moved = gate.rotate(owner, mine, pinned, "to the role key")
    assert moved.status_code in (403, 409), moved.text
    assert "not registered on the subnet" not in moved.text
    assert gate.count("admission_history", "hotkey=?", (pinned.ss58,)) == 0


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="LCK-15: a pinned auditor, referee or coordinator hotkey can join as a miner identity "
    "[candidate]",
)
@pytest.mark.parametrize("role", sorted(ROLE_KEYS))
def test_lck15_a_pinned_protocol_role_hotkey_cannot_join_as_a_miner(gate: Gate, role: str) -> None:
    # Given: an ordinary application is admitted
    ordinary = gate.join(HOTS[1], COLDS[1], "ordinary")
    precondition(ordinary.status_code == 200, f"control join refused: {ordinary.text}")
    pinned = ROLE_KEYS[role]
    precondition(pinned.ss58 in gate.subnet.registered, "the role key is on the subnet")
    # When: the role key signs an application as hotkey
    joined = gate.join(pinned, COLDS[0], "role key")
    # Then
    assert joined.status_code in (403, 409), joined.text
    assert gate.count("admission_history", "hotkey=?", (pinned.ss58,)) == 0


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="SG-08+RTA-01: a BANNED admission can still rotate onto a new hotkey",
)
def test_sg08_a_banned_admission_cannot_rotate(gate: Gate) -> None:
    # Given: an admission that rotated once (the control), then was banned
    owner, mine = service.COLD[0], HOTS[0]
    admission_id = gate.admission_id(service.HOT[0], owner)
    own = gate.rotate(owner, service.HOT[0], mine, "before the ban")
    precondition(own.status_code == 200, f"the owner's own rotation was refused: {own.text}")
    gate.admission.store.transition(admission_id, "BANNED", "TEST", gate.network.now)
    before = gate.world.db_digest()
    # When / Then
    moved = gate.rotate(owner, mine, HOTS[1], "after the ban")
    assert moved.status_code in (403, 409), moved.text
    assert gate.world.db_digest() == before


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="SG-08+RTA-01: the rotations of one admission have no bound per window",
)
def test_sg08_rotations_of_one_admission_are_bounded_per_window(gate: Gate) -> None:
    # Given: the bound the policy gives identity-creating requests
    burst = gate.policy.join_burst
    owner = service.COLD[0]
    precondition(gate.join(service.HOT[0], owner).status_code == 200, "setup join")
    chain = [service.HOT[0], *HOTS[: burst + 1]]
    # When: the owner rotates on and on within one beacon
    answers = [
        gate.rotate(owner, old, new, f"hop-{i}")
        for i, (old, new) in enumerate(zip(chain[:-1], chain[1:], strict=True))
    ]
    # Then: the first `burst` are served, the next one is not
    precondition(
        [r.status_code for r in answers[:burst]] == [200] * burst,
        f"the first {burst} rotations were not all served: {[r.status_code for r in answers]}",
    )
    assert answers[burst].status_code in (403, 409, 429), answers[burst].text


# GT-01, LCK-20: only the owner can open, expire or suspend its own trial.


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="GT-01+LCK-20+RTA-04: a challenge request that does not prove ownership of the "
    "admission still opens a trial for it",
)
def test_gt01_only_the_owner_can_open_the_trial_of_an_admission(gate: Gate) -> None:
    # Given: two applicants; the first one asks for its own trial with its own signed headers
    mine, rival = service.HOT[1], service.HOT[2]
    mine_id = gate.admission_id(mine, service.COLD[1])
    rival_id = gate.admission_id(rival, service.COLD[2])
    gate.push(2)
    own = gate.challenge(mine_id, mine)
    precondition(own.status_code == 200, f"the owner's own request failed: {own.text}")
    before = gate.world.db_digest()
    # When: someone who is not the rival asks for the rival's challenge, unsigned or signed
    # with the wrong registered hotkey
    anonymous = gate.challenge(rival_id)
    wrong_key = gate.challenge(rival_id, as_hotkey=service.HOT[3])
    # Then: refused, and no trial exists for the rival
    assert anonymous.status_code in (401, 403), anonymous.text
    assert wrong_key.status_code in (401, 403), wrong_key.text
    assert gate.row(rival)["challenge"] is None
    assert gate.count("admission_trials", "admission_id=?", (rival_id,)) == 0
    assert gate.world.db_digest() == before


@pytest.mark.parametrize("state", ["APPLIED", "ACTIVE"])
def test_gt01_a_poll_after_the_deadline_by_anyone_expires_or_suspends_nobody(
    gate: Gate, state: str
) -> None:
    # Given: an admission that opened its trial and never proved it; the beacon passes the
    # deadline, which closes the trial on the clock (REFUTED: the lazy suspension of RTA-04/05)
    hot, cold = service.HOT[1], service.COLD[1]
    admission_id = gate.admission_id(hot, cold)
    gate.push(2)
    own = gate.challenge(admission_id, hot)
    precondition(own.status_code == 200, f"the owner's own request failed: {own.text}")
    if state == "ACTIVE":
        gate.admission.store.transition(admission_id, "ACTIVE", "TEST", 2)
    deadline = json.loads(gate.row(hot)["challenge"])["deadline_beacon"]
    gate.push(deadline + 1)
    outcome = gate.snapshot()["admission_trial_results"][0][-1]
    precondition(outcome == "EXPIRED", f"the clock did not close the trial: {outcome}")
    before = gate.world.db_digest()
    # When: a caller that is not the owner polls
    stranger = gate.challenge(admission_id)
    # Then: nothing is written, nobody is suspended, struck or credited
    assert stranger.status_code in (401, 403, 409), stranger.text
    assert gate.world.db_digest() == before
    row = gate.row(hot)
    assert (row["state"], row["strikes"], row["clean_count"], row["pending_dispute"]) == (
        state,
        0,
        0,
        0,
    )


@pytest.mark.parametrize("state", ["SUSPENDED", "BANNED"])
def test_gt03_the_clock_closes_the_trial_of_a_suspended_or_banned_holder(
    gate: Gate, state: str
) -> None:
    # Given: a trial its owner opened, then a holder that is suspended or banned: no request of
    # that holder can ever free the place the trial keeps
    hot, cold = service.HOT[1], service.COLD[1]
    admission_id = gate.admission_id(hot, cold)
    gate.push(2)
    own = gate.challenge(admission_id, hot)
    precondition(own.status_code == 200, f"the owner's own request failed: {own.text}")
    gate.admission.store.transition(admission_id, state, "TEST", 2)
    precondition(gate.count("admission_trial_results", "outcome='OPEN'") == 1, "no open trial")
    deadline = json.loads(gate.row(hot)["challenge"])["deadline_beacon"]
    # When: the beacon passes the deadline
    gate.push(deadline + 1)
    # Then: the trial is closed on the clock and the holder's standing is untouched
    assert gate.count("admission_trial_results", "outcome='OPEN'") == 0
    assert gate.row(hot)["state"] == state


# AD-01, LCK-21, AD-05: capacity and identity rules of the application queue.


def test_ad01_the_pending_places_are_bounded_and_a_refusal_changes_nothing(gate: Gate) -> None:
    # Given: every pending place taken, one of them by an application that passed its trial
    fill_pending_places(gate)
    passed = gate.row(HOTS[0])["admission_id"]
    gate.admission.store.transition(passed, "PROBATION", "TEST", gate.network.now)
    before = gate.world.db_digest()
    # When / Then: one more is refused, and the refusal leaves no trace
    refused = gate.join(service.HOT[0], service.COLD[0])
    assert (refused.status_code, detail(refused)) == (409, "PENDING_CAPACITY")
    assert gate.world.db_digest() == before


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="AD-01+LCK-21+RTA-02: unfunded applications that can no longer open a trial keep "
    "their pending places for good",
)
def test_ad01_idle_unfunded_applications_do_not_hold_the_pending_places_for_ever(
    gate: Gate,
) -> None:
    # Given: every pending place taken by applications that hold no unit and never ask for a trial
    fill_pending_places(gate)
    full = gate.join(service.HOT[0], service.COLD[0])
    precondition(
        (full.status_code, detail(full)) == (409, "PENDING_CAPACITY"), "the places are not full"
    )
    # When: the challenge window of every one of them is long gone
    gate.push(gate.network.now + 2 * gate.policy.challenge_rounds)
    # Then: an honest application is admitted
    honest = gate.join(service.HOT[0], service.COLD[0], "honest")
    assert honest.status_code == 200, honest.text


def test_ad05_a_hotkey_is_admitted_once_ever_even_after_it_moved_to_another_key(
    gate: Gate,
) -> None:
    # Given: an admitted application, and another participant's coldkey
    hot, cold, stranger_cold = service.HOT[0], service.COLD[0], service.COLD[1]
    precondition(gate.join(hot, cold).status_code == 200, "setup join")
    # When / Then: the same hotkey under another coldkey is refused
    before = gate.world.db_digest()
    again = gate.join(hot, stranger_cold, "same hotkey")
    assert (again.status_code, detail(again)) == (409, "HOTKEY_HISTORY_EXISTS")
    assert gate.world.db_digest() == before
    # And: after the admission moved to another key, the retired key and the new one are spent
    moved = gate.rotate(cold, hot, HOTS[0], "moves")
    precondition(moved.status_code == 200, f"the owner's own rotation was refused: {moved.text}")
    for name, spent in (("retired", hot), ("current", HOTS[0])):
        before = gate.world.db_digest()
        refused = gate.join(spent, stranger_cold, name)
        assert (refused.status_code, detail(refused)) == (409, "HOTKEY_HISTORY_EXISTS"), name
        assert gate.world.db_digest() == before, name
    # And: another participant cannot move its own admission onto a spent hotkey either
    other = gate.join(service.HOT[1], stranger_cold, "other participant")
    precondition(other.status_code == 200, f"the second application was refused: {other.text}")
    for name, spent in (("retired", hot), ("current", HOTS[0])):
        before = gate.world.db_digest()
        refused = gate.rotate(stranger_cold, service.HOT[1], spent, f"onto {name}")
        assert (refused.status_code, detail(refused)) == (409, "NEW_HOTKEY_HISTORY"), name
        assert gate.world.db_digest() == before, name


def test_ad05_a_coldkey_has_one_non_active_admission_and_an_active_one_may_apply_again(
    gate: Gate,
) -> None:
    # Given: an admission of COLD[0] that is not ACTIVE
    cold = service.COLD[0]
    first = gate.admission_id(service.HOT[0], cold)
    # When / Then: a second hotkey of the same coldkey is refused, in every non-ACTIVE state of
    # the first admission, and nothing is reserved
    for state in ("APPLIED", "PROBATION", "SUSPENDED", "BANNED"):
        gate.admission.store.transition(first, state, "TEST", gate.network.now)
        before = gate.world.db_digest()
        blocked = gate.join(service.HOT[1], cold, "second")
        assert (blocked.status_code, detail(blocked)) == (409, "COLDKEY_PROBATION_EXISTS"), state
        assert gate.world.db_digest() == before, state
    # And: once the first one is ACTIVE the same request is admitted
    gate.admission.store.transition(first, "ACTIVE", "TEST", gate.network.now)
    allowed = gate.join(service.HOT[1], cold, "second")
    assert allowed.status_code == 200, allowed.text
    assert gate.count("admissions_v2", "coldkey=?", (cold.ss58,)) == 2
    # And: the new one is not ACTIVE, so a third is refused again
    third = gate.join(service.HOT[2], cold, "third")
    assert (third.status_code, detail(third)) == (409, "COLDKEY_PROBATION_EXISTS")


# RL-01: a state-creating route other than join has a per-identity bound.


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="RL-01+ESC-02: one key can add ledger events through the escrow routes without any "
    "bound per beacon [candidate]",
)
def test_rl01_the_escrow_transfers_of_one_key_are_bounded_in_one_beacon(
    gate: Gate,
) -> None:
    # Given: the largest burst the policy gives any identity-creating request (the repository
    # states no bound for the escrow routes), and a funded key
    bound = gate.policy.ip_prefix_burst
    owner, origin = service.COLD[0], gate.network.origin_ids[0]

    def self_transfer(index: int):
        operation = EscrowTransfer(
            operation_id=sha256_hex(f"bounded-{index}".encode()),
            owner=owner.ss58,
            units=1,
            origin_ids=[origin],
            admission_id=None,
            dispute_id=None,
            recipient=owner.ss58,
        )
        return gate.client.post(
            gate.url + "/escrow/transfer",
            json=gate.network.signed(owner, "EscrowTransfer", operation),
        )

    first = self_transfer(0)
    precondition(first.status_code == 200, f"the first transfer was refused: {first.text}")
    # When: it sends distinct operations until the first refusal, or one more than that burst
    served = 1
    for index in range(1, bound + 1):
        if self_transfer(index).status_code != 200:
            break
        served += 1
    # Then: the service refused before serving more than the burst
    assert served <= bound, f"{served} transfers were served in one beacon"


# AD-02: a trial is finalized at most once. A real shadow trial, finalized by the service itself.


def test_ad02_a_finalized_trial_credits_one_participation_however_often_it_is_replayed(
    reserved_trial,
) -> None:
    n, identity, body = reserved_trial

    def standing() -> tuple:
        db = n.store._db
        row = db.execute("SELECT state, clean_count, seed_beacon FROM admissions_v2").fetchone()
        finals = db.execute(
            "SELECT COUNT(*) FROM admission_reservations WHERE reservation LIKE 'trial-final|%'"
        ).fetchone()[0]
        stamp = db.execute(
            "SELECT finalized_beacon, evidence_hash FROM admission_trials"
        ).fetchone()
        return tuple(row), finals, tuple(stamp)

    # Given: the participation was credited once, at beacon 2
    credited = standing()
    precondition(
        credited[0][:2] == ("PROBATION", 1) and credited[1] == 1,
        f"the trial was not finalized once: {credited}",
    )
    accepted = n.store._db.execute("SELECT proof, screen FROM admission_trial_results").fetchone()
    # When: a later beacon arrives, the miner re-posts its accepted pair, and the operator posts
    # the same Finalize and the same Finalize signed again
    n.push(3)
    replays = [
        n.client.post(
            n.url + "/join/proof",
            json={"proof": json.loads(accepted["proof"]), "screen": json.loads(accepted["screen"])},
        ),
        n.client.post(
            n.url + "/admin/join/" + identity + "/finalize",
            json=body["finalize"],
            headers=service.admin(),
        ),
        n.client.post(
            n.url + "/admin/join/" + identity + "/finalize",
            json=n.signed(service.COORD, "Finalize", body["finalize"]["body"]),
            headers=service.admin(),
        ),
    ]
    # Then: every replay is answered and none is credited
    assert [r.status_code for r in replays] == [200, 200, 200]
    assert standing() == credited


# SHD-04: shadow collateral and the funded admission lock.


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="SHD-04+RTA-02: a shadow reservation is issued to an admission that holds no funded "
    "admission lock [candidate]",
)
def test_shd04_a_shadow_reservation_needs_a_funded_admission_lock(reserved_trial) -> None:
    n, identity, body = reserved_trial
    escrow = n.store._services(n.manifest.run_id())[0]
    # Given: a finalized trial whose admission locked nothing; the body is the one the service
    # tests reserve with
    locked, _ = escrow.locked(identity, service.COLD[0].ss58)
    precondition(locked == 0, f"the admission holds {locked} locked units")
    before = escrow.balances()
    # When
    response = n.client.post(
        n.url + "/admin/join/" + identity + "/shadow-reservation",
        json=body,
        headers=service.admin(),
    )
    # Then: refused, with nothing reserved
    assert response.status_code == 409, response.text
    reserved = n.store._db.execute(
        "SELECT COUNT(*) FROM records_v2 WHERE kind='shadow-reservation'"
    ).fetchone()[0]
    assert reserved == 0
    assert escrow.balances() == before
