"""Sanctions after a caught cheat, and the cheapest ways a rostered miner wedges a round.

Every strict xfail below is a hole of the v2 protocol path: its body is the minimal reproducer and
its failing assertion is the DESIRED defence (it flips to a failure the day the hole is closed, so
the marker must then be removed). Nothing here asserts today's behaviour as if it were the
contract; what the master does instead is recorded as evidence (record()) for the ADR. The one
test without a marker pins a layer that holds today (the audit behind an intake hole) and says so.

The five-miner roster is the default shape of ac_world.py; the cheater is identity 4. A step that
must hold today (the cheat is caught, the honest miners settle) is a precondition: it raises
PreconditionError, which is not an AssertionError, so a broken scenario cannot hide behind the
expected failure of the defence under test.
"""

from __future__ import annotations

from typing import Any

import pytest
from ac_cheats import BY_NAME, Cheat
from ac_precondition import PreconditionError, precondition
from ac_sanctions import (
    COPIED_PEER_WITHOUT_UPLOAD,
    FRESH_HOT,
    NON_FINITE_LEAF,
    REPLAYED_ROUND_WITHOUT_UPLOAD,
    SHORT_LIVED_COMMIT,
    audit_queue_order,
    join_fresh_hotkey,
    lose_a_funded_contest,
    post_delta_manifest,
    recorded_envelope,
    register_on_metagraph,
)
from ac_world import MinerOutcome, Observation, Settlement, World, record

from hypertrain.challenge.store import ChallengeError
from hypertrain.protocol.hashing import sha256_hex

slow = pytest.mark.slow

ROSTER = list(range(5))
CHEATER = 4
PEER, COPIER = 0, 1
FORGED_DELTA = BY_NAME["forged-delta-valid-hash"]


def run_round(
    world: World, cheats: dict[int, Cheat | None], roster: list[int] = ROSTER
) -> tuple[list[MinerOutcome], Settlement]:
    world.open_round(0, roster)
    outcomes = world.run_roster({i: cheats.get(i) for i in roster})
    return outcomes, world.settle(0)


def evidence(
    test: str,
    world: World,
    cheat: Cheat,
    cheater: int,
    outcomes: list[MinerOutcome],
    settlement: Settlement,
    seen: Observation,
    hole: str,
    **extra: object,
) -> None:
    """One evidence line per scenario: the raw material of the ADR table."""
    bad = world.hotkey(cheater)
    intake = next(o for o in outcomes if o.index == cheater)
    weights = seen.weights.weights if seen.weights else {}
    verdict = next((v for v in settlement.verdicts if v.hotkey == bad), None)
    record(
        {
            "test": test,
            "cheat": cheat.cheat,
            "double": cheat.name,
            "seam": cheat.seam,
            "intake": {
                "status": intake.status,
                "http_status": intake.http_status,
                "endpoint": intake.endpoint,
                "detail": intake.detail,
                "stage": intake.stage,
            },
            "verdict": verdict.result if verdict else None,
            "first_bad_leaf": verdict.first_bad_leaf if verdict else None,
            "excluded": seen.excluded.get(f"0:{bad}"),
            "settled": settlement.settled,
            "stalled_at": settlement.stalled_at,
            "cheater_weight": weights.get(bad),
            "honest_weights": {k: v for k, v in weights.items() if k != bad},
            "cheater_admission": seen.admissions.get(bad),
            "honest_admissions": {k: v["state"] for k, v in seen.admissions.items() if k != bad},
            "hole": hole,
            **extra,
        }
    )


def next_round_refusal(world: World) -> str | None:
    """What opening round 1 answers when the whole roster, the cheater included, is proposed."""
    try:
        world.open_round(1, ROSTER, prev_final=None)
    except Exception as error:  # noqa: BLE001 - the refusal text is the evidence
        return f"{type(error).__name__}: {error}"
    return None


@slow
@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="AC-SANCTION: complete_audit_v2 (store.py:7600-7606) only writes excluded/UNVERIFIED on "
    "a round MISMATCH; it never calls Admission.suspend. Strikes are write-only (admission.py:733 "
    "is the only writer, nothing reads them), so a strike alone would sanction nothing",
)
def test_a_caught_round_cheater_is_not_left_active(ac_clone: World) -> None:
    """Desired defence: an uncontested round MISMATCH is not free. After the audit catches a
    cheater its admission is no longer ACTIVE (suspended until a way back is defined; docs/
    mechanism.md promises the cost of being caught exceeds the saving). A strike alone does not
    count: nothing reads strikes, so it would change no behaviour of the network."""
    world = ac_clone
    bad = world.hotkey(CHEATER)

    outcomes, settlement = run_round(world, {CHEATER: FORGED_DELTA})
    refusal = next_round_refusal(world)
    paid = world.weights()
    seen = world.observe()
    evidence(
        "caught-cheater-sanction",
        world,
        FORGED_DELTA,
        CHEATER,
        outcomes,
        settlement,
        seen,
        "AC-SANCTION",
        next_round_roster_with_cheater=refusal,
        cheater_escrow=seen.escrow_by_owner[world.svc.COLD[CHEATER].ss58],
        get_weights_status=paid.status_code,
    )

    precondition(settlement.results.get(bad) == "MISMATCH", f"not caught: {settlement.results}")
    precondition(settlement.settled, f"round did not settle: {settlement.stalled_at}")
    admission = seen.admissions[bad]
    assert admission["state"] != "ACTIVE", admission


@slow
@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="AC-COLDKEY (MINEUR, follows AC-SANCTION): join refuses a coldkey only while one of its "
    "identities is not ACTIVE (admission.py:164-166), and a round verdict leaves the cheater "
    "ACTIVE, so its coldkey joins a fresh hotkey at once. That hotkey cannot be rostered at w>0 "
    "in this run (no anchor, store.py:5152-5155); the residual risk is a later run or a later "
    "roster policy",
)
def test_a_caught_cheater_cannot_start_another_identity_with_the_same_coldkey(
    ac_clone: World, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Desired defence: while the identity of a caught cheater is unresolved its coldkey cannot
    join another hotkey, and an honest coldkey is not blocked. It holds only if AC-SANCTION
    chooses suspension, and it would then also block the coldkey of an honest miner whose replay
    drifted (Lens DET-01) until a way back exists: that outcome is the ADR's, not asserted here."""
    world = ac_clone
    register_on_metagraph(world, monkeypatch, FRESH_HOT)
    bad = world.hotkey(CHEATER)

    outcomes, settlement = run_round(world, {CHEATER: FORGED_DELTA})
    honest_join = join_fresh_hotkey(
        world, world.svc.COLD[0], FRESH_HOT[0], "honest-second-identity"
    )
    cheater_join = join_fresh_hotkey(world, world.svc.COLD[CHEATER], FRESH_HOT[1], "cheater-second")
    seen = world.observe()
    evidence(
        "caught-cheater-coldkey",
        world,
        FORGED_DELTA,
        CHEATER,
        outcomes,
        settlement,
        seen,
        "AC-COLDKEY",
        honest_second_join=[honest_join.status_code, honest_join.text[:120]],
        cheater_second_join=[cheater_join.status_code, cheater_join.text[:120]],
    )

    precondition(settlement.results.get(bad) == "MISMATCH", f"not caught: {settlement.results}")
    precondition(honest_join.status_code == 200, f"control join: {honest_join.text}")
    assert cheater_join.status_code == 409 and "COLDKEY" in cheater_join.text, cheater_join.text


@slow
def test_one_exclusion_in_a_roster_of_four_is_a_handled_refusal_not_a_server_error(
    ac_clone: World,
) -> None:
    """Desired defence: when the three survivors of a roster of four cannot carry the round,
    POST /admin/rounds/{w}/aggregate answers a 4xx that names the reason instead of an uncaught
    server error. (That the run can go on afterwards is the next test.)"""
    world = ac_clone
    roster, cheater = [0, 1, 2, 3], 3
    bad = world.hotkey(cheater)

    outcomes, settlement = run_round(world, {cheater: FORGED_DELTA}, roster)
    seen = world.observe()
    aggregate = settlement.step("aggregate")
    evidence(
        "exclusion-in-a-roster-of-four",
        world,
        FORGED_DELTA,
        cheater,
        outcomes,
        settlement,
        seen,
        "AC-CLIFF",
        aggregate=None if aggregate is None else [aggregate.status, aggregate.exception],
        finality=seen.finality,
    )

    precondition(settlement.results.get(bad) == "MISMATCH", f"not caught: {settlement.results}")
    if aggregate is None:
        raise PreconditionError(f"audits did not complete: {settlement.stalled_at}")
    assert 400 <= aggregate.status < 500 and aggregate.detail, (aggregate.status, aggregate.detail)


@slow
@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="AC-CLIFF-LIVENESS: with exactly four cap slots one exclusion leaves the round "
    "unaggregatable and unapplied; its opening cannot be replaced (store.py:5084-5091, 409 "
    "conflicting round opening) and round 1 cannot open ('missing accepted applied'), so the run "
    "halts for good",
)
def test_a_roster_of_four_run_can_go_on_after_one_exclusion(ac_clone: World) -> None:
    """Desired defence: the loss of one miner of a roster of four does not halt the run. Either the
    round settles with the survivors, or the operator can supersede the opening of the round with
    the three survivors and the fifth graduated identity. Both are one observable: no refusal of
    that re-opening and no unsettled round. (A design that refuses four-slot rosters at opening
    never reaches this point: run_round then raises and the test is to be rewritten.)"""
    world = ac_clone
    roster, cheater = [0, 1, 2, 3], 3
    survivors_and_a_fifth = [0, 1, 2, 4]
    bad = world.hotkey(cheater)

    outcomes, settlement = run_round(world, {cheater: FORGED_DELTA}, roster)
    precondition(settlement.results.get(bad) == "MISMATCH", f"not caught: {settlement.results}")
    precondition(
        settlement.step("aggregate") is not None,
        f"audits did not complete: {settlement.stalled_at}",
    )
    reopening = None
    if not settlement.settled:
        try:
            world.open_round(0, survivors_and_a_fifth)
        except (RuntimeError, ChallengeError) as error:
            reopening = f"{type(error).__name__}: {error}"
    seen = world.observe()
    evidence(
        "run-of-four-after-one-exclusion",
        world,
        FORGED_DELTA,
        cheater,
        outcomes,
        settlement,
        seen,
        "AC-CLIFF-LIVENESS",
        reopening_refusal=reopening,
        finality=seen.finality,
    )

    assert settlement.settled or reopening is None, (settlement.stalled_at, reopening)


def wedge_round(
    world: World, wedge: Cheat, where: str
) -> tuple[int, list[str], list[MinerOutcome], Settlement]:
    """A roster of five whose offender sits at the `where` ("head", "middle" or "tail") of the
    audit queue.

    The position comes from a copy of the master's job-id formula; once the audits are scheduled
    it is checked against the master's own lease table, so a changed formula is a
    PreconditionError and never a silent change of what the scenario tests.
    """
    order = audit_queue_order(world, 0, ROSTER)
    position = {"head": 0, "middle": len(ROSTER) // 2, "tail": len(ROSTER) - 1}[where]
    offender = order[position]
    outcomes, settlement = run_round(world, {offender: wedge})
    queue = [row["hotkey"] for row in sorted(world.observe().leases, key=lambda r: r["id"])]
    precondition(
        world.hotkey(offender) not in queue or queue.index(world.hotkey(offender)) == position,
        f"the offender is not at the {where} of the lease queue: {queue}",
    )
    honest = [world.hotkey(i) for i in ROSTER if i != offender]
    return offender, honest, outcomes, settlement


def assert_round_settles_and_pays_the_honest(
    world: World, offender: int, honest: list[str], settlement: Settlement
) -> None:
    """The desired defence of the wedges: the four others are audited, the round settles and
    get_weights pays each of them (no figure is pinned: the split of a round with an exclusion is
    an open design question) and the offender nothing."""
    assert {hotkey: settlement.results.get(hotkey) for hotkey in honest} == dict.fromkeys(
        honest, "MATCH"
    ), settlement.stalled_at
    assert settlement.settled, settlement.stalled_at
    paid = world.weights()
    assert paid.status_code == 200 and all(paid.weights.get(h, 0) > 0 for h in honest), paid
    assert paid.weights.get(world.hotkey(offender), 0) == 0, paid


WEDGE_POSITIONS = ("head", "middle", "tail")


@slow
@pytest.mark.parametrize("where", WEDGE_POSITIONS)
def test_a_non_finite_leaf_cannot_stop_the_audits_of_the_others(
    ac_clone: World, where: str
) -> None:
    """Desired defence: a rostered miner whose leaves carry a non-finite metric is excluded alone;
    the four other miners are audited with MATCH, the round settles and they are paid."""
    world = ac_clone
    offender, honest, outcomes, settlement = wedge_round(world, NON_FINITE_LEAF, where)
    seen = world.observe()
    lease = settlement.step("lease")
    evidence(
        "wedge-non-finite-leaf",
        world,
        NON_FINITE_LEAF,
        offender,
        outcomes,
        settlement,
        seen,
        "AC-WEDGE-LEAF",
        queue_position=where,
        lease=None
        if lease is None
        else [lease.status, lease.exception, (lease.detail or "")[:160]],
        honest_verdicts=settlement.results,
        leases=[row["state"] for row in seen.leases],
    )

    assert_round_settles_and_pays_the_honest(world, offender, honest, settlement)


@slow
@pytest.mark.parametrize("where", WEDGE_POSITIONS)
def test_a_short_lived_commit_cannot_stop_the_audits_of_the_others(
    ac_clone: World, where: str
) -> None:
    """Desired defence: a commit whose envelope expires before d_audit (here at d_commit, the
    latest value /commit still accepts) leaves its miner excluded alone; the four other miners are
    audited with MATCH, the round settles and they are paid."""
    world = ac_clone
    offender, honest, outcomes, settlement = wedge_round(world, SHORT_LIVED_COMMIT, where)
    seen = world.observe()
    stalled = settlement.step(settlement.stalled_at) if settlement.stalled_at else None
    evidence(
        "wedge-short-lived-commit",
        world,
        SHORT_LIVED_COMMIT,
        offender,
        outcomes,
        settlement,
        seen,
        "AC-WEDGE-EXP",
        queue_position=where,
        stalled=None
        if stalled is None
        else [stalled.name, stalled.status, (stalled.detail or "")[:160]],
        honest_verdicts=settlement.results,
    )

    assert_round_settles_and_pays_the_honest(world, offender, honest, settlement)


def borrow_peer_acceptance(world: World) -> tuple[MinerOutcome, MinerOutcome, str, str, Any]:
    """Round 0: miner 0 uploads honestly; miner 1 commits the same tree and stops before the relay
    grant, then posts /delta carrying miner 0's acceptance. Returns both outcomes, the copier's
    hotkey, the borrowed delta hash and the master's answer to the borrowed /delta."""
    peer, copier = PEER, COPIER
    world.open_round(0, ROSTER)
    peer_outcome = world.run_miner(peer, None)
    copier_outcome = world.run_miner(copier, COPIED_PEER_WITHOUT_UPLOAD)
    borrowed = recorded_envelope(world, "delta", 0, world.hotkey(peer))
    response = post_delta_manifest(
        world, copier, {**borrowed["body"], "hotkey": world.hotkey(copier)}, borrowed["exp_drand"]
    )
    precondition(peer_outcome.uploaded, f"peer did not upload: {peer_outcome}")
    precondition(
        (copier_outcome.status, copier_outcome.stage) == ("withdrawn", "upload-grant"),
        f"copier did not stop before the grant: {copier_outcome}",
    )
    commit = recorded_envelope(world, "commit", 0, world.hotkey(copier))
    precondition(
        commit["body"]["delta_hash"] == borrowed["body"]["delta_hash"],
        "the copier did not commit the peer's delta: the intake would refuse the borrow on the "
        "delta hash, not on the acceptance",
    )
    return (
        peer_outcome,
        copier_outcome,
        world.hotkey(copier),
        commit["body"]["delta_hash"],
        response,
    )


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="AC-ACCEPTANCE-BINDING (MINEUR, defence in depth): DeltaManifestV2 intake "
    "(store.py:6192-6200) looks the acceptance up by its digest run-wide and compares only its "
    "delta_hash with the commit; the acceptance's w and hotkey and the manifest's grant_hash are "
    "never read. The audit still catches the miner (see the passing sibling test)",
)
def test_a_delta_cannot_borrow_the_acceptance_of_another_miner(ac_clone: World) -> None:
    """Desired defence: a /delta that references the master acceptance minted for another miner's
    upload is refused with 409 or 422, whoever signed it. Without the check the miner skips its
    own grant, relay upload and receipt; it gains no reward and no custody is lost."""
    world = ac_clone
    _, copier_outcome, _, _, response = borrow_peer_acceptance(world)
    record(
        {
            "test": "delta-borrows-a-peer-acceptance",
            "cheat": COPIED_PEER_WITHOUT_UPLOAD.cheat,
            "double": COPIED_PEER_WITHOUT_UPLOAD.name,
            "seam": COPIED_PEER_WITHOUT_UPLOAD.seam,
            "intake": [copier_outcome.status, copier_outcome.stage],
            "delta_status": response.status_code,
            "delta_detail": response.text[:160],
            "hole": "AC-ACCEPTANCE-BINDING",
        }
    )

    assert response.status_code in (409, 422), (response.status_code, response.text[:160])


@slow
def test_a_delta_that_borrowed_an_acceptance_still_earns_nothing_at_the_audit(
    ac_clone: World,
) -> None:
    """The layer behind AC-ACCEPTANCE-BINDING holds today: the borrowed /delta is accepted, yet the
    replay of the copier's job does not match its commit, the copier is excluded as UNVERIFIED,
    the four others settle and the master holds bytes whose sha256 is the committed delta hash.

    Obsolete the day /delta refuses the borrow: the precondition then fails and this test goes."""
    world = ac_clone
    _, copier_outcome, copier, delta_hash, response = borrow_peer_acceptance(world)
    precondition(
        response.status_code == 200,
        f"the borrow is refused now ({response.status_code}): retire this test",
    )
    honest = [world.hotkey(i) for i in ROSTER if world.hotkey(i) != copier]
    run_miners = [i for i in ROSTER if i not in (PEER, COPIER)]
    for index in run_miners:
        precondition(world.run_miner(index, None).uploaded, f"miner {index} did not upload")

    settlement = world.settle(0)
    seen = world.observe()
    evidence(
        "delta-borrows-a-peer-acceptance-audit",
        world,
        COPIED_PEER_WITHOUT_UPLOAD,
        COPIER,
        [copier_outcome],
        settlement,
        seen,
        "AC-ACCEPTANCE-BINDING",
        delta_status=response.status_code,
    )

    assert settlement.results == {**dict.fromkeys(honest, "MATCH"), copier: "MISMATCH"}
    assert seen.excluded[f"0:{copier}"]["reason"] == "UNVERIFIED"
    assert settlement.settled, settlement.stalled_at
    held = world.network.store.objects.get(delta_hash)
    assert sha256_hex(held) == delta_hash


@slow
@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="AC-ACCEPTANCE-BINDING (MINEUR, defence in depth): same intake check as for a peer's "
    "acceptance (store.py:6192-6200); an acceptance of round w-1 with the same delta_hash is "
    "accepted for round w because its w is never read. The audit still returns MISMATCH",
)
def test_a_delta_cannot_borrow_the_acceptance_of_an_earlier_round_of_the_same_miner(
    ac_clone: World,
) -> None:
    """Desired defence: a /delta of round 1 that references the master acceptance minted for the
    same miner's round-0 upload is refused with 409 or 422."""
    world = ac_clone
    cheater = CHEATER
    _, settlement = run_round(world, {i: None for i in ROSTER})
    precondition(settlement.settled, f"round 0 did not settle: {settlement.stalled_at}")
    world.open_round(1, ROSTER)
    outcome = world.run_miner(cheater, REPLAYED_ROUND_WITHOUT_UPLOAD, w=1)
    precondition(
        (outcome.status, outcome.stage) == ("withdrawn", "upload-grant"),
        f"the replayer did not stop before the grant: {outcome}",
    )
    earlier = recorded_envelope(world, "delta", 0, world.hotkey(cheater))
    commit = recorded_envelope(world, "commit", 1, world.hotkey(cheater))
    precondition(
        commit["body"]["delta_hash"] == earlier["body"]["delta_hash"],
        "the replayer did not commit its old delta: the borrow would fail on the delta hash",
    )
    response = post_delta_manifest(
        world, cheater, {**earlier["body"], "w": 1}, world.opening(1).d_upload
    )
    record(
        {
            "test": "delta-borrows-an-earlier-round-acceptance",
            "cheat": REPLAYED_ROUND_WITHOUT_UPLOAD.cheat,
            "double": REPLAYED_ROUND_WITHOUT_UPLOAD.name,
            "seam": REPLAYED_ROUND_WITHOUT_UPLOAD.seam,
            "intake": [outcome.status, outcome.stage],
            "delta_status": response.status_code,
            "delta_detail": response.text[:160],
            "hole": "AC-ACCEPTANCE-BINDING",
        }
    )

    assert response.status_code in (409, 422), (response.status_code, response.text[:160])


@slow
def test_a_lost_fraud_contest_ends_in_a_banned_admission(ac_clone: World) -> None:
    """Desired defence: when the referee rules FRAUD against a cheater that contested its own
    MISMATCH, its admission ends BANNED. (The slash of its admission lock is RTA-06, asserted at
    the escrow level by test_ledger_defences.py; this test keeps the admission-state half and
    does not assert the burn.)"""
    world = ac_clone
    bad = world.hotkey(CHEATER)

    outcomes, settlement = run_round(world, {CHEATER: FORGED_DELTA})
    precondition(settlement.results.get(bad) == "MISMATCH", f"not caught: {settlement.results}")
    contest = lose_a_funded_contest(world, CHEATER)
    seen = world.observe()
    evidence(
        "lost-fraud-contest",
        world,
        FORGED_DELTA,
        CHEATER,
        outcomes,
        settlement,
        seen,
        "AC-BAN",
        dispute=contest["dispute_id"],
        cheater_escrow=seen.escrow_by_owner[world.svc.COLD[CHEATER].ss58],
    )

    assert seen.admissions[bad]["state"] == "BANNED", seen.admissions[bad]
