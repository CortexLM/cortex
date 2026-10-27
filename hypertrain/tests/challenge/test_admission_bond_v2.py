"""The admission bond: release to the owner when quiet, seizure by the service (ADR 0010)."""

from __future__ import annotations

import importlib.util
import json
import sys
from dataclasses import replace
from pathlib import Path

import pytest
from pydantic import ValidationError

from hypertrain.auditor.island_bisect import RefereeEvidence
from hypertrain.challenge.admission import Admission, SeizureReason
from hypertrain.challenge.admission_store import AdmissionError
from hypertrain.challenge.disputes_v2 import Contest, DisputesV2
from hypertrain.ledger.escrow_v2 import EscrowError
from hypertrain.protocol.envelope_v2 import seal
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages_v2 import (
    DisputePolicyV2,
    DisputeV2,
    EscrowLock,
    EscrowRelease,
    EscrowTransfer,
    ResolutionV2,
    RunManifestV2,
)

_spec = importlib.util.spec_from_file_location(
    "bond_admission_fixture", Path(__file__).parent / "test_admission_v2.py"
)
assert _spec is not None and _spec.loader is not None
T = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = T
_spec.loader.exec_module(T)
HOT, COLD, OTHER, COORD, H = T.HOT, T.COLD, T.OTHER, T.COORD, T.H
REFEREE = Keypair(b"\x71" * 32)
NEW_HOT = Keypair(b"\x72" * 32)


class World:
    """One Admission and one DisputesV2 on the same ledger, with an admission funded twice."""

    def __init__(self, tmp_path: Path) -> None:
        self.policy = DisputePolicyV2(
            referees=[REFEREE.ss58],
            max_open=32,
            max_transcript_entries=256,
            max_entry_bytes=65536,
            fanout=2,
            max_referee_reassignments=1,
            max_referee_cost_units=100,
        )
        s = T.setup()
        body = s.manifest.body()
        body["training"]["inner"].update(H=2, J=1)
        body["network"]["dispute_policy_hash"] = self.policy.digest()
        self.s = replace(s, manifest=RunManifestV2.model_validate(body))
        self.a: Admission = T.service(self.s, tmp_path)
        self.origin, self.other_origin = T.fund(self.a.escrow)
        self.run_id = self.a.escrow.run_id
        self.admission = T.apply(self.a, self.s)
        self.contest = Contest(
            verdict_hash="11" * 32,
            challenge_hash="22" * 32,
            w=0,
            miner=HOT.ss58,
            auditor=T.fixtures.AUDITOR.ss58,
            referee=REFEREE.ss58,
            step_span=2,
            layer_span=2,
            op_span=2,
            admission_id=self.admission,
            coldkey=COLD.ss58,
            evidence_hash="55" * 32,
        )
        self.d = DisputesV2(
            self.a.escrow,
            self.policy,
            COORD,
            contest_of=lambda _: self.contest,
            owner_of=lambda hotkey: COLD.ss58 if hotkey == HOT.ss58 else OTHER.ss58,
            span=lambda c, level, ctx: 2,
        )
        self.dispute_id = sha256_hex(
            canonicalize([self.run_id, self.contest.verdict_hash, self.contest.miner])
        )

        def settlement(reference: str):
            """What the store answers: a dispute row, else nothing accepted."""
            row = self.a.store.db.execute(
                "SELECT 1 FROM disputes_v2 WHERE id=? OR json_extract(turn,'$.lock_id')=?",
                (reference, reference),
            ).fetchone()
            if row is None:
                raise EscrowError("UNACCEPTED_SETTLEMENT")
            return self.d.settlement(reference)

        self.a.escrow.settlement = settlement

    def bond(self, units: int = 1000, operation: str = "22" * 32, owner=COLD) -> EscrowLock:
        lock = EscrowLock(
            operation_id=operation,
            owner=owner.ss58,
            units=units,
            origin_ids=[self.origin if owner is COLD else self.other_origin],
            admission_id=self.admission,
            dispute_id=None,
            kind="LOCK_ADMISSION",
        )
        self.a.lock(canonicalize(seal(owner, "EscrowLock", self.run_id, lock, 500)), now=1)
        return lock

    def convict(self, loser: str = HOT.ss58, *, bind: bool = True) -> None:
        """The miner contests its verdict and the referee rules against `loser`."""
        a = self.a
        if bind:  # what the store's `dispute` action does when the contest opens
            a.suspend(
                HOT.ss58,
                reason="PENDING_DISPUTE",
                now=2,
                pending_dispute=True,
                dispute_id=self.dispute_id,
                evidence_hash=self.contest.evidence_hash,
            )
        contest_lock = EscrowLock(
            operation_id="44" * 32,
            owner=COLD.ss58,
            units=100,
            origin_ids=[self.origin],
            admission_id=None,
            dispute_id=self.dispute_id,
            kind="LOCK_CONTEST",
        )
        a.escrow.lock(contest_lock, signer=COLD.ss58)
        request = DisputeV2(
            hotkey=HOT.ss58,
            verdict_hash=self.contest.verdict_hash,
            action="contest",
            bond_lock=contest_lock.operation_id,
        )
        turn = self.d.open(
            canonicalize(seal(HOT, "DisputeV2", self.run_id, request, 100000)), now=2
        )
        evidence = RefereeEvidence(
            dispute_id=turn.dispute_id,
            transcript_hash=turn.transcript_hash,
            reason="FRAUD",
            loser=loser,
            predecessor_hash="11" * 32,
            inputs_hash="22" * 32,
            output_hash="33" * 32,
            op_spec="verified.actual.op",
        )
        self.d.register_evidence(evidence)
        resolution = ResolutionV2(
            dispute_id=turn.dispute_id,
            transcript_hash=turn.transcript_hash,
            reason="FRAUD",
            loser=loser,
            evidence_hash=evidence.digest(),
        )
        self.d.resolution(
            canonicalize(seal(REFEREE, "ResolutionV2", self.run_id, resolution, 100000)), now=3
        )
        self.d.settle_lock(turn.dispute_id, now=3)

    def release_op(self, lock_id: str, units: int = 1000, *, tamper: bool = False) -> EscrowRelease:
        (entry,) = [
            e for e in self.a.bond(self.admission, now=10_000)["locks"] if e["lock_id"] == lock_id
        ]  # type: ignore[index]
        return EscrowRelease(
            operation_id="66" * 32,
            owner=COLD.ss58,
            units=units,
            origin_ids=[self.origin],
            admission_id=self.admission,
            dispute_id=None,
            lock_id=lock_id,
            finality_hash=H if tamper else entry["finality_hash"],
            closed_dispute_root=entry["closed_dispute_root"],
        )


@pytest.fixture
def world(tmp_path: Path) -> World:
    return World(tmp_path)


def balances(w: World):
    return w.a.escrow.balances()


def events(w: World) -> int:
    return w.a.store.db.execute("SELECT COUNT(*) FROM escrow_events").fetchone()[0]


def rostered_round(w: World) -> None:
    """A round that lists the admission, before the store wrote any finality record for it."""
    roster = [{"admission_id": w.admission, "hotkey": HOT.ss58}]
    w.a.store.db.execute(
        "INSERT OR REPLACE INTO records_v2 VALUES(?,?,?,?)",
        (w.run_id, "round", "0", json.dumps({"body": {"roster": roster}})),
    )


def unclosed_round(w: World, *, closed: bool) -> None:
    """A round that lists the admission, with the finality record of the store."""
    rostered_round(w)
    status = {
        "run_id": w.run_id,
        "w": 0,
        "applied": True,
        "disposition": "SETTLED" if closed else "PENDING",
        "unresolved_disputes": 0,
        "unresolved_audits": 0 if closed else 1,
        "rollback_complete": False,
        "state_hash": H,
        "resolution_hash": None,
    }
    w.a.store.db.execute(
        "INSERT OR REPLACE INTO records_v2 VALUES(?,?,?,?)",
        (w.run_id, "finality", "0", json.dumps(status)),
    )


def reward(w: World, *, vested: bool, mature_at: int = 50, hotkey: str = HOT.ss58) -> None:
    """A reward origin of one hotkey of the admission, still pending or already matured."""
    db = w.a.store.db
    db.execute(
        "INSERT OR REPLACE INTO escrow_origins VALUES(?,?,?,?,?)",
        ("aa" * 32, 500, mature_at, 0, hotkey),
    )
    db.execute("DELETE FROM escrow_units WHERE origin=?", ("aa" * 32,))
    db.execute(
        "INSERT INTO escrow_units VALUES(?,?,?,?,?)",
        ("aa" * 32, COLD.ss58, "available" if vested else "reward_pending", "", 500),
    )


def test_release_bond_gives_the_bond_back_once_the_admission_is_quiet(world: World) -> None:
    # Given: a funded application that never joined a round
    world.bond()
    assert balances(world).admission_locked == 1000
    # When
    receipts = world.a.release_bond(world.admission, now=5)
    # Then: the units are the owner's again, once; a second call has nothing left to release
    assert len(receipts) == 1
    assert balances(world).admission_locked == 0 and balances(world).conserved()
    assert world.a.escrow.balances(COLD.ss58).available == 20000
    before = events(world)
    assert world.a.release_bond(world.admission, now=6) == []
    assert events(world) == before
    assert world.a.bond(world.admission, now=6) == {"locks": []}


@pytest.mark.parametrize(
    "hold",
    [
        "pending_dispute",
        "unresolved_pending_row",
        "outbox",
        "open_round",
        "round_without_finality",
        "unvested_reward",
        "banned",
    ],
)
def test_release_bond_is_refused_while_something_the_bond_covers_is_open(
    world: World, hold: str
) -> None:
    # Given: a funded application with one open obligation
    world.bond()
    db = world.a.store.db
    match hold:
        case "pending_dispute":
            db.execute("UPDATE admissions_v2 SET pending_dispute=1")
        case "unresolved_pending_row":
            db.execute(
                "INSERT INTO admission_pending VALUES(?,?,?,?,0)",
                (H, world.admission, H, COLD.ss58),
            )
        case "outbox":
            queued = EscrowLock(
                operation_id="77" * 32,
                owner=COLD.ss58,
                units=10,
                origin_ids=[world.origin],
                admission_id=world.admission,
                dispute_id=None,
                kind="LOCK_ADMISSION",
            )
            world.a.store.enqueue_lock(queued, COLD.ss58)
        case "open_round":
            unclosed_round(world, closed=False)
        case "round_without_finality":
            rostered_round(world)
        case "unvested_reward":
            reward(world, vested=False)
        case "banned":
            db.execute("UPDATE admissions_v2 SET state='BANNED'")
    # When / Then: refused, nothing moves
    before = events(world)
    with pytest.raises(AdmissionError, match="BOND_NOT_RELEASABLE"):
        world.a.release_bond(world.admission, now=1000)
    assert events(world) == before and balances(world).admission_locked == 1000
    # and it is released as soon as the obligation is closed
    match hold:
        case "pending_dispute":
            db.execute("UPDATE admissions_v2 SET pending_dispute=0")
        case "unresolved_pending_row":
            db.execute("UPDATE admission_pending SET resolved=1")
        case "outbox":
            world.a.store.reconcile_lock(queued.operation_id)
        case "open_round" | "round_without_finality":
            unclosed_round(world, closed=True)
        case "unvested_reward":
            reward(world, vested=True)
        case "banned":
            db.execute("UPDATE admissions_v2 SET state='APPLIED'")
    world.a.release_bond(world.admission, now=1000)
    assert balances(world).admission_locked == 0 and balances(world).conserved()


def test_release_bond_waits_for_the_last_reward_to_vest_and_for_a_live_screen_to_end(
    world: World,
) -> None:
    # Given: a reward that vests at round 50, and a live miner whose screen runs to 200
    world.bond()
    reward(world, vested=True, mature_at=50)
    with pytest.raises(AdmissionError, match="BOND_NOT_RELEASABLE"):
        world.a.release_bond(world.admission, now=49)
    world.a.store.db.execute(
        "UPDATE admissions_v2 SET state='ACTIVE',screen_until=200 WHERE admission_id=?",
        (world.admission,),
    )
    # a live miner keeps its bond: it is what makes it eligible
    with pytest.raises(AdmissionError, match="BOND_NOT_RELEASABLE"):
        world.a.release_bond(world.admission, now=200)
    # When: the screen has lapsed
    world.a.release_bond(world.admission, now=201)
    assert balances(world).admission_locked == 0


@pytest.mark.parametrize(
    "vested,now,released",
    [(False, 1000, False), (True, 49, False), (True, 50, True)],
    ids=["unvested", "vested_not_mature", "mature"],
)
def test_a_rotated_admission_keeps_its_bond_for_the_rewards_of_its_old_hotkey(
    world: World, vested: bool, now: int, released: bool
) -> None:
    # Given: the miner rotated to a new hotkey; a reward (maturing at round 50) was earned by
    # the old one
    world.bond()
    world.a.rotate(T.rotation(world.a, HOT, NEW_HOT, "33" * 32), now=2)
    assert world.a.store.record(NEW_HOT.ss58).admission_id == world.admission
    reward(world, vested=vested, mature_at=50, hotkey=HOT.ss58)
    # When / Then: the bond waits for what the old hotkey earned, as for the current one
    if released:
        world.a.release_bond(world.admission, now=now)
        assert balances(world).admission_locked == 0
    else:
        with pytest.raises(AdmissionError, match="BOND_NOT_RELEASABLE"):
            world.a.release_bond(world.admission, now=now)
        assert balances(world).admission_locked == 1000


def test_the_status_of_a_bond_reads_the_rounds_once_however_many_locks_it_holds(
    world: World,
) -> None:
    # Given: an admission with three locks, on a route anyone may poll
    world.bond(300)
    world.bond(300, operation="23" * 32)
    world.bond(300, operation="24" * 32)
    scans: list[str] = []
    world.a.store.db.set_trace_callback(
        lambda sql: scans.append(sql) if "json_each" in sql else None
    )
    # When
    status = world.a.bond(world.admission, now=10)
    # Then: all three are reported from a single scan of the run's rounds
    assert [entry["units"] for entry in status["locks"]] == [300, 300, 300]  # type: ignore[union-attr,index]
    assert len(scans) == 1


def test_a_probation_miner_with_four_clean_trials_is_live_and_keeps_its_bond(world: World) -> None:
    # Given: PROBATION is live from the fourth clean trial (it may then be eligible), not before
    world.bond()
    db = world.a.store.db
    db.execute("UPDATE admissions_v2 SET state='PROBATION',clean_count=4,screen_until=200")
    with pytest.raises(AdmissionError, match="BOND_NOT_RELEASABLE"):
        world.a.release_bond(world.admission, now=200)
    db.execute("UPDATE admissions_v2 SET clean_count=3")
    world.a.release_bond(world.admission, now=5)
    assert balances(world).admission_locked == 0


def test_owner_signed_release_must_carry_the_hashes_the_service_derived(world: World) -> None:
    # Given: a quiet application; the owner reads `bond` and signs the existing EscrowRelease
    lock = world.bond()
    # When / Then: the hashes of another state are refused, the derived ones accepted
    with pytest.raises(EscrowError, match="DISPUTED_OR_UNVESTED"):
        world.a.escrow.release(
            world.release_op(lock.operation_id, tamper=True), signer=COLD.ss58, now_beacon=5
        )
    world.a.escrow.release(world.release_op(lock.operation_id), signer=COLD.ss58, now_beacon=5)
    assert balances(world).admission_locked == 0 and balances(world).conserved()


def test_a_lock_cannot_borrow_the_settlement_of_another_id(world: World) -> None:
    # Given: the store's callback answers MATCH for any reward-origin-like id, and the
    # admission still has an open round. The owner names its lock after such an id.
    reference = "99" * 32
    world.a.store.db.execute(
        "INSERT INTO records_v2 VALUES(?,?,?,?)",
        (
            world.run_id,
            "settlement",
            reference,
            json.dumps(
                {
                    "finality_hash": H,
                    "closed_dispute_root": H,
                    "release_beacon": 1,
                    "unresolved": False,
                    "outcome": "MATCH",
                }
            ),
        ),
    )
    from hypertrain.ledger.escrow_v2 import Settlement

    world.a.escrow.settlement = lambda ref: Settlement.model_validate_json(
        world.a.store.db.execute(
            "SELECT data FROM records_v2 WHERE kind='settlement' AND id=?", (ref,)
        ).fetchone()[0]
    )
    lock = world.bond(operation=reference)
    unclosed_round(world, closed=False)
    op = EscrowRelease(
        operation_id="66" * 32,
        owner=COLD.ss58,
        units=1000,
        origin_ids=[world.origin],
        admission_id=world.admission,
        dispute_id=None,
        lock_id=lock.operation_id,
        finality_hash=H,
        closed_dispute_root=H,
    )
    # When / Then: the generic record would release it; the admission's own state holds it
    with pytest.raises(EscrowError, match="DISPUTED_OR_UNVESTED"):
        world.a.escrow.release(op, signer=COLD.ss58, now_beacon=100)
    assert balances(world).admission_locked == 1000


def test_seize_bond_suspends_strikes_and_burns_the_bond_of_a_no_show(world: World) -> None:
    # Given: two locks and unvested rewards of the same coldkey, plus a second admission
    world.bond(1000)
    world.bond(500, operation="23" * 32)
    reward(world, vested=False)
    other = T.request(world.s, "99" * 32, Keypair(b"\x41" * 32), OTHER)
    bystander = world.a.join(canonicalize(other.body()), now=1, ip_prefix="elsewhere").admission_id
    held = EscrowLock(
        operation_id="88" * 32,
        owner=OTHER.ss58,
        units=300,
        origin_ids=[world.other_origin],
        admission_id=bystander,
        dispute_id=None,
        kind="LOCK_ADMISSION",
    )
    world.a.lock(canonicalize(seal(OTHER, "EscrowLock", world.run_id, held, 500)), now=1)
    # and a sibling admission of the same coldkey (allowed once the first one is ACTIVE)
    world.a.store.db.execute(
        "UPDATE admissions_v2 SET state='ACTIVE' WHERE admission_id=?", (world.admission,)
    )
    sibling_request = T.request(world.s, "98" * 32, Keypair(b"\x42" * 32), COLD)
    sibling = world.a.join(
        canonicalize(sibling_request.body()), now=1, ip_prefix="sibling"
    ).admission_id
    kept = EscrowLock(
        operation_id="89" * 32,
        owner=COLD.ss58,
        units=300,
        origin_ids=[world.origin],
        admission_id=sibling,
        dispute_id=None,
        kind="LOCK_ADMISSION",
    )
    world.a.lock(canonicalize(seal(COLD, "EscrowLock", world.run_id, kept, 500)), now=1)
    # When
    burned = world.a.seize_bond(world.admission, reason="NO_SHOW", evidence_hash=H, now=5)
    # Then: the whole bond burns and the admission is suspended with one strike; the rewards,
    # the other admissions and the ledger are left alone
    assert burned == 1500
    b = balances(world)
    assert (b.burned, b.reward_pending, b.admission_locked) == (1500, 500, 600) and b.conserved()
    record = world.a.store.by_id(world.admission)
    assert (record.state, record.reason, record.strikes) == ("SUSPENDED", "NO_SHOW", 1)
    assert not record.pending_dispute
    assert world.a.store.by_id(bystander).state == "APPLIED"
    assert world.a.store.by_id(sibling).strikes == 0
    assert world.a.bond(sibling, now=1000)["locks"][0]["units"] == 300  # type: ignore[index]
    rows = world.a.store.db.execute("SELECT reason FROM admission_seizures").fetchall()
    assert [r[0] for r in rows] == ["NO_SHOW", "NO_SHOW"]
    world.a.escrow.verify()
    # a second call finds nothing left, writes nothing and adds no strike
    before = events(world)
    assert world.a.seize_bond(world.admission, reason="NO_SHOW", evidence_hash=H, now=6) == 0
    assert events(world) == before
    assert world.a.store.by_id(world.admission).strikes == 1
    # a seized lock is never releasable, and the miner may fund again
    assert world.a.bond(world.admission, now=7) == {"locks": []}
    world.bond(200, operation="24" * 32)
    reward(world, vested=True, mature_at=1)  # the unvested reward the burn spared has vested
    assert world.a.bond(world.admission, now=1000)["locks"][0]["releasable"] is True  # type: ignore[index]
    release = EscrowRelease(
        operation_id="67" * 32,
        owner=COLD.ss58,
        units=1,
        origin_ids=[world.origin],
        admission_id=world.admission,
        dispute_id=None,
        lock_id="22" * 32,
        finality_hash=H,
        closed_dispute_root=H,
    )
    with pytest.raises(EscrowError, match="DISPUTED_OR_UNVESTED"):
        world.a.escrow.release(release, signer=COLD.ss58, now_beacon=100)


def test_seize_bond_for_a_confirmed_audit_mismatch_suspends_strikes_and_burns_the_bond(
    world: World,
) -> None:
    # Given: a funded admission with an unvested reward, and a round-stage mismatch that the
    # caller has confirmed as the miner's
    world.bond(1000)
    reward(world, vested=False)
    # When
    burned = world.a.seize_bond(world.admission, reason="AUDIT_MISMATCH", evidence_hash=H, now=5)
    # Then: suspended, one strike, the bond burned and nothing else; no dispute is invented
    record = world.a.store.by_id(world.admission)
    assert (record.state, record.reason, record.strikes) == ("SUSPENDED", "AUDIT_MISMATCH", 1)
    assert not record.pending_dispute
    b = balances(world)
    assert burned == 1000 and (b.burned, b.reward_pending, b.admission_locked) == (1000, 500, 0)
    assert b.conserved()
    # the coldkey stays bound as in probation
    again = T.request(world.s, "97" * 32, Keypair(b"\x43" * 32), COLD)
    with pytest.raises(AdmissionError, match="COLDKEY_PROBATION_EXISTS"):
        world.a.join(canonicalize(again.body()), now=6, ip_prefix="elsewhere")
    # a retry of the same confirmation is one strike and no new event
    before = events(world)
    assert world.a.seize_bond(world.admission, reason="AUDIT_MISMATCH", evidence_hash=H, now=7) == 0
    assert world.a.store.by_id(world.admission).strikes == 1 and events(world) == before
    # a different confirmed mismatch is another strike
    world.a.seize_bond(world.admission, reason="AUDIT_MISMATCH", evidence_hash="77" * 32, now=8)
    assert world.a.store.by_id(world.admission).strikes == 2


def test_audit_mismatch_of_an_unfunded_admission_still_suspends_and_strikes(world: World) -> None:
    assert world.a.seize_bond(world.admission, reason="AUDIT_MISMATCH", evidence_hash=H, now=5) == 0
    record = world.a.store.by_id(world.admission)
    assert (record.state, record.strikes) == ("SUSPENDED", 1)


def test_audit_mismatch_does_not_lift_a_ban(world: World) -> None:
    world.bond()
    world.a.seize_bond(world.admission, reason="TRAINING_FRAUD", evidence_hash=H, now=5)
    world.a.seize_bond(world.admission, reason="AUDIT_MISMATCH", evidence_hash="77" * 32, now=6)
    record = world.a.store.by_id(world.admission)
    assert (record.state, record.strikes) == ("BANNED", 0)


@pytest.mark.parametrize(
    ("prior", "expected"),
    [
        (None, ("SUSPENDED", "NO_SHOW", 1)),
        ("TRAINING_FRAUD", ("BANNED", "FINALIZED_TRAINING_FRAUD", 0)),
    ],
    ids=["unfunded", "banned"],
)
def test_a_no_show_suspends_an_unfunded_admission_and_does_not_lift_a_ban(
    world: World, prior: SeizureReason | None, expected: tuple[str, str, int]
) -> None:
    # Given: an ACTIVE admission with no bond to burn, or one already banned for fraud
    world.a.store.db.execute(
        "UPDATE admissions_v2 SET state='ACTIVE' WHERE admission_id=?", (world.admission,)
    )
    if prior:
        world.bond()
        world.a.seize_bond(world.admission, reason=prior, evidence_hash=H, now=5)
    # When
    world.a.seize_bond(world.admission, reason="NO_SHOW", evidence_hash="77" * 32, now=6)
    # Then: the state, its reason and the strikes follow the rule, and the coldkey stays bound
    record = world.a.store.by_id(world.admission)
    assert (record.state, record.reason, record.strikes) == expected
    again = T.request(world.s, "97" * 32, Keypair(b"\x43" * 32), COLD)
    with pytest.raises(AdmissionError, match="COLDKEY_PROBATION_EXISTS"):
        world.a.join(canonicalize(again.body()), now=7, ip_prefix="elsewhere")


def test_seize_bond_for_fraud_also_burns_unvested_rewards_and_bans(world: World) -> None:
    world.bond(1000)
    reward(world, vested=False)
    # When
    assert (
        world.a.seize_bond(world.admission, reason="TRAINING_FRAUD", evidence_hash=H, now=5) == 1000
    )
    # Then
    b = balances(world)
    assert (b.burned, b.reward_pending, b.admission_locked) == (1500, 0, 0) and b.conserved()
    assert world.a.store.by_id(world.admission).state == "BANNED"
    transitions = world.a.store.db.execute(
        "SELECT COUNT(*) FROM admission_transitions WHERE state='BANNED'"
    ).fetchone()[0]
    # idempotent: no second ban row, no second burn; and nothing new can be locked
    world.a.seize_bond(world.admission, reason="TRAINING_FRAUD", evidence_hash=H, now=6)
    assert (
        world.a.store.db.execute(
            "SELECT COUNT(*) FROM admission_transitions WHERE state='BANNED'"
        ).fetchone()[0]
        == transitions
        == 1
    )
    with pytest.raises(AdmissionError, match="LOCK_BANNED"):
        world.bond(10, operation="25" * 32)


def test_seize_bond_refuses_an_unknown_reason_and_malformed_evidence(world: World) -> None:
    world.bond()
    with pytest.raises(AdmissionError, match="SEIZURE_REASON"):
        world.a.seize_bond(world.admission, reason="GREED", evidence_hash=H, now=5)  # type: ignore[arg-type]
    with pytest.raises(ValidationError):
        world.a.seize_bond(world.admission, reason="NO_SHOW", evidence_hash="xyz", now=5)
    assert balances(world).admission_locked == 1000


def test_seizure_cannot_be_blocked_by_registering_its_operation_id(world: World) -> None:
    # Given: the owner registers, as a self-transfer, the id a predictable derivation would use
    lock = world.bond()
    squat = EscrowTransfer(
        operation_id=sha256_hex(canonicalize([lock.operation_id, "settle"])),
        owner=COLD.ss58,
        units=1,
        origin_ids=[world.origin],
        admission_id=None,
        dispute_id=None,
        recipient=COLD.ss58,
    )
    world.a.escrow.transfer(squat, signer=COLD.ss58)
    # When / Then: the seizure still goes through
    assert world.a.seize_bond(world.admission, reason="NO_SHOW", evidence_hash=H, now=5) == 1000


def test_a_referee_verdict_against_the_miner_seizes_the_bond_and_bans(world: World) -> None:
    # Given: a funded miner that contests its verdict and loses
    world.bond(1000)
    world.bond(500, operation="23" * 32)
    reward(world, vested=False)
    world.convict(loser=HOT.ss58)
    assert balances(world).burned == 100  # the contest lock only: settle_lock's own sanction
    # the verdict was accepted at round 3: it convicts no earlier
    with pytest.raises(AdmissionError, match="UNPROVEN_TRAINING_FRAUD"):
        world.a.settle_fraud(world.dispute_id, now=2)
    assert balances(world).admission_locked == 1500
    # When
    assert world.a.settle_fraud(world.dispute_id, now=3) is True
    # Then: both locks and the unvested reward burn, the admission is banned, the dispute closed
    b = balances(world)
    assert (b.burned, b.admission_locked, b.reward_pending) == (100 + 1500 + 500, 0, 0)
    assert b.conserved()
    assert world.a.store.by_id(world.admission).state == "BANNED"
    assert world.a.store.db.execute("SELECT resolved FROM admission_pending").fetchone()[0] == 1
    world.a.escrow.verify()
    # idempotent
    before = events(world)
    assert world.a.settle_fraud(world.dispute_id, now=4) is True
    assert events(world) == before


@pytest.mark.parametrize("loser", ["auditor", "no-pending"])
def test_a_verdict_that_does_not_convict_the_miner_leaves_the_bond_alone(
    world: World, loser: str
) -> None:
    world.bond()
    if loser == "auditor":
        world.convict(loser=T.fixtures.AUDITOR.ss58)
        dispute_id = world.dispute_id
    else:
        dispute_id = "ab" * 32  # no pending reference was ever bound to it
    assert world.a.settle_fraud(dispute_id, now=3) is False
    assert balances(world).admission_locked == 1000
    # a fault of the auditor is no strike either
    assert world.a.store.by_id(world.admission).strikes == 0
    assert world.a.store.by_id(world.admission).state != "BANNED"


def test_settle_fraud_refuses_a_verdict_bound_to_another_admission(world: World) -> None:
    # Given: the pending reference names this admission but the verdict's evidence differs
    world.bond()
    world.contest = world.contest.model_copy(update={"evidence_hash": "66" * 32})
    world.a.suspend(
        HOT.ss58,
        reason="PENDING_DISPUTE",
        now=2,
        pending_dispute=True,
        dispute_id=world.dispute_id,
        evidence_hash="55" * 32,
    )
    world.convict(bind=False)
    # When / Then
    with pytest.raises(AdmissionError, match="UNPROVEN_TRAINING_FRAUD"):
        world.a.settle_fraud(world.dispute_id, now=3)
    assert balances(world).admission_locked == 1000
