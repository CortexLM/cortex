"""Disputes: who may open, move and close a funded contest, and what happens to its bond.

Each test states a defence the protocol must hold: a refusal pins its code and leaves every
dispute and escrow row as it was. The bare rig (`make_rig`) is the state machine over an
in-memory escrow, with no store; `test_disputes_2.py` drives the same machine through the real
routes.
"""

from __future__ import annotations

import threading
from dataclasses import replace
from types import ModuleType

import pytest
from hypothesis import example, given
from hypothesis import strategies as st

from hypertrain.challenge.disputes_v2 import DisputeError
from hypertrain.ledger.escrow_v2 import EscrowError
from hypertrain.protocol import envelope_v2
from hypertrain.protocol.envelope import UnauthorizedSigner
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages import ReplayVerdict
from hypertrain.protocol.messages_v2 import EscrowRelease

from .support import Real, bare_escrow, error_code
from .support_disputes import (
    AGREE,
    AUDITOR_SAYS,
    BOND,
    MINER_SAYS,
    VERDICT,
    Rig,
    make_rig,
    raised,
    refusal,
    settlement_events,
    signed_variant_on,
)

V1 = "12" * 32


V2 = "13" * 32


def locked(rig: Rig, lock_id: str) -> int:
    """Units still in dispute_locked under one contest lock."""
    return int(
        rig.db.execute(
            "SELECT COALESCE(SUM(units),0) FROM escrow_units "
            "WHERE bucket='dispute_locked' AND ref=?",
            (lock_id,),
        ).fetchone()[0]
    )


def refused_unchanged(rig: Rig, code: str, call) -> None:
    """`call` is refused with `code` and no dispute or escrow row moved."""
    before = rig.state()
    assert refusal(call) == code
    assert rig.state() == before


def dispute_rows(rig: Rig) -> int:
    return int(rig.db.execute("SELECT COUNT(*) FROM disputes_v2").fetchone()[0])


# --- DIA-02, DIB-16: one dispute per lock and per verdict, a bond that is the miner's own -----


def test_a_contest_lock_is_bound_to_one_dispute_and_the_first_binding_never_moves(
    li_escrow: ModuleType,
) -> None:
    # Covers DIA-02.
    # DIA-02. Given: miner A contests with L1; miner B has its own funded bond; A has a spare.
    rig = make_rig(li_escrow)
    first, _ = rig.open(VERDICT)
    miner_b = rig.miner(1)
    rig.contest(V1, miner_b)
    rig.lock(V1)
    spare = rig.lock(VERDICT)
    # When B names the lock that already carries A's dispute id
    reuse = rig.request(miner_b, V1, first.lock_id)
    # Then: refused, and A's bond is untouched
    refused_unchanged(rig, "CONTEST_LOCK_BINDING_OR_COST", lambda: rig.d.open(reuse, now=1))
    assert locked(rig, first.lock_id) == BOND
    # When A names another funded lock for the same verdict
    swap = rig.request(rig.f.HOT, VERDICT, spare.operation_id)
    # Then: the first binding stands; the spare stays a plain unspent bond
    refused_unchanged(rig, "CONTEST_REPLAY_CONFLICT", lambda: rig.d.open(swap, now=1))
    assert rig.d.get(first.dispute_id).lock_id == first.lock_id
    assert (dispute_rows(rig), locked(rig, spare.operation_id)) == (1, BOND)
    # When the dispute is closed and its bond released, the old lock cannot back a new dispute
    rig.close(first, "MATCH", None)
    assert locked(rig, first.lock_id) == 0
    rig.contest(V2, rig.f.HOT)
    again = rig.request(rig.f.HOT, V2, first.lock_id)
    refused_unchanged(rig, "CONTEST_LOCK_BINDING_OR_COST", lambda: rig.d.open(again, now=4))


@pytest.mark.parametrize(
    ("lock", "code"),
    [
        pytest.param(
            {"dispute_id": "9f" * 32}, "CONTEST_LOCK_BINDING_OR_COST", id="another-dispute"
        ),
        pytest.param({"owner": "other"}, "CONTEST_LOCK_BINDING_OR_COST", id="another-coldkey"),
        pytest.param({"units": BOND - 1}, "CONTEST_LOCK_BINDING_OR_COST", id="below-referee-cost"),
        pytest.param({"kind": "LOCK_ADMISSION"}, "UNFUNDED_CONTEST", id="admission-stake"),
        pytest.param({"fund": False}, "UNFUNDED_CONTEST", id="never-funded"),
    ],
)
def test_a_contest_is_opened_only_on_the_miners_own_funded_bond_of_that_dispute(
    li_escrow: ModuleType, lock: dict, code: str
) -> None:
    # Covers DIB-16.
    # DIB-16. Given: an accepted contest and a bond with one defect
    rig = make_rig(li_escrow)
    rig.contest(VERDICT)
    if lock.get("owner") == "other":
        lock = {**lock, "owner": rig.f.OTHER.ss58}
    bad = rig.lock(VERDICT, **lock)
    raw = rig.request(rig.f.HOT, VERDICT, bad.operation_id)
    # When / Then: refused with the code of the check that failed, nothing recorded
    refused_unchanged(rig, code, lambda: rig.d.open(raw, now=1))
    assert (dispute_rows(rig), rig.d.events(rig.f.HOT.ss58, 0)) == (0, [])
    # And the same miner is admitted on a correct bond: the scenario reaches the checks
    good = rig.lock(VERDICT)
    assert rig.d.open(rig.request(rig.f.HOT, VERDICT, good.operation_id), now=1).lock_id == (
        good.operation_id
    )


# --- DIA-03, DS-01: only the audited miner, signing for itself, opens a contest -----------------


def test_only_the_audited_miner_signing_for_itself_can_contest_its_recorded_verdict(
    li_escrow: ModuleType,
) -> None:
    # Covers DIA-03, DS-01.
    # Given: the accepted contest of the miner's verdict and its funded bond
    rig = make_rig(li_escrow)
    hot, stranger = rig.f.HOT, rig.miner(1)
    other_cold = rig.f.OTHER.ss58
    rig.contest(VERDICT)
    bond = rig.lock(VERDICT).operation_id
    # A hotkey that is not the audited miner signs for itself over the miner's verdict
    taken = rig.request(stranger, VERDICT, bond)
    refused_unchanged(rig, "CONTEST_IDENTITY", lambda: rig.d.open(taken, now=1))
    # A body that names the miner but is signed by another hotkey
    forged = rig.request(stranger, VERDICT, bond, hotkey=hot.ss58)
    before = rig.state()
    raised(lambda: rig.d.open(forged, now=1), UnauthorizedSigner)
    assert rig.state() == before
    # Only a contest is funded: an accept names no bond
    accept = rig.request(hot, VERDICT, None, action="accept")
    refused_unchanged(rig, "FUNDED_CONTEST_REQUIRED", lambda: rig.d.open(accept, now=1))
    # The record the service derived is cross-checked, never trusted
    honest = rig.request(hot, VERDICT, bond)
    linkage = {"admission_id": "55" * 32, "coldkey": rig.f.COLD.ss58, "evidence_hash": "66" * 32}
    for updates, code in (
        ({"auditor": stranger.ss58}, "CONTEST_IDENTITY"),
        ({"admission_id": "55" * 32}, "INCOMPLETE_ACCEPTED_RECOVERY_LINKAGE"),
        ({**linkage, "coldkey": other_cold}, "ACCEPTED_RECOVERY_OWNER"),
        ({"auditor_coldkey": other_cold}, "ACCEPTED_AUDITOR_OWNER"),
        ({"step_span": 3}, "CONTEST_WINDOW_GEOMETRY"),
    ):
        rig.contest(VERDICT, **updates)
        refused_unchanged(rig, code, lambda: rig.d.open(honest, now=1))
    # Control: the unmodified record and the audited miner open the dispute
    rig.contest(VERDICT)
    opened = rig.d.open(honest, now=1)
    assert (opened.contest.miner, opened.expected_party) == (hot.ss58, hot.ss58)


# --- DIA-04: the referee is a pinned hotkey independent of the parties and the coordinator -----


def test_a_policy_that_pins_a_party_as_referee_cannot_make_it_the_referee_of_its_own_dispute(
    li_escrow: ModuleType,
) -> None:
    # Covers DIA-04.
    # Given: an operator who broke the condition and pinned the miner, the auditor and the
    # coordinator among the referees
    fixture = li_escrow
    rig = make_rig(
        fixture,
        coordinator_allowlisted=True,
        pin=(fixture.HOT, fixture.AUDITOR),
    )
    rig.contest(VERDICT)
    bond = rig.lock(VERDICT).operation_id
    raw = rig.request(fixture.HOT, VERDICT, bond)
    # When / Then: the dispute-level guard still refuses each of them, and an unpinned hotkey
    for key in (fixture.HOT, fixture.AUDITOR, fixture.COORD, rig.miner(7)):
        rig.contest(VERDICT, referee=key.ss58)
        refused_unchanged(rig, "INDEPENDENT_REFEREE_REQUIRED", lambda: rig.d.open(raw, now=1))
    # Control: the independent pinned referee is accepted
    rig.contest(VERDICT)
    assert rig.d.open(raw, now=1).contest.referee == rig.ref.ss58


def test_only_the_referee_of_the_dispute_can_sign_its_resolution(li_escrow: ModuleType) -> None:
    # Covers DIA-04, DIB-01, DS-05.
    # Given: an open dispute with registered MATCH evidence
    rig = make_rig(li_escrow, coordinator_allowlisted=True)
    turn, _ = rig.open(VERDICT)
    evidence = rig.evidence(turn)
    rig.d.register_evidence(evidence)
    before = rig.state()
    # When / Then: no other key resolves it, the next policy referee and the coordinator included
    for key in (rig.f.HOT, rig.f.AUDITOR, rig.f.COORD, rig.next_ref, rig.miner(3)):
        raw = rig.signed_resolution(turn, evidence, key=key)
        raised(lambda raw=raw: rig.d.resolution(raw, now=3), UnauthorizedSigner)
        assert rig.state() == before
    assert rig.d.get(turn.dispute_id).resolution is None
    assert rig.resolve(turn, evidence).resolution is not None


def test_a_reassigned_referee_stays_independent_and_the_absolute_horizon_never_moves(
    li_escrow: ModuleType,
) -> None:
    # Covers DIA-04.
    # Given: a paused dispute (both parties agree) whose policy also pins the parties
    rig = make_rig(li_escrow, coordinator_allowlisted=True, pin=(li_escrow.HOT, li_escrow.AUDITOR))
    opened, _ = rig.open(VERDICT)
    horizon, old = opened.absolute_deadline, opened.contest.referee
    refused_unchanged(
        rig,
        "REFEREE_REASSIGNMENT_BOUND",
        lambda: rig.d.reassign(opened.dispute_id, rig.next_ref.ss58, now=2),
    )
    rig.say(opened.dispute_id, rig.f.HOT, AGREE)
    paused, _ = rig.say(opened.dispute_id, rig.f.AUDITOR, AGREE)
    assert paused.paused
    # When / Then: neither party, the coordinator, the sitting referee nor an unpinned key
    for key in (rig.f.HOT, rig.f.AUDITOR, rig.f.COORD, rig.ref, rig.miner(4)):
        refused_unchanged(
            rig,
            "REFEREE_REASSIGNMENT_BOUND",
            lambda key=key: rig.d.reassign(opened.dispute_id, key.ss58, now=3),
        )
    # The one allowed reassignment moves the referee and nothing else
    moved = rig.d.reassign(opened.dispute_id, rig.next_ref.ss58, now=3)
    assert (moved.contest.referee, moved.reassignments) == (rig.next_ref.ss58, 1)
    assert (moved.absolute_deadline, moved.turn_deadline) == (horizon, paused.turn_deadline)
    refused_unchanged(
        rig, "REFEREE_REASSIGNMENT_BOUND", lambda: rig.d.reassign(opened.dispute_id, old, now=4)
    )
    # And the replaced referee can no longer decide the dispute; the new one can
    evidence = rig.evidence(moved)
    rig.d.register_evidence(evidence)
    raised(lambda: rig.resolve(moved, evidence, key=rig.ref), UnauthorizedSigner)
    assert rig.resolve(moved, evidence, key=rig.next_ref).resolution is not None


# --- DIA-05: one unresolved dispute per miner, 32 overall, an idempotent open ------------------


def test_a_miner_has_one_unresolved_dispute_and_resolving_it_frees_the_slot(
    li_escrow: ModuleType,
) -> None:
    # Covers DIA-05.
    # Given: a miner with an open dispute and a second verdict with its own funded bond
    rig = make_rig(li_escrow)
    first, raw = rig.open(VERDICT)
    rig.contest(V1, rig.f.HOT)
    second_bond = rig.lock(V1)
    request = rig.request(rig.f.HOT, V1, second_bond.operation_id)
    before = rig.state()
    # When / Then: the byte-identical request is a no-op that returns the current turn
    assert rig.d.open(raw, now=1) == first
    assert rig.state() == before
    # A second dispute of the same miner is refused and its bond stays a plain funded lock
    refused_unchanged(
        rig, "QUEUE_CAPACITY_EXCLUDE_WITHOUT_FRAUD", lambda: rig.d.open(request, now=1)
    )
    assert locked(rig, second_bond.operation_id) == BOND
    # When the first dispute is closed, the slot is free again
    rig.close(first, "MATCH", None)
    second = rig.d.open(request, now=4)
    assert (second.dispute_id != first.dispute_id, second.lock_id) == (
        True,
        second_bond.operation_id,
    )


def test_at_most_thirty_two_disputes_are_unresolved_and_a_closed_one_frees_a_slot(
    li_escrow: ModuleType,
) -> None:
    # Covers DIA-05.
    # Given: 32 audited miners, each with an open dispute
    rig = make_rig(li_escrow)
    opened = [rig.open(f"{i + 1:064x}", miner=rig.miner(i))[0] for i in range(32)]
    late = rig.miner(32)
    rig.contest(V1, late)
    bond = rig.lock(V1)
    raw = rig.request(late, V1, bond.operation_id)
    # When / Then: the 33rd is refused with its bond untouched
    refused_unchanged(rig, "QUEUE_CAPACITY_EXCLUDE_WITHOUT_FRAUD", lambda: rig.d.open(raw, now=1))
    assert (len(rig.d.unresolved()), dispute_rows(rig)) == (32, 32)
    # When one of the 32 closes, the 33rd is accepted
    rig.close(opened[5], "MATCH", None)
    assert rig.d.open(raw, now=4).contest.miner == late.ss58
    assert len(rig.d.unresolved()) == 32


# --- DIA-09: a bisect advances a dispute only on the exact turn --------------------------------


def test_a_bisect_advances_the_dispute_only_on_the_exact_turn_and_a_replay_never_rewinds(
    li_escrow: ModuleType,
) -> None:
    # Covers DIA-09.
    # Given: the miner has answered turn 0, so the auditor is expected at seq 1
    rig = make_rig(li_escrow)
    hot, auditor = rig.f.HOT, rig.f.AUDITOR
    opened, _ = rig.open(VERDICT)
    dispute = opened.dispute_id
    first, sent = rig.say(dispute, hot, MINER_SAYS)
    assert (first.seq, first.expected_party) == (1, auditor.ss58)
    # A different body for the already accepted seq is a conflict, not a second answer
    other = rig.answer(
        dispute,
        hot,
        AGREE,
        seq=0,
        level="step",
        ctx=[],
        interval=(0, 2),
        previous_transcript_hash=opened.transcript_hash,
    )
    refused_unchanged(rig, "SEQUENCE_REPLAY_CONFLICT", lambda: rig.d.bisect(other, now=2))
    # Every binding of the turn is checked: party, hash, level, context, interval and sequence
    for override in (
        {"party": hot.ss58},
        {"previous_transcript_hash": "aa" * 32},
        {"level": "layer", "ctx": [1]},
        {"interval": (0, 1)},
        {"seq": 2},
    ):
        key = hot if override.get("party") else auditor
        raw = rig.answer(dispute, key, AUDITOR_SAYS, **override)
        refused_unchanged(
            rig, "TURN_SEQUENCE_CONTEXT_PARTY_OR_HASH", lambda raw=raw: rig.d.bisect(raw, now=2)
        )
    # A body whose party is not its signer never reaches the turn check
    liar = rig.answer(dispute, hot, AUDITOR_SAYS, party=auditor.ss58)
    before = rig.state()
    raised(lambda: rig.d.bisect(liar, now=2), UnauthorizedSigner)
    assert rig.state() == before
    # A late answer is refused and does not renew the deadline
    on_time = rig.answer(dispute, auditor, AUDITOR_SAYS)
    refused_unchanged(
        rig,
        "CLOSED_PAUSED_OR_EXPIRED",
        lambda: rig.d.bisect(on_time, now=first.turn_deadline + 1),
    )
    # The exact turn advances; the replay of the first answer then returns the stored receipt
    second = rig.d.bisect(on_time, now=2)
    assert (second.seq, second.level, second.expected_party) == (2, "layer", hot.ss58)
    state = rig.state()
    assert rig.d.bisect(sent, now=3) == first
    assert rig.state() == state


def test_an_empty_next_interval_is_refused_and_the_answer_leaves_no_entry(
    li_escrow: ModuleType,
) -> None:
    # Covers DIA-09.
    # Given: a service whose accepted geometry reports an empty span for the next level
    rig = make_rig(li_escrow)
    opened, _ = rig.open(VERDICT)
    rig.say(opened.dispute_id, rig.f.HOT, MINER_SAYS)
    answer = rig.answer(opened.dispute_id, rig.f.AUDITOR, AUDITOR_SAYS)
    rig.d.span = lambda contest, level, ctx: 0
    # When / Then: the answer is refused whole, entry included
    refused_unchanged(rig, "NONEMPTY_INTERVAL_REQUIRED", lambda: rig.d.bisect(answer, now=2))
    assert rig.db.execute("SELECT COUNT(*) FROM dispute_entries_v2").fetchone()[0] == 1
    # And the very same answer is accepted once the span is real: nothing else stood in the way
    rig.d.span = lambda contest, level, ctx: 2
    assert rig.d.bisect(answer, now=2).level == "layer"


def test_no_answer_is_taken_from_a_paused_or_resolved_dispute(li_escrow: ModuleType) -> None:
    # Covers DIA-09, DIA-07.
    # Given: a dispute paused by two agreeing answers, and one closed by its referee
    rig = make_rig(li_escrow)
    paused, _ = rig.open(VERDICT)
    rig.say(paused.dispute_id, rig.f.HOT, AGREE)
    frozen, _ = rig.say(paused.dispute_id, rig.f.AUDITOR, AGREE)
    assert frozen.paused
    closed, _ = rig.open(V1, miner=rig.miner(1))
    live = rig.answer(closed.dispute_id, rig.miner(1), MINER_SAYS)
    rig.close(closed, "MATCH", None)
    # When / Then: both are shut to further answers
    late = rig.answer(paused.dispute_id, rig.f.HOT, MINER_SAYS)
    refused_unchanged(rig, "CLOSED_PAUSED_OR_EXPIRED", lambda: rig.d.bisect(late, now=3))
    refused_unchanged(rig, "CLOSED_PAUSED_OR_EXPIRED", lambda: rig.d.bisect(live, now=3))


# --- DIA-12, DIB-10: the bond burns only when the contesting miner loses by FRAUD --------------


@pytest.mark.parametrize(
    ("reason", "loser", "burned"),
    [
        pytest.param("MATCH", None, 0, id="audit-confirmed"),
        pytest.param("INFRASTRUCTURE", None, 0, id="infrastructure"),
        pytest.param("FRAUD", "auditor", 0, id="the-auditor-cheated"),
        pytest.param("FRAUD", "miner", BOND, id="the-miner-cheated"),
    ],
)
def test_the_bond_is_burned_only_when_the_contesting_miner_loses_by_fraud(
    li_escrow: ModuleType, reason: str, loser: str | None, burned: int
) -> None:
    # Covers DIA-12, DIB-10.
    # Given: a funded contest; production pays the referee nothing (referee_cost 0)
    rig = make_rig(li_escrow)
    turn, _ = rig.open(VERDICT)
    who = {None: None, "miner": rig.f.HOT.ss58, "auditor": rig.f.AUDITOR.ss58}[loser]
    cold = rig.f.COLD.ss58
    assert rig.escrow.balances(cold).available == 20000 - BOND
    # When the referee decides and the decision and the bond settle together
    rig.close(turn, reason, who)
    # Then: the whole bond is burned for a miner FRAUD, returned in every other case
    status = rig.d.settlement(turn.dispute_id)
    assert status.outcome == ("FRAUD" if burned else "MATCH")
    balances = rig.escrow.balances()
    assert (balances.burned, balances.dispute_locked) == (burned, 0)
    assert rig.escrow.balances(cold).available == 20000 - burned
    assert balances.conserved()
    # The disposal is one event, and settling again, even at a later round, repeats nothing
    assert settlement_events(rig.db, turn.lock_id) == ["SLASH" if burned else "RELEASE"]
    state = rig.state()
    rig.d.settle_lock(turn.dispute_id, now=4)
    assert rig.state() == state


def test_an_unresolved_contest_releases_nothing_whatever_the_clock_says(
    li_escrow: ModuleType,
) -> None:
    # Covers DIA-12, DIB-10.
    # Given: an open contest; its settlement is unresolved and points at the absolute horizon
    rig = make_rig(li_escrow)
    turn, _ = rig.open(VERDICT)
    status = rig.d.settlement(turn.dispute_id)
    assert (status.unresolved, status.release_beacon) == (True, turn.absolute_deadline)
    stub = EscrowRelease(
        operation_id="77" * 32,
        owner=rig.f.COLD.ss58,
        units=BOND,
        origin_ids=[rig.origin],
        admission_id=None,
        dispute_id=turn.dispute_id,
        lock_id=turn.lock_id,
        finality_hash=status.finality_hash,
        closed_dispute_root=status.closed_dispute_root,
    )
    late = turn.absolute_deadline + 10_000
    # When / Then: no release, slash or settle moves the bond while the contest is open
    before = rig.state()
    code = raised(
        lambda: rig.escrow.release(stub, signer=rig.f.COLD.ss58, now_beacon=late), EscrowError
    ).code
    assert code == "DISPUTED_OR_UNVESTED"
    code = raised(
        lambda: rig.escrow.slash(
            stub, authority=rig.f.COORD.ss58, now_beacon=late, max_referee_cost=BOND
        ),
        EscrowError,
    ).code
    assert code == "UNSETTLED_SLASH"
    assert refusal(lambda: rig.d.settle_lock(turn.dispute_id, now=late)) == "UNRESOLVED_CONTEST"
    assert rig.state() == before
    assert locked(rig, turn.lock_id) == BOND


# --- DIB-13: exactly one decision per dispute ---------------------------------------------------


def test_a_resolved_dispute_takes_no_second_decision_and_an_identical_replay_changes_nothing(
    li_escrow: ModuleType,
) -> None:
    # Covers DIB-13.
    # Given: a dispute closed MATCH and settled; fraud evidence is also on file for it
    rig = make_rig(li_escrow)
    turn, _ = rig.open(VERDICT)
    closed = rig.close(turn, "MATCH", None)
    fraud = rig.evidence(turn, "FRAUD", rig.f.HOT.ss58)
    rig.d.register_evidence(fraud)
    now = turn.turn_deadline + 1
    proofs = rig.availability(turn, (rig.f.COORD, rig.ref), now)
    rig.d.acknowledge(rig.ack(rig.f.HOT, 2), now=2)
    before = rig.state()
    # When / Then: a different resolution, a timeout, a bisect and a pause are all refused
    conflict = rig.signed_resolution(turn, fraud)
    assert refusal(lambda: rig.d.resolution(conflict, now=3)) == "RESOLUTION_CONFLICT"
    assert refusal(lambda: rig.d.timeout(turn.dispute_id, proofs, now=now)) == "TIMEOUT_NOT_DUE"
    late = rig.answer(turn.dispute_id, rig.f.HOT, MINER_SAYS)
    assert refusal(lambda: rig.d.bisect(late, now=3)) == "CLOSED_PAUSED_OR_EXPIRED"
    assert refusal(lambda: rig.d.pause(turn.dispute_id)) == "CLOSED_DISPUTE"
    assert rig.state() == before
    # An exclusion returns the standing decision; the identical resolution is accepted again
    assert rig.d.exclude(turn.dispute_id, now=now) == closed
    same = rig.signed_resolution(turn, rig.evidence(turn))
    assert rig.d.resolution(same, now=3) == closed
    rig.d.settle_lock(turn.dispute_id, now=3)
    assert rig.state() == before
    assert rig.d.get(turn.dispute_id).resolution == closed.resolution
    assert rig.escrow.balances().burned == 0


# --- DIA-19, DIB-01, DS-05: a resolution needs the referee and evidence the service registered -


def test_a_resolution_must_cite_evidence_the_service_registered_for_this_dispute_and_transcript(
    li_escrow: ModuleType,
) -> None:
    # Covers DIA-19, DIB-01, DS-05.
    # Given: two open disputes; evidence is on file only for the first
    rig = make_rig(li_escrow)
    one, _ = rig.open(VERDICT)
    two, _ = rig.open(V1, miner=rig.miner(1))
    evidence = rig.evidence(one, "FRAUD", rig.f.AUDITOR.ss58)
    rig.d.register_evidence(evidence)
    before = rig.state()

    def resolve(turn, cited, **override):
        return lambda: rig.d.resolution(rig.signed_resolution(turn, cited, **override), now=3)

    # When / Then: the referee cannot cite evidence nobody registered
    ghost = rig.evidence(one, "MATCH", op="never-registered")
    assert refusal(resolve(one, ghost)) == "UNVERIFIED_REFEREE_EVIDENCE"
    # ... nor another dispute's evidence ...
    stolen = rig.signed_resolution(two, evidence, dispute_id=two.dispute_id)
    assert refusal(lambda: rig.d.resolution(stolen, now=3)) == "REFEREE_EVIDENCE_BINDING"
    # ... nor a reason or a loser the evidence does not state
    assert refusal(resolve(one, evidence, reason="MATCH", loser=None)) == "REFEREE_EVIDENCE_BINDING"
    assert refusal(resolve(one, evidence, loser=rig.f.HOT.ss58)) == "REFEREE_EVIDENCE_BINDING"
    # ... nor sign after the absolute horizon
    assert (
        refusal(
            lambda: rig.d.resolution(
                rig.signed_resolution(one, evidence), now=one.absolute_deadline + 1
            )
        )
        == "RESOLUTION_ABSOLUTE_DEADLINE"
    )
    assert rig.state() == before
    # Control: the citation that matches is accepted
    assert rig.resolve(one, evidence).resolution is not None


def test_evidence_and_resolutions_of_an_older_transcript_are_refused(li_escrow: ModuleType) -> None:
    # Covers DIA-19, DIB-01.
    # Given: evidence registered at the opening transcript; then the dispute moves on
    rig = make_rig(li_escrow)
    opened, _ = rig.open(VERDICT)
    old = rig.evidence(opened)
    rig.d.register_evidence(old)
    moved, _ = rig.say(opened.dispute_id, rig.f.HOT, MINER_SAYS)
    before = rig.state()
    # When / Then: old evidence cannot be registered again, cited, or signed over the old hash
    assert refusal(lambda: rig.d.register_evidence(old)) == "REFEREE_STALE_EVIDENCE"
    cited = rig.signed_resolution(moved, old)
    assert refusal(lambda: rig.d.resolution(cited, now=3)) == "REFEREE_EVIDENCE_BINDING"
    stale = rig.signed_resolution(opened, old)
    assert refusal(lambda: rig.d.resolution(stale, now=3)) == "RESOLUTION_TYPE_OR_TRANSCRIPT"
    assert rig.state() == before


def test_a_resolution_can_name_only_one_of_the_two_parties_as_loser(li_escrow: ModuleType) -> None:
    # Covers DIA-19, DIB-01.
    # Given: registered FRAUD evidence against a hotkey that is in neither party
    rig = make_rig(li_escrow)
    turn, _ = rig.open(VERDICT)
    evidence = rig.evidence(turn, "FRAUD", rig.miner(9).ss58)
    rig.d.register_evidence(evidence)
    # When / Then: the referee's resolution cannot convict a bystander
    raw = rig.signed_resolution(turn, evidence)
    refused_unchanged(rig, "UNKNOWN_LOSER", lambda: rig.d.resolution(raw, now=3))
    assert rig.d.get(turn.dispute_id).resolution is None


# --- DIA-20: a state serve answers a coordinator request, once -----------------------------------


def test_a_state_serve_is_stored_only_against_a_live_request_addressed_to_the_signer(
    li_escrow: ModuleType,
    # Covers DIA-20.
) -> None:
    from hypertrain.protocol.envelope_v2 import seal
    from hypertrain.protocol.messages import StateServe

    # Given: an open dispute and one coordinator request for checkpoint 1
    rig = make_rig(li_escrow)
    hot, auditor = rig.f.HOT, rig.f.AUDITOR
    turn, _ = rig.open(VERDICT)

    def serve(key, t=1, challenge=None, uri="https://objects.invalid/checkpoint"):
        body = StateServe(
            hotkey=key.ss58,
            challenge_hash=challenge or turn.contest.challenge_hash,
            t=t,
            uri=uri,
            tensor_root="33" * 32,
            merkle_proof_leaf_in_leaves_root=[],
        )
        return canonicalize(seal(key, "StateServe", rig.d.run_id, body, 100_000))

    assert refusal(lambda: rig.d.state_serve(serve(hot), now=2)) == "UNREQUESTED_STATE_SERVE"
    rig.d.state_request(turn.dispute_id, 1, now=2)
    # When / Then: another checkpoint, another challenge, the auditor, a late serve: all refused
    for raw, now in (
        (serve(hot, t=0), 2),
        (serve(hot, challenge="44" * 32), 2),
        (serve(auditor), 2),
        (serve(hot), turn.absolute_deadline + 1),
    ):
        refused_unchanged(
            rig, "UNREQUESTED_STATE_SERVE", lambda raw=raw, now=now: rig.d.state_serve(raw, now=now)
        )
    # The answer is stored once; the same bytes again are a no-op, other bytes a conflict
    rig.d.state_serve(serve(hot), now=2)
    stored = rig.state()
    rig.d.state_serve(serve(hot), now=3)
    assert rig.state() == stored
    refused_unchanged(
        rig,
        "STATE_SERVE_REPLAY_CONFLICT",
        lambda: rig.d.state_serve(serve(hot, uri="https://objects.invalid/other"), now=3),
    )
    assert rig.db.execute("SELECT COUNT(*) FROM dispute_states_v2").fetchone()[0] == 1


def test_a_state_request_is_refused_for_a_closed_dispute_a_late_round_or_a_bad_checkpoint(
    li_escrow: ModuleType,
) -> None:
    # Covers DIA-20.
    # Given: a live dispute and one that is closed
    rig = make_rig(li_escrow)
    live, _ = rig.open(VERDICT)
    done, _ = rig.open(V1, miner=rig.miner(1))
    rig.close(done, "MATCH", None)
    # When / Then
    for dispute, checkpoint, now in (
        (done.dispute_id, 1, 4),
        (live.dispute_id, 1, live.absolute_deadline + 1),
        (live.dispute_id, live.contest.step_span + 1, 2),
        (live.dispute_id, -1, 2),
    ):
        refused_unchanged(
            rig,
            "STATE_REQUEST_CONTEXT",
            lambda dispute=dispute, checkpoint=checkpoint, now=now: rig.d.state_request(
                dispute, checkpoint, now=now
            ),
        )
    assert rig.d.state_request(live.dispute_id, live.contest.step_span, now=2).checkpoint == 2


# --- DIA-21: the dispute survives a restart and event cursors only grow ------------------------


def test_events_are_durable_cursors_grow_across_a_restart_and_a_lost_wakeup_delays_only(
    li_escrow: ModuleType,
    # Covers DIA-21.
) -> None:
    from hypertrain.challenge.disputes_v2 import DisputesV2

    # Given: three transitions (open, miner answer, auditor answer) with events for both parties
    rig = make_rig(li_escrow)
    hot, auditor = rig.f.HOT.ss58, rig.f.AUDITOR.ss58
    opened, _ = rig.open(VERDICT)
    rig.say(opened.dispute_id, rig.f.HOT, MINER_SAYS)
    layer, _ = rig.say(opened.dispute_id, rig.f.AUDITOR, AUDITOR_SAYS)
    seen = [e.cursor for p in (hot, auditor) for e in rig.d.events(p, 0)]
    assert len(seen) == 3
    # When the service restarts over the same database
    restarted = DisputesV2(
        rig.escrow,
        rig.d.policy,
        rig.f.COORD,
        contest_of=rig.d.contest_of,
        owner_of=rig.d.owner_of,
        span=rig.d.span,
    )
    assert restarted.get(opened.dispute_id) == layer
    rig.d = restarted
    more, _ = rig.say(opened.dispute_id, rig.f.HOT, MINER_SAYS)
    # Then: the new event has a larger cursor than every earlier one; a cursor reads only newer
    latest = restarted.events(auditor, 0)[-1]
    assert latest.cursor > max(seen) and latest.turn == more
    mine = restarted.events(hot, 0)
    assert [e.cursor for e in mine] == sorted(e.cursor for e in mine)
    assert [e.cursor for e in restarted.events(hot, mine[0].cursor)] == [e.cursor for e in mine[1:]]
    assert restarted.events(hot, mine[-1].cursor) == []

    # And a wake-up that never arrives only delays a waiting reader: the table is the truth.
    # The reader's wait ends when the test says its timeout has run out, so no wall-clock delay
    # decides whether the event was written before the reader looked again.
    class Lapsing(threading.Condition):
        def __init__(self) -> None:
            super().__init__()
            self.waiting, self.lapsed = threading.Event(), threading.Event()

        def wait_for(self, predicate, timeout=None):
            self.waiting.set()
            assert self.lapsed.wait(10)
            return super().wait_for(predicate, timeout=0)

    restarted.notify = lambda: None
    restarted.condition = lapsing = Lapsing()
    heard: list = []
    reader = threading.Thread(
        target=lambda: heard.extend(restarted.events(hot, mine[-1].cursor, timeout=30))
    )
    reader.start()
    assert lapsing.waiting.wait(10)
    rig.say(opened.dispute_id, rig.f.AUDITOR, AUDITOR_SAYS)
    lapsing.lapsed.set()
    reader.join(10)
    assert not reader.is_alive()
    assert [e.turn.level for e in heard] == ["op"]


# --- DIA-18, SG-14: an acknowledgement binds the addressed party to the exact event -------------


def test_an_ack_is_stored_only_from_the_addressed_party_over_the_exact_event(
    li_escrow: ModuleType,
) -> None:
    # Covers DIA-18, SG-14.
    # Given: a dispute opened at beacon 3, so the miner's event was issued at 3
    rig = make_rig(li_escrow)
    hot, auditor = rig.f.HOT, rig.f.AUDITOR
    turn, _ = rig.open(VERDICT, now=3)
    now, late = 5, turn.turn_deadline + 1
    forged = rig.ack(hot, now).model_copy(update={"sig": "00" * 64})
    cases = [
        (rig.ack(hot, now, signer=auditor), now, "ACK_AUTHORITY_OR_TIME"),
        (forged, now, "ACK_AUTHORITY_OR_TIME"),
        (rig.ack(hot, now, run_id="aa" * 32), now, "ACK_AUTHORITY_OR_TIME"),
        (rig.ack(hot, now + 1), now, "ACK_AUTHORITY_OR_TIME"),
        (rig.ack(hot, now, party=auditor.ss58, signer=auditor), now, "ACK_UNKNOWN_EVENT"),
        (rig.ack(hot, now, event_hash="ab" * 32), now, "ACK_EVENT_OR_DEADLINE"),
        (rig.ack(hot, 2), now, "ACK_EVENT_OR_DEADLINE"),
        (rig.ack(hot, late), late, "ACK_EVENT_OR_DEADLINE"),
    ]
    # When / Then: each is refused with its own code and nothing is stored
    for ack, at, code in cases:
        refused_unchanged(rig, code, lambda ack=ack, at=at: rig.d.acknowledge(ack, now=at))
    assert rig.db.execute("SELECT COUNT(*) FROM dispute_acks_v2").fetchone()[0] == 0
    # The addressed party over the exact event is stored once; the same ack again is a no-op
    good = rig.ack(hot, now)
    rig.d.acknowledge(good, now=now)
    stored = rig.state()
    rig.d.acknowledge(good, now=now)
    assert rig.state() == stored
    assert rig.db.execute("SELECT COUNT(*) FROM dispute_acks_v2").fetchone()[0] == 1


# --- DIA-17: watch events are the coordinator's, and authenticity is not authority --------------


def test_a_served_event_is_signed_by_the_coordinator_and_no_field_can_be_altered(
    li_escrow: ModuleType,
    # Covers DIA-17.
) -> None:
    from hypertrain.challenge.disputes_v2 import authentic

    # Given: the events the service issued for an opened dispute
    rig = make_rig(li_escrow)
    turn, _ = rig.open(VERDICT)
    event = rig.d.events(rig.f.HOT.ss58, 0)[0]
    # Then: the coordinator signed it and it verifies
    assert (event.signer, authentic(event)) == (rig.f.COORD.ss58, True)
    # When any field the consumer relies on is altered, the signature no longer verifies
    for update in (
        {"cursor": event.cursor + 1},
        {"party": rig.f.AUDITOR.ss58},
        {"issued_beacon": event.issued_beacon + 1},
        {"turn": event.turn.model_copy(update={"turn_deadline": event.turn.turn_deadline + 1})},
        {"checkpoint": 1},
        {"kind": "state"},
    ):
        assert authentic(event.model_copy(update=update)) is False
    assert event.turn == turn


def test_a_self_signed_event_is_authentic_but_only_the_coordinator_signer_is_authority(
    li_escrow: ModuleType,
    # Covers DIA-17.
) -> None:
    from hypertrain.challenge.disputes_v2 import WatchEvent, authentic, signed_message

    # Given: the condition of DIA-17 broken in a stand-in: a stranger writes an event for the
    # miner and signs it with its own key
    rig = make_rig(li_escrow)
    turn, _ = rig.open(VERDICT)
    real = rig.d.events(rig.f.HOT.ss58, 0)[0]
    stranger = rig.miner(5)
    fake = real.model_copy(update={"signer": stranger.ss58, "sig": ""})
    fake = fake.model_copy(update={"sig": stranger.sign(signed_message(fake)).hex()})
    assert isinstance(fake, WatchEvent) and turn.dispute_id == fake.turn.dispute_id
    # Then: the signature check alone accepts it, which is why consumers pin the signer; the
    # service's own events carry the coordinator key and a forged one does not
    assert authentic(fake) is True
    assert {e.signer for e in rig.d.events(rig.f.HOT.ss58, 0)} == {rig.f.COORD.ss58}
    assert fake.signer != rig.f.COORD.ss58
    assert authentic(fake.model_copy(update={"signer": rig.f.COORD.ss58})) is False


# --- DIB-14, DIA-17: a timeout needs delivery proof and two distinct healthy permitted witnesses


@pytest.mark.parametrize("silent", ["miner", "auditor"])
def test_a_timeout_needs_the_acknowledged_turn_and_two_distinct_healthy_permitted_witnesses(
    li_escrow: ModuleType, silent: str
) -> None:
    # Covers DIB-14, DIA-17, DIA-12.
    # Given: the silent party's turn is open and past its deadline; the earlier turn of the same
    # party was acknowledged, which must not count as delivery of this one. The policy pins both
    # parties as referees, so only the witness rule itself can keep them from witnessing.
    rig = make_rig(li_escrow, pin=(li_escrow.HOT, li_escrow.AUDITOR))
    hot, auditor, coord, ref = rig.f.HOT, rig.f.AUDITOR, rig.f.COORD, rig.ref
    opened, _ = rig.open(VERDICT)
    rig.d.acknowledge(rig.ack(hot, 2), now=2)
    turn = opened
    if silent == "auditor":
        turn, _ = rig.say(opened.dispute_id, hot, MINER_SAYS)
    else:
        rig.say(opened.dispute_id, hot, MINER_SAYS)
        turn, _ = rig.say(opened.dispute_id, auditor, AUDITOR_SAYS)
    quiet = hot if silent == "miner" else auditor
    assert turn.expected_party == quiet.ss58
    due = turn.turn_deadline + 1
    good = rig.availability(turn, (coord, ref), due)

    def timeout(proofs, now=due):
        return lambda: rig.d.timeout(turn.dispute_id, proofs, now=now)

    # When / Then: not before the deadline; not without delivery of this very turn
    refused_unchanged(
        rig, "TIMEOUT_NOT_DUE", timeout(rig.availability(turn, (coord, ref), due - 1), due - 1)
    )
    refused_unchanged(rig, "NO_AUTHENTICATED_TURN_DELIVERY", timeout(good))
    rig.d.acknowledge(rig.ack(quiet, due - 1), now=due - 1)
    # ... not with a witness who is a party, a stranger, or the same observer twice
    stranger = rig.miner(6)
    for proofs in (
        rig.availability(turn, (hot, ref), due),
        rig.availability(turn, (auditor, ref), due),
        rig.availability(turn, (stranger, ref), due),
        (good[0], good[0]),
    ):
        refused_unchanged(rig, "UNCONFIRMED_AVAILABILITY", timeout(proofs))
    # ... not when a proof is about another turn, another head, unhealthy or not signed
    for override in (
        {"seq": turn.seq + 1},
        {"transcript_hash": "aa" * 32},
        {"dispute_id": "bb" * 32},
        {"run_id": "cc" * 32},
        {"observed_beacon": due - 1},
        {"service_healthy": False},
        {"beacon_healthy": False},
        {"reference_healthy": False},
    ):
        refused_unchanged(
            rig,
            "UNCONFIRMED_AVAILABILITY",
            timeout((good[0], rig.availability(turn, (ref, ref), due, **override)[0])),
        )
    unsigned = good[1].model_copy(update={"sig": "00" * 64})
    refused_unchanged(rig, "UNCONFIRMED_AVAILABILITY", timeout((good[0], unsigned)))
    assert rig.d.get(turn.dispute_id).resolution is None
    # Control: with the delivery and two honest permitted observers the party loses the turn,
    # not the bond
    closed = rig.d.timeout(turn.dispute_id, good, now=due)
    rig.d.settle_lock(turn.dispute_id, now=due)
    assert closed.resolution is not None
    assert (closed.resolution.reason, closed.resolution.loser) == (
        "PARTY_TIMEOUT" if silent == "miner" else "AUDITOR_FAULT",
        quiet.ss58,
    )
    assert rig.d.settlement(turn.dispute_id).outcome == "MATCH"
    balances = rig.escrow.balances()
    assert (balances.burned, balances.dispute_locked, balances.conserved()) == (0, 0, True)


# --- DIA-07: transitions are monotone and a terminal state never changes ------------------------


STEPS = (
    "diverge",
    "diverge",
    "diverge",
    "agree",
    "replay",
    "stale",
    "pause",
    "reassign",
    "match",
    "fraud_miner",
    "infra",
    "exclude",
    "tick",
    "jump",
)


CLOSERS = {
    "match": ("MATCH", None),
    "fraud_miner": ("FRAUD", "miner"),
    "infra": ("INFRASTRUCTURE", None),
}


# A decided dispute that was also paused: the only state in which a late reassignment is not
# already refused for being unpaused, so a random draw would rarely reach it.
@example(ops=["agree", "agree", "match", "reassign"])
@given(ops=st.lists(st.sampled_from(STEPS), max_size=14))
def test_any_sequence_of_moves_keeps_a_dispute_monotone_and_its_decision_final(
    li_escrow: ModuleType, ops: list[str]
) -> None:
    # Covers DIA-07, DIB-13.
    # Given: an open funded contest and a clock that only ever moves forward
    rig = make_rig(li_escrow)
    hot, auditor = rig.f.HOT, rig.f.AUDITOR
    opened, _ = rig.open(VERDICT)
    dispute, now = opened.dispute_id, 2
    sent = []
    for op in ops:
        before = rig.d.get(dispute)
        digest = rig.state()
        accepted = True
        try:
            if op in ("agree", "diverge"):
                key = hot if before.expected_party == hot.ss58 else auditor
                says = AGREE if op == "agree" else (MINER_SAYS if key is hot else AUDITOR_SAYS)
                raw = rig.answer(dispute, key, says)
                sent.append((raw, rig.d.bisect(raw, now=now)))
            elif op == "replay":
                if sent:
                    assert rig.d.bisect(sent[0][0], now=now) == sent[0][1]
                    assert rig.state() == digest
            elif op == "stale":
                stale = rig.answer(dispute, hot, AGREE, previous_transcript_hash="aa" * 32)
                raised(lambda raw=stale, at=now: rig.d.bisect(raw, now=at), DisputeError)
                accepted = False
            elif op == "pause":
                rig.d.pause(dispute)
            elif op == "reassign":
                rig.d.reassign(dispute, rig.next_ref.ss58, now=now)
            elif op in CLOSERS:
                reason, loser = CLOSERS[op]
                evidence = rig.evidence(before, reason, hot.ss58 if loser else None)
                rig.d.register_evidence(evidence)
                digest = rig.state()
                with rig.escrow.tx():
                    rig.resolve(before, evidence, now=now)
                    rig.d.settle_lock(dispute, now=now)
            elif op == "exclude":
                with rig.escrow.tx():
                    rig.d.exclude(dispute, now=now)
                    rig.d.settle_lock(dispute, now=now)
            else:
                now += 1 if op == "tick" else 700
        except (DisputeError, EscrowError):
            accepted = False
        after = rig.d.get(dispute)
        # A refused move leaves no row behind
        if not accepted:
            assert rig.state() == digest
        # Monotone: the sequence grows by one per accepted answer, nothing is ever un-paused
        # and the horizon is the one fixed at opening
        assert after.seq in (before.seq, before.seq + 1)
        assert (after.seq == before.seq + 1) == (op in ("agree", "diverge") and accepted)
        assert after.paused >= before.paused
        assert after.absolute_deadline == opened.absolute_deadline
        assert after.turn_deadline <= after.absolute_deadline
        assert before.reassignments <= after.reassignments <= 1
        if op == "pause" and accepted:
            assert after.paused
        if op == "reassign" and accepted:
            assert (after.contest.referee, after.reassignments) == (rig.next_ref.ss58, 1)
        # Final: once decided, the turn, its decision and its beacon never change
        if before.resolution is not None:
            assert after == before
        if op in CLOSERS and accepted or op == "exclude" and accepted:
            assert after.resolution is not None and after.resolved_beacon in (
                now,
                before.resolved_beacon,
            )
        # The bond follows the decision: held while open, then released, or burned for FRAUD
        balances = rig.escrow.balances()
        verdict_burns = after.resolution is not None and (
            (after.resolution.reason, after.resolution.loser) == ("FRAUD", hot.ss58)
        )
        assert balances.conserved()
        assert balances.dispute_locked == (0 if after.resolution is not None else BOND)
        assert balances.burned == (BOND if verdict_burns else 0)


def test_an_answer_near_the_end_never_extends_its_turn_past_the_absolute_horizon(
    li_escrow: ModuleType,
) -> None:
    # Covers DIA-07.
    # Given: levels of 64 positions, deeper than the horizon (12 turn windows, sized at opening
    # from the contest's own spans) allows for, so a descent can outlast it
    rig = make_rig(li_escrow)
    rig.d.span = lambda contest, level, ctx: 64
    opened, _ = rig.open(VERDICT)
    window = rig.escrow.manifest.training.verify.T.dispute_per_level
    turn, now = opened, opened.turn_deadline - 1
    # When each answer arrives one beacon before the deadline it was given
    for _ in range(30):
        key = rig.f.HOT if turn.expected_party == rig.f.HOT.ss58 else rig.f.AUDITOR
        says = MINER_SAYS if key is rig.f.HOT else [MINER_SAYS[0], MINER_SAYS[1], "dd" * 32]
        turn, _ = rig.say(opened.dispute_id, key, says, now=now)
        # Then: no turn ever outlives the horizon fixed at the opening
        assert turn.turn_deadline <= opened.absolute_deadline
        assert turn.absolute_deadline == opened.absolute_deadline
        if now + window > opened.absolute_deadline:
            break
        now = turn.turn_deadline - 1
    # Control: the last answer came so late that its own window alone would have crossed the
    # horizon, and the horizon is what ended the turn
    assert now + window > opened.absolute_deadline
    assert turn.turn_deadline == opened.absolute_deadline


# --- DIA-13: the decision and the settlement of the bond commit together (CONDITIONAL) ----------


class SettleFails(Exception):
    """A fault injected into the bond release, after the decision has been written."""


def fail_release(rig: Rig, monkeypatch: pytest.MonkeyPatch) -> None:
    def release(*args, **kwargs):
        raise SettleFails

    # settle_lock releases through the ledger's service entry point where there is one
    for name in ("release", "release_settlement"):
        if hasattr(rig.escrow, name):
            monkeypatch.setattr(rig.escrow, name, release)


def test_a_failing_settlement_leaves_the_dispute_unresolved_when_both_share_one_transaction(
    li_escrow: ModuleType, monkeypatch: pytest.MonkeyPatch
) -> None:
    # Covers DIA-13, DIB-10.
    # Given: the condition holds: decision and settlement run in one transaction, as in the store
    rig = make_rig(li_escrow)
    turn, _ = rig.open(VERDICT)
    evidence = rig.evidence(turn)
    rig.d.register_evidence(evidence)
    before = rig.state()
    fail_release(rig, monkeypatch)
    # When the release fails after the resolution was written
    with pytest.raises(SettleFails), rig.escrow.tx():
        rig.resolve(turn, evidence)
        rig.d.settle_lock(turn.dispute_id, now=3)
    # Then: nothing of the decision survives, so the bond is never stranded behind a closed dispute
    assert rig.state() == before
    assert rig.d.get(turn.dispute_id).resolution is None
    assert locked(rig, turn.lock_id) == BOND
    monkeypatch.undo()
    with rig.escrow.tx():
        rig.resolve(turn, evidence)
        rig.d.settle_lock(turn.dispute_id, now=3)
    assert (rig.d.get(turn.dispute_id).resolution is not None, locked(rig, turn.lock_id)) == (
        True,
        0,
    )


# --- DIA-19: the referee cannot introduce the evidence it signs over (CONDITIONAL) --------------


def test_a_referee_cannot_convict_with_evidence_the_service_never_registered(
    li_escrow: ModuleType,
) -> None:
    # Covers DIA-19, DIB-01, DS-05.
    # Given: an open dispute and a referee who signs FRAUD against the miner over evidence it wrote.
    # The condition of DIA-19 is that register_evidence stays an internal capability of the
    # service, so nothing a claimant or the referee signs ever reaches it.
    rig = make_rig(li_escrow)
    turn, _ = rig.open(VERDICT)
    invented = rig.evidence(turn, "FRAUD", rig.f.HOT.ss58, op="invented-by-the-referee")
    # When / Then: the resolution is refused and the bond is untouched
    raw = rig.signed_resolution(turn, invented)
    refused_unchanged(rig, "UNVERIFIED_REFEREE_EVIDENCE", lambda: rig.d.resolution(raw, now=3))
    assert rig.escrow.balances().burned == 0


# --- DIB-06: an adverse audit verdict earns the miner no reward ------------------------------


@pytest.mark.parametrize("result", ["MISMATCH", "WITHHELD", "BAD_PROOF", "ASSIGNMENT_VIOLATION"])
def test_a_miner_with_an_adverse_audit_verdict_is_minted_nothing_even_if_the_finalize_names_it(
    li_escrow: ModuleType, real_reward: Real, result: str
) -> None:
    # Covers DIB-06.
    # Given: the real w=0 round, with the auditor's verdict replaced by an adverse one it signed
    setup, evidence, reward = real_reward
    work = evidence.works[0]
    run_id = setup.manifest.run_id()
    matched = ReplayVerdict.model_validate(work.verdict.body)
    adverse = ReplayVerdict(
        challenge_hash=matched.challenge_hash,
        first_bad_leaf=0 if result == "MISMATCH" else None,
        result=result,  # type: ignore[arg-type]
        recomputed_leaves_root=matched.recomputed_leaves_root,
        replay_env=matched.replay_env,
    )
    signed = envelope_v2.EnvelopeV2.model_validate(
        envelope_v2.seal(li_escrow.AUDITOR, "ReplayVerdict", run_id, adverse, 200)
    )
    refused = replace(evidence, works=(replace(work, verdict=signed),))
    escrow = bare_escrow(li_escrow, setup)
    # When / Then: the coordinator-signed reward that lists the miner is refused and mints nothing
    assert error_code(lambda: escrow.reward_finalize(reward, refused)) == "UNVERIFIED_REWARD_WORK"
    assert (escrow.balances().issued, escrow.balances().reward_pending) == (0, 0)
    # Control: the same reward with the MATCH verdict it was built on is issued in full
    escrow.reward_finalize(reward, evidence)
    assert escrow.balances().issued == 1_000_000


def test_a_reward_cannot_be_minted_for_a_hotkey_without_verified_work(
    li_escrow: ModuleType, real_reward: Real
) -> None:
    # Covers DIB-06.
    # Given: a coordinator-signed Finalize that also lists a hotkey with no verified work
    setup, evidence, _ = real_reward
    extra = [li_escrow.HOT.ss58, li_escrow.OTHER.ss58]
    escrow = bare_escrow(li_escrow, setup)
    reward, listed = signed_variant_on(escrow, li_escrow, real_reward, included=extra)
    # When / Then: the included set must equal the verified set exactly
    assert error_code(lambda: escrow.reward_finalize(reward, listed)) == (
        "NO_VERIFIED_WORK_OR_EXCLUSIONS"
    )
    assert escrow.balances().issued == 0


# --- DIB-24: racing closers end in one decision; the rollback revision sees disputes (CONDITIONAL)


def test_a_timeout_racing_a_resolution_has_one_winner_and_the_loser_is_refused(
    li_escrow: ModuleType,
    # Covers DIB-24, DIB-13.
) -> None:
    def prepared() -> tuple[Rig, object, tuple]:
        rig = make_rig(li_escrow)
        turn, _ = rig.open(VERDICT)
        rig.d.acknowledge(rig.ack(rig.f.HOT, 2), now=2)
        due = turn.turn_deadline + 1
        return rig, turn, rig.availability(turn, (rig.f.COORD, rig.ref), due)

    # Given (control): the timeout alone is valid and closes the dispute against the miner
    alone, turn, proofs = prepared()
    due = turn.turn_deadline + 1
    assert alone.d.timeout(turn.dispute_id, proofs, now=due).resolution.reason == "PARTY_TIMEOUT"
    # Given: the same dispute while a referee resolution is half way through its transaction
    rig, turn, proofs = prepared()
    evidence = rig.evidence(turn)
    rig.d.register_evidence(evidence)
    inside, proceed, finished = threading.Event(), threading.Event(), threading.Event()
    attempting = threading.Event()
    seen: dict[str, str] = {}

    def resolver() -> None:
        with rig.escrow.tx():
            rig.resolve(turn, evidence, now=due)
            inside.set()
            assert proceed.wait(10)
            rig.d.settle_lock(turn.dispute_id, now=due)

    def timer() -> None:
        assert inside.wait(10)
        attempting.set()
        try:
            rig.d.timeout(turn.dispute_id, proofs, now=due)
        except DisputeError as error:
            seen["timeout"] = error.code
        finished.set()

    threads = [threading.Thread(target=resolver), threading.Thread(target=timer)]
    for thread in threads:
        thread.start()
    assert inside.wait(10)
    # When: the timeout is attempted, it waits behind the open transaction
    assert attempting.wait(10)
    assert not finished.wait(0.3)
    proceed.set()
    for thread in threads:
        thread.join(10)
        assert not thread.is_alive()
    # Then: the referee's decision stands alone, settled once, and the timeout was refused
    assert seen == {"timeout": "TIMEOUT_NOT_DUE"}
    decided = rig.d.get(turn.dispute_id).resolution
    assert decided is not None and decided.reason == "MATCH"
    settles = settlement_events(rig.db, turn.lock_id)
    balances = rig.escrow.balances()
    assert (settles, balances.burned, balances.dispute_locked) == (["RELEASE"], 0, 0)
