"""A lock id, a dispute id and a reward-origin id are three different things.

ADR 0014 part 2: a contest bond is settled by the dispute that took it, found through the
lock's own signed dispute id; a dispute is found by its id and an origin by its id; the name
a client gave a lock is never looked up in the dispute or origin tables. `DisputesV2.open`
also refuses a bond whose name is already a dispute or an origin.
"""

from __future__ import annotations

import importlib.util
import sys
from dataclasses import asdict
from pathlib import Path
from types import SimpleNamespace

import pytest

from hypertrain.auditor.island_bisect import RefereeEvidence
from hypertrain.challenge.disputes_v2 import Contest, DisputeError
from hypertrain.challenge.finality_v2 import RoundStatus
from hypertrain.ledger.escrow_v2 import EscrowError, Settlement
from hypertrain.protocol.envelope_v2 import seal
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages import Receipt
from hypertrain.protocol.messages_v2 import (
    DisputeV2,
    EscrowLock,
    EscrowRelease,
    ResolutionV2,
)


def _load(name: str, filename: str):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).parent / filename)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


D = _load("settlement_typing_disputes_fixture", "test_disputes_v2.py")
service = _load("settlement_typing_service_fixture", "test_service_network_v2.py")
E = _load("settlement_typing_escrow_fixture", "../ledger/test_escrow_v2.py")
HOT, COLD, AUDITORS, REFEREE = service.HOT, service.COLD, service.AUDITORS, service.REFEREE


def free_origin(db, owner):
    return db.execute(
        "SELECT origin FROM escrow_units WHERE owner=? AND bucket='available' AND units>=100",
        (owner,),
    ).fetchone()[0]


def referee_resolution(reason, loser, turn):
    return RefereeEvidence(
        dispute_id=turn.dispute_id,
        transcript_hash=turn.transcript_hash,
        reason=reason,
        loser=loser,
        predecessor_hash="11" * 32,
        inputs_hash="22" * 32,
        output_hash="33" * 32,
        op_spec="verified.actual.op",
    )


def resolve(register, post, key, run_id, turn, reason="MATCH", loser=None):
    evidence = referee_resolution(reason, loser, turn)
    register(evidence)
    resolution = ResolutionV2(
        dispute_id=turn.dispute_id,
        transcript_hash=turn.transcript_hash,
        reason=reason,
        loser=loser,
        evidence_hash=evidence.digest(),
    )
    return post(seal(key, "ResolutionV2", run_id, resolution, 100000), resolution)


def contest_request(d, f, bond_id: str, verdict_hash: str):
    """The miner's signed contest of `verdict_hash`, bonded by the lock named `bond_id`."""
    request = DisputeV2(
        hotkey=f.HOT.ss58, verdict_hash=verdict_hash, action="contest", bond_lock=bond_id
    )
    return canonicalize(seal(f.HOT, "DisputeV2", d.run_id, request, 100000))


def second_verdict(d, verdict_hash: str):
    contest = d.contest_of("x").model_copy(update={"verdict_hash": verdict_hash})
    d.contest_of = lambda _: contest
    return contest, sha256_hex(canonicalize([d.run_id, verdict_hash, contest.miner]))


def bond(f, origin: str, operation_id: str, dispute_id: str) -> EscrowLock:
    return EscrowLock(
        operation_id=operation_id,
        owner=f.COLD.ss58,
        units=100,
        origin_ids=[origin],
        admission_id=None,
        dispute_id=dispute_id,
        kind="LOCK_CONTEST",
    )


def first_dispute_closed(tmp_path):
    d, f, ref, _, raw = D.service(tmp_path)
    first = d.open(raw, now=1)
    resolve(
        d.register_evidence,
        lambda envelope, _: d.resolution(canonicalize(envelope), now=3),
        ref,
        d.run_id,
        first,
    )
    d.settle_lock(first.dispute_id, now=3)
    assert d.escrow.balances(f.COLD.ss58).dispute_locked == 0
    return d, f, ref, first


@pytest.mark.parametrize("named_after", ["dispute", "origin"])
def test_open_refuses_a_bond_whose_name_is_already_a_dispute_or_an_origin(
    tmp_path: Path, named_after: str
) -> None:
    d, f, ref, first = first_dispute_closed(tmp_path)
    _, second = second_verdict(d, "12" * 32)
    origin = free_origin(d.db, f.COLD.ss58)
    name = first.dispute_id if named_after == "dispute" else origin
    d.escrow.lock(bond(f, origin, name, second), signer=f.COLD.ss58)
    with pytest.raises(DisputeError, match="CONTEST_LOCK_ID_COLLISION"):
        d.open(contest_request(d, f, name, "12" * 32), now=4)
    assert d.db.execute("SELECT COUNT(*) FROM disputes_v2").fetchone()[0] == 1


def test_a_bond_named_after_a_later_dispute_does_not_answer_for_that_dispute(
    tmp_path: Path,
) -> None:
    # Given: the first bond is named after the id the miner's SECOND dispute will have (the name was
    # free when the first dispute opened, so the point-in-time refusal does not apply).
    d, f, ref, _, _ = D.service(tmp_path)
    contest, later = second_verdict(d, "12" * 32)
    first_contest = contest.model_copy(update={"verdict_hash": "11" * 32})
    d.contest_of = lambda h: first_contest if h == "11" * 32 else contest
    origin = free_origin(d.db, f.COLD.ss58)
    first_id = sha256_hex(canonicalize([d.run_id, "11" * 32, contest.miner]))
    d.escrow.lock(bond(f, origin, later, first_id), signer=f.COLD.ss58)
    first = d.open(contest_request(d, f, later, "11" * 32), now=1)
    assert first.lock_id == later
    resolve(
        d.register_evidence,
        lambda envelope, _: d.resolution(canonicalize(envelope), now=3),
        ref,
        d.run_id,
        first,
    )
    # When: the second dispute opens; its id is the name of the first (resolved) dispute's bond.
    other = free_origin(d.db, f.COLD.ss58)
    d.escrow.lock(bond(f, other, "45" * 32, later), signer=f.COLD.ss58)
    second = d.open(contest_request(d, f, "45" * 32, "12" * 32), now=4)
    assert second.dispute_id == later and first.dispute_id != later
    # Then: the reference to the second dispute is the second dispute, still open, and the first
    # dispute settles its own bond when it closes.
    assert d.settlement(second.dispute_id).unresolved
    held = d.escrow.balances(f.COLD.ss58).dispute_locked
    d.settle_lock(first.dispute_id, now=4)
    assert d.escrow.balances(f.COLD.ss58).dispute_locked == held - 100  # the first bond came back
    assert d.settlement(second.dispute_id).unresolved


def test_a_contest_lock_that_no_dispute_took_is_not_released_by_a_dispute_named_like_it(
    tmp_path: Path,
) -> None:
    # Given: the first dispute is closed and vested; a second contest lock, never accepted as the
    # bond of any dispute, is named after that dispute (id) and names it (body).
    d, f, ref, first = first_dispute_closed(tmp_path)
    closed = d.settlement(first.dispute_id)
    origin = free_origin(d.db, f.COLD.ss58)
    d.escrow.lock(bond(f, origin, first.dispute_id, first.dispute_id), signer=f.COLD.ss58)
    release = EscrowRelease(
        operation_id="78" * 32,
        owner=f.COLD.ss58,
        units=100,
        origin_ids=[origin],
        admission_id=None,
        dispute_id=first.dispute_id,
        lock_id=first.dispute_id,
        finality_hash=closed.finality_hash,
        closed_dispute_root=closed.closed_dispute_root,
    )
    # When / Then: it is not the dispute's bond, so the dispute's settlement does not release it.
    with pytest.raises(EscrowError):
        d.escrow.release(release, signer=f.COLD.ss58, now_beacon=5)
    assert d.escrow.balances(f.COLD.ss58).dispute_locked == 100


@pytest.fixture
def net(tmp_path):
    yield from service.network.__wrapped__(tmp_path, SimpleNamespace(param=None))


def free(n, owner):
    return free_origin(n.store._db, owner)


def make_contest(n, run_id, index, verdict_hash, admission_id):
    inner = n.manifest.training.inner
    contest = Contest(
        verdict_hash=verdict_hash,
        challenge_hash="22" * 32,
        w=0,
        miner=HOT[index].ss58,
        auditor=AUDITORS[0].ss58,
        referee=REFEREE.ss58,
        step_span=inner.H // inner.J,
        layer_span=1,
        op_span=1,
        admission_id=admission_id,
        coldkey=COLD[index].ss58,
        evidence_hash=verdict_hash,
    )
    n.store._put_record_v2(run_id, "contest", verdict_hash, contest.body())
    return contest, sha256_hex(canonicalize([run_id, verdict_hash, HOT[index].ss58]))


def post_lock(n, index, op_id, dispute_id):
    lock = bond(SimpleNamespace(COLD=COLD[index]), free(n, COLD[index].ss58), op_id, dispute_id)
    response = n.client.post(n.url + "/escrow/lock", json=n.signed(COLD[index], "EscrowLock", lock))
    assert response.status_code == 200, response.text


def post_dispute(n, index, contest, op_id):
    request = DisputeV2(
        hotkey=HOT[index].ss58, verdict_hash=contest.verdict_hash, action="contest", bond_lock=op_id
    )
    return n.client.post(n.url + "/dispute", json=n.signed(HOT[index], "DisputeV2", request))


def post_resolution_match(n, disputes, dispute_id):
    turn = disputes.get(dispute_id)
    return resolve(
        disputes.register_evidence,
        lambda _, resolution: n.client.post(
            n.url + "/resolution", json=n.signed(REFEREE, "ResolutionV2", resolution)
        ),
        REFEREE,
        n.manifest.run_id(),
        turn,
    )


def seed_round_zero(n, run_id):
    status = RoundStatus(run_id, 0, False, "PENDING", 0, 0, False, "00" * 32, None)
    n.store._put_record_v2(run_id, "finality", "0", asdict(status))


def test_the_lens_scenario_cannot_wedge_a_dispute_over_the_routes(net) -> None:
    # Given: one miner, two completed audits; the first dispute is closed and recovered.
    n = net
    run_id = n.manifest.run_id()
    admission_id = n.join(0).json()["admission_id"]
    escrow, _, disputes = n.store._services(run_id)
    seed_round_zero(n, run_id)
    c1, d1 = make_contest(n, run_id, 0, "a1" * 32, admission_id)
    c2, d2 = make_contest(n, run_id, 0, "a2" * 32, admission_id)
    n.push(2)
    post_lock(n, 0, "44" * 32, d1)
    assert post_dispute(n, 0, c1, "44" * 32).status_code == 200
    assert post_resolution_match(n, disputes, d1).status_code == 200
    n.push(4)
    recover = n.client.post(
        n.url + "/admission/recover",
        json=n.signed(HOT[0], "Receipt", Receipt(w=0, commit_hash=d1, received_round=1)),
    )
    assert recover.status_code == 200, recover.text
    # When: the second bond is named after the first dispute's id, the contest is refused...
    post_lock(n, 0, d1, d2)
    refused = post_dispute(n, 0, c2, d1)
    assert refused.status_code == 409 and "CONTEST_LOCK_ID_COLLISION" in refused.text
    # ...and the miner contests again with a bond of its own, which the referee can decide.
    post_lock(n, 0, "45" * 32, d2)
    assert post_dispute(n, 0, c2, "45" * 32).status_code == 200
    n.push(6)
    response = post_resolution_match(n, disputes, d2)
    assert response.status_code == 200, response.text
    assert net.store._record_v2(run_id, "finality", "0")["unresolved_disputes"] == 0
    assert escrow.balances(COLD[0].ss58).dispute_locked == 100  # the refused bond is still held


def seed_reward_origin(n, run_id, name: bytes, owner: str):
    """What finalize_v2 leaves for one reward origin: its settlement record, minus the round."""
    final_hash, origin = "ab" * 32, sha256_hex(name)
    db = n.store._db
    db.execute("INSERT INTO escrow_origins VALUES(?,?,?,?,?)", (origin, 50, 10, 0, HOT[3].ss58))
    db.execute(
        "INSERT INTO escrow_units VALUES(?,?,?,?,?)", (origin, owner, "reward_pending", "", 50)
    )
    db.execute("INSERT OR IGNORE INTO escrow_finalized VALUES(?,?)", (0, final_hash))
    n.store._put_record_v2(
        run_id,
        "settlement",
        origin,
        Settlement(
            finality_hash=final_hash,
            closed_dispute_root=sha256_hex(canonicalize([])),
            release_beacon=10,
            unresolved=False,
            outcome="MATCH",
        ).body(),
    )
    return origin


def test_a_bond_named_after_a_later_reward_origin_does_not_stop_the_origin_maturing(net) -> None:
    n = net
    run_id = n.manifest.run_id()
    admission_id = n.join(0).json()["admission_id"]
    escrow, _, disputes = n.store._services(run_id)
    seed_round_zero(n, run_id)
    victim = sha256_hex(b"victim")  # the name of an origin that does not exist yet
    contest, dispute = make_contest(n, run_id, 0, "a1" * 32, admission_id)
    n.push(2)
    post_lock(n, 0, victim, dispute)
    assert post_dispute(n, 0, contest, victim).status_code == 200  # the name was free
    # When: the origin appears and vests while the dispute is open.
    assert seed_reward_origin(n, run_id, b"victim", COLD[3].ss58) == victim
    escrow.mature(sha256_hex(b"m-victim"), (victim,), 50)
    # Then: it matured; the dispute's bond did not stand in for its settlement.
    assert escrow.balances(COLD[3].ss58).reward_pending == 0


def test_a_dispute_id_is_not_answered_by_an_older_dispute_whose_bond_is_named_after_it(net) -> None:
    n = net
    run_id = n.manifest.run_id()
    admission_id = n.join(0).json()["admission_id"]
    escrow, _, disputes = n.store._services(run_id)
    seed_round_zero(n, run_id)
    c1, d1 = make_contest(n, run_id, 0, "a1" * 32, admission_id)
    c2, d2 = make_contest(n, run_id, 0, "a2" * 32, admission_id)
    n.push(2)
    post_lock(n, 0, d2, d1)  # the first bond is named after the second dispute's future id
    assert post_dispute(n, 0, c1, d2).status_code == 200
    assert post_resolution_match(n, disputes, d1).status_code == 200
    n.push(4)
    recover = n.client.post(
        n.url + "/admission/recover",
        json=n.signed(HOT[0], "Receipt", Receipt(w=0, commit_hash=d1, received_round=1)),
    )
    assert recover.status_code == 200, recover.text
    post_lock(n, 0, "45" * 32, d2)
    assert post_dispute(n, 0, c2, "45" * 32).status_code == 200
    # When / Then: the second dispute is open, and the reference d2 says so.
    answer = n.store._settlement_v2(run_id, d2)
    assert (answer.dispute_id, answer.unresolved) == (d2, True)


# ------------------------------------------------------------------ ledger boundary
def contest_lock(origin, dispute):
    return EscrowLock(
        operation_id="22" * 32,
        owner=E.COLD.ss58,
        units=1000,
        origin_ids=[origin],
        admission_id=None,
        dispute_id=dispute,
        kind="LOCK_CONTEST",
    )


def contest_release(origin, dispute):
    return EscrowRelease(
        operation_id="33" * 32,
        owner=E.COLD.ss58,
        units=1000,
        origin_ids=[origin],
        admission_id=None,
        dispute_id=dispute,
        lock_id="22" * 32,
        finality_hash=E.H,
        closed_dispute_root=E.H,
    )


@pytest.mark.parametrize("outcome", ["MATCH", "FRAUD"])
def test_contest_lock_is_not_settled_by_the_settlement_of_another_dispute(
    tmp_path: Path, outcome: str
) -> None:
    # Given: a contest lock for dispute H; the resolver answers with the settlement of dispute X.
    status = Settlement(
        finality_hash=E.H,
        closed_dispute_root=E.H,
        release_beacon=10,
        unresolved=False,
        outcome=outcome,
        dispute_id="55" * 32,
    )
    e = E.ledger(E.setup(), tmp_path / "ledger.db", settlement=lambda _: status)
    e.contest_settlement = lambda lock: status
    origin, _ = E.fund(e)
    e.lock(contest_lock(origin, E.H), signer=E.COLD.ss58)
    op = contest_release(origin, E.H)
    before = e.balances()
    # When / Then: neither release nor slash accepts it.
    with pytest.raises(EscrowError):
        if outcome == "MATCH":
            e.release(op, signer=E.COLD.ss58, now_beacon=10)
        else:
            e.slash(op, authority=E.COORD.ss58, now_beacon=10)
    assert e.balances() == before


def test_contest_lock_has_no_settlement_without_a_contest_resolver(tmp_path: Path) -> None:
    # Given: only the untyped callback exists, and it would happily answer MATCH for any id.
    status = Settlement(
        finality_hash=E.H,
        closed_dispute_root=E.H,
        release_beacon=10,
        unresolved=False,
        outcome="MATCH",
    )
    e = E.ledger(E.setup(), tmp_path / "ledger.db", settlement=lambda _: status)
    origin, _ = E.fund(e)
    e.lock(contest_lock(origin, E.H), signer=E.COLD.ss58)
    # When / Then: a contest bond is never settled by the untyped callback.
    with pytest.raises(EscrowError, match="UNACCEPTED_SETTLEMENT"):
        e.release(contest_release(origin, E.H), signer=E.COLD.ss58, now_beacon=10)
    assert e.balances().dispute_locked == 1000


# ------------------------------------------------------------------ admission locks are typed too
A = _load("settlement_typing_admission_fixture", "test_admission_v2.py")


def admission_lock_world(tmp_path: Path, name: str, owner_funds: bool = True):
    """An admission whose coldkey holds one admission lock under `name`; no admission resolver."""
    s = A.setup()
    a = A.service(s, tmp_path)
    admission_id = A.apply(a, s)
    origin, _ = A.fixtures.fund(a.escrow)
    lock = EscrowLock(
        operation_id=name,
        owner=A.COLD.ss58,
        units=1000,
        origin_ids=[origin],
        admission_id=admission_id,
        dispute_id=None,
        kind="LOCK_ADMISSION",
    )
    a.escrow.lock(lock, signer=A.COLD.ss58)
    a.escrow.admission_settlement = None  # what the store wires today: nothing settles a bond
    return a, lock


def release_of(lock: EscrowLock, finality_hash: str, root: str) -> EscrowRelease:
    return EscrowRelease(
        operation_id="88" * 32,
        owner=lock.owner,
        units=lock.units,
        origin_ids=lock.origin_ids,
        admission_id=lock.admission_id,
        dispute_id=None,
        lock_id=lock.operation_id,
        finality_hash=finality_hash,
        closed_dispute_root=root,
    )


def test_an_admission_lock_has_no_settlement_without_an_admission_resolver(tmp_path: Path) -> None:
    # Given: the callback that settles disputes and origins would answer MATCH for any name.
    a, lock = admission_lock_world(tmp_path, "77" * 32)
    match = Settlement(
        finality_hash=E.H,
        closed_dispute_root=E.H,
        release_beacon=10,
        unresolved=False,
        outcome="MATCH",
    )
    a.escrow.settlement = lambda reference: match
    # When / Then: an admission lock is not settled by it.
    with pytest.raises(EscrowError, match="UNACCEPTED_SETTLEMENT"):
        a.escrow.release(release_of(lock, E.H, E.H), signer=A.COLD.ss58, now_beacon=20)
    assert a.escrow.balances(A.COLD.ss58).admission_locked == 1000
    # and with a resolver for admission locks, the same release settles.
    a.escrow.admission_settlement = lambda reference: match
    a.escrow.release(release_of(lock, E.H, E.H), signer=A.COLD.ss58, now_beacon=20)
    assert a.escrow.balances(A.COLD.ss58).admission_locked == 0


def test_an_admission_lock_named_like_a_closed_dispute_is_not_released_by_it(
    tmp_path: Path,
) -> None:
    # Given: a closed, vested dispute, and an admission lock that carries its id.
    d, f, ref, first = first_dispute_closed(tmp_path)
    closed = d.settlement(first.dispute_id)
    origin = free_origin(d.db, f.COLD.ss58)
    borrowed = EscrowLock(
        operation_id=first.dispute_id,
        owner=f.COLD.ss58,
        units=100,
        origin_ids=[origin],
        admission_id=E.H,
        dispute_id=None,
        kind="LOCK_ADMISSION",
    )
    d.escrow.lock(borrowed, signer=f.COLD.ss58)
    d.escrow.admission_settlement = None
    # When / Then: the dispute's settlement does not release it.
    with pytest.raises(EscrowError, match="UNACCEPTED_SETTLEMENT"):
        d.escrow.release(
            release_of(borrowed, closed.finality_hash, closed.closed_dispute_root),
            signer=f.COLD.ss58,
            now_beacon=5,
        )
    assert d.escrow.balances(f.COLD.ss58).admission_locked == 100


def test_an_admission_lock_named_like_a_reward_origin_is_not_released_over_the_routes(net) -> None:
    n = net
    run_id = n.manifest.run_id()
    admission_id = n.join(0).json()["admission_id"]
    escrow = n.store._services(run_id)[0]
    origin = seed_reward_origin(n, run_id, b"settled-origin", COLD[3].ss58)
    n.push(2)
    held = free(n, COLD[0].ss58)
    lock = EscrowLock(
        operation_id=origin,
        owner=COLD[0].ss58,
        units=1000,
        origin_ids=[held],
        admission_id=admission_id,
        dispute_id=None,
        kind="LOCK_ADMISSION",
    )
    posted = n.client.post(n.url + "/escrow/lock", json=n.signed(COLD[0], "EscrowLock", lock))
    assert posted.status_code == 200, posted.text
    n.push(12)  # past the origin's release beacon
    release = release_of(lock, "ab" * 32, sha256_hex(canonicalize([])))
    response = n.client.post(
        n.url + "/escrow/release", json=n.signed(COLD[0], "EscrowRelease", release)
    )
    # The master wires the admission resolver (ADR 0010 part 6): the bond is refused by its own
    # settlement, not released with the origin's, whose release beacon had passed.
    assert response.status_code == 409 and "UNACCEPTED_SETTLEMENT" not in response.text
    assert escrow.balances(COLD[0].ss58).admission_locked == 1000


def test_an_unknown_lock_is_refused_before_any_settlement_is_asked(tmp_path: Path) -> None:
    asked: list[str] = []

    def settlement(reference: str) -> Settlement:
        asked.append(reference)
        return Settlement(
            finality_hash=E.H,
            closed_dispute_root=E.H,
            release_beacon=10,
            unresolved=False,
            outcome="MATCH",
        )

    e = E.ledger(E.setup(), tmp_path / "ledger.db", settlement=settlement)
    origin, _ = E.fund(e)
    ghost = contest_release(origin, E.H).model_copy(update={"lock_id": "99" * 32})
    with pytest.raises(EscrowError, match="UNKNOWN_LOCK"):
        e.release(ghost, signer=E.COLD.ss58, now_beacon=10)
    assert asked == []  # a name that is no lock is not looked up in the dispute or origin tables
