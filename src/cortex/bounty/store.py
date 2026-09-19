"""Transactional SQLite persistence for Bounty intake, replay protection and quotas."""

import hashlib
import hmac
import sqlite3
import threading
from contextlib import contextmanager
from pathlib import Path

from cortex.state import secure_sqlite_path


class StoreError(Exception):
    def __init__(self, status: int, message: str):
        super().__init__(message)
        self.status = status


def normalize_text(text: str) -> str:
    # ASCII lowering preserves the existing Rust fingerprint contract.
    return " ".join(text.split()).translate(
        str.maketrans("ABCDEFGHIJKLMNOPQRSTUVWXYZ", "abcdefghijklmnopqrstuvwxyz")
    )


def report_fingerprint(title: str, body: str) -> str:
    return hashlib.sha256(
        b"base-bounty-report-v1"
        + normalize_text(title).encode()
        + b"\xff"
        + normalize_text(body).encode()
    ).hexdigest()


def session_token(secret: bytes, session_id: str, account: str, hotkey: str) -> str:
    payload = b"base-bounty-session-v1\x00" + b"\x00".join(
        value.encode() for value in (session_id, account, hotkey)
    )
    return hmac.new(secret, payload, hashlib.sha256).hexdigest()


class BountyStore:
    """Writes use BEGIN IMMEDIATE, including quota and duplicate checks."""

    def __init__(self, path: str | Path):
        self.path = secure_sqlite_path(path)
        self._lock = threading.RLock()
        self._db = sqlite3.connect(
            str(self.path), check_same_thread=False, isolation_level=None, timeout=10
        )
        self._db.row_factory = sqlite3.Row
        self._db.execute("PRAGMA journal_mode=WAL")
        self._db.execute("PRAGMA synchronous=FULL")
        self._db.execute("PRAGMA foreign_keys=ON")
        self._db.executescript("""
            CREATE TABLE IF NOT EXISTS bounty_nonces (nonce TEXT PRIMARY KEY);
            CREATE TABLE IF NOT EXISTS bounty_pair_grants (
                account_id TEXT NOT NULL,
                miner_hotkey TEXT NOT NULL,
                expires_at INTEGER NOT NULL,
                granted_at INTEGER NOT NULL,
                PRIMARY KEY (account_id, miner_hotkey)
            );
            CREATE TABLE IF NOT EXISTS bounty_ids (id INTEGER PRIMARY KEY AUTOINCREMENT);
            CREATE TABLE IF NOT EXISTS bounty_sessions (
                session_id TEXT PRIMARY KEY, token_hash TEXT UNIQUE NOT NULL,
                account_id TEXT NOT NULL, miner_hotkey TEXT NOT NULL, bound_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS bounty_pairings (
                account_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL REFERENCES bounty_sessions(session_id)
            );
            CREATE TABLE IF NOT EXISTS bounty_reports (
                id TEXT PRIMARY KEY, miner_hotkey TEXT NOT NULL, account_id TEXT NOT NULL,
                title TEXT NOT NULL, body TEXT NOT NULL, repro_steps TEXT NOT NULL,
                fingerprint TEXT NOT NULL, state TEXT NOT NULL, adjudication TEXT,
                severity TEXT, duplicate_of TEXT REFERENCES bounty_reports(id),
                created_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS bounty_reports_miner
                ON bounty_reports(miner_hotkey, state, created_at);
            CREATE INDEX IF NOT EXISTS bounty_reports_fingerprint
                ON bounty_reports(fingerprint, id);
        """)

    @contextmanager
    def _transaction(self):
        with self._lock:
            self._db.execute("BEGIN IMMEDIATE")
            try:
                yield self._db
                self._db.execute("COMMIT")
            except BaseException:
                self._db.execute("ROLLBACK")
                raise

    def _next_id(self, prefix: str) -> str:
        cursor = self._db.execute("INSERT INTO bounty_ids DEFAULT VALUES")
        return f"{prefix}_{cursor.lastrowid:016x}"

    def grant_pair(self, account: str, hotkey: str, *, expires_at: int, now: int) -> dict:
        if expires_at <= now:
            raise StoreError(400, "pair grant must expire in the future")
        with self._transaction() as db:
            db.execute("DELETE FROM bounty_pair_grants WHERE expires_at<=?", (now,))
            db.execute(
                "INSERT INTO bounty_pair_grants "
                "(account_id, miner_hotkey, expires_at, granted_at) VALUES (?,?,?,?) "
                "ON CONFLICT(account_id, miner_hotkey) DO UPDATE SET "
                "expires_at=excluded.expires_at, granted_at=excluded.granted_at",
                (account, hotkey, expires_at, now),
            )
            return {
                "account_id": account,
                "miner_hotkey": hotkey,
                "expires_at": expires_at,
            }

    def bind_pair(self, account: str, hotkey: str, nonce: str, now: int, secret: bytes) -> dict:
        with self._transaction() as db:
            if db.execute("SELECT 1 FROM bounty_nonces WHERE nonce=?", (nonce,)).fetchone():
                raise StoreError(409, "nonce reused")
            consumed = db.execute(
                "DELETE FROM bounty_pair_grants "
                "WHERE account_id=? AND miner_hotkey=? AND expires_at>?",
                (account, hotkey, now),
            )
            if consumed.rowcount != 1:
                raise StoreError(403, "pairing not authorized by account operator")
            db.execute("INSERT INTO bounty_nonces VALUES (?)", (nonce,))
            session_id = self._next_id("bs")
            token = session_token(secret, session_id, account, hotkey)
            db.execute(
                "INSERT INTO bounty_sessions VALUES (?,?,?,?,?)",
                (session_id, hashlib.sha256(token.encode()).hexdigest(), account, hotkey, now),
            )
            db.execute(
                "INSERT INTO bounty_pairings VALUES (?,?) ON CONFLICT(account_id) "
                "DO UPDATE SET session_id=excluded.session_id",
                (account, session_id),
            )
            return {
                "session": token,
                "session_id": session_id,
                "account_id": account,
                "miner_hotkey": hotkey,
            }

    def lookup_session(self, token: str, secret: bytes) -> dict:
        with self._lock:
            row = self._db.execute(
                "SELECT s.* FROM bounty_sessions s "
                "JOIN bounty_pairings p ON p.account_id=s.account_id AND p.session_id=s.session_id "
                "WHERE s.token_hash=?",
                (hashlib.sha256(token.encode()).hexdigest(),),
            ).fetchone()
            if row is None or not hmac.compare_digest(
                token,
                session_token(secret, row["session_id"], row["account_id"], row["miner_hotkey"]),
            ):
                raise StoreError(401, "invalid_session")
            return {
                key: row[key] for key in ("account_id", "miner_hotkey", "session_id", "bound_at")
            }

    @staticmethod
    def _check_report_admission(db: sqlite3.Connection, hotkey: str, now: int) -> None:
        pending = db.execute(
            "SELECT count(*) FROM bounty_reports WHERE miner_hotkey=? AND state='pending'",
            (hotkey,),
        ).fetchone()[0]
        if pending >= 5:
            raise StoreError(429, "5 reports already awaiting adjudication for this hotkey (max 5)")
        last = db.execute(
            "SELECT max(created_at) FROM bounty_reports WHERE miner_hotkey=?", (hotkey,)
        ).fetchone()[0]
        if last is not None and now - last < 60:
            raise StoreError(429, "one report per 60s per hotkey")

    def check_report_admission(self, pairing: dict, now: int) -> None:
        with self._lock:
            self._check_report_admission(self._db, pairing["miner_hotkey"], now)

    def insert_report(self, pairing: dict, title: str, body: str, repro: str, now: int) -> dict:
        fingerprint = report_fingerprint(title, body)
        hotkey = pairing["miner_hotkey"]
        with self._transaction() as db:
            self._check_report_admission(db, hotkey, now)
            original = db.execute(
                "SELECT id FROM bounty_reports WHERE fingerprint=? ORDER BY id LIMIT 1",
                (fingerprint,),
            ).fetchone()
            report_id = self._next_id("by")
            state = "duplicate" if original else "pending"
            db.execute(
                "INSERT INTO bounty_reports VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
                (
                    report_id,
                    hotkey,
                    pairing["account_id"],
                    title,
                    body,
                    repro,
                    fingerprint,
                    state,
                    "duplicate" if original else None,
                    None,
                    original["id"] if original else None,
                    now,
                ),
            )
            return self.get_report(report_id)

    def get_report(self, report_id: str) -> dict:
        with self._lock:
            row = self._db.execute(
                "SELECT * FROM bounty_reports WHERE id=?", (report_id,)
            ).fetchone()
            if row is None:
                raise StoreError(404, "not_found")
            return {**dict(row), "champion_verdict": None}

    def list_reports(self) -> list[dict]:
        with self._lock:
            return [
                {**dict(row), "champion_verdict": None}
                for row in self._db.execute("SELECT * FROM bounty_reports ORDER BY id DESC")
            ]

    def adjudicate(
        self, report_id: str, verdict: str, severity: str | None, duplicate_of: str | None
    ) -> dict:
        if verdict == "valid" and severity is None:
            raise StoreError(409, "severity required for valid verdict")
        if verdict != "valid" and severity is not None:
            raise StoreError(409, "severity is only valid for a valid verdict")
        with self._transaction() as db:
            row = self.get_report(report_id)
            if row["state"] != "pending" and row["adjudication"] != "duplicate":
                raise StoreError(409, "already adjudicated")
            if verdict == "duplicate":
                if not duplicate_of:
                    raise StoreError(409, "duplicate_of required")
                if duplicate_of == report_id:
                    raise StoreError(409, "report cannot duplicate itself")
                self.get_report(duplicate_of)
            elif duplicate_of is not None:
                raise StoreError(409, "duplicate_of is only valid for duplicate verdicts")
            db.execute(
                "UPDATE bounty_reports SET state=?, adjudication=?, severity=?, duplicate_of=? "
                "WHERE id=?",
                (
                    verdict,
                    verdict,
                    severity if verdict == "valid" else None,
                    duplicate_of,
                    report_id,
                ),
            )
            return self.get_report(report_id)

    def close(self) -> None:
        with self._lock:
            self._db.close()
