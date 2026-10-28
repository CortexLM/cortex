"""Gap tests of the admission-lock group (D39, wave 2): the rows the reconciler found missing or
partial against the Lens acceptance tests T-LOCK-01..30.

Every test states a defence the service must hold ("pays the hotkey that holds the admission",
"matures what is due after a restart", "a read never sees another thread's uncommitted state").
A hole is a strict xfail whose body asserts the defence, with a control beside it that proves the
scenario reaches the point; the steps that make a scenario meaningful raise PreconditionError,
which is not an AssertionError, so a broken scenario fails loudly instead of hiding behind the
expected failure. Everything runs in-process on the v2 `network` fixture.
"""

from __future__ import annotations

import contextlib
import threading
from collections.abc import Iterator
from pathlib import Path
from types import SimpleNamespace
from typing import Any

import pytest

from hypertrain.challenge.admission_store import AdmissionError
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair

from .support import SERVICE_FIXTURE, load_fixture_module, service_world
from .support_admission_http import detail, precondition
from .support_gaps_admission_lock import (
    Calls,
    beacon,
    buckets_of,
    funded_cohort,
    holder,
    lock_is_free,
    poison_shadow_origin,
    proof_request,
    reduce_stake,
    reopened,
    rotate,
    snapshot,
    spare_hotkey,
    weights_at_now,
)
from .support_locks import (
    STAKE,
    Lab,
    Rig,
    ident,
    rotation_envelope,
    seed_live_origin,
    vested,
)
from .support_shadow import add_due_origin, db_digest, push_round

service = load_fixture_module(*SERVICE_FIXTURE)
# The module-scoped real shadow trial of the service tests, bound here so pytest finds it.
shadow_reservation_work = service.shadow_reservation_work
reserved_trial = service.reserved_trial
shadow_acceptance = service.shadow_acceptance


@pytest.fixture
def rig(tmp_path: Path) -> Iterator[Rig]:
    with service_world(tmp_path) as world:
        yield Rig(world)


# --- M-05 / CL-29: the payee of a matured reward is the hotkey that holds the admission --------


def test_matured_reward_units_are_emitted_to_the_hotkey_that_holds_the_admission(rig: Rig) -> None:
    """CL-29 + M-05: the units that matured for an admission are emitted to the hotkey that holds
    it, also when the coldkey rotated the admission while the origin was still pending."""
    # Given: an origin issued to the miner's hotkey and still pending, then its coldkey moves the
    # admission to another registered hotkey (a service that refuses that rotation is also right)
    miner = rig.join(0)
    origin = seed_live_origin(rig, miner)
    spare = spare_hotkey()
    rotated = rotate(rig, miner, spare)
    precondition(
        rotated.status_code == 200 or 400 <= rotated.status_code < 500,
        f"the rotation was neither accepted nor refused: {rotated.status_code} {rotated.text}",
    )
    payee = holder(rig, miner)
    precondition(
        payee == (spare.ss58 if rotated.status_code == 200 else miner.hotkey),
        f"the holder {payee} does not follow the answer of the rotation",
    )
    # When: the origin vests and the master asks for the epoch
    rig.net.push(5)
    precondition(vested(rig, origin) == 500, "the origin did not vest at its maturity round")
    answer = weights_at_now(rig.world)
    # Then: the whole origin is paid, to the hotkey that holds the admission
    assert answer["metadata"]["units_paid"] == 500
    assert answer["weights"] == {payee: 500.0}


# --- GAP-07: a beacon matures what is due without waiting for a request after a restart --------


@pytest.mark.parametrize(
    "services",
    [
        pytest.param("rebuilt-by-a-request", id="services-built-before-the-push"),
        pytest.param(
            "not-built",
            id="no-request-since-the-restart",
            marks=pytest.mark.xfail(
                strict=True,
                raises=AssertionError,
                reason="GAP-07: a beacon pushed to a restarted store matures what is due at "
                "once, but push_beacon walks only the services already built, and nothing "
                "builds them until a request asks, so units stay pending and no lease expires "
                "[fix-lot: none (no design yet)]",
            ),
        ),
    ],
)
def test_a_beacon_pushed_to_a_restarted_store_matures_what_is_due(
    tmp_path: Path, services: str
) -> None:
    with service_world(tmp_path) as world:
        # Given: an origin due at round 5, and the process restarted before it vested
        rig = Rig(world)
        miner = rig.join(0)
        origin = seed_live_origin(rig, miner)
        rig.lock_stake(miner)  # a ledger event after the seed, so the restart verifies the chain
        run_id = world.run_id
        with reopened(world) as store:
            precondition(store._services_v2 == {}, "the reopened store already holds services")
            precondition(
                buckets_of(store._db, origin) == {"reward_pending": 500},
                "the origin is not pending on the reopened store",
            )
            if services == "rebuilt-by-a-request":
                store._services(run_id)  # what any unrelated request does first
            leases = Calls(store, "_expire_leases_v2")
            # When: the beacon of round 5 is pushed and nothing else happens
            store.push_beacon(beacon(5))
            # Then: the units vested and the leases of the run were looked at for that round
            assert buckets_of(store._db, origin) == {"available": 500, "reward_pending": 0}
            assert leases.calls == [(run_id, 5)]


# --- GAP-08: a read that feeds an authority decision runs under the store lock -----------------


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="GAP-08: a read of the admission row on the shared connection never observes another "
    "thread's uncommitted transaction, but AdmissionStore.by_id takes no lock and the "
    "connection's transaction state is shared by every thread "
    "[fix-lot: none (no design yet)]",
)
def test_an_admission_read_never_sees_the_uncommitted_state_of_another_thread(rig: Rig) -> None:
    # Given: a writer holds a transaction on the shared connection that suspends the admission
    # and will fail; a raw read of the connection shows the suspension (the control)
    miner = rig.join(0)
    store, admissions = rig.world.store, rig.admission.store
    holding, release, started, finished = (threading.Event() for _ in range(4))
    seen: list[str] = []

    def writer() -> None:
        with contextlib.suppress(RuntimeError), store._tx():
            store._db.execute(
                "UPDATE admissions_v2 SET state='SUSPENDED' WHERE admission_id=?",
                (miner.admission_id,),
            )
            holding.set()
            release.wait(timeout=60)
            raise RuntimeError("the transaction of the writer fails")

    def reader() -> None:
        started.set()
        seen.append(admissions.by_id(miner.admission_id).state)
        finished.set()

    threads = [threading.Thread(target=writer), threading.Thread(target=reader)]
    threads[0].start()
    try:
        precondition(holding.wait(timeout=60), "the writer never opened its transaction")
        raw = store._db.execute(
            "SELECT state FROM admissions_v2 WHERE admission_id=?", (miner.admission_id,)
        ).fetchone()
        precondition(raw[0] == "SUSPENDED", "the uncommitted write is not visible to the reader")
        # When: another thread reads the admission meanwhile; the writer then rolls back
        threads[1].start()
        precondition(started.wait(timeout=60), "the reader never started")
        finished.wait(timeout=5)
    finally:
        release.set()
        for thread in threads:
            if thread.ident is not None:
                thread.join(timeout=60)
    precondition(not any(t.is_alive() for t in threads), "a thread did not finish")
    precondition(
        rig.admission.store.by_id(miner.admission_id).state == "APPLIED",
        "the writer's transaction was not rolled back",
    )
    # Then: the read returned the committed state, either waiting for the writer or before it
    assert seen == ["APPLIED"]


# --- T-LOCK-06: one stake backs exactly one admission ------------------------------------------


def test_one_stake_backs_exactly_one_admission_of_a_coldkey(tmp_path: Path) -> None:
    # Given: two live admissions of one coldkey (an ACTIVE admission may admit a further hotkey)
    lab = Lab(tmp_path)
    first = lab.join()
    lab.live(first)
    second = lab.join(hot=lab.A.OTHER, cold=lab.A.COLD, request="33" * 32)
    lab.live(second)
    first_hot, second_hot, cold = lab.A.HOT.ss58, lab.A.OTHER.ss58, lab.A.COLD.ss58
    escrow = lab.a.escrow

    def standing(hotkey: str) -> tuple[bool, int]:
        status = lab.a.status(hotkey, now=5)
        return status.eligible, status.funding.locked_units

    # When: the coldkey locks the requirement for the first admission only
    lab.stake(first, STAKE)
    # Then: the second admission sees none of it, and holds no lock of its own
    assert standing(first_hot) == (True, STAKE)
    assert standing(second_hot) == (False, 0)
    assert escrow.locked(first, cold)[0] == STAKE
    assert escrow.locked(second, cold) == (0, "0" * 64)
    assert escrow.balances().admission_locked == STAKE
    # When: it locks the requirement again, for the second admission under its own operation id
    lab.stake(second, STAKE, operation="24" * 32)
    # Then: each admission is backed by its own lock and the ledger holds both
    assert standing(first_hot) == (True, STAKE)
    assert standing(second_hot) == (True, STAKE)
    balances = escrow.balances()
    assert balances.admission_locked == 2 * STAKE
    assert balances.conserved()


# --- CL-30: the store lock is not held while the service computes or hashes a trial -----------


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="CL-30+LV-01: no request holds the store lock while the trusted reference replays, "
    "whichever route asks for it, but a miner's POST /join/proof (reference_due, then "
    "trial_reference_v2) holds it for the whole replay with no operator credential "
    "[fix-lot: executor-perf ADR0012-P2]",
)
def test_the_store_lock_is_free_while_the_public_proof_route_computes_the_reference(
    rig: Rig, monkeypatch: pytest.MonkeyPatch
) -> None:
    # Given: an admission with an open trial, and a reference computation that stalls
    miner = rig.join(0)
    rig.net.push(2)
    opened = rig.open_trial(miner)
    precondition(opened.status_code == 200, f"the trial was not opened: {opened.text}")
    hosted = rig.world.store.for_run(rig.run_id)
    started, resume = threading.Event(), threading.Event()

    def computing(*args: object, **kwargs: object) -> str:
        started.set()
        resume.wait(timeout=60)
        return ident("gap-reference")

    monkeypatch.setattr(hosted._services(rig.run_id)[1], "prepare_reference", computing)
    body, replies = proof_request(rig, miner), []
    caller = threading.Thread(
        target=lambda: replies.append(rig.net.client.post(f"{rig.net.url}/join/proof", json=body))
    )
    precondition(lock_is_free(hosted), "the lock is held while the service is idle")
    # When: the miner posts its proof and the reference is being computed
    caller.start()
    try:
        precondition(started.wait(timeout=60), "the reference computation never started")
        free = lock_is_free(hosted)
    finally:
        resume.set()
        caller.join(timeout=60)
    precondition(not caller.is_alive(), "the proof route never answered")
    precondition(
        (replies[0].status_code, detail(replies[0])) == (409, "REFERENCE_NOT_COMMITTED"),
        f"the route did not run past the stalled reference: {replies[0].text}",
    )
    # Then: another thread could have taken the store lock meanwhile
    assert free, "the store lock is held for as long as the reference computes"


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="CL-30+LV-01: no request holds the store lock while the staged inputs of a trial are "
    "hashed, but export_trial_v2 puts every staged input into the object store inside "
    "`with self._lock` [fix-lot: none (no design yet)]",
)
def test_the_store_lock_is_free_while_the_staged_inputs_of_a_trial_are_hashed(
    rig: Rig, monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    # Given: an open trial whose staging is a stand-in (one staged input) and whose object store
    # stalls while it hashes that input
    miner = rig.join(0)
    rig.net.push(2)
    opened = rig.open_trial(miner)
    precondition(opened.status_code == 200, f"the trial was not opened: {opened.text}")
    hosted = rig.world.store.for_run(rig.run_id)
    staged = tmp_path / "staged"
    staged.mkdir()
    (staged / "state.bin").write_bytes(b"staged state")
    job = SimpleNamespace(
        object_paths={"state": "state.bin"},
        model_dump_json=lambda: '{"job":1}',
        body=lambda: {"job": 1},
    )
    monkeypatch.setattr(hosted, "stage_trial_v2", lambda *args, **kwargs: (job, staged))
    started, resume = threading.Event(), threading.Event()
    put = hosted.objects.put

    def hashing(data: bytes) -> str:
        started.set()
        resume.wait(timeout=60)
        return str(put(data))

    monkeypatch.setattr(hosted.objects, "put", hashing)
    replies = []
    caller = threading.Thread(target=lambda: replies.append(rig.read_job(miner)))
    precondition(lock_is_free(hosted), "the lock is held while the service is idle")
    # When: the owner asks for its trial inputs and the hashing of the staged input stalls
    caller.start()
    try:
        precondition(started.wait(timeout=60), "the input was never hashed")
        free = lock_is_free(hosted)
    finally:
        resume.set()
        caller.join(timeout=60)
    precondition(not caller.is_alive(), "the job route never answered")
    precondition(replies[0].status_code == 200, f"the inputs were refused: {replies[0].text}")
    # Then: another thread could have taken the store lock meanwhile
    assert free, "the store lock is held for as long as the trial inputs are hashed"


# --- T-LOCK-03: the stake of a roster member is read again when it accepts --------------------


def test_an_accept_is_refused_when_the_stake_of_the_miner_fell_after_the_opening(rig: Rig) -> None:
    # Given: an opened round whose four members hold exactly the required stake
    miners = funded_cohort(rig)
    opening = rig.opening(miners)
    assert rig.open_round(opening).status_code == 200
    rig.net.push(opening.d_assign)
    hashes = {m.hotkey: rig.assignment_hash(m) for m in miners}
    # control: a member that kept its stake is accepted
    precondition(rig.accept(miners[1], hashes[miners[1].hotkey]).status_code == 200, "control")
    # When: another member's stake falls one unit short of the requirement
    short = miners[0]
    reduce_stake(rig, short)
    precondition(not rig.status(short).eligible, "a member one unit short is still eligible")
    before = rig.world.db_digest()
    refused = rig.accept(short, hashes[short.hotkey])
    # Then: its accept is refused for its funding and nothing is written; the other member that
    # kept its stake is still accepted
    assert (refused.status_code, detail(refused)) == (
        403,
        "screen or funded admission stale",
    )
    assert rig.world.db_digest() == before
    assert rig.escrow.balances().conserved()
    assert rig.accept(miners[2], hashes[miners[2].hotkey]).status_code == 200


# --- T-LOCK-10: a reward is released once the dispute on its round is settled for the miner ----


def test_a_reward_origin_vests_after_a_dispute_the_miner_won(rig: Rig) -> None:
    # Given: round 0 issued the miner an origin maturing at round 5, and the miner's contest of
    # round 0 is resolved against the auditor
    miner = rig.join(0)
    origin = seed_live_origin(rig, miner)
    contest = rig.open_contest(miner)
    auditor = rig.service.AUDITORS[0].ss58
    ruled = rig.refereed(contest.dispute_id, "FRAUD", auditor)
    precondition(ruled.status_code == 200, f"the resolution was refused: {ruled.text}")
    settlement = rig.disputes.settlement(contest.dispute_id)
    precondition(
        (settlement.unresolved, settlement.outcome) == (False, "MATCH"),
        f"the dispute did not settle for the miner: {settlement}",
    )
    # When: the beacon reaches the later of the origin's round and the dispute's release round
    rig.net.push(max(5, settlement.release_beacon))
    # Then: the units vested; a hold that has no end would fail here
    assert vested(rig, origin) == 500
    assert rig.escrow.balances().conserved()


# --- T-LOCK-19: rotating a hotkey that never held an admission ---------------------------------


def rotation_of(lab: Lab, stranger: Keypair, target: Keypair) -> bytes:
    run_id = lab.a.escrow.run_id
    return canonicalize(
        rotation_envelope(run_id, lab.A.COLD, stranger.ss58, target, "7a" * 32, 200)
    )


def test_a_hotkey_without_an_admission_history_cannot_be_rotated(tmp_path: Path) -> None:
    # Given: one admission, and a rotation request for a hotkey that never held any
    lab = Lab(tmp_path)
    lab.join()
    stranger, target = Keypair(b"\x0b" * 32), Keypair(b"\x0c" * 32)
    raw = rotation_of(lab, stranger, target)
    before = snapshot(lab)
    # When / Then: refused for its missing history, and no table changed
    with pytest.raises(AdmissionError) as refused:
        lab.a.rotate(raw, now=2)
    assert refused.value.code == "UNKNOWN_ROTATION_HISTORY"
    assert snapshot(lab) == before


def test_the_rotation_route_answers_409_for_a_hotkey_without_an_admission_history(
    rig: Rig,
) -> None:
    # Given: an admitted miner, and its coldkey naming a registered hotkey that never joined as
    # the one to rotate away from
    miner = rig.join(0)
    stranger = rig.service.HOT[2]
    envelope = rotation_envelope(
        rig.run_id, miner.cold, stranger.ss58, spare_hotkey(), ident("unknown-history")
    )
    before = rig.world.db_digest()
    # When
    response = rig.net.client.post(f"{rig.net.url}/admission/{stranger.ss58}/rotate", json=envelope)
    # Then: the route refuses it and writes nothing; the admitted miner keeps its hotkey
    assert (response.status_code, detail(response)) == (409, "UNKNOWN_ROTATION_HISTORY")
    assert rig.world.db_digest() == before
    assert holder(rig, miner) == miner.hotkey


# --- T-LOCK-24: a reservation cites only accepted shadow-execution records ---------------------


@pytest.mark.parametrize("cause", ["none", "unknown-hashes", "private-runtime-custody"])
def test_a_reservation_names_the_accepted_execution_it_cannot_find(
    reserved_trial: Any, cause: str
) -> None:
    # Given: a finalized first trial and the reservation body of its service tests
    n, identity, body = reserved_trial
    custody = body["custody"]
    missing = custody["reference_operation_hash"]
    if cause == "unknown-hashes":
        # two valid hashes that no accepted execution carries
        custody["reference_operation_hash"] = missing = "ab" * 32
        custody["miner_operation_hash"] = "ac" * 32
    elif cause == "private-runtime-custody":
        # the two retained executions, relabelled as the private custody of the runtime
        n.store._db.execute(
            "UPDATE records_v2 SET kind='runtime-custody' WHERE kind='shadow-execution'"
        )
    before = db_digest(n.store._db)
    # When: the operator posts the reservation
    response = n.client.post(
        n.url + f"/admin/join/{identity}/shadow-reservation",
        json=body,
        headers=service.admin(),
    )
    reserved = n.store._db.execute(
        "SELECT COUNT(*) FROM records_v2 WHERE kind='shadow-reservation'"
    ).fetchone()[0]
    # Then: the honest body is reserved once; any other is refused naming the hash it could not
    # resolve, with nothing reserved and nothing written
    if cause == "none":
        assert response.status_code == 200, response.text
        assert reserved == 1
        return
    assert (response.status_code, detail(response)) == (
        409,
        f"missing accepted shadow-execution: {missing}",
    )
    assert reserved == 0
    assert db_digest(n.store._db) == before


# --- T-LOCK-27: one poisoned shadow origin never stops the work of a beacon --------------------

# SHD-19+SHD-21+CL-14+CL-36 (F6): a beacon push matures every due origin, expires the leases,
# closes the blocked disputes and reaps the trials whatever one shadow origin's custody or
# identity has become, and the next push is no different. The journal row of the origin whose
# re-validation raises is pinned by tests/challenge/test_beacon_maturity_v2.py, not here.
POISONS = [
    "none",
    "the shadow-custody record is missing",
    "a retained publication changed",
    "the admission was rotated to a spare hotkey",
]


@pytest.mark.parametrize("fault", POISONS)
def test_one_poisoned_shadow_origin_never_stops_the_work_of_a_beacon(
    shadow_acceptance: Any, fault: str
) -> None:
    # Given: a credited shadow origin, a second origin due at the same beacon, and the work a
    # beacon push does after it matures (recorded, still run)
    n, identity, body = shadow_acceptance
    accepted = n.client.post(
        n.url + f"/admin/join/{identity}/shadow-reward", json=body, headers=service.admin()
    )
    precondition(accepted.status_code == 200, f"the credit was refused: {accepted.text}")
    due = accepted.json()["mature_at"]
    run_id, store = n.manifest.run_id(), n.store
    escrow, admission = store._services(run_id)[:2]
    witness = add_due_origin(escrow, service.COLD[1].ss58, 500, due)
    poison_shadow_origin(n, body, fault)
    leases = Calls(store, "_expire_leases_v2")
    disputes = Calls(store, "_close_blocked_disputes_v2")
    trials = Calls(admission, "reap_expired_trials")
    # When: the beacon of the maturity round is pushed
    first = push_round(n, due)
    # Then: it is accepted, the independent origin vests, and the rest of the push ran
    assert first.status_code == 200, first.text
    assert buckets_of(escrow.db, witness).get("available") == 500
    assert store.latest_beacon()["round"] == due
    assert leases.calls == [(run_id, due)]
    assert disputes.calls == [(run_id,)]
    assert len(trials.calls) == 1
    # and the next beacon is no different
    second = push_round(n, due + 1)
    assert second.status_code == 200, second.text
    assert store.latest_beacon()["round"] == due + 1
    assert leases.calls[-1] == (run_id, due + 1)
    assert (len(disputes.calls), len(trials.calls)) == (2, 2)
    assert escrow.balances().conserved()
