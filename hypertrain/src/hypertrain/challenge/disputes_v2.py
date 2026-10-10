"""Durable funded STEP/LAYER/OP progression on L2's shared transaction boundary.

SIZE_OK: one indivisible dispute state machine; transaction, event and evidence
updates must commit together within the seven-path lane ownership boundary.
"""

from __future__ import annotations

import math
import threading
from collections.abc import Callable
from typing import Literal

from pydantic import Field, JsonValue, StrictBool, StrictInt

from hypertrain.auditor.island_bisect import RefereeEvidence
from hypertrain.ledger.escrow_v2 import EscrowV2, Settlement
from hypertrain.protocol.envelope_v2 import Intake, parse_envelope
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair, decode_hotkey, verify
from hypertrain.protocol.messages import SS58, Hex64, StateServe
from hypertrain.protocol.messages_v2 import (
    BisectV2,
    DisputePolicyV2,
    DisputeV2,
    EscrowLock,
    EscrowRelease,
    ResolutionV2,
    WireModel,
)


class DisputeError(ValueError):
    def __init__(self, code: str) -> None:
        self.code = code
        super().__init__(code)


class Contest(WireModel):
    """L0 derives this from an authorized accepted verdict, never an HTTP body."""

    verdict_hash: Hex64
    challenge_hash: Hex64
    w: StrictInt = Field(ge=0)
    miner: SS58
    auditor: SS58
    referee: SS58
    step_span: StrictInt = Field(ge=1)
    layer_span: StrictInt = Field(ge=1)
    op_span: StrictInt = Field(ge=1)
    admission_id: Hex64 | None = None
    coldkey: SS58 | None = None
    evidence_hash: Hex64 | None = None
    auditor_coldkey: SS58 | None = None


class DisputeSettlement(Settlement):
    """Internal accepted-resolution authority, scoped to one actual funded lock.

    outcome describes this lock's disposition, not guilt of another identity.
    Unknown auditor ownership remains None; no synthetic penalty account.
    """

    loser: SS58 | None
    loser_coldkey: SS58 | None
    referee: SS58
    lock_id: Hex64
    lock_owner: SS58
    resolution_hash: Hex64 | None


class Turn(WireModel):
    run_id: Hex64
    dispute_id: Hex64
    contest: Contest
    seq: StrictInt = Field(ge=0, le=256)
    level: Literal["step", "layer", "op"]
    ctx: list[StrictInt]
    interval: tuple[StrictInt, StrictInt]
    expected_party: SS58
    transcript_hash: Hex64
    turn_deadline: StrictInt
    absolute_deadline: StrictInt
    lock_id: Hex64
    paused: StrictBool = False
    reassignments: StrictInt = 0
    resolution: ResolutionV2 | None = None
    resolved_beacon: StrictInt | None = None


class WatchEvent(WireModel):
    run_id: Hex64
    cursor: StrictInt = Field(ge=1)
    party: SS58
    kind: Literal["turn", "state"]
    turn: Turn
    checkpoint: StrictInt | None
    issued_beacon: StrictInt = Field(ge=1)
    signer: SS58
    sig: str


class EventAck(WireModel):
    run_id: Hex64
    cursor: StrictInt = Field(ge=1)
    event_hash: Hex64
    party: SS58
    received_beacon: StrictInt = Field(ge=1)
    sig: str


class Availability(WireModel):
    run_id: Hex64
    dispute_id: Hex64
    seq: StrictInt
    transcript_hash: Hex64
    observed_beacon: StrictInt
    service_healthy: StrictBool
    beacon_healthy: StrictBool
    reference_healthy: StrictBool
    signer: SS58
    sig: str


def signed_message(model: WatchEvent | EventAck | Availability) -> bytes:
    return (
        b"hypertrain/dispute/2|"
        + type(model).__name__.encode()
        + b"|"
        + canonicalize(
            model.model_dump(mode="json", exclude={"sig"}),
        )
    )


def authentic(model: WatchEvent | EventAck | Availability) -> bool:
    signer = model.party if isinstance(model, EventAck) else model.signer
    try:
        signature = bytes.fromhex(model.sig)
    except ValueError:
        return False
    return verify(decode_hotkey(signer), signed_message(model), signature)


class DisputesV2:
    """Mutable service; shared SQLite plus condition subscription, no training dependency.

    L0 supplies authoritative verdict/identity lookup, qualified span bounds and
    independent local replay evidence. Cross-process route notification calls
    notify() after the shared transaction commits; the durable event table is truth.
    """

    def __init__(
        self,
        escrow: EscrowV2,
        policy: DisputePolicyV2,
        coordinator: Keypair,
        *,
        contest_of: Callable[[str], Contest],
        owner_of: Callable[[str], str],
        span: Callable[[Contest, str, tuple[int, ...]], int],
    ) -> None:
        if coordinator.ss58 != escrow.manifest.training.coord_pubkey:
            raise DisputeError("COORDINATOR_AUTHORITY")
        if policy.digest() != escrow.manifest.network.dispute_policy_hash:
            raise DisputeError("DISPUTE_POLICY_HASH")
        self.escrow, self.policy, self.key = escrow, policy, coordinator
        self.run_id, self.db = escrow.run_id, escrow.db
        self.contest_of, self.owner_of, self.span = contest_of, owner_of, span
        self.condition = threading.Condition()
        with escrow.tx():
            for sql in (
                "CREATE TABLE IF NOT EXISTS disputes_v2(id TEXT PRIMARY KEY,verdict TEXT UNIQUE,"
                "miner TEXT,turn TEXT,request TEXT)",
                "CREATE TABLE IF NOT EXISTS dispute_entries_v2(id TEXT,seq INTEGER,"
                "body TEXT,envelope BLOB,PRIMARY KEY(id,seq))",
                "CREATE TABLE IF NOT EXISTS dispute_events_v2(cursor INTEGER PRIMARY KEY "
                "AUTOINCREMENT,party TEXT,event TEXT)",
                "CREATE TABLE IF NOT EXISTS dispute_acks_v2(cursor INTEGER,party TEXT,"
                "ack TEXT,PRIMARY KEY(cursor,party))",
                "CREATE TABLE IF NOT EXISTS dispute_evidence_v2(hash TEXT PRIMARY KEY,body TEXT)",
                "CREATE TABLE IF NOT EXISTS dispute_states_v2(challenge TEXT,t INTEGER,"
                "party TEXT,body TEXT,envelope BLOB,PRIMARY KEY(challenge,t,party))",
                "CREATE TABLE IF NOT EXISTS dispute_receipts_v2(id TEXT,seq INTEGER,turn TEXT,"
                "PRIMARY KEY(id,seq))",
            ):
                self.db.execute(sql)

    def notify(self) -> None:
        with self.condition:
            self.condition.notify_all()

    def get(self, dispute_id: str) -> Turn:
        row = self.db.execute("SELECT turn FROM disputes_v2 WHERE id=?", (dispute_id,)).fetchone()
        if row is None:
            raise DisputeError("UNKNOWN_DISPUTE")
        return Turn.model_validate_json(row[0])

    def _save(self, turn: Turn) -> None:
        self.db.execute(
            "UPDATE disputes_v2 SET turn=? WHERE id=?",
            (
                turn.model_dump_json(),
                turn.dispute_id,
            ),
        )

    def _event(self, turn: Turn, now: int, checkpoint: int | None = None) -> WatchEvent:
        cursor = self.db.execute(
            "INSERT INTO dispute_events_v2(party,event) VALUES(?,?)",
            (
                turn.expected_party,
                "",
            ),
        ).lastrowid
        assert cursor is not None
        event = WatchEvent(
            run_id=self.run_id,
            cursor=cursor,
            party=turn.expected_party,
            kind="turn" if checkpoint is None else "state",
            turn=turn,
            checkpoint=checkpoint,
            issued_beacon=now,
            signer=self.key.ss58,
            sig="",
        )
        event = event.model_copy(update={"sig": self.key.sign(signed_message(event)).hex()})
        self.db.execute(
            "UPDATE dispute_events_v2 SET event=? WHERE cursor=?",
            (
                event.model_dump_json(),
                cursor,
            ),
        )
        return event

    def open(self, raw: bytes, *, now: int) -> Turn:
        env = parse_envelope(raw)
        with self.escrow.tx():
            model = Intake(self.run_id, {"DisputeV2": lambda h: h == env.body["hotkey"]}).accept(
                raw,
                now,
            )
            if not isinstance(model, DisputeV2) or model.action != "contest":
                raise DisputeError("FUNDED_CONTEST_REQUIRED")
            contest = self.contest_of(model.verdict_hash)
            recovery = (contest.admission_id, contest.coldkey, contest.evidence_hash)
            if any(value is not None for value in recovery) and not all(
                value is not None for value in recovery
            ):
                raise DisputeError("INCOMPLETE_ACCEPTED_RECOVERY_LINKAGE")
            if contest.coldkey is not None and contest.coldkey != self.owner_of(contest.miner):
                raise DisputeError("ACCEPTED_RECOVERY_OWNER")
            if contest.auditor_coldkey is not None and contest.auditor_coldkey != self.owner_of(
                contest.auditor,
            ):
                raise DisputeError("ACCEPTED_AUDITOR_OWNER")
            if (
                contest.step_span
                != self.escrow.manifest.training.inner.H // self.escrow.manifest.training.inner.J
            ):
                raise DisputeError("CONTEST_WINDOW_GEOMETRY")
            if contest.miner != env.signer or contest.auditor not in self.escrow.auditors:
                raise DisputeError("CONTEST_IDENTITY")
            if contest.referee not in self.policy.referees or contest.referee in (
                contest.miner,
                contest.auditor,
                self.key.ss58,
            ):
                raise DisputeError("INDEPENDENT_REFEREE_REQUIRED")
            dispute_id = sha256_hex(
                canonicalize([self.run_id, contest.verdict_hash, contest.miner])
            )
            old = self.db.execute(
                "SELECT request FROM disputes_v2 WHERE id=?", (dispute_id,)
            ).fetchone()
            if old is not None:
                if old[0] != model.model_dump_json():
                    raise DisputeError("CONTEST_REPLAY_CONFLICT")
                return self.get(dispute_id)
            active = [
                Turn.model_validate_json(r[0])
                for r in self.db.execute(
                    "SELECT turn FROM disputes_v2",
                )
            ]
            if sum(t.resolution is None for t in active) >= self.policy.max_open or any(
                t.resolution is None and t.contest.miner == contest.miner for t in active
            ):
                raise DisputeError("QUEUE_CAPACITY_EXCLUDE_WITHOUT_FRAUD")
            lockrow = self.db.execute(
                "SELECT kind,request FROM escrow_events WHERE operation_id=?", (model.bond_lock,)
            ).fetchone()
            if lockrow is None or lockrow[0] != "LOCK_CONTEST":
                raise DisputeError("UNFUNDED_CONTEST")
            lock = EscrowLock.model_validate_json(lockrow[1])
            amount = self.db.execute(
                "SELECT COALESCE(SUM(units),0) FROM escrow_units WHERE "
                "owner=? AND bucket='dispute_locked' AND ref=?",
                (lock.owner, lock.operation_id),
            ).fetchone()[0]
            if (lock.owner, lock.dispute_id) != (self.owner_of(contest.miner), dispute_id) or (
                amount != lock.units or amount < self.policy.max_referee_cost_units
            ):
                raise DisputeError("CONTEST_LOCK_BINDING_OR_COST")
            timeout = self.escrow.manifest.training.verify.T.dispute_per_level
            budget = (
                2
                * timeout
                * (
                    sum(
                        math.ceil(math.log2(n))
                        for n in (
                            contest.step_span,
                            contest.layer_span,
                            contest.op_span,
                        )
                    )
                    + 3
                )
            )
            turn = Turn(
                run_id=self.run_id,
                dispute_id=dispute_id,
                contest=contest,
                seq=0,
                level="step",
                ctx=[],
                interval=(0, contest.step_span),
                expected_party=contest.miner,
                transcript_hash=model.digest(),
                turn_deadline=now + timeout,
                absolute_deadline=now + budget,
                lock_id=lock.operation_id,
            )
            self.db.execute(
                "INSERT INTO disputes_v2 VALUES(?,?,?,?,?)",
                (
                    dispute_id,
                    contest.verdict_hash,
                    contest.miner,
                    turn.model_dump_json(),
                    model.model_dump_json(),
                ),
            )
            self._event(turn, now)
        self.notify()
        return turn

    def bisect(self, raw: bytes, *, now: int) -> Turn:
        env = parse_envelope(raw)
        with self.escrow.tx():
            model = Intake(self.run_id, {"BisectV2": lambda h: h == env.body["party"]}).accept(
                raw, now
            )
            if not isinstance(model, BisectV2):
                raise DisputeError("BISECT_TYPE")
            turn = self.get(model.dispute_id)
            old = self.db.execute(
                "SELECT body FROM dispute_entries_v2 WHERE id=? AND seq=?",
                (
                    model.dispute_id,
                    model.seq,
                ),
            ).fetchone()
            if old is not None:
                if old[0] != model.model_dump_json():
                    raise DisputeError("SEQUENCE_REPLAY_CONFLICT")
                receipt = self.db.execute(
                    "SELECT turn FROM dispute_receipts_v2 WHERE id=? AND seq=?",
                    (model.dispute_id, model.seq),
                ).fetchone()
                return Turn.model_validate_json(receipt[0])
            if (
                turn.resolution is not None
                or turn.paused
                or now
                > min(
                    turn.turn_deadline,
                    turn.absolute_deadline,
                )
            ):
                raise DisputeError("CLOSED_PAUSED_OR_EXPIRED")
            if (
                model.seq,
                model.level,
                model.ctx,
                model.interval,
                model.party,
                model.previous_transcript_hash,
            ) != (
                turn.seq,
                turn.level,
                turn.ctx,
                turn.interval,
                turn.expected_party,
                turn.transcript_hash,
            ):
                raise DisputeError("TURN_SEQUENCE_CONTEXT_PARTY_OR_HASH")
            at = [model.interval[0], -(-(sum(model.interval)) // 2), model.interval[1]]
            if any(
                a == b and left != right
                for a, b, left, right in zip(
                    at[:-1],
                    at[1:],
                    model.hashes[:-1],
                    model.hashes[1:],
                    strict=True,
                )
            ):
                raise DisputeError("CONTRADICTORY_REPEATED_POINT")
            transcript = sha256_hex(
                bytes.fromhex(turn.transcript_hash) + canonicalize(model.body())
            )
            self.db.execute(
                "INSERT INTO dispute_entries_v2 VALUES(?,?,?,?)",
                (
                    turn.dispute_id,
                    model.seq,
                    model.model_dump_json(),
                    raw,
                ),
            )
            update: dict[str, JsonValue] = dict(seq=turn.seq + 1, transcript_hash=transcript)
            if model.party == turn.contest.miner:
                update["expected_party"] = turn.contest.auditor
            else:
                first = BisectV2.model_validate_json(
                    self.db.execute(
                        "SELECT body FROM dispute_entries_v2 WHERE id=? AND seq=?",
                        (turn.dispute_id, model.seq - 1),
                    ).fetchone()[0]
                )
                if first.hashes[0] != model.hashes[0]:
                    update["paused"] = True
                else:
                    at = [turn.interval[0], -(-(sum(turn.interval)) // 2), turn.interval[1]]
                    i = next((i for i in (1, 2) if first.hashes[i] != model.hashes[i]), None)
                    if i is None:
                        update["paused"] = True  # referee distinguishes MATCH from both-wrong
                    else:
                        lo, hi = at[i - 1], at[i]
                        if hi - lo == 1 and turn.level != "op":
                            level = "layer" if turn.level == "step" else "op"
                            ctx = [hi] if level == "layer" else [turn.ctx[0], lo]
                            update.update(
                                level=level,
                                ctx=[int(n) for n in ctx],
                                interval=[
                                    0,
                                    self.span(
                                        turn.contest,
                                        level,
                                        tuple(ctx),
                                    ),
                                ],
                            )
                        else:
                            update["interval"] = [lo, hi]
                            update["paused"] = hi - lo == 1
                update["expected_party"] = turn.contest.miner
            update["turn_deadline"] = min(
                now + self.escrow.manifest.training.verify.T.dispute_per_level,
                turn.absolute_deadline,
            )
            next_turn = Turn.model_validate({**turn.body(), **update})
            if next_turn.interval[0] >= next_turn.interval[1]:
                raise DisputeError("NONEMPTY_INTERVAL_REQUIRED")
            if next_turn.seq >= self.policy.max_transcript_entries:
                next_turn = next_turn.model_copy(update={"paused": True})
            self._save(next_turn)
            self.db.execute(
                "INSERT INTO dispute_receipts_v2 VALUES(?,?,?)",
                (
                    turn.dispute_id,
                    model.seq,
                    next_turn.model_dump_json(),
                ),
            )
            if not next_turn.paused and next_turn.seq < self.policy.max_transcript_entries:
                self._event(next_turn, now)
        self.notify()
        return next_turn

    def events(self, party: str, cursor: int, *, timeout: float = 0) -> list[WatchEvent]:
        if cursor < 0 or not 0 <= timeout <= 30:
            raise DisputeError("CURSOR_OR_LONG_POLL_BOUND")

        def available() -> list[WatchEvent]:
            with self.escrow.mutex:
                return [
                    WatchEvent.model_validate_json(r[0])
                    for r in self.db.execute(
                        "SELECT event FROM dispute_events_v2 WHERE party=? AND cursor>? "
                        "ORDER BY cursor LIMIT 64",
                        (party, cursor),
                    )
                ]

        with self.condition:
            self.condition.wait_for(lambda: bool(available()), timeout=timeout)
            return available()

    def acknowledge(self, ack: EventAck, *, now: int) -> None:
        if not authentic(ack) or ack.run_id != self.run_id or ack.received_beacon > now:
            raise DisputeError("ACK_AUTHORITY_OR_TIME")
        with self.escrow.tx():
            row = self.db.execute(
                "SELECT event FROM dispute_events_v2 WHERE cursor=? AND party=?",
                (
                    ack.cursor,
                    ack.party,
                ),
            ).fetchone()
            if row is None:
                raise DisputeError("ACK_UNKNOWN_EVENT")
            event = WatchEvent.model_validate_json(row[0])
            if (
                ack.event_hash != event.digest()
                or not event.issued_beacon
                <= ack.received_beacon
                <= min(
                    event.turn.turn_deadline,
                    event.turn.absolute_deadline,
                )
            ):
                raise DisputeError("ACK_EVENT_OR_DEADLINE")
            old = self.db.execute(
                "SELECT ack FROM dispute_acks_v2 WHERE cursor=? AND party=?",
                (
                    ack.cursor,
                    ack.party,
                ),
            ).fetchone()
            if (
                old is not None
                and EventAck.model_validate_json(old[0]).event_hash != ack.event_hash
            ):
                raise DisputeError("ACK_CONFLICT")
            self.db.execute(
                "INSERT OR IGNORE INTO dispute_acks_v2 VALUES(?,?,?)",
                (
                    ack.cursor,
                    ack.party,
                    ack.model_dump_json(),
                ),
            )

    def state_request(self, dispute_id: str, checkpoint: int, *, now: int) -> WatchEvent:
        with self.escrow.tx():
            turn = self.get(dispute_id)
            if (
                turn.resolution is not None
                or now > turn.absolute_deadline
                or not (0 <= checkpoint <= turn.contest.step_span)
            ):
                raise DisputeError("STATE_REQUEST_CONTEXT")
            event = self._event(
                turn.model_copy(update={"expected_party": turn.contest.miner}), now, checkpoint
            )
        self.notify()
        return event

    def reassign(self, dispute_id: str, referee: str, *, now: int) -> Turn:
        """One persisted infrastructure retry, never extends the absolute budget."""
        with self.escrow.tx():
            turn = self.get(dispute_id)
            if (
                not turn.paused
                or turn.resolution is not None
                or now >= turn.absolute_deadline
                or turn.reassignments >= self.policy.max_referee_reassignments
                or referee not in self.policy.referees
                or referee
                in (
                    turn.contest.miner,
                    turn.contest.auditor,
                    turn.contest.referee,
                    self.key.ss58,
                )
            ):
                raise DisputeError("REFEREE_REASSIGNMENT_BOUND")
            contest = turn.contest.model_copy(update={"referee": referee})
            turn = turn.model_copy(update={"contest": contest, "reassignments": 1})
            self._save(turn)
        return turn

    def pause(self, dispute_id: str) -> Turn:
        """Internal beacon/reference infrastructure freeze; immutable absolute horizon."""
        with self.escrow.tx():
            turn = self.get(dispute_id)
            if turn.resolution is not None:
                raise DisputeError("CLOSED_DISPUTE")
            turn = turn.model_copy(update={"paused": True})
            self._save(turn)
        return turn

    def register_evidence(self, evidence: RefereeEvidence) -> str:
        """Internal independently anchored executor capability; NOT signed claimant intake."""
        with self.escrow.tx():
            turn = self.get(evidence.dispute_id)
            if evidence.transcript_hash != turn.transcript_hash:
                raise DisputeError("REFEREE_STALE_EVIDENCE")
            self.db.execute(
                "INSERT OR IGNORE INTO dispute_evidence_v2 VALUES(?,?)",
                (
                    evidence.digest(),
                    evidence.model_dump_json(),
                ),
            )
        return evidence.digest()

    def state_serve(self, raw: bytes, *, now: int) -> StateServe:
        """Persist signed claims; independent replay verifies the served bytes."""
        env = parse_envelope(raw)
        with self.escrow.tx():
            model = Intake(
                self.run_id,
                {
                    "StateServe": lambda h: h == env.body["hotkey"],
                },
            ).accept(raw, now)
            if not isinstance(model, StateServe):
                raise DisputeError("STATE_SERVE_TYPE")
            requests = [
                WatchEvent.model_validate_json(r[0])
                for r in self.db.execute(
                    "SELECT event FROM dispute_events_v2 WHERE party=?",
                    (env.signer,),
                )
            ]
            stride = self.escrow.manifest.training.inner.J
            if not any(
                e.kind == "state"
                and e.checkpoint is not None
                and (
                    e.turn.contest.challenge_hash,
                    e.checkpoint * stride,
                )
                == (model.challenge_hash, model.t)
                and now <= e.turn.absolute_deadline
                for e in requests
            ):
                raise DisputeError("UNREQUESTED_STATE_SERVE")
            old = self.db.execute(
                "SELECT body FROM dispute_states_v2 WHERE challenge=? AND t=? AND party=?",
                (model.challenge_hash, model.t, env.signer),
            ).fetchone()
            if old is not None and old[0] != model.model_dump_json():
                raise DisputeError("STATE_SERVE_REPLAY_CONFLICT")
            self.db.execute(
                "INSERT OR IGNORE INTO dispute_states_v2 VALUES(?,?,?,?,?)",
                (
                    model.challenge_hash,
                    model.t,
                    env.signer,
                    model.model_dump_json(),
                    raw,
                ),
            )
            return model

    def resolution(self, raw: bytes, *, now: int) -> Turn:
        env = parse_envelope(raw)
        with self.escrow.tx():
            turn = self.get(str(env.body["dispute_id"]))
            model = Intake(
                self.run_id, {"ResolutionV2": lambda h: h == turn.contest.referee}
            ).accept(
                raw,
                now,
            )
            if not isinstance(model, ResolutionV2) or model.transcript_hash != turn.transcript_hash:
                raise DisputeError("RESOLUTION_TYPE_OR_TRANSCRIPT")
            if turn.resolution is not None:
                if model != turn.resolution:
                    raise DisputeError("RESOLUTION_CONFLICT")
                return turn
            if now > turn.absolute_deadline:
                raise DisputeError("RESOLUTION_ABSOLUTE_DEADLINE")
            row = self.db.execute(
                "SELECT body FROM dispute_evidence_v2 WHERE hash=?", (model.evidence_hash,)
            ).fetchone()
            if row is None:
                raise DisputeError("UNVERIFIED_REFEREE_EVIDENCE")
            evidence = RefereeEvidence.model_validate_json(row[0])
            if (evidence.dispute_id, evidence.transcript_hash, evidence.reason, evidence.loser) != (
                turn.dispute_id,
                turn.transcript_hash,
                model.reason,
                model.loser,
            ):
                raise DisputeError("REFEREE_EVIDENCE_BINDING")
            if model.loser is not None and model.loser not in (
                turn.contest.miner,
                turn.contest.auditor,
            ):
                raise DisputeError("UNKNOWN_LOSER")
            turn = turn.model_copy(update={"resolution": model, "resolved_beacon": now})
            self._save(turn)
        self.notify()
        return turn

    def timeout(
        self, dispute_id: str, proofs: tuple[Availability, Availability], *, now: int
    ) -> Turn:
        """Signed delivery and healthy distinct observers; silence is not computation fraud."""
        with self.escrow.tx():
            turn = self.get(dispute_id)
            if turn.resolution is not None or now <= min(
                turn.turn_deadline, turn.absolute_deadline
            ):
                raise DisputeError("TIMEOUT_NOT_DUE")
            valid = all(
                authentic(p)
                and (
                    p.run_id,
                    p.dispute_id,
                    p.seq,
                    p.transcript_hash,
                    p.observed_beacon,
                    p.service_healthy,
                    p.beacon_healthy,
                    p.reference_healthy,
                )
                == (self.run_id, dispute_id, turn.seq, turn.transcript_hash, now, True, True, True)
                for p in proofs
            )
            authorities = {p.signer for p in proofs}
            allowed = set(self.policy.referees) | {self.key.ss58}
            if (
                not valid
                or len(authorities) != 2
                or not authorities <= allowed
                or authorities
                & {
                    turn.contest.miner,
                    turn.contest.auditor,
                }
            ):
                raise DisputeError("UNCONFIRMED_AVAILABILITY")
            delivered = False
            for row in self.db.execute(
                "SELECT e.event FROM dispute_events_v2 e JOIN dispute_acks_v2 a "
                "ON e.cursor=a.cursor AND e.party=a.party WHERE e.party=?",
                (turn.expected_party,),
            ):
                event = WatchEvent.model_validate_json(row[0])
                delivered |= (
                    event.kind == "turn"
                    and event.turn.dispute_id == dispute_id
                    and (event.turn.seq, event.turn.transcript_hash)
                    == (turn.seq, turn.transcript_hash)
                )
            if not delivered:
                raise DisputeError("NO_AUTHENTICATED_TURN_DELIVERY")
            reason: Literal["AUDITOR_FAULT", "PARTY_TIMEOUT"] = (
                "AUDITOR_FAULT" if turn.expected_party == turn.contest.auditor else "PARTY_TIMEOUT"
            )
            evidence_hash = sha256_hex(canonicalize([p.body() for p in proofs]))
            self.db.execute(
                "INSERT OR IGNORE INTO dispute_evidence_v2 VALUES(?,?)",
                (
                    evidence_hash,
                    canonicalize([p.body() for p in proofs]).decode(),
                ),
            )
            resolution = ResolutionV2(
                dispute_id=dispute_id,
                transcript_hash=turn.transcript_hash,
                reason=reason,
                loser=turn.expected_party,
                evidence_hash=evidence_hash,
            )
            turn = turn.model_copy(update={"resolution": resolution, "resolved_beacon": now})
            self._save(turn)
        self.notify()
        return turn

    def exclude(self, dispute_id: str, *, now: int) -> Turn:
        """Absolute exhaustion or infrastructure exclusion; no claimant loses collateral."""
        with self.escrow.tx():
            turn = self.get(dispute_id)
            if turn.resolution is not None:
                return turn
            if not turn.paused and now < turn.absolute_deadline:
                raise DisputeError("EXCLUSION_NOT_DUE")
            evidence = RefereeEvidence(
                dispute_id=dispute_id,
                transcript_hash=turn.transcript_hash,
                reason="INFRASTRUCTURE",
                loser=None,
                predecessor_hash="0" * 64,
                inputs_hash="0" * 64,
                output_hash="0" * 64,
                op_spec="bounded-infrastructure-exclusion",
            )
            self.register_evidence(evidence)
            turn = turn.model_copy(
                update={
                    "resolved_beacon": now,
                    "resolution": ResolutionV2(
                        dispute_id=dispute_id,
                        transcript_hash=turn.transcript_hash,
                        reason="INFRASTRUCTURE",
                        loser=None,
                        evidence_hash=evidence.digest(),
                    ),
                }
            )
            self._save(turn)
        self.notify()
        return turn

    def settlement(self, lock_id: str) -> DisputeSettlement:
        row = self.db.execute(
            "SELECT turn FROM disputes_v2 WHERE id=? OR json_extract(turn,'$.lock_id')=?",
            (lock_id, lock_id),
        ).fetchone()
        if row is None:
            raise DisputeError("UNKNOWN_CONTEST_LOCK")
        turn = Turn.model_validate_json(row[0])
        lockrow = self.db.execute(
            "SELECT request,kind FROM escrow_events WHERE operation_id=?", (turn.lock_id,)
        ).fetchone()
        if lockrow is None or lockrow[1] != "LOCK_CONTEST":
            raise DisputeError("UNKNOWN_CONTEST_LOCK")
        lock = EscrowLock.model_validate_json(lockrow[0])
        if lock.dispute_id != turn.dispute_id:
            raise DisputeError("SETTLEMENT_LOCK_REFERENCE")
        resolution = turn.resolution
        losing = (
            resolution is not None
            and resolution.reason == "FRAUD"
            and (resolution.loser == turn.contest.miner)
        )
        loser = resolution.loser if resolution else None
        return DisputeSettlement(
            finality_hash=resolution.digest() if resolution else "0" * 64,
            closed_dispute_root=turn.transcript_hash,
            release_beacon=turn.resolved_beacon
            if turn.resolved_beacon is not None
            else turn.absolute_deadline,
            unresolved=resolution is None,
            outcome="FRAUD" if losing else "MATCH",
            run_id=turn.run_id,
            admission_id=turn.contest.admission_id,
            coldkey=turn.contest.coldkey,
            dispute_id=turn.dispute_id,
            evidence_hash=turn.contest.evidence_hash,
            loser=loser,
            loser_coldkey=lock.owner
            if loser == turn.contest.miner
            else (turn.contest.auditor_coldkey if loser == turn.contest.auditor else None),
            referee=turn.contest.referee,
            lock_id=lock.operation_id,
            lock_owner=lock.owner,
            resolution_hash=resolution.digest() if resolution else None,
        )

    def settle_lock(self, dispute_id: str, *, now: int, referee_cost: int = 0) -> None:
        with self.escrow.tx():
            turn = self.get(dispute_id)
            status = self.settlement(turn.lock_id)
            if status.unresolved:
                raise DisputeError("UNRESOLVED_CONTEST")
            row = self.db.execute(
                "SELECT request FROM escrow_events WHERE operation_id=?", (turn.lock_id,)
            ).fetchone()
            lock = EscrowLock.model_validate_json(row[0])
            if (status.lock_id, status.lock_owner, lock.dispute_id) != (
                lock.operation_id,
                lock.owner,
                turn.dispute_id,
            ):
                raise DisputeError("SETTLEMENT_LOCK_OWNERSHIP")
            release = EscrowRelease(
                **lock.model_dump(exclude={"kind", "operation_id"}),
                operation_id=sha256_hex(canonicalize([turn.dispute_id, "settle"])),
                lock_id=turn.lock_id,
                finality_hash=status.finality_hash,
                closed_dispute_root=status.closed_dispute_root,
            )
            if (
                turn.resolution is not None
                and turn.resolution.reason == "FRAUD"
                and status.loser == turn.contest.miner
                and status.loser_coldkey == lock.owner
            ):
                self.escrow.slash(
                    release,
                    authority=self.key.ss58,
                    now_beacon=now,
                    referee=self.owner_of(turn.contest.referee) if referee_cost else None,
                    referee_cost=referee_cost,
                    max_referee_cost=self.policy.max_referee_cost_units,
                )
            else:
                self.escrow.release(release, signer=lock.owner, now_beacon=now)
