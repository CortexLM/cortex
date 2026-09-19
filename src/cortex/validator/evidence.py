"""Durable, authenticated peer observations and signed dissent evidence."""

import sqlite3
from hashlib import sha256

from fastapi import FastAPI, HTTPException
from fastapi.responses import JSONResponse, Response

from cortex.protocol import Bundle, ProtocolError
from cortex.protocol.consensus import Dissent, RootStatement


class EvidenceStore:
    def __init__(self, connection: sqlite3.Connection):
        self.connection = connection
        connection.executescript("""
            CREATE TABLE IF NOT EXISTS consensus_roots (
                epoch TEXT NOT NULL, hotkey BLOB NOT NULL, root BLOB NOT NULL,
                statement BLOB NOT NULL, local INTEGER NOT NULL,
                PRIMARY KEY(epoch, hotkey, root)
            );
            CREATE TABLE IF NOT EXISTS consensus_bundles (
                digest BLOB PRIMARY KEY, epoch TEXT NOT NULL, root BLOB NOT NULL,
                encoded BLOB NOT NULL
            );
            CREATE TABLE IF NOT EXISTS consensus_dissents (
                body_digest BLOB PRIMARY KEY, encoded BLOB NOT NULL
            );
            CREATE TABLE IF NOT EXISTS consensus_watermarks (
                name TEXT PRIMARY KEY, value TEXT NOT NULL
            );
        """)

    def watermark(self, name: str, value: int) -> None:
        row = self.connection.execute(
            "SELECT value FROM consensus_watermarks WHERE name=?", (name,)
        ).fetchone()
        if row and int(row[0]) > value:
            raise ProtocolError("consensus rollback refused")
        self.connection.execute(
            "INSERT INTO consensus_watermarks VALUES (?,?) "
            "ON CONFLICT(name) DO UPDATE SET value=excluded.value",
            (name, str(value)),
        )

    def root(self, statement: RootStatement, *, local: bool = False) -> bool:
        statement.verify()
        epoch = f"{statement.epoch:020d}"
        conflicts = self.connection.execute(
            "SELECT 1 FROM consensus_roots WHERE epoch=? AND hotkey=? AND root<>?",
            (epoch, statement.hotkey, statement.merkle_root),
        ).fetchone()
        if local and conflicts:
            return False
        self.connection.execute(
            "INSERT INTO consensus_roots VALUES (?,?,?,?,?) "
            "ON CONFLICT(epoch,hotkey,root) DO UPDATE SET local=max(local,excluded.local)",
            (epoch, statement.hotkey, statement.merkle_root, statement.encode(), int(local)),
        )
        return not conflicts

    def local_root(self, epoch: int) -> RootStatement | None:
        row = self.connection.execute(
            "SELECT statement FROM consensus_roots WHERE epoch=? AND local=1", (f"{epoch:020d}",)
        ).fetchone()
        return RootStatement.decode(row[0]) if row else None

    def bundle(self, bundle: Bundle) -> None:
        encoded = bundle.encode()
        self.connection.execute(
            "INSERT OR IGNORE INTO consensus_bundles VALUES (?,?,?,?)",
            (
                sha256(encoded).digest(),
                f"{bundle.body.epoch:020d}",
                bundle.body.merkle_root,
                encoded,
            ),
        )

    def dissent(self, statement: Dissent) -> None:
        statement.verify()
        self.connection.execute(
            "INSERT OR IGNORE INTO consensus_dissents VALUES (?,?)",
            (sha256(statement.payload()).digest(), statement.encode()),
        )

    def dissents(self) -> list[Dissent]:
        return [
            Dissent.decode(row[0])
            for row in self.connection.execute(
                "SELECT encoded FROM consensus_dissents ORDER BY rowid"
            ).fetchall()
        ]


def peer_app(store: EvidenceStore) -> FastAPI:
    app = FastAPI(title="Cortex validator evidence")

    @app.get("/v1/consensus/root/{epoch}")
    async def root(epoch: int):
        statement = store.local_root(epoch)
        if statement is None:
            raise HTTPException(404, "root not observed")
        return statement.to_json()

    @app.get("/v1/bundle/root/{root}")
    async def bundle(root: str):
        try:
            raw = bytes.fromhex(root)
        except ValueError:
            raise HTTPException(400, "invalid root") from None
        if len(raw) != 32:
            raise HTTPException(400, "invalid root")
        row = store.connection.execute(
            "SELECT encoded FROM consensus_bundles WHERE root=? ORDER BY rowid DESC LIMIT 1",
            (raw,),
        ).fetchone()
        if row is None:
            raise HTTPException(404, "bundle not observed")
        return Response(row[0], media_type="application/octet-stream")

    @app.get("/v1/dissent/{epoch}")
    async def dissent(epoch: int):
        return {
            "dissents": [item.encode().hex() for item in store.dissents() if item.epoch == epoch]
        }

    @app.post("/v1/attest/nonce")
    @app.post("/v1/attest/submit")
    async def unattested():
        return JSONResponse(
            {"error": "DCAP verifier unavailable", "verified": False}, status_code=503
        )

    return app
