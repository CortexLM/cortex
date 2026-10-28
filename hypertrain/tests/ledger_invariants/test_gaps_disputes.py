"""Gaps of the disputes group: what the first wave left unpinned (D39, disputes gap wave).

Each test states a defence the protocol must hold: "the system refuses X and its state is
unchanged", "the contest is closed without a loser", "the next round is paid". A hole is a strict
xfail that first runs a control proving the scenario reaches the point under test, then asserts
the desired defence inside `defence()`, so that only a missing defence (`DefenceMissing`, or
`StepRefused` for a step the service takes for itself) is the expected failure and a broken
control fails the test. Its reason starts with the finding id and says [candidate] only when the
failure was not reproduced. The heavy world (`audited`) is a real five-member round on the tiny
CPU model, about 25 s; every other test runs in-process in a second or two.
"""

from __future__ import annotations

import json
from collections.abc import Iterator
from dataclasses import replace
from pathlib import Path
from types import ModuleType
from typing import Any

import pytest
from fastapi.testclient import TestClient

from hypertrain.challenge.disputes_v2 import EXCLUSION_ABANDONED, Turn
from hypertrain.challenge.finality_v2 import exclude, require_open
from hypertrain.protocol.messages import Receipt
from hypertrain.protocol.messages_v2 import BisectV2, ResolutionV2

from . import support_exit_disputes as X
from . import support_exit_roster as R
from .support import World, at, service_world
from .support_disputes import BOND, MINER_SAYS, VERDICT, Rig, contested, make_rig, refusal
from .support_gaps_disputes import (
    PENDING,
    THETA,
    admission_code,
    admission_snapshot,
    applied_round,
    audited_round,
    available_of,
    client_transfer,
    closing_round,
    contest_request,
    defence,
    dispute_id_of_member,
    evidence_tag,
    fund_extra_bond,
    lock_of,
    post_dispute_on,
    put_finality,
    refused_unchanged,
    replayable_ledger,
    reservation_refusal,
    says,
    seed_round_origins,
    settlement_id,
    signed_round,
    state_of_member,
    suspended_on_a_dispute,
    two_windows,
    verdict_of_member,
)
from .support_locks import STAKE, Lab
from .support_locks import Rig as ServiceRig
from .support_opids import DefenceMissing

MISMATCH_EVIDENCE = "cd" * 32  # the evidence a confirmed audit mismatch seizes the bond under

# --- T-DSP-12 (DIB-02): a final op reached from an op interval longer than one op --------------


def descend(rig: Rig, op_span: int) -> Turn:
    """Both parties answer every turn, the auditor disagreeing from the first op on, until the
    dispute pauses. The op level of the layout has `op_span` ops."""
    rig.d.span = lambda contest, level, ctx: op_span if level == "op" else 2
    turn, _ = rig.open(VERDICT)
    hot, auditor = rig.f.HOT, rig.f.AUDITOR
    while not turn.paused:
        miner_turn = turn.expected_party == hot.ss58
        turn, _ = rig.say(
            turn.dispute_id, hot if miner_turn else auditor, says(turn, miner=miner_turn)
        )
    return turn


def final_op_answers(rig: Rig, turn: Turn) -> list[BisectV2]:
    """The two latest signed answers of the dispute, as the referee reads them for a final op."""
    return [
        BisectV2.model_validate_json(row[0])
        for row in rig.db.execute(
            "SELECT body FROM dispute_entries_v2 WHERE id=? ORDER BY seq DESC LIMIT 2",
            (turn.dispute_id,),
        )
    ]


@pytest.mark.xfail(
    strict=True,
    raises=DefenceMissing,
    reason="DIB-02: bisect() pauses on the narrowed one-op interval while the two last signed "
    "answers still carry the previous, longer one, so referee_v2 of the store can never "
    "see both claimants' outputs of that op; reproduced at the engine level, whether the shipped "
    "trace layout has an op level of more than one op is not determined "
    "[fix-lot: none (no design yet)]",
)
@pytest.mark.parametrize("op_span", [2, 5])
def test_a_dispute_paused_on_a_final_op_holds_both_parties_signed_claims_about_that_op(
    li_escrow: ModuleType, op_span: int
) -> None:
    """DIB-02 / T-DSP-12: the referee of a final op reads the input and the output of that very
    op from the two latest signed answers (their first and last hash), so both must be answers
    over exactly the paused turn's level, context and one-op interval, whatever the op count of
    the layer the dispute went down."""
    # Control: with a one-op layer the engine pauses on the interval both parties answered over.
    single = make_rig(li_escrow)
    paused = descend(single, op_span=1)
    answers = final_op_answers(single, paused)
    assert (paused.level, paused.at_final_op) == ("op", True)
    assert {a.party for a in answers} == {paused.contest.miner, paused.contest.auditor}
    assert [(a.level, a.ctx, a.interval) for a in answers] == [
        ("op", paused.ctx, paused.interval)
    ] * 2
    # Given: the same dispute on a layer with several ops, narrowed to one of them.
    rig = make_rig(li_escrow)
    paused = descend(rig, op_span=op_span)
    assert (paused.level, paused.at_final_op) == ("op", True)
    # When / Then: the answers the referee will read are about that op.
    answers = final_op_answers(rig, paused)
    with defence():
        assert {a.party for a in answers} == {paused.contest.miner, paused.contest.auditor}
        assert [(a.level, a.ctx, a.interval) for a in answers] == [
            ("op", paused.ctx, paused.interval)
        ] * 2


# --- T-DSP-30 (CL-59): a resolution route refuses an envelope that names no dispute -----------


@pytest.fixture
def world(tmp_path: Path) -> Iterator[World]:
    with service_world(tmp_path) as built:
        yield built


@pytest.mark.xfail(
    strict=True,
    raises=DefenceMissing,
    reason="CL-59: DisputesV2.resolution() reads env.body['dispute_id'] before it looks at the "
    "envelope type, so a signed envelope of another type ends as an unhandled KeyError, HTTP 500 "
    "(the handlers of app.py cover DisputeError, not KeyError) [fix-lot: none (no design yet)]",
)
def test_a_resolution_route_refuses_a_signed_envelope_that_names_no_dispute(world: World) -> None:
    """CL-59 / T-DSP-30: whatever a registered key signs and posts to /resolution, the answer is
    a stable refusal (409 or 422), never a server error, and no row moves."""
    # Given: a registered miner and the signed envelope of another message type, which has no
    # dispute id (the service answers a server error as a response, not as a raised exception).
    member = contested(world, 0)
    net = world.network
    quiet = TestClient(world.app, raise_server_exceptions=False)
    before = world.db_digest()
    unrelated = Receipt(w=0, commit_hash="ab" * 32, received_round=net.now)
    # Control: a resolution of an unknown dispute is refused with its own code, and the same
    # envelope on the bisect route is refused as the wrong type: the scenario reaches the routes.
    nobody = ResolutionV2(
        dispute_id=VERDICT,
        transcript_hash=VERDICT,
        reason="MATCH",
        loser=None,
        evidence_hash=VERDICT,
    )
    refused_unknown = quiet.post(
        net.url + "/resolution", json=net.signed(X.service().REFEREE, "ResolutionV2", nobody)
    )
    assert (refused_unknown.status_code, refused_unknown.json()["detail"]) == (
        409,
        "UNKNOWN_DISPUTE",
    )
    wrong_route = quiet.post(net.url + "/bisect", json=net.signed(member.hot, "Receipt", unrelated))
    assert (wrong_route.status_code, wrong_route.json()["detail"]) == (409, "BISECT_TYPE")
    assert world.db_digest() == before
    # When: the same envelope goes to the resolution route.
    answer = quiet.post(net.url + "/resolution", json=net.signed(member.hot, "Receipt", unrelated))
    # Then: refused with a stable status, nothing moved.
    with defence():
        assert answer.status_code in (409, 422), answer.text
        assert world.db_digest() == before


# --- X-DSP03-SQUAT (ESC-04): the beacon's closure of a stuck contest cannot be vetoed ---------


def stalled_outcome(world: World, *, squat: bool) -> tuple[Any, ...]:
    """Open miner 0's contest, let nobody answer it, optionally let a funded bystander sign an
    escrow event under the operation id the service will use to release the bond, then push the
    beacon to the closing round. Returns what the world looks like afterwards."""
    escrow = world.store._services(world.run_id)[0]
    dispute_id = X.open_contest(world, 0)
    if squat:
        bystander = X.service().COLD[3]
        client_transfer(world, bystander, settlement_id(dispute_id), world.network.origin_ids[3])
    world.network.push(closing_round(world, dispute_id))
    if not squat:
        # Control of the squat: it takes exactly the id the service releases the bond under.
        released = escrow.db.execute("SELECT receipt FROM escrow_events WHERE kind='RELEASE'")
        assert [json.loads(row[0])["operation_id"] for row in released] == [
            settlement_id(dispute_id)
        ]
    closed = X.disputes(world).get(dispute_id).resolution
    balances = escrow.balances()
    return (
        None if closed is None else (closed.reason, closed.loser),
        (balances.dispute_locked, balances.burned, balances.conserved()),
        X.unresolved_of(world),
        X.miner_state(world, 0)[:2],
    )


def test_a_client_event_cannot_stop_the_beacon_from_closing_a_stuck_contest(
    tmp_path: Path,
) -> None:
    """X-DSP03-SQUAT / ESC-04 x DIA-14: the exit of a contest nobody can decide ends in
    settle_lock, so it must not depend on an id a bystander can occupy. With or without a
    bystander event under the settlement id, the contest closes with no loser, the bond is back,
    the hold is gone and the miner is on probation."""
    # Given: the control world, where nobody touches the settlement id, closes the contest.
    with service_world(tmp_path / "control") as control:
        closed = stalled_outcome(control, squat=False)
    assert closed == (("INFRASTRUCTURE", None), (0, 0, True), 0, ("PROBATION", 0))
    # When: the same contest, the same beacon, and a bystander event under the settlement id.
    with service_world(tmp_path / "contended") as world:
        contended = stalled_outcome(world, squat=True)
    # Then: the closure happened all the same.
    assert contended == closed


# --- T-DSP-04 (DIB-04): a member contesting its own MATCH verdict does not wedge its round -----


@pytest.fixture
def audited(tmp_path: Path) -> Iterator[R.Round]:
    with audited_round(tmp_path) as rnd:
        yield rnd


def test_a_member_contesting_its_own_matching_verdict_does_not_stop_the_round_of_the_others(
    audited: R.Round,
) -> None:
    """DIB-04 / T-DSP-04: one roster member files a contest against its own MATCH verdict. Either
    the contest is refused and the miner is untouched, or the operator still aggregates the round
    with that member left out, finalizes the four others and pays the four others."""
    rnd, contester = audited, 2
    others = [i for i in rnd.members if i != contester]
    # Control: five audited members, nothing applied, the round is ready for its aggregation.
    R.control(rnd.finality()["unresolved_audits"] == 0, "every audit of the round is complete")
    R.control(not rnd.finality()["applied"], "the round is not aggregated yet")
    R.control(state_of_member(rnd, contester)[:2] == ("ACTIVE", 0), "the contester is active")
    # When: the member contests its own MATCH verdict.
    contest = contest_request(rnd, contester, verdict_of_member(rnd, contester))
    if contest.status_code != 200:
        # Then (refused): a client error, the member untouched.
        assert 400 <= contest.status_code < 500, contest.text
        assert state_of_member(rnd, contester)[:2] == ("ACTIVE", 0)
        return
    R.control(state_of_member(rnd, contester)[:2] == ("SUSPENDED", 1), "the contest suspends it")
    # Then (admitted): the operator's aggregation of the round answers 200 ...
    response = R.aggregate(rnd)
    assert response.status_code == 200, response.text
    tape = response.json()["tape"]["body"]["inputs"]
    assert sorted(item["roster"]["hotkey"] for item in tape) == sorted(
        rnd.hotkey(i) for i in others
    )
    # ... the contest is decided and the four others are settled and paid for it.
    dispute_id = dispute_id_of_member(rnd, contester)
    X.resolve(R.world_of(rnd.network), dispute_id, reason="MATCH", loser=None)
    finalized = rnd.network.client.post(
        **R.finalize_request(rnd, others, response.json()["theta_hash"])
    )
    paid = rnd.store._db.execute("SELECT hotkey FROM escrow_origins WHERE reward_round=0")
    assert finalized.status_code == 200, finalized.text
    assert finalized.json()["status"] == "SETTLED"
    assert sorted(row[0] for row in paid) == sorted(rnd.hotkey(i) for i in others)


def test_a_member_back_from_a_closed_contest_does_not_stop_the_round_of_the_others(
    audited: R.Round,
) -> None:
    """DIB-04 / T-DSP-04, second stage: one roster member contests its own MATCH verdict, the
    referee confirms the audit and the member recovers (probation, no clean round). Either the
    contest was refused and the miner is untouched, or the operator still aggregates the round
    with that member left out: no 403 for the whole roster."""
    rnd, contester = audited, 2
    others = [i for i in rnd.members if i != contester]
    R.control(state_of_member(rnd, contester)[:2] == ("ACTIVE", 0), "the contester is active")
    # When: the member contests its own MATCH verdict.
    contest = contest_request(rnd, contester, verdict_of_member(rnd, contester))
    if contest.status_code != 200:
        # Then (refused): a client error, the member untouched.
        assert 400 <= contest.status_code < 500, contest.text
        assert state_of_member(rnd, contester)[:2] == ("ACTIVE", 0)
        return
    # When: the referee confirms the audit, and the member recovers.
    dispute_id = dispute_id_of_member(rnd, contester)
    X.resolve(R.world_of(rnd.network), dispute_id, reason="MATCH", loser=None)
    net = rnd.network
    recovered = net.client.post(
        net.url + "/admission/recover",
        json=net.signed(
            R.hot(contester),
            "Receipt",
            {"w": 0, "commit_hash": dispute_id, "received_round": net.now},
        ),
    )
    R.control(recovered.status_code == 200, recovered.text)
    R.control(state_of_member(rnd, contester)[:2] == ("PROBATION", 0), "it is back on probation")
    # Then (admitted): the operator's aggregation answers 200 for the four others.
    response = R.aggregate(rnd)
    assert response.status_code == 200, response.text
    tape = response.json()["tape"]["body"]["inputs"]
    assert sorted(item["roster"]["hotkey"] for item in tape) == sorted(
        rnd.hotkey(i) for i in others
    )


# --- T-DSP-06 (DIB-21): the round after an accepted rollback can still be paid -----------------


def test_a_round_opened_after_a_two_round_rollback_is_credited(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """DIB-21 / T-DSP-06: rounds 0 and 1 were rolled back and repaired (closed, model history
    only, reward units 0). Round 2 is then applied and finalized with valid evidence: the ledger
    pays it, as it would have paid it after two credited rounds."""
    # Given (control): the same evidence for round 2 is paid when rounds 0 and 1 were credited.
    with service_world(tmp_path / "control") as control:
        escrow = replayable_ledger(control, monkeypatch)
        for w in (0, 1, 2):
            put_finality(control, applied_round(control, w))
        before = escrow.balances().issued
        for w in (0, 1, 2):
            _, reward, evidence = signed_round(control, escrow, w)
            escrow.reward_finalize(reward, evidence)
        assert escrow.balances().issued - before == 3 * escrow.policy.round_reward_units
    # Given: a run whose rounds 0 and 1 a rollback closed without any reward, and round 2 applied.
    with service_world(tmp_path / "rolled-back") as world:
        escrow = replayable_ledger(world, monkeypatch)
        for w in (0, 1):
            repaired = replace(exclude(applied_round(world, w), THETA), rollback_complete=True)
            assert repaired.closed
            put_finality(world, repaired)
        put_finality(world, applied_round(world, 2))
        require_open(world.run_id, 3, world.store._history_v2(world.run_id))
        _, reward, evidence = signed_round(world, escrow, 2)
        before = escrow.balances().issued
        # When: the coordinator's Finalize of round 2 reaches the ledger.
        escrow.reward_finalize(reward, evidence)
        # Then: round 2 is paid.
        assert escrow.balances().issued - before == escrow.policy.round_reward_units
        assert escrow.balances().conserved()


# --- T-DSP-03 (DIA-14, DIB-03), the silent party is the auditor ---------------------------------


@pytest.mark.parametrize("settled", [False, True], ids=["round-pending", "round-settled"])
def test_a_contest_whose_auditor_never_answers_is_closed_as_abandoned_and_nobody_is_convicted(
    world: World, settled: bool
) -> None:
    """DIA-14 / DIB-03 / DIA-15: the miner answered its turn and the auditor, whose turn it now
    is, stays silent. The beacon closes the contest after the operator's window with no loser,
    for the cause of an abandoned turn (never a party's fault: only the witnessed timeout route
    convicts), returns the bond in full, releases the hold and puts the miner back on probation."""
    # Given: a contest of a pending or settled round, answered by the miner only.
    hot = X.service().HOT[0]
    X.seed_round(
        world,
        0,
        applied=True,
        state_hash="aa" * 32,
        disposition="SETTLED" if settled else "PENDING",
    )
    X.seed_round(world, 1)
    escrow = world.store._services(world.run_id)[0]
    dispute_id = X.open_contest(world, 0)
    X.answer(world, dispute_id, hot, says(X.disputes(world).get(dispute_id), miner=True))
    waiting = X.disputes(world).get(dispute_id)
    assert (waiting.expected_party, waiting.paused) == (waiting.contest.auditor, False)
    opened = escrow.balances()
    excluded = world.store._record_v2(world.run_id, "excluded", f"0:{hot.ss58}")
    close_at = closing_round(world, dispute_id)
    assert close_at < waiting.absolute_deadline
    # When: the beacon reaches the last round of the operator's window, then the next one.
    world.network.push(close_at - 1)
    # Then: still open at the last round of the window ...
    assert X.disputes(world).get(dispute_id).resolution is None
    assert settled or X.hold_blocks(world, settled=False)
    world.network.push(close_at)
    # ... and closed without a loser, for an abandoned turn, one round later.
    closed = X.disputes(world).get(dispute_id)
    assert closed.resolution is not None
    assert (closed.resolution.reason, closed.resolution.loser) == ("INFRASTRUCTURE", None)
    assert (closed.resolved_beacon, evidence_tag(world, dispute_id)) == (
        close_at,
        EXCLUSION_ABANDONED,
    )
    assert escrow.balances() == replace(
        opened, available=opened.available + X.BOND, dispute_locked=0
    )
    assert escrow.balances().burned == 0 and X.miner_state(world, 0)[:2] == ("PROBATION", 0)
    assert X.unresolved_of(world) == 0 and not X.hold_blocks(world, settled=settled)
    assert world.store._record_v2(world.run_id, "excluded", f"0:{hot.ss58}") == excluded


# --- T-DSP-16 (DIB-07, DIB-17): a live reward origin follows the standing of its miner ----------


@pytest.fixture
def service_rig(tmp_path: Path) -> Iterator[ServiceRig]:
    with service_world(tmp_path) as built:
        yield ServiceRig(built)


@pytest.mark.parametrize("ruling", ["MATCH", "AUDITOR-LOSES"])
def test_a_reward_origin_vests_at_its_maturity_once_its_miners_contest_is_closed_without_fraud(
    service_rig: ServiceRig, ruling: str
) -> None:
    """DIB-07 / T-DSP-16 (b), the other side of the hold: a miner whose contest the referee closed
    without convicting the miner (it confirmed the audit, or convicted the auditor) is paid the
    origin its round issued, at the origin's maturity. Whatever holds an origin while a dispute is
    open must let go of it here."""
    # Given: an origin of round 0 maturing at round 5, and the miner's contest of that round,
    # closed by the referee in the miner's favour or at least not against it.
    miner = service_rig.join(0)
    (origin,) = seed_round_origins(service_rig, [miner]).values()
    contest = service_rig.open_contest(miner)
    loser = service_rig.service.AUDITORS[0].ss58 if ruling == "AUDITOR-LOSES" else None
    reason = "FRAUD" if ruling == "AUDITOR-LOSES" else "MATCH"
    assert service_rig.refereed(contest.dispute_id, reason, loser).status_code == 200
    resolved = service_rig.disputes.get(contest.dispute_id)
    assert resolved.resolution is not None and resolved.resolution.loser == loser
    assert service_rig.escrow.balances(miner.coldkey).dispute_locked == 0
    # When: the miner is back from its suspension and the beacon passes the maturity round.
    service_rig.net.push(resolved.resolved_beacon + 1)
    recovered = service_rig.net.client.post(
        service_rig.net.url + "/admission/recover",
        json=service_rig.net.signed(
            miner.hot,
            "Receipt",
            {"w": 0, "commit_hash": contest.dispute_id, "received_round": service_rig.net.now},
        ),
    )
    assert recovered.status_code == 200, recovered.text
    service_rig.net.push(10)
    # Then: the origin is vested and the ledger conserves.
    assert available_of(service_rig, origin) == 500
    assert service_rig.escrow.balances().conserved()


@pytest.mark.parametrize(
    "ruling",
    [
        pytest.param(
            "open",
            marks=pytest.mark.xfail(
                strict=True,
                raises=DefenceMissing,
                reason="DIB-07+DIB-17: the settlement of a live reward origin is the record "
                "written at finalize, which no dispute of the miner touches, so the origin of a "
                "miner whose contest of that round is open vests at its maturity like any other "
                "and is paid [fix-lot: none (no design yet)]",
            ),
        ),
        pytest.param("lost", id="lost"),
        "lost-after-a-bond-only-seizure",
    ],
)
def test_only_the_origin_of_the_disputed_miner_is_held_and_nothing_is_paid_for_it(
    service_rig: ServiceRig, ruling: str
) -> None:
    """DIB-07 / DIB-17 / T-DSP-16: two funded miners earned an origin in round 0; one of them
    contests that round and the contest is open or lost. At the maturity round the other miner's
    origin vests and is paid; the disputed miner's origin does not vest, is not payable and earns
    the miner no weight, whether the ledger burns it or keeps it frozen. A lost contest burns it
    with the admission bond (admission-hardening, ADR 0010 Part 6), also when a confirmed audit
    mismatch had already burned that bond (the flow of D47)."""
    # Given: two origins of round 0 maturing at round 5, one miner with a funded contest.
    miners = [service_rig.join(0), service_rig.join(1)]
    for miner in miners:
        service_rig.lock_stake(miner)
    origins = seed_round_origins(service_rig, miners)
    if ruling == "lost-after-a-bond-only-seizure":
        with service_rig.world.store._tx():
            service_rig.admission.seize_bond(
                miners[0].admission_id,
                reason="AUDIT_MISMATCH",
                evidence_hash=MISMATCH_EVIDENCE,
                now=service_rig.net.now,
            )
        seized = service_rig.escrow.balances(miners[0].coldkey)
        assert (seized.admission_locked, seized.burned) == (0, STAKE)
    contest = service_rig.open_contest(miners[0])
    if ruling.startswith("lost"):
        assert (
            service_rig.refereed(contest.dispute_id, "FRAUD", miners[0].hotkey).status_code == 200
        )
    # When: the beacon passes the maturity round and the epoch is answered.
    disputed, clean = miners[0].hotkey, miners[1].hotkey
    service_rig.net.push(10)
    vested = {hotkey: available_of(service_rig, origin) for hotkey, origin in origins.items()}
    payable = {row["hotkey"] for row in service_rig.escrow.payable(10)}
    paid = json.loads(service_rig.world.store.weights(0, at(service_rig.escrow, 10), 1))["weights"]
    # Control: the other miner's origin vested, is payable and earned weight, so the round did
    # reach its maturity.
    assert (vested[clean], clean in payable, paid.get(clean)) == (500, True, 500.0)
    # Then: the disputed miner's origin is held and unpaid.
    with defence():
        assert vested[disputed] == 0
        assert disputed not in payable
        assert disputed not in paid
    assert service_rig.escrow.balances().conserved()


# --- T-DSP-17 (DIA-25): a bond that never became a dispute can be taken back -------------------


@pytest.mark.xfail(
    strict=True,
    raises=DefenceMissing,
    reason="DIA-25: a LOCK_CONTEST whose dispute never opened has no settlement (the store's "
    "lookup finds no dispute row and no record: UNACCEPTED_SETTLEMENT), so the units stay in "
    "dispute_locked and the owner has no path back [fix-lot: none (no design yet)]",
)
@pytest.mark.parametrize("opened", [True, False], ids=["refused-second-contest", "never-opened"])
def test_a_bond_beside_an_open_dispute_that_never_became_one_is_taken_back_alone(
    world: World, opened: bool
) -> None:
    """DIA-25 / T-DSP-17: the miner has one open contest (D1) and locked a second bond for a
    contest the service refused for room, or never filed. The owner takes the second bond back
    through the escrow release route: the units return to its free balance and none is burned.
    The bond of D1 is not moved by it, and stays locked while D1 is open."""
    # Given: D1 open with its bond, and a second bond for a verdict that never became a dispute.
    escrow = world.store._services(world.run_id)[0]
    X.open_contest(world, 0)
    first_lock = lock_of(world, X.lock_id_of(0))
    verdict, spare = fund_extra_bond(world, 0, "spare")
    if opened:
        refused = post_dispute_on(world, 0, verdict, spare)
        assert (refused.status_code, refused.json()["detail"]) == (
            409,
            "QUEUE_CAPACITY_EXCLUDE_WITHOUT_FRAUD",
        )
    before = escrow.balances(X.service().COLD[0].ss58)
    assert before.dispute_locked == 2 * X.BOND
    # When: the owner releases the second bond.
    released = X.post_release(world, 0, spare)
    # Then: one bond is back, the other is where it was, the ledger conserves.
    after = escrow.balances(X.service().COLD[0].ss58)
    with defence():
        assert released.status_code == 200, released.text
        assert (after.dispute_locked, after.available) == (X.BOND, before.available + X.BOND)
    assert after.burned == before.burned and escrow.balances().conserved()
    # And D1's own bond is still held, whoever asks.
    held = X.post_release(world, 0, first_lock)
    assert (held.status_code, held.json()["detail"]) == (409, "DISPUTED_OR_UNVESTED")
    assert escrow.balances(X.service().COLD[0].ss58).dispute_locked == X.BOND


# --- T-DSP-19 (DIB-22): a failed rollback attempt does not block the rollback of another window


@pytest.mark.parametrize(
    "outstanding",
    [
        pytest.param("none", id="no-attempt"),
        pytest.param("completed", id="completed-attempt-of-another-window"),
        pytest.param(
            "running",
            id="failed-attempt-of-another-window",
            marks=pytest.mark.xfail(
                strict=True,
                raises=DefenceMissing,
                reason="DIB-22: only COMPLETE ever leaves a RUNNING repair reservation, and a new "
                "request is refused 'repair reservation already occupied' while any reservation "
                "runs, whatever its age or its window: the committed reservation of a rollback of "
                "rounds 0 and 1 that crashed in its repair work blocks the rollback of rounds 1 "
                "and 2 for ever; reproduced with the reservation such a crash leaves, written by "
                "the test [fix-lot: none (no design yet)]",
            ),
        ),
    ],
)
def test_a_rollback_of_one_window_is_not_blocked_by_a_failed_attempt_on_another(
    world: World, outstanding: str
) -> None:
    """DIB-22 / T-DSP-19: the first attempt of a rollback of window 0 crashed after its
    reservation committed, and its 200 beacon rounds are over. A rollback of window 1 (another
    miner's proven fraud in round 1) is not turned away by it: it passes the reservation gate and
    goes on to the repair work. It does when no attempt is outstanding, and when the attempt of
    the other window is complete (the controls)."""
    # Given: two justified rollbacks, and the first window's attempt crashed (or never started).
    rig, first, second = two_windows(world)
    if outstanding != "none":
        rig.leave_running_reservation(first)
        world.network.push(world.network.now + 201)
    if outstanding == "completed":
        with world.store._tx():
            key = rig.operation(first)
            reservation = world.store._record_v2(world.run_id, "repair-reservation", key)
            world.store._put_record_v2(
                world.run_id, "repair-reservation", key, {**reservation, "state": "COMPLETE"}
            )
    # When: the second window is asked for.
    detail = reservation_refusal(world, second)
    # Then: the reservation gate lets it through.
    with defence():
        assert detail is None, detail


# --- T-DSP-23 (DIA-08, DIB-13, DIB-24) and CL-44: terminal transitions and the exclusion guard --


@pytest.mark.parametrize("silent", ["miner", "auditor"])
def test_a_dispute_closed_by_a_witnessed_timeout_takes_no_further_decision(
    li_escrow: ModuleType, silent: str
) -> None:
    """DIA-08 / DIB-13: the third way a dispute ends, the witnessed timeout of the party whose turn
    it is, is as final as a referee's resolution or an exclusion. Afterwards a referee resolution
    over registered evidence is a conflict, an identical one changes nothing, a second timeout is
    not due, an exclusion returns the stored turn, nobody can answer, reassign or pause it, and
    a second settlement of the bond adds no event."""
    # Given: the party whose turn it is acknowledged it and went silent; two honest observers.
    rig = make_rig(li_escrow)
    hot, auditor, coord = rig.f.HOT, rig.f.AUDITOR, rig.f.COORD
    opened, _ = rig.open(VERDICT)
    dispute = opened.dispute_id
    turn = opened if silent == "miner" else rig.say(dispute, hot, MINER_SAYS)[0]
    quiet = hot if silent == "miner" else auditor
    assert turn.expected_party == quiet.ss58
    rig.d.acknowledge(rig.ack(quiet, 2), now=2)
    due = turn.turn_deadline + 1
    # When: the timeout is witnessed and the bond is settled.
    closed = rig.d.timeout(dispute, rig.availability(turn, (coord, rig.ref), due), now=due)
    rig.d.settle_lock(dispute, now=due)
    stored = closed.resolution
    assert stored is not None
    assert (stored.reason, stored.loser) == (
        "PARTY_TIMEOUT" if silent == "miner" else "AUDITOR_FAULT",
        quiet.ss58,
    )
    # Then: every later move is refused with its own code, or returns the stored turn.
    evidence = rig.evidence(closed)
    rig.d.register_evidence(evidence)
    late = due + 1
    conflicting = rig.signed_resolution(closed, evidence)
    same = rig.signed_resolution(
        closed,
        evidence,
        reason=stored.reason,
        loser=stored.loser,
        evidence_hash=stored.evidence_hash,
    )
    again = rig.availability(closed, (coord, rig.ref), late)
    stale = rig.answer(dispute, quiet, MINER_SAYS)
    frozen = rig.state()
    for code, call in (
        ("RESOLUTION_CONFLICT", lambda: rig.d.resolution(conflicting, now=late)),
        ("TIMEOUT_NOT_DUE", lambda: rig.d.timeout(dispute, again, now=late)),
        ("CLOSED_PAUSED_OR_EXPIRED", lambda: rig.d.bisect(stale, now=late)),
        (
            "REFEREE_REASSIGNMENT_BOUND",
            lambda: rig.d.reassign(dispute, rig.next_ref.ss58, now=late),
        ),
        ("CLOSED_DISPUTE", lambda: rig.d.pause(dispute)),
    ):
        assert refusal(call) == code
        assert rig.state() == frozen
    assert rig.d.resolution(same, now=late) == closed
    assert rig.d.exclude(dispute, now=late) == closed
    rig.d.settle_lock(dispute, now=late)
    assert (rig.d.get(dispute), rig.state()) == (closed, frozen)
    balances = rig.escrow.balances()
    assert (balances.burned, balances.dispute_locked, balances.conserved()) == (0, 0, True)


def test_an_exclusion_is_due_for_a_paused_dispute_at_once_and_for_a_running_one_at_the_horizon(
    li_escrow: ModuleType,
) -> None:
    """CL-44 / T-DSP-23 (ii): exclude() is the server's bounded exit, so it is not available to
    cut a live dispute short: a running dispute is refused EXCLUSION_NOT_DUE until its absolute
    horizon round, where it closes without a loser. A paused one, which nobody can answer, is due
    at any round (the beacon exit relies on it)."""
    # Given: a running dispute and a second miner's dispute that was paused.
    rig = make_rig(li_escrow)
    running, _ = rig.open(VERDICT)
    second = "12" * 32
    paused, _ = rig.open(second, miner=rig.miner(1))
    rig.d.pause(paused.dispute_id)
    horizon = running.absolute_deadline
    # When / Then: the running one cannot be excluded before its horizon, not even one round early.
    for now in (2, horizon - 1):
        refused_unchanged(
            rig, "EXCLUSION_NOT_DUE", lambda now=now: rig.d.exclude(running.dispute_id, now=now)
        )
    assert rig.d.get(running.dispute_id).resolution is None
    # The paused one is due now and its bond comes back in full.
    done = rig.d.exclude(paused.dispute_id, now=2)
    rig.d.settle_lock(paused.dispute_id, now=2)
    assert done.resolution is not None
    assert (done.resolution.reason, done.resolution.loser, done.resolved_beacon) == (
        "INFRASTRUCTURE",
        None,
        2,
    )
    assert rig.escrow.balances().dispute_locked == BOND
    # At the horizon the running one closes the same way.
    last = rig.d.exclude(running.dispute_id, now=horizon)
    rig.d.settle_lock(running.dispute_id, now=horizon)
    assert last.resolution is not None
    assert (last.resolution.reason, last.resolution.loser) == ("INFRASTRUCTURE", None)
    balances = rig.escrow.balances()
    assert (balances.burned, balances.dispute_locked, balances.conserved()) == (0, 0, True)


# --- T-DSP-26 (DIB-09, DIA-23): the identity a lost contest leaves has no way back -------------


@pytest.mark.parametrize(
    "record",
    [{"outcome": "FRAUD"}, {"unresolved": True}],
    ids=["fraud-settled", "unresolved"],
)
def test_an_admission_suspended_on_a_lost_or_open_dispute_cannot_be_resumed_by_the_operator(
    tmp_path: Path, record: dict[str, object]
) -> None:
    """DIB-09 / DIA-23 / T-DSP-26: the only way out of a suspension that carries a pending dispute
    is the nonfraud settlement of that dispute. The operator's plain resume is refused whatever the
    record says, the identity stays ineligible, and no admission row moves, so the miner who lost
    a fraud contest cannot be put back on probation by the other door."""
    # Given: a suspended admission whose pending dispute is settled FRAUD, or still open, at a
    # round the clock has long passed.
    lab = Lab(tmp_path)
    suspended_on_a_dispute(lab, **record)
    hotkey = lab.A.HOT.ss58
    before = admission_snapshot(lab)
    # Control: the settlement door is the one that is shut by the record, with its own code.
    assert admission_code(lambda: lab.a.resolve_pending(hotkey, PENDING, now=3)) == (
        "PENDING_SETTLEMENT"
    )
    # When / Then: the plain resume is refused as well, the identity is not eligible, nothing moved.
    assert admission_code(lambda: lab.a.resume(hotkey, now=3)) == "RESUME_STATE"
    assert not lab.a.status(hotkey, now=3).eligible
    standing = lab.a.store.record(hotkey)
    assert (standing.state, standing.pending_dispute) == ("SUSPENDED", True)
    assert admission_snapshot(lab) == before
