"""Builders of the disputes gap wave. Not collected: no test_ prefix.

They only compose what `support`, `support_disputes`, `support_exit_disputes` and
`support_opids` already build; nothing here touches src.
"""

from __future__ import annotations

import traceback
from collections.abc import Callable, Iterator
from contextlib import contextmanager
from dataclasses import asdict
from pathlib import Path
from typing import Any

import pytest

from hypertrain.auditor.island_bisect import RefereeEvidence
from hypertrain.challenge.admission_store import AdmissionError
from hypertrain.challenge.disputes_v2 import Turn
from hypertrain.challenge.finality_v2 import RoundStatus
from hypertrain.challenge.store import ChallengeError
from hypertrain.ledger.escrow_v2 import (
    EscrowV2,
    FinalityEvidence,
    RewardWork,
    authority_message,
    digest,
)
from hypertrain.protocol.envelope_v2 import EnvelopeV2, seal
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import Finalize, ReplayEnv, ReplayVerdict, Rollback
from hypertrain.protocol.messages_v2 import (
    AuditChallengeV2,
    CommitV2,
    DisputeV2,
    EscrowLock,
    OriginAllocation,
    RewardFinalize,
)

from . import support_exit_disputes as X
from . import support_exit_roster as R
from .support import H, World
from .support_disputes import Rig as DisputeRig
from .support_disputes import refusal, reward_origin_of
from .support_locks import Lab, Miner, Rig, settled_status
from .support_opids import DefenceMissing, fresh_id

AGREED = "11" * 32  # what both parties say at the lower end of every interval
MINER_CLAIM = "22" * 32
AUDITOR_CLAIM = "aa" * 32


def says(turn: Turn, *, miner: bool) -> list[str]:
    """The three hashes a party signs for `turn`: both agree at its lower end and disagree
    everywhere else. A point that appears twice (an interval of one op) carries one hash, which
    is what the engine requires of a repeated point."""
    low, high = turn.interval
    points = [low, -(-(low + high) // 2), high]
    return [
        AGREED if point == low else (MINER_CLAIM if miner else AUDITOR_CLAIM) for point in points
    ]


@contextmanager
def defence() -> Iterator[None]:
    """The assertions inside state the defence under test. A failure of theirs is a missing
    defence (`DefenceMissing`), so a strict xfail that expects it is never satisfied by a control
    that broke before it, which fails with a plain AssertionError."""
    try:
        yield
    except AssertionError as error:
        raise DefenceMissing(str(error)) from error


def settlement_id(dispute_id: str) -> str:
    """The operation id under which the service releases the bond of `dispute_id` when the
    dispute is closed: derived from the public dispute id alone."""
    return sha256_hex(canonicalize([dispute_id, "settle"]))


def closing_round(world: World, dispute_id: str) -> int:
    """First beacon round at which the exit closes an unanswered, unpaused contest: the round
    after the operator's window (turn_deadline + T.dispute_per_level) is over."""
    turn = X.disputes(world).get(dispute_id)
    grace = world.network.manifest.training.verify.T.dispute_per_level
    return turn.turn_deadline + grace + 1


# --- a real audited round whose members can contest their own verdict ------------------------


@contextmanager
def audited_round(root: Path) -> Iterator[R.Round]:
    """Round 0 of five real members (the tiny CPU model runs), every one audited MATCH, with the
    aggregation not yet requested. Five, because the weight allocation needs four eligible members
    and the scenarios drop one. About 25 s."""
    with R.activated_network(root) as network:
        rnd = R.open_round(network, 0, [0, 1, 2, 3, 4])
        R.train(rnd, rnd.members)
        response = R.schedule(rnd)
        R.control(response.status_code == 200, response.text)
        R.audit_queue(rnd, len(rnd.members))
        yield rnd


def verdict_of_member(rnd: R.Round, index: int) -> str:
    """Hash of the verdict the audit recorded for member `index` (its contest record's key)."""
    row = rnd.store._db.execute(
        "SELECT id FROM records_v2 WHERE run_id=? AND kind='contest' "
        "AND json_extract(data,'$.miner')=? AND json_extract(data,'$.w')=?",
        (rnd.run_id, rnd.hotkey(index), rnd.w),
    ).fetchone()
    R.control(row is not None, f"the audit left no contest record for member {index}")
    return str(row[0])


def dispute_id_of_member(rnd: R.Round, index: int) -> str:
    """Id of the dispute that member `index` opens against its own verdict."""
    verdict = verdict_of_member(rnd, index)
    return sha256_hex(canonicalize([rnd.run_id, verdict, rnd.hotkey(index)]))


def contest_request(rnd: R.Round, index: int, verdict: str) -> Any:
    """Fund a bond from member `index`'s coldkey and POST its contest of `verdict`; returns the
    response of the dispute route unchecked."""
    net, store = rnd.network, rnd.store
    origin = store._db.execute(
        "SELECT origin FROM escrow_units WHERE owner=? AND bucket='available' AND units>=?",
        (R.cold(index).ss58, X.BOND),
    ).fetchone()[0]
    dispute_id = sha256_hex(canonicalize([rnd.run_id, verdict, rnd.hotkey(index)]))
    lock = EscrowLock(
        operation_id=fresh_id(f"gaps-bond-{index}"),
        owner=R.cold(index).ss58,
        units=X.BOND,
        origin_ids=[origin],
        admission_id=None,
        dispute_id=dispute_id,
        kind="LOCK_CONTEST",
    )
    funded = net.client.post(
        net.url + "/escrow/lock", json=net.signed(R.cold(index), "EscrowLock", lock)
    )
    R.control(funded.status_code == 200, funded.text)
    body = DisputeV2(
        hotkey=rnd.hotkey(index),
        verdict_hash=verdict,
        action="contest",
        bond_lock=lock.operation_id,
    )
    return net.client.post(net.url + "/dispute", json=net.signed(R.hot(index), "DisputeV2", body))


def state_of_member(rnd: R.Round, index: int) -> tuple[str, int, int]:
    """(state, pending_dispute, clean_count) of member `index`."""
    row = rnd.store._db.execute(
        "SELECT state,pending_dispute,clean_count FROM admissions_v2 WHERE hotkey=?",
        (rnd.hotkey(index),),
    ).fetchone()
    return str(row["state"]), int(row["pending_dispute"]), int(row["clean_count"])


# --- the ledger boundary of a run whose first rounds were closed without a reward --------------

THETA = "bb" * 32  # the model state every round of `signed_round` ends in


def signed_round(
    world: World, escrow: EscrowV2, w: int
) -> tuple[Finalize, RewardFinalize, FinalityEvidence]:
    """Coordinator-signed Finalize and RewardFinalize of round `w` over one audited MATCH commit
    of miner 0, with the evidence the ledger replays. Valid for any `w`: only the order differs."""
    service = X.service()
    manifest, run = world.network.manifest, world.run_id
    hot, cold = service.HOT[0], service.COLD[0]
    samples = tuple(range(manifest.training.batch_samples()))
    final = Finalize(w=w, final_theta_hash_w1=THETA, included=[hot.ss58], entitlements_root=H)
    commit = CommitV2(
        w=w,
        hotkey=hot.ss58,
        leaf_scheme="ht-leaf-v1",
        n_leaves=manifest.training.inner.H // manifest.training.inner.J + 1,
        leaves_root="aa" * 32,
        metrics_root=H,
        final_theta_hash=THETA,
        ef_in_hash=H,
        ef_out_hash=H,
        delta_hash="cc" * 32,
        delta_bytes=1,
        tokens=len(samples) * manifest.training.model.seq_len,
    )
    challenge = AuditChallengeV2(
        w=w,
        target=hot.ss58,
        beacon_round=1,
        beacon_sig_sha256=H,
        mode="full",
        segments=[],
        reasons=["probation"],
        serve_deadline=200,
        anchor_hash=H,
        audit_mode="anchored-full",
    )
    reference = manifest.training.reference_spec
    verdict = ReplayVerdict(
        challenge_hash=digest(challenge),
        first_bad_leaf=None,
        result="MATCH",
        recomputed_leaves_root=commit.leaves_root,
        replay_env=ReplayEnv(
            image_digest=reference.image_digest,
            driver=reference.driver_allowlist[0],
            gpu_uuid_sha256=H,
            sm_count=1,
        ),
    )

    def sealed(key: Keypair, kind: str, body: Any) -> EnvelopeV2:
        return EnvelopeV2.model_validate(seal(key, kind, run, body, 200))

    evidence = FinalityEvidence(
        sealed(service.COORD, "Finalize", final),
        (
            RewardWork(
                sealed(hot, "CommitV2", commit),
                sealed(service.AUDITORS[0], "ReplayVerdict", verdict),
                sealed(service.COORD, "AuditChallengeV2", challenge),
                cold.ss58,
                samples,
            ),
        ),
        "dd" * 32,
        False,
        10,
        20,
    )
    origin = OriginAllocation(
        origin_id=reward_origin_of(escrow, run, w, hot.ss58, digest(final)),
        owner=cold.ss58,
        units=escrow.policy.round_reward_units,
        mature_at=20,
    )
    unsigned = RewardFinalize(
        run_id=run,
        w=w,
        finalize_hash=digest(final),
        tape_hash=evidence.tape_hash,
        verdict_root=sha256_hex(canonicalize([digest(verdict)])),
        allocation_hash=sha256_hex(canonicalize([origin.body()])),
        budget_units=escrow.policy.round_reward_units,
        origin_ids=[origin.origin_id],
        mature_at=20,
        authority_sig="0" * 128,
    )
    reward = RewardFinalize.model_validate(
        {**unsigned.body(), "authority_sig": service.COORD.sign(authority_message(unsigned)).hex()}
    )
    return final, reward, evidence


def replayable_ledger(world: World, monkeypatch: pytest.MonkeyPatch) -> EscrowV2:
    """The run's own escrow over the store database, with the external replay and the sample
    assignment fixed to what `signed_round` signs."""
    escrow: EscrowV2 = world.store._services(world.run_id)[0]
    samples = tuple(range(world.network.manifest.training.batch_samples()))
    monkeypatch.setattr(escrow, "replay_finality", lambda _evidence: THETA)
    monkeypatch.setattr(escrow, "owner_of", lambda _hotkey: X.service().COLD[0].ss58)
    monkeypatch.setattr(escrow, "assignment_of", lambda _w, _hotkey: samples)
    if hasattr(escrow, "accepted_at"):  # the evidence is judged at its acceptance, round 1 here
        monkeypatch.setattr(escrow, "accepted_at", lambda _envelope: 1)
    return escrow


def put_finality(world: World, status: RoundStatus) -> None:
    with world.store._tx():
        world.store._put_record_v2(status.run_id, "finality", str(status.w), asdict(status))


def applied_round(world: World, w: int) -> RoundStatus:
    """Round `w` applied, with no audit or dispute outstanding, neither settled nor excluded."""
    return RoundStatus(world.run_id, w, True, "PENDING", 0, 0, False, THETA, None)


def evidence_tag(world: World, dispute_id: str) -> str:
    """The tag (`op_spec`) of the evidence under which the server closed `dispute_id` itself."""
    resolution = X.disputes(world).get(dispute_id).resolution
    assert resolution is not None
    body = world.store._db.execute(
        "SELECT body FROM dispute_evidence_v2 WHERE hash=?", (resolution.evidence_hash,)
    ).fetchone()[0]
    return RefereeEvidence.model_validate_json(body).op_spec


# --- live reward origins of several miners of one round -----------------------------------------


def seed_round_origins(
    rig: Rig, miners: list[Miner], *, units: int = 500, mature_at: int = 5
) -> dict[str, str]:
    """What `finalize_v2` leaves for round 0 when each of `miners` earned `units`: one issued
    origin pending in the miner's coldkey, the finalized round (once) and the accepted
    settlement record of each origin (a MATCH vesting at `mature_at`). Written directly, as
    `support_locks.seed_live_origin` does for one miner. Returns hotkey -> origin id."""
    final = fresh_id("gaps-live-finalize")
    store, db = rig.world.store, rig.escrow.db
    origins = {}
    with store._tx():
        db.execute("INSERT INTO escrow_finalized VALUES(0,?)", (final,))
        for miner in miners:
            origin = fresh_id(f"gaps-live-origin-{miner.index}")
            record = settled_status(
                finality_hash=final,
                closed_dispute_root=sha256_hex(canonicalize([])),
                release_beacon=mature_at,
            )
            db.execute(
                "INSERT INTO escrow_origins VALUES(?,?,?,0,?)",
                (origin, units, mature_at, miner.hotkey),
            )
            db.execute(
                "INSERT INTO escrow_units VALUES(?,?,'reward_pending','',?)",
                (origin, miner.coldkey, units),
            )
            store._put_record_v2(rig.run_id, "settlement", origin, record.body())
            origins[miner.hotkey] = origin
    return origins


def available_of(rig: Rig, origin: str) -> int:
    """Units of `origin` that left reward_pending and sit in the owner's free balance."""
    return int(
        rig.escrow.db.execute(
            "SELECT COALESCE(SUM(units),0) FROM escrow_units WHERE origin=? AND bucket='available'",
            (origin,),
        ).fetchone()[0]
    )


# --- a bond that never became a dispute ---------------------------------------------------------


def fund_extra_bond(world: World, index: int, label: str) -> tuple[str, EscrowLock]:
    """Miner `index` records a further verdict (the audit's contest record) and its coldkey locks a
    second bond for the contest of it, over HTTP. Returns the verdict hash and the lock. The
    contest itself is not opened here."""
    net, fixture = world.network, X.service()
    verdict = fresh_id(f"gaps-verdict-{label}")
    first = X.disputes(world).get(X.dispute_id_of(world, index)).contest
    record = first.model_copy(update={"verdict_hash": verdict})
    with world.store._lock:
        world.store._put_record_v2(world.run_id, "contest", verdict, record.body())
    cold = fixture.COLD[index]
    origin = world.store._db.execute(
        "SELECT origin FROM escrow_units WHERE owner=? AND bucket='available' AND units>=?",
        (cold.ss58, X.BOND),
    ).fetchone()[0]
    lock = EscrowLock(
        operation_id=fresh_id(f"gaps-bond-{label}"),
        owner=cold.ss58,
        units=X.BOND,
        origin_ids=[origin],
        admission_id=None,
        dispute_id=sha256_hex(canonicalize([world.run_id, verdict, fixture.HOT[index].ss58])),
        kind="LOCK_CONTEST",
    )
    response = net.client.post(net.url + "/escrow/lock", json=net.signed(cold, "EscrowLock", lock))
    assert response.status_code == 200, response.text
    return verdict, lock


def post_dispute_on(world: World, index: int, verdict: str, lock: EscrowLock) -> Any:
    """POST /dispute: miner `index` contests `verdict` on the bond `lock`; unchecked response."""
    net, hot = world.network, X.service().HOT[index]
    body = DisputeV2(
        hotkey=hot.ss58, verdict_hash=verdict, action="contest", bond_lock=lock.operation_id
    )
    return net.client.post(net.url + "/dispute", json=net.signed(hot, "DisputeV2", body))


def lock_of(world: World, operation_id: str) -> EscrowLock:
    """The lock a coldkey signed under `operation_id`, as the ledger recorded it."""
    row = world.store._db.execute(
        "SELECT request FROM escrow_events WHERE operation_id=?", (operation_id,)
    ).fetchone()
    return EscrowLock.model_validate_json(row[0])


# --- rollbacks of two windows of one run --------------------------------------------------------


def request_rollback(world: World, w: int, miner: int, cause: str, *, old_theta: str) -> bytes:
    """The coordinator-signed Rollback of rounds `w` and `w + 1` that excludes miner `miner` for the
    referee resolution whose digest is `cause`."""
    body = Rollback(
        w=w,
        excluded=[X.service().HOT[miner].ss58],
        old_theta_hash_w2=old_theta,
        new_theta_hash_w2="0" * 63 + "1",
        new_outer_state_hash="00" * 32,
        recomputed=["agg_w", "step_w", "agg_w1", "step_w1"],
        cause_hashes=[cause],
    )
    return canonicalize(world.network.signed(X.service().COORD, "Rollback", body))


def two_windows(world: World) -> tuple[X.RollbackRig, bytes, bytes]:
    """A run of three applied rounds in which miner 0 lost a contest of round 0 and miner 1 a
    contest of round 1: a rollback of window 0 (rounds 0 and 1) and one of window 1 (rounds 1 and
    2) are each justified by a referee cause. The repair work itself is not seeded, so a request
    that passes the reservation gate stops in it (`RollbackRig.attempt` calls that None).

    Returns the rig of window 0 and the signed requests of window 0 and of window 1."""
    store, run, fixture = world.store, world.run_id, X.service()
    rig = X.rollback_rig(world)  # rounds 0 and 1 applied, miner 0 lost its contest of round 0
    request_0 = rig.request()
    X.seed_round(world, 2, applied=True)
    X.seed_open_round(world, 2, [0])
    dispute = X.open_contest(world, 1, w=1)
    ruling = X.resolve(world, dispute, reason="FRAUD", loser=fixture.HOT[1].ss58)
    with store._lock:
        store._put_record_v2(
            run,
            "applied",
            "1",
            {"theta_hash": "bb" * 32, "prev_state": "dd" * 32, "predecessor_tape_hash": "ee" * 32},
        )
        store._put_record_v2(run, "applied", "2", {"theta_hash": "cc" * 32})
        store._put_record_v2(run, "tape-inputs", "2", {"inputs": [1, 2, 3, 4, 5]})
    request_1 = request_rollback(world, 1, 1, ruling.digest(), old_theta="cc" * 32)
    return rig, request_0, request_1


def reservation_refusal(world: World, raw: bytes) -> str | None:
    """The detail of the 409 by which the repair-reservation gate of `rollback_v2` turns `raw`
    away, or None when the call got past the gate: it then fails inside the repair work, which
    this scenario does not seed (the call is required to have reached `_rollback_context_v2`,
    which only the code after the gate calls; a refusal before the gate is a broken scenario)."""
    try:
        world.store.rollback_v2(world.run_id, raw)
    except ChallengeError as error:
        if error.status == 409 and (
            "reservation" in error.detail or "reserved source revision" in error.detail
        ):
            return error.detail
        raise
    except Exception as error:  # the unseeded repair work
        frames = {frame.name for frame in traceback.extract_tb(error.__traceback__)}
        assert "_rollback_context_v2" in frames, f"{type(error).__name__} before the gate"
        return None
    raise AssertionError("the unseeded repair work cannot have succeeded")


def refused_unchanged(rig: DisputeRig, code: str, call: Callable[[], object]) -> None:
    """`call` is refused with the dispute error `code` and no dispute or escrow row moved."""
    before = rig.state()
    assert refusal(call) == code
    assert rig.state() == before


# --- an admission suspended on a pending dispute ------------------------------------------------

PENDING = "33" * 32  # the dispute id the admission is suspended on
PENDING_EVIDENCE = "44" * 32


def admission_code(call: Callable[[], object]) -> str:
    """Code of the AdmissionError `call` raises; any other outcome fails the test."""
    try:
        call()
    except AdmissionError as error:
        return error.code
    except Exception as error:
        raise AssertionError(f"expected an AdmissionError, got {type(error).__name__}") from error
    raise AssertionError("expected an AdmissionError, nothing was raised")


def admission_snapshot(lab: Lab) -> tuple[tuple[object, ...], ...]:
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


def suspended_on_a_dispute(lab: Lab, **settlement: object) -> str:
    """A joined admission suspended with the pending reference PENDING, whose settlement record is
    the accepted, linked nonfraud MATCH at round 2 that `settlement` overrides."""
    admission = lab.join()
    lab.a.suspend(
        lab.A.HOT.ss58,
        reason="pending",
        now=2,
        pending_dispute=True,
        dispute_id=PENDING,
        evidence_hash=PENDING_EVIDENCE,
    )
    lab.set(admission, strikes=2)
    fields: dict[str, object] = {
        "admission_id": admission,
        "coldkey": lab.A.COLD.ss58,
        "dispute_id": PENDING,
        "evidence_hash": PENDING_EVIDENCE,
        "release_beacon": 2,
    }
    lab.settle(**{**fields, **settlement})
    return admission
