"""Durable exact-beacon pins: pause infrastructure, never replace a draw seed."""

from __future__ import annotations

import hashlib
import sqlite3
from dataclasses import dataclass

from hypertrain.beacon.core import (
    Beacon,
    BeaconRound,
    BeaconUnavailable,
    BeaconVerificationError,
)
from hypertrain.beacon.drand import verify_quicknet


@dataclass(frozen=True, slots=True)
class FreshnessStatus:
    run_id: str
    operation_id: str
    pinned_round: int
    signature_hash: str | None
    paused: bool
    reason: str | None


class FreshnessGate:
    """Use L0's SQLite connection; caller commits pin/pause before returning.

    Calling refresh after an arrival event resumes the *same* round. This class
    does not poll, move deadlines, or grant eligibility during an outage.
    """

    def __init__(self, db: sqlite3.Connection, run_id: str) -> None:
        self.db, self.run_id = db, run_id
        db.execute("""CREATE TABLE IF NOT EXISTS beacon_pins_v2 (
            run_id TEXT NOT NULL, operation_id TEXT NOT NULL, pinned_round INTEGER NOT NULL,
            signature TEXT, signature_hash TEXT, paused INTEGER NOT NULL,
            reason TEXT, PRIMARY KEY(run_id, operation_id))""")

    def pin(self, operation_id: str, round_number: int) -> FreshnessStatus:
        if type(round_number) is not int or round_number < 1:
            raise BeaconVerificationError("invalid pinned round")
        self.db.execute(
            """INSERT OR IGNORE INTO beacon_pins_v2
            VALUES (?, ?, ?, NULL, NULL, 1, 'AWAITING_PINNED_BEACON')""",
            (self.run_id, operation_id, round_number),
        )
        status = self.status(operation_id)
        if status.pinned_round != round_number:
            raise BeaconVerificationError("replacement seed forbidden")
        return status

    def status(self, operation_id: str) -> FreshnessStatus:
        row = self.db.execute(
            """SELECT pinned_round, signature_hash, paused, reason
            FROM beacon_pins_v2 WHERE run_id=? AND operation_id=?""",
            (self.run_id, operation_id),
        ).fetchone()
        if row is None:
            raise BeaconUnavailable("draw not pinned")
        return FreshnessStatus(self.run_id, operation_id, row[0], row[1], bool(row[2]), row[3])

    def refresh(
        self,
        operation_id: str,
        beacon: Beacon,
        *,
        now_seconds: int,
        genesis_seconds: int,
        period_seconds: int,
        max_lag_rounds: int,
    ) -> BeaconRound:
        """Verify current freshness separately; historical draw stays exactly pinned."""
        status = self.status(operation_id)
        if period_seconds < 1 or max_lag_rounds < 0 or now_seconds < genesis_seconds:
            raise BeaconVerificationError("invalid freshness clock")
        emitted = (now_seconds - genesis_seconds) // period_seconds + 1
        health_round = max(1, emitted - max_lag_rounds)
        try:
            draw = beacon.get(status.pinned_round)
            health = draw if health_round == status.pinned_round else beacon.get(health_round)
            for br, expected in ((draw, status.pinned_round), (health, health_round)):
                if (
                    br.round != expected
                    or not verify_quicknet(br.round, br.signature)
                    or (hashlib.sha256(bytes.fromhex(br.signature)).hexdigest() != br.randomness)
                ):
                    raise BeaconVerificationError("exact beacon signature/randomness mismatch")
            digest = hashlib.sha256(bytes.fromhex(draw.signature)).hexdigest()
            if status.signature_hash is not None and status.signature_hash != digest:
                raise BeaconVerificationError("pinned signature changed")
        except (BeaconUnavailable, BeaconVerificationError, ValueError) as exc:
            self.db.execute(
                """UPDATE beacon_pins_v2 SET paused=1, reason=?
                WHERE run_id=? AND operation_id=?""",
                (type(exc).__name__, self.run_id, operation_id),
            )
            raise BeaconUnavailable("infrastructure pause; exact seed retained") from exc
        self.db.execute(
            """UPDATE beacon_pins_v2 SET signature=?, signature_hash=?,
            paused=0, reason=NULL WHERE run_id=? AND operation_id=?""",
            (draw.signature, digest, self.run_id, operation_id),
        )
        return draw

    def require_fresh(self, operation_id: str) -> FreshnessStatus:
        status = self.status(operation_id)
        if status.paused or status.signature_hash is None:
            raise BeaconUnavailable("eligibility/draw/deadline advancement frozen")
        return status
