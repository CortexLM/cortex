"""Builders private to the exit_disputes package. Not collected: no test_ prefix.

They drive the same routes a miner and the operator use (escrow lock, dispute, resolution, beacon
push) on the service world of `support.service_world`. What the audit would have written, the
contest record of a verdict and its exclusion, is seeded the way `complete_audit_v2` writes it:
a verdict that is not MATCH also writes `excluded/UNVERIFIED`, a MATCH verdict does not.
"""

from __future__ import annotations

from dataclasses import asdict, dataclass
from typing import Any

from env_pin_fixtures import honest_env
from hypertrain.aggregator.core import FinalityError
from hypertrain.auditor.island_bisect import RefereeEvidence
from hypertrain.challenge.disputes_v2 import (
    Availability,
    Contest,
    DisputeError,
    DisputesV2,
    EventAck,
    Turn,
    signed_message,
)
from hypertrain.challenge.finality_v2 import RoundStatus, require_open, settle
from hypertrain.challenge.store import ChallengeError
from hypertrain.protocol import messages_v2
from hypertrain.protocol.envelope import body_digest
from hypertrain.protocol.envelope_v2 import parse_envelope, seal
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import Finalize, Rollback
from hypertrain.protocol.messages_v2 import (
    BisectV2,
    DisputeV2,
    EscrowLock,
    EscrowRelease,
    PolicyHashes,
    RankWorkResult,
    ResolutionV2,
    RoundOpenV2,
    WorkScreenV2,
)

from .support import World, load_fixture_module

SERVICE = "li_service_fixture"
BOND = 100  # max_referee_cost_units of the fixture policy: the smallest bond open() accepts
AGREE = ["11" * 32] * 3


def service() -> Any:
    """The service fixture module the world was built from (HOT, COLD, AUDITORS, REFEREE...)."""
    return load_fixture_module(SERVICE, "challenge/test_service_network_v2.py")


def disputes(world: World) -> DisputesV2:
    return world.store._services(world.run_id)[2]


def verdict_of(index: int) -> str:
    return sha256_hex(f"exit-disputes-verdict-{index}".encode())


def dispute_id_of(world: World, index: int) -> str:
    miner = service().HOT[index].ss58
    return sha256_hex(canonicalize([world.run_id, verdict_of(index), miner]))


def error_code_of(call: Any) -> str:
    """Code of the DisputeError `call` raises; any other outcome fails the test."""
    try:
        call()
    except DisputeError as error:
        return error.code
    raise AssertionError("expected a DisputeError, nothing was raised")


def lock_id_of(index: int) -> str:
    return f"{0xC0DE00 + index:064x}"


def round_status(w: int, **fields: Any) -> dict[str, Any]:
    """A finality record as `open_round_v2` writes it, plus the fields in `fields`."""
    status = RoundStatus(
        run_id="",
        w=w,
        applied=False,
        disposition="PENDING",
        unresolved_disputes=0,
        unresolved_audits=0,
        rollback_complete=False,
        state_hash="00" * 32,
        resolution_hash=None,
    )
    return {**asdict(status), **fields}


def seed_round(world: World, w: int, **fields: Any) -> None:
    """The finality record of round `w` (the store keeps one per opened round)."""
    with world.store._lock:
        world.store._put_record_v2(
            world.run_id,
            "finality",
            str(w),
            round_status(w, run_id=world.run_id, **fields),
        )


def join(world: World, index: int) -> str:
    """Join miner `index` (APPLIED); returns its admission id."""
    response = world.network.join(index)
    assert response.status_code == 200, response.text
    return str(response.json()["admission_id"])


def funding_units(world: World) -> int:
    """The admission bond a funded miner holds here: the funding floor of the economics policy."""
    return int(world.store._services(world.run_id)[0].policy.S_min_units)


def activate(world: World, index: int) -> str:
    """Join miner `index` and make it ACTIVE and funded by SQL, as the e2e `activated_network`
    does: the 12-trial ladder costs about 35 s per identity and is not what is under test."""
    net, fixture = world.network, service()
    admission_id = join(world, index)
    spec = net.manifest.training.reference_spec
    lock = EscrowLock(
        operation_id=f"{index + 1:064x}",
        owner=fixture.COLD[index].ss58,
        units=funding_units(world),
        origin_ids=[net.origin_ids[index]],
        admission_id=admission_id,
        dispute_id=None,
        kind="LOCK_ADMISSION",
    )
    response = net.client.post(
        net.url + "/escrow/lock", json=net.signed(fixture.COLD[index], "EscrowLock", lock)
    )
    assert response.status_code == 200, response.text
    screen = WorkScreenV2(
        env=honest_env(),
        evidence_kind="work-proof-v1",
        admission_id=admission_id,
        nonce=sha256_hex(b"nonce"),
        policy_hash=net.manifest.network.admission_policy_hash,
        image_digest=spec.image_digest,
        layout=spec.layout,
        challenge_hash=sha256_hex(b"challenge"),
        rank_results=[
            RankWorkResult(
                rank=rank,
                fingerprint=sha256_hex(b"rank"),
                elapsed_ns=1,
                allocated_bytes=1,
                reserved_bytes=1,
                total_bytes=1 << 30,
            )
            for rank in range(spec.layout.n_gpus)
        ],
        artifact_hashes=[sha256_hex(b"artifact")],
    )
    with world.store._tx():
        world.store._db.execute(
            "UPDATE admissions_v2 SET state='ACTIVE',clean_count=12,screen_json=?,"
            "screen_until=1000000000 WHERE admission_id=?",
            (screen.model_dump_json(), admission_id),
        )
    return admission_id


def seed_contest(
    world: World, index: int, *, w: int = 0, mismatch: bool = True, referee: str | None = None
) -> str:
    """The records the audit of round `w` leaves for miner `index`; returns the dispute id.

    mismatch=True is a verdict that is not MATCH (completed MISMATCH lease, contest and
    exclusion); False is a MATCH verdict (completed MATCH lease and contest). The finality record
    of the round is created if it is missing.
    """
    net, fixture = world.network, service()
    hot, cold = fixture.HOT[index], fixture.COLD[index]
    row = world.store._db.execute(
        "SELECT admission_id FROM admissions_v2 WHERE hotkey=?", (hot.ss58,)
    ).fetchone()
    # The lease row is what a service that refuses to contest a verdict that upholds the work
    # reads (audit-integrity-2); complete_audit_v2 always leaves it, with the result of the verdict.
    with world.store._tx():
        world.store._db.execute(
            "INSERT INTO audit_leases_v2(id,run_id,w,hotkey,created,absolute,state,result) "
            "VALUES(?,?,?,?,?,?,'completed',?)",
            (
                sha256_hex(f"exit-disputes-lease-{w}-{index}".encode()),
                world.run_id,
                w,
                hot.ss58,
                net.now,
                net.now + 1,
                "MISMATCH" if mismatch else "MATCH",
            ),
        )
    contest = {
        "verdict_hash": verdict_of(index),
        "challenge_hash": sha256_hex(b"exit-disputes-challenge"),
        "w": w,
        "miner": hot.ss58,
        "auditor": fixture.AUDITORS[0].ss58,
        "referee": referee or fixture.REFEREE.ss58,
        "step_span": net.manifest.training.inner.H // net.manifest.training.inner.J,
        "layer_span": 2,
        "op_span": 2,
        "admission_id": row["admission_id"],
        "coldkey": cold.ss58,
        "evidence_hash": sha256_hex(b"exit-disputes-evidence"),
        "auditor_coldkey": None,
    }
    with world.store._lock:
        world.store._put_record_v2(world.run_id, "contest", verdict_of(index), contest)
        if mismatch:
            world.store._put_record_v2(
                world.run_id, "excluded", f"{w}:{hot.ss58}", {"reason": "UNVERIFIED"}
            )
    seeded = world.store._db.execute(
        "SELECT 1 FROM records_v2 WHERE run_id=? AND kind='finality' AND id=?",
        (world.run_id, str(w)),
    ).fetchone()
    if seeded is None:
        seed_round(world, w)
    return dispute_id_of(world, index)


def fund_contest(world: World, index: int, *, dispute_id: str | None = None) -> EscrowLock:
    """POST /escrow/lock: miner `index`'s coldkey locks the smallest accepted bond."""
    net, fixture = world.network, service()
    cold = fixture.COLD[index]
    origin = world.store._db.execute(
        "SELECT origin FROM escrow_units WHERE owner=? AND bucket='available' AND units>=?",
        (cold.ss58, BOND),
    ).fetchone()[0]
    lock = EscrowLock(
        operation_id=lock_id_of(index),
        owner=cold.ss58,
        units=BOND,
        origin_ids=[origin],
        admission_id=None,
        dispute_id=dispute_id or dispute_id_of(world, index),
        kind="LOCK_CONTEST",
    )
    response = net.client.post(net.url + "/escrow/lock", json=net.signed(cold, "EscrowLock", lock))
    assert response.status_code == 200, response.text
    return lock


def post_dispute(world: World, index: int) -> Any:
    """POST /dispute for miner `index`'s funded contest; returns the response unchecked."""
    net, fixture = world.network, service()
    body = DisputeV2(
        hotkey=fixture.HOT[index].ss58,
        verdict_hash=verdict_of(index),
        action="contest",
        bond_lock=lock_id_of(index),
    )
    return net.client.post(
        net.url + "/dispute", json=net.signed(fixture.HOT[index], "DisputeV2", body)
    )


def open_contest(world: World, index: int, *, w: int = 0, mismatch: bool = True) -> str:
    """Join miner `index`, seed its audit records, fund and open its contest; returns the id."""
    join(world, index)
    dispute_id = seed_contest(world, index, w=w, mismatch=mismatch)
    fund_contest(world, index)
    response = post_dispute(world, index)
    assert response.status_code == 200, response.text
    return dispute_id


def answer(world: World, dispute_id: str, party: Any, hashes: list[str]) -> None:
    """POST /bisect: `party` answers the current turn of the dispute with `hashes`."""
    net = world.network
    turn = disputes(world).get(dispute_id)
    body = BisectV2(
        dispute_id=dispute_id,
        seq=turn.seq,
        level=turn.level,
        ctx=turn.ctx,
        interval=turn.interval,
        N=2,
        hashes=hashes,
        party=party.ss58,
        previous_transcript_hash=turn.transcript_hash,
    )
    response = net.client.post(net.url + "/bisect", json=net.signed(party, "BisectV2", body))
    assert response.status_code == 200, response.text


def pause_at_step(world: World, dispute_id: str) -> Turn:
    """Both parties tell the same truth at step level: the turn pauses off the final op."""
    fixture = service()
    answer(world, dispute_id, fixture.HOT[_index_of(world, dispute_id)], AGREE)
    answer(world, dispute_id, fixture.AUDITORS[0], AGREE)
    turn = disputes(world).get(dispute_id)
    assert (turn.paused, turn.level, turn.resolution) == (True, "step", None)
    return turn


def _index_of(world: World, dispute_id: str) -> int:
    miner = disputes(world).get(dispute_id).contest.miner
    return next(i for i, key in enumerate(service().HOT) if key.ss58 == miner)


def register(world: World, turn: Turn, *, reason: str, loser: str | None) -> ResolutionV2:
    """The unsigned half of a referee resolution: the evidence the service holds for it."""
    evidence = RefereeEvidence(
        dispute_id=turn.dispute_id,
        transcript_hash=turn.transcript_hash,
        reason=reason,  # type: ignore[arg-type]
        loser=loser,
        predecessor_hash="11" * 32,
        inputs_hash="22" * 32,
        output_hash="33" * 32,
        op_spec="exit-disputes-evidence",
    )
    disputes(world).register_evidence(evidence)
    return ResolutionV2(
        dispute_id=turn.dispute_id,
        transcript_hash=turn.transcript_hash,
        reason=reason,  # type: ignore[arg-type]
        loser=loser,
        evidence_hash=evidence.digest(),
    )


def post_resolution(world: World, resolution: ResolutionV2) -> Any:
    """POST /resolution signed by the pinned referee; returns the response unchecked."""
    net = world.network
    return net.client.post(
        net.url + "/resolution",
        json=net.signed(service().REFEREE, "ResolutionV2", resolution, expiry=10**6),
    )


def resolve(world: World, dispute_id: str, *, reason: str, loser: str | None) -> ResolutionV2:
    turn = disputes(world).get(dispute_id)
    resolution = register(world, turn, reason=reason, loser=loser)
    response = post_resolution(world, resolution)
    assert response.status_code == 200, response.text
    return resolution


def watch_poll(client: Any, world: World, party: Keypair, *, timeout: int) -> Any:
    """One long poll of the dispute watch as `party`, returned unchecked.

    The one place that knows how the watch authenticates: a signature over run and party in an
    HTTP header of a GET where the service reads it there (SIG-01), and a signed `WatchRequest`
    in the body of a POST where it reads it there (ADR 0014 part 8, audit-integrity-2)."""
    net = world.network
    if hasattr(messages_v2, "WatchRequest"):
        asked = messages_v2.WatchRequest(party=party.ss58, cursor=0, wait_seconds=timeout)
        return client.post(
            net.url + "/disputes", json=net.signed(party, "WatchRequest", asked, net.now + 20)
        )
    signature = party.sign(f"hypertrain/watch/2|{world.run_id}|{party.ss58}".encode()).hex()
    return client.get(
        net.url + "/disputes",
        params={"party": party.ss58, "timeout": timeout},
        headers={"x-dispute-signature": signature},
    )


def post_release(world: World, index: int, lock: EscrowLock) -> Any:
    """POST /escrow/release signed by the lock's owner, with neutral settlement hashes."""
    net, cold = world.network, service().COLD[index]
    release = EscrowRelease(
        **lock.model_dump(exclude={"kind", "operation_id"}),
        operation_id=sha256_hex(b"release-" + lock.operation_id.encode()),
        lock_id=lock.operation_id,
        finality_hash="00" * 32,
        closed_dispute_root="00" * 32,
    )
    return net.client.post(
        net.url + "/escrow/release", json=net.signed(cold, "EscrowRelease", release)
    )


def unresolved_of(world: World, w: int = 0) -> int:
    """The finality hold of round `w` as the store records it."""
    return int(world.store._record_v2(world.run_id, "finality", str(w))["unresolved_disputes"])


def open_disputes_of(world: World, w: int = 0) -> int:
    """The same number recounted from the dispute table: what the hold must always equal."""
    return int(
        world.store._db.execute(
            "SELECT COUNT(*) FROM disputes_v2 WHERE json_extract(turn,'$.contest.w')=? "
            "AND json_extract(turn,'$.resolution') IS NULL",
            (w,),
        ).fetchone()[0]
    )


def sanction_rows(world: World) -> int:
    """Rows that record a seizure or a sanction of an admission bond (ADR 0010 part 6); a tree
    without that part has neither table, so there is nothing to find."""
    return sum(
        world.count(table)
        for table in ("admission_seizures", "admission_sanctions")
        if world.store._db.execute("SELECT 1 FROM sqlite_master WHERE name=?", (table,)).fetchone()
    )


def miner_state(world: World, index: int) -> tuple[str, int, int]:
    """(state, pending_dispute, clean_count) of miner `index`."""
    row = world.store._db.execute(
        "SELECT state,pending_dispute,clean_count FROM admissions_v2 WHERE hotkey=?",
        (service().HOT[index].ss58,),
    ).fetchone()
    return str(row["state"]), int(row["pending_dispute"]), int(row["clean_count"])


def seed_open_round(world: World, w: int, indices: list[int]) -> None:
    """The signed opening of round `w` with the joined miners `indices` on its roster, as
    `open_round_v2` stores it (the heavy start-state and anchor work behind it is not seeded:
    only what `verified_inputs_v2` reads before it looks at a miner's commit)."""
    net, fixture = world.network, service()
    roster = []
    for slot, index in enumerate(indices):
        record = world.store._services(world.run_id)[1].store.record(fixture.HOT[index].ss58)
        roster.append(
            {
                "hotkey": fixture.HOT[index].ss58,
                "slot": slot,
                "q_i": "0000803f",
                "admission_id": record.admission_id,
                "coldkey_group": record.coldkey,
                "state": record.state,
                "eligible_weight": 4194304,
            }
        )
    zero = "00" * 32
    opening = RoundOpenV2(
        w=w,
        prev_final_hash=zero,
        theta_hash=zero,
        outer_state_hash=zero,
        center_hash=zero,
        roster_hash=sha256_hex(canonicalize(roster)),
        honeypot_commit=zero,
        d_open=net.now + 1,
        d_assign=net.now + 2,
        d_commit=net.now + 3,
        d_audit=net.now + 4,
        d_upload=net.now + 5,
        d_final=net.now + 7,
        contract_version=2,
        policy_hashes=PolicyHashes.model_validate(
            {name: getattr(net.manifest.network, name) for name in PolicyHashes.model_fields}
        ),
        registry_epoch=0,
        start_state_index_hash=zero,
        audit_mode="anchored-full",
        roster=roster,
    )
    with world.store._lock:
        world.store._put_record_v2(
            world.run_id, "round", str(w), net.signed(fixture.COORD, "RoundOpenV2", opening)
        )


def acknowledge_turn(world: World, dispute_id: str) -> None:
    """The expected party signs the receipt of the latest turn event (what its watcher does)."""
    fixture, d = service(), disputes(world)
    party = d.get(dispute_id).expected_party
    key = next(k for k in (*fixture.HOT, *fixture.AUDITORS) if k.ss58 == party)
    event = d.events(party, 0)[-1]
    ack = EventAck(
        run_id=d.run_id,
        cursor=event.cursor,
        event_hash=event.digest(),
        party=party,
        received_beacon=world.network.now,
        sig="",
    )
    ack = ack.model_copy(update={"sig": key.sign(signed_message(ack)).hex()})
    with world.store._tx():
        d.acknowledge(ack, now=world.network.now)


def timeout_request(world: World, dispute_id: str) -> dict[str, Any]:
    """The body of the operator's timeout route: two permitted observers sign the current head."""
    fixture, d = service(), disputes(world)
    turn = d.get(dispute_id)
    proofs = []
    for key in (fixture.COORD, fixture.REFEREE):
        proof = Availability(
            run_id=d.run_id,
            dispute_id=dispute_id,
            seq=turn.seq,
            transcript_hash=turn.transcript_hash,
            observed_beacon=world.network.now,
            service_healthy=True,
            beacon_healthy=True,
            reference_healthy=True,
            signer=key.ss58,
            sig="",
        )
        proofs.append(
            proof.model_copy(update={"sig": key.sign(signed_message(proof)).hex()}).model_dump(
                mode="json"
            )
        )
    return {"witnesses": proofs}


def hold_blocks(world: World, *, settled: bool) -> bool:
    """Whether the dispute hold of round 0 still blocks what the finality predicates guard:
    the opening of round 2 once round 0 is settled, else the settlement of round 0 itself."""
    store = world.store
    try:
        if settled:
            require_open(world.run_id, 2, store._history_v2(world.run_id))
        else:
            status = RoundStatus(**store._record_v2(world.run_id, "finality", "0"))
            final = Finalize(
                w=0, final_theta_hash_w1=status.state_hash, included=[], entitlements_root="00" * 32
            )
            settle(status, final, "ab" * 32)
    except FinalityError:
        return True
    return False


def synthetic_miners(world: World) -> dict[str, Contest]:
    """Give the hosted engine a test-owned lookup for contests of miners that are not on the
    subnet, so 32 of them can fill its queue. Returns the table `queue_contest` adds to."""
    d, fixture = disputes(world), service()
    table: dict[str, Contest] = {}
    hosted = d.contest_of
    d.contest_of = lambda verdict: table[verdict] if verdict in table else hosted(verdict)
    d.owner_of = lambda _hotkey: fixture.COLD[0].ss58
    return table


def queue_contest(
    world: World, table: dict[str, Contest], index: int, *, w: int = 0
) -> tuple[bytes, str]:
    """A funded contest of synthetic miner `index`: the lock is real, the signed DisputeV2 is
    returned unopened with the dispute id."""
    d, fixture = disputes(world), service()
    miner = Keypair(bytes([150 + index]) * 32)
    inner = world.network.manifest.training.inner
    contest = Contest(
        verdict_hash=sha256_hex(f"queue-verdict-{index}".encode()),
        challenge_hash=sha256_hex(b"exit-disputes-challenge"),
        w=w,
        miner=miner.ss58,
        auditor=fixture.AUDITORS[0].ss58,
        referee=fixture.REFEREE.ss58,
        step_span=inner.H // inner.J,
        layer_span=2,
        op_span=2,
    )
    table[contest.verdict_hash] = contest
    dispute_id = sha256_hex(canonicalize([world.run_id, contest.verdict_hash, miner.ss58]))
    origin = world.store._db.execute(
        "SELECT origin FROM escrow_units WHERE owner=? AND bucket='available' AND units>=?",
        (fixture.COLD[0].ss58, BOND),
    ).fetchone()[0]
    lock = EscrowLock(
        operation_id=f"{0xA000 + index:064x}",
        owner=fixture.COLD[0].ss58,
        units=BOND,
        origin_ids=[origin],
        admission_id=None,
        dispute_id=dispute_id,
        kind="LOCK_CONTEST",
    )
    d.escrow.lock(lock, signer=fixture.COLD[0].ss58)
    body = DisputeV2(
        hotkey=miner.ss58,
        verdict_hash=contest.verdict_hash,
        action="contest",
        bond_lock=lock.operation_id,
    )
    return canonicalize(seal(miner, "DisputeV2", world.run_id, body, 10**6)), dispute_id


@dataclass
class RollbackRig:
    """A run in which `rollback_v2` can reach its repair reservation without a real repair.

    Rounds 0 and 1 are applied with a contest of miner 0 on round 0 that the referee closed as
    FRAUD against it (the cause a rollback needs). The repair work itself is not seeded: past the
    reservation gate the call fails inside `_rollback_context_v2` with `KeyError('tape_hash')`,
    which is what `attempt` reports as "got past the gate".
    """

    world: World
    cause: str

    def request(self, variant: int = 0) -> bytes:
        """The coordinator-signed Rollback of round 0; each `variant` is another operation."""
        fixture = service()
        model = Rollback(
            w=0,
            excluded=[fixture.HOT[0].ss58],
            old_theta_hash_w2="bb" * 32,
            new_theta_hash_w2=f"{variant:064x}",
            new_outer_state_hash="00" * 32,
            recomputed=["agg_w", "step_w", "agg_w1", "step_w1"],
            cause_hashes=[self.cause],
        )
        return canonicalize(self.world.network.signed(fixture.COORD, "Rollback", model))

    def operation(self, raw: bytes) -> str:
        return body_digest(parse_envelope(raw).body)

    def leave_running_reservation(self, raw: bytes) -> None:
        """What a first attempt that crashed after its reservation committed leaves behind: the
        signed request accepted, a RUNNING reservation bound to the revision it started from."""
        store, run = self.world.store, self.world.run_id
        with store._tx():
            store._authenticated_v2(run, raw, "Rollback", service().COORD.ss58)
            now = store._now(store._db)
            store._put_record_v2(
                run,
                "repair-reservation",
                self.operation(raw),
                {
                    "state": "RUNNING",
                    "created": now,
                    "absolute": now + 200,
                    "max_attempts": 2,
                    "source_revision": store._rollback_snapshot_v2(run),
                    "attempts": 1,
                    "steps": 8,
                },
            )

    def attempt(self, raw: bytes) -> str | None:
        """The detail of the 409 by which the reservation gate refuses `raw`, or None when the
        call got past the gate (it then stops in the unseeded repair work)."""
        try:
            self.world.store.rollback_v2(self.world.run_id, raw)
        except ChallengeError as error:
            if error.status == 409 and (
                "reservation" in error.detail or "reserved source revision" in error.detail
            ):
                return error.detail
            raise
        except KeyError as error:
            assert error.args == ("tape_hash",), error
            return None
        raise AssertionError("the unseeded repair work cannot have succeeded")


def rollback_rig(world: World) -> RollbackRig:
    """Seed rounds 0 and 1 as applied and close miner 0's contest of round 0 as FRAUD."""
    fixture, store, run = service(), world.store, world.run_id
    seed_round(world, 0, applied=True)
    seed_round(world, 1, applied=True)
    dispute_id = open_contest(world, 0)
    resolution = resolve(world, dispute_id, reason="FRAUD", loser=fixture.HOT[0].ss58)
    seed_open_round(world, 0, [0])
    seed_open_round(world, 1, [0])
    with store._lock:
        store._put_record_v2(run, "applied", "1", {"theta_hash": "bb" * 32})
        store._put_record_v2(
            run, "applied", "0", {"prev_state": "dd" * 32, "predecessor_tape_hash": "ee" * 32}
        )
        for w in (0, 1):
            store._put_record_v2(run, "tape-inputs", str(w), {"inputs": [1, 2, 3, 4, 5]})
    return RollbackRig(world, resolution.digest())
