"""SQLite registry of coldkey-granted server keys; a revoked server key stays revoked forever."""

from __future__ import annotations

import sqlite3
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path
from typing import Any

from hypertrain.challenge.store import ChallengeError
from hypertrain.protocol.server_keys import SignedGrant, SignedRevoke, check_sig, signing_message

SCHEMA = """CREATE TABLE IF NOT EXISTS server_keys(
    key_id TEXT PRIMARY KEY,
    server_pub TEXT NOT NULL UNIQUE,
    coldkey TEXT NOT NULL,
    server_name TEXT NOT NULL,
    run_id TEXT NOT NULL,
    scope TEXT NOT NULL,
    issued_at INTEGER NOT NULL,
    grant_json TEXT NOT NULL,
    revoked_at INTEGER,
    revoke_json TEXT
)"""
SELECT_RECORD = (
    "SELECT key_id,server_pub,coldkey,server_name,run_id,scope,issued_at,revoked_at"
    " FROM server_keys WHERE key_id=?"
)


class ServerKeys:
    def __init__(self, path: Path) -> None:
        self.path = path
        path.parent.mkdir(parents=True, exist_ok=True)
        with self._db() as db:
            db.execute(SCHEMA)

    @contextmanager
    def _db(self) -> Iterator[sqlite3.Connection]:
        db = sqlite3.connect(self.path, timeout=30, isolation_level="IMMEDIATE")
        db.row_factory = sqlite3.Row
        try:
            with db:
                yield db
        finally:
            db.close()

    def _record(self, db: sqlite3.Connection, key_id: str) -> dict[str, Any]:
        row = db.execute(SELECT_RECORD, (key_id,)).fetchone()
        if row is None:
            raise ChallengeError(404, "unknown server key")
        return {**dict(row), "revoked": row["revoked_at"] is not None}

    def grant(self, signed: SignedGrant) -> dict[str, Any]:
        g = signed.grant
        message = signing_message("grant", g)
        if not check_sig(g.coldkey, message, signed.cold_sig):
            raise ChallengeError(403, "coldkey grant signature mismatch")
        if not check_sig(g.server_pub, message, signed.server_sig):
            raise ChallengeError(403, "server key possession signature mismatch")
        if g.server_pub == g.coldkey:
            raise ChallengeError(422, "the server key must differ from the coldkey")
        with self._db() as db:
            existing = db.execute(
                "SELECT grant_json FROM server_keys WHERE key_id=? OR server_pub=?",
                (g.key_id, g.server_pub),
            ).fetchall()
            if existing:
                if [r["grant_json"] for r in existing] == [signed.model_dump_json()]:
                    return self._record(db, g.key_id)
                raise ChallengeError(409, "key_id or server key already granted")
            db.execute(
                "INSERT INTO server_keys VALUES(?,?,?,?,?,?,?,?,NULL,NULL)",
                (
                    g.key_id,
                    g.server_pub,
                    g.coldkey,
                    g.server_name,
                    g.run_id,
                    g.scope,
                    g.issued_at,
                    signed.model_dump_json(),
                ),
            )
            return self._record(db, g.key_id)

    def revoke(self, signed: SignedRevoke) -> dict[str, Any]:
        r = signed.revoke
        if not check_sig(r.coldkey, signing_message("revoke", r), signed.cold_sig):
            raise ChallengeError(403, "coldkey revoke signature mismatch")
        with self._db() as db:
            record = self._record(db, r.key_id)
            if record["coldkey"] != r.coldkey:
                raise ChallengeError(403, "only the granting coldkey may revoke")
            if not record["revoked"]:
                db.execute(
                    "UPDATE server_keys SET revoked_at=?, revoke_json=? WHERE key_id=?",
                    (r.revoked_at, signed.model_dump_json(), r.key_id),
                )
            return self._record(db, r.key_id)

    def list(self, coldkey: str) -> list[dict[str, Any]]:
        with self._db() as db:
            rows = db.execute(
                "SELECT key_id FROM server_keys WHERE coldkey=? ORDER BY issued_at, key_id",
                (coldkey,),
            ).fetchall()
            return [self._record(db, row["key_id"]) for row in rows]

    def is_revoked(self, hotkey: str) -> bool:
        with self._db() as db:
            row = db.execute(
                "SELECT 1 FROM server_keys WHERE server_pub=? AND revoked_at IS NOT NULL",
                (hotkey,),
            ).fetchone()
            return row is not None
