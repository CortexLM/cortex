"""Shared-connection admission persistence, durable reservations and escrow outbox."""

from __future__ import annotations

import sqlite3
from dataclasses import dataclass

from pydantic import TypeAdapter

from hypertrain.ledger.escrow_v2 import EscrowError, EscrowV2
from hypertrain.protocol.messages_v2 import AdmissionState, EscrowLock, EscrowReceipt, Hex64


class AdmissionError(ValueError):
    def __init__(self, code: str) -> None:
        self.code = code
        super().__init__(code)


@dataclass(frozen=True, slots=True)
class AdmissionRecord:
    admission_id: str
    hotkey: str
    coldkey: str
    state: AdmissionState
    clean_count: int
    canary_blocks: tuple[int, ...]
    strikes: int
    pending_dispute: bool
    reason: str
    policy_hash: str
    screen_until: int
    challenge_json: str | None


class AdmissionStore:
    """Use the same SQLite connection as EscrowV2/L0, never independent eligibility copies."""

    def __init__(self, escrow: EscrowV2) -> None:
        self.escrow, self.db = escrow, escrow.db
        self.db.executescript("""
          CREATE TABLE IF NOT EXISTS admissions_v2(
            admission_id TEXT PRIMARY KEY, run_id TEXT, hotkey TEXT UNIQUE, coldkey TEXT,
            state TEXT, clean_count INTEGER DEFAULT 0, canary_blocks TEXT DEFAULT '',
            strikes INTEGER DEFAULT 0, pending_dispute INTEGER DEFAULT 0, reason TEXT,
            policy_hash TEXT, received_beacon INTEGER, seed_beacon INTEGER,
            screen_until INTEGER DEFAULT 0, challenge TEXT, proof_digest TEXT,
            reference_commitment TEXT, reference_json TEXT, reference_beacon INTEGER,
            trial_epoch INTEGER, screen_json TEXT);
          CREATE TABLE IF NOT EXISTS admission_reservations(
            reservation TEXT PRIMARY KEY, digest TEXT NOT NULL, receipt TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS admission_quota(
            identity TEXT PRIMARY KEY, beacon INTEGER, tokens INTEGER);
          CREATE TABLE IF NOT EXISTS admission_history(
            hotkey TEXT PRIMARY KEY, admission_id TEXT, coldkey TEXT);
          CREATE TABLE IF NOT EXISTS admission_trials(
            epoch INTEGER PRIMARY KEY, admission_id TEXT, nonce TEXT, received_beacon INTEGER,
            finalized_beacon INTEGER, evidence_hash TEXT);
          CREATE TABLE IF NOT EXISTS admission_trial_results(
            epoch INTEGER PRIMARY KEY, challenge TEXT NOT NULL, reference TEXT,
            proof TEXT, screen TEXT, outcome TEXT NOT NULL DEFAULT 'OPEN');
          CREATE TABLE IF NOT EXISTS admission_outbox(
            operation_id TEXT PRIMARY KEY, request TEXT NOT NULL, signer TEXT NOT NULL,
            receipt TEXT);
          CREATE TABLE IF NOT EXISTS admission_transitions(
            seq INTEGER PRIMARY KEY, admission_id TEXT, state TEXT, reason TEXT, beacon INTEGER);
          CREATE TABLE IF NOT EXISTS admission_pending(
            dispute_id TEXT PRIMARY KEY, admission_id TEXT NOT NULL, evidence_hash TEXT NOT NULL,
            coldkey TEXT NOT NULL, resolved INTEGER NOT NULL DEFAULT 0);
        """)

    def record(self, hotkey: str) -> AdmissionRecord:
        row = self.db.execute("SELECT * FROM admissions_v2 WHERE hotkey=?", (hotkey,)).fetchone()
        if row is None:
            raise AdmissionError("UNKNOWN_HOTKEY")
        return self._record(row)

    def by_id(self, admission_id: str) -> AdmissionRecord:
        row = self.db.execute(
            "SELECT * FROM admissions_v2 WHERE admission_id=?", (admission_id,)
        ).fetchone()
        if row is None:
            raise AdmissionError("UNKNOWN_ADMISSION")
        return self._record(row)

    @staticmethod
    def _record(row: sqlite3.Row) -> AdmissionRecord:
        state: AdmissionState
        match row["state"]:
            case "APPLIED":
                state = "APPLIED"
            case "PROBATION":
                state = "PROBATION"
            case "ACTIVE":
                state = "ACTIVE"
            case "SUSPENDED":
                state = "SUSPENDED"
            case "BANNED":
                state = "BANNED"
            case _:
                raise AdmissionError("CORRUPT_STATE")
        return AdmissionRecord(
            row["admission_id"],
            row["hotkey"],
            row["coldkey"],
            state,
            row["clean_count"],
            tuple(int(i) for i in row["canary_blocks"].split(",") if i),
            row["strikes"],
            bool(row["pending_dispute"]),
            row["reason"],
            row["policy_hash"],
            row["screen_until"],
            row["challenge"],
        )

    def reserve(self, key: str, digest: str, receipt: str) -> str:
        row = self.db.execute(
            "SELECT * FROM admission_reservations WHERE reservation=?", (key,)
        ).fetchone()
        if row:
            if row["digest"] != digest:
                raise AdmissionError("REPLAY_CONFLICT")
            return str(row["receipt"])
        self.db.execute("INSERT INTO admission_reservations VALUES(?,?,?)", (key, digest, receipt))
        return receipt

    def quota(self, identity: str, now: int, rate: int, burst: int) -> None:
        row = self.db.execute(
            "SELECT * FROM admission_quota WHERE identity=?", (identity,)
        ).fetchone()
        if row is not None and now < row["beacon"]:
            raise AdmissionError("BEACON_REGRESSION")
        tokens = min(burst, row["tokens"] + (now - row["beacon"]) * rate) if row else burst
        if not tokens:
            raise AdmissionError("JOIN_QUOTA")
        self.db.execute(
            "INSERT INTO admission_quota VALUES(?,?,?) ON CONFLICT(identity) DO "
            "UPDATE SET beacon=excluded.beacon,tokens=excluded.tokens",
            (identity, now, tokens - 1),
        )

    def transition(
        self, admission_id: str, state: AdmissionState, reason: str, beacon: int
    ) -> None:
        self.db.execute(
            "UPDATE admissions_v2 SET state=?,reason=? WHERE admission_id=?",
            (state, reason, admission_id),
        )
        self.db.execute(
            "INSERT INTO admission_transitions(admission_id,state,reason,beacon) VALUES(?,?,?,?)",
            (admission_id, state, reason, beacon),
        )

    def bind_pending(self, record: AdmissionRecord, dispute_id: str, evidence_hash: str) -> None:
        """Bind current evidence; settled IDs cannot authorize later incidents."""
        TypeAdapter(Hex64).validate_python(dispute_id)
        TypeAdapter(Hex64).validate_python(evidence_hash)
        existing = self.db.execute(
            "SELECT * FROM admission_pending WHERE dispute_id=?", (dispute_id,)
        ).fetchone()
        if existing is not None:
            if (
                existing["admission_id"],
                existing["coldkey"],
                existing["evidence_hash"],
                existing["resolved"],
            ) != (record.admission_id, record.coldkey, evidence_hash, 0):
                raise AdmissionError("PENDING_REFERENCE_CONFLICT")
            return
        current = self.db.execute(
            "SELECT 1 FROM admission_pending WHERE admission_id=? AND resolved=0",
            (record.admission_id,),
        ).fetchone()
        if current is not None:
            raise AdmissionError("CURRENT_PENDING_REFERENCE_EXISTS")
        self.db.execute(
            "INSERT INTO admission_pending VALUES(?,?,?,?,0)",
            (dispute_id, record.admission_id, evidence_hash, record.coldkey),
        )

    def enqueue_lock(self, operation: EscrowLock, signer: str) -> None:
        with self.escrow.tx():
            row = self.db.execute(
                "SELECT * FROM admission_outbox WHERE operation_id=?", (operation.operation_id,)
            ).fetchone()
            if row and (row["request"] != operation.model_dump_json() or row["signer"] != signer):
                raise EscrowError("OUTBOX_CONFLICT")
            self.db.execute(
                "INSERT OR IGNORE INTO admission_outbox(operation_id,request,signer) VALUES(?,?,?)",
                (operation.operation_id, operation.model_dump_json(), signer),
            )

    def reconcile_lock(self, operation_id: str) -> EscrowReceipt:
        """Crash after ledger commit/before receipt acknowledgment is exactly-once."""
        row = self.db.execute(
            "SELECT * FROM admission_outbox WHERE operation_id=?", (operation_id,)
        ).fetchone()
        if row is None:
            raise AdmissionError("UNKNOWN_OUTBOX")
        receipt = self.escrow.lock(
            EscrowLock.model_validate_json(row["request"]), signer=row["signer"]
        )
        with self.escrow.tx():
            self.db.execute(
                "UPDATE admission_outbox SET receipt=? WHERE operation_id=?",
                (receipt.model_dump_json(), operation_id),
            )
        return receipt

    def pending_outbox(self, admission_id: str) -> bool:
        for row in self.db.execute("SELECT request FROM admission_outbox WHERE receipt IS NULL"):
            if EscrowLock.model_validate_json(row["request"]).admission_id == admission_id:
                return True
        return False
