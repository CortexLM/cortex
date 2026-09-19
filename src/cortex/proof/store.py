"""Durable Proof state with transactional nonce and worker claims."""

from __future__ import annotations

import json
import sqlite3
import threading
from contextlib import contextmanager
from pathlib import Path

from cortex.errors import ServiceError
from cortex.proof.artifacts import valid_env_name
from cortex.proof.models import Submission, Topic, canonical_json, digest
from cortex.state import secure_sqlite_path

SCHEMA = """
CREATE TABLE IF NOT EXISTS proof_nonces (
    hotkey TEXT NOT NULL, nonce TEXT NOT NULL, payload_digest TEXT NOT NULL,
    job_id TEXT NOT NULL, epoch INTEGER NOT NULL, PRIMARY KEY (hotkey, nonce)
);
CREATE TABLE IF NOT EXISTS proof_topics (
    id TEXT NOT NULL, revision INTEGER NOT NULL, digest TEXT NOT NULL UNIQUE,
    document TEXT NOT NULL, PRIMARY KEY (id, revision)
);
CREATE TABLE IF NOT EXISTS proof_holdouts (
    commitment TEXT PRIMARY KEY, content_hashes TEXT NOT NULL, dataset_ids TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS proof_topic_publications (
    digest TEXT PRIMARY KEY, epoch INTEGER NOT NULL,
    FOREIGN KEY (digest) REFERENCES proof_topics(digest)
);
CREATE TABLE IF NOT EXISTS proof_evidence (
    digest TEXT PRIMARY KEY, topic_id TEXT NOT NULL, report TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS proof_jobs (
    id TEXT PRIMARY KEY, topic_digest TEXT NOT NULL, body TEXT NOT NULL,
    env_names TEXT NOT NULL, epoch INTEGER NOT NULL, state TEXT NOT NULL,
    lease_until INTEGER NOT NULL DEFAULT 0, owner TEXT, error TEXT,
    FOREIGN KEY (topic_digest) REFERENCES proof_topics(digest)
);
CREATE INDEX IF NOT EXISTS proof_job_state ON proof_jobs(state, lease_until);
CREATE TABLE IF NOT EXISTS proof_submissions (
    id TEXT PRIMARY KEY, hotkey TEXT NOT NULL, topic_id TEXT NOT NULL,
    topic_digest TEXT NOT NULL, epoch INTEGER NOT NULL, status TEXT NOT NULL,
    body TEXT NOT NULL, report TEXT, reason TEXT,
    FOREIGN KEY (topic_digest) REFERENCES proof_topics(digest)
);
CREATE INDEX IF NOT EXISTS proof_submission_epoch ON proof_submissions(epoch, topic_id);
"""


class ProofStore:
    def __init__(self, path: str | Path):
        path = secure_sqlite_path(path)
        self._connection = sqlite3.connect(
            str(path), timeout=10, check_same_thread=False, isolation_level=None
        )
        self._connection.row_factory = sqlite3.Row
        self._connection.execute("PRAGMA journal_mode=WAL")
        self._connection.execute("PRAGMA synchronous=FULL")
        self._connection.execute("PRAGMA foreign_keys=ON")
        self._connection.executescript(SCHEMA)
        self._migrate_nonce_bindings()
        self._lock = threading.RLock()

    def _migrate_nonce_bindings(self) -> None:
        """Add recovery bindings to databases created before receipt lookup existed."""
        columns = {row[1] for row in self._connection.execute("PRAGMA table_info(proof_nonces)")}
        for name, kind in (
            ("payload_digest", "TEXT"),
            ("job_id", "TEXT"),
            ("epoch", "INTEGER"),
        ):
            if name not in columns:
                self._connection.execute(f"ALTER TABLE proof_nonces ADD COLUMN {name} {kind}")

    def __enter__(self):
        return self

    def __exit__(self, *args):
        self.close()

    def close(self):
        self._connection.close()

    @contextmanager
    def transaction(self):
        with self._lock:
            self._connection.execute("BEGIN IMMEDIATE")
            try:
                yield self._connection
                self._connection.commit()
            except BaseException:
                self._connection.rollback()
                raise

    def reserve_nonce(
        self,
        hotkey: str,
        nonce: str,
        payload_digest: str,
        job_id: str,
        *,
        epoch: int,
    ) -> None:
        try:
            with self.transaction() as connection:
                connection.execute(
                    "INSERT INTO proof_nonces "
                    "(hotkey,nonce,payload_digest,job_id,epoch) VALUES (?,?,?,?,?)",
                    (hotkey, nonce, payload_digest, job_id, epoch),
                )
        except sqlite3.IntegrityError:
            raise ServiceError(401, "submit_nonce reused") from None

    def lookup_submission(self, hotkey: str, nonce: str, payload_digest: str) -> dict | None:
        with self._lock:
            binding = self._connection.execute(
                "SELECT job_id,epoch FROM proof_nonces "
                "WHERE hotkey=? AND nonce=? AND payload_digest=?",
                (hotkey, nonce, payload_digest),
            ).fetchone()
            if binding is None or binding["job_id"] is None or binding["epoch"] is None:
                return None
            submission = self._connection.execute(
                "SELECT * FROM proof_submissions WHERE id=? AND hotkey=?",
                (binding["job_id"], hotkey),
            ).fetchone()
            job = self._connection.execute(
                "SELECT state,error,body FROM proof_jobs WHERE id=?",
                (binding["job_id"],),
            ).fetchone()
        if submission is not None:
            result = self._submission(submission)
            if result["status"] == "queued":
                result["status"] = "pending"
            return result
        result = {
            "id": binding["job_id"],
            "hotkey": hotkey,
            "epoch": binding["epoch"],
            "status": "failed" if job is None else "pending",
            "reason": "submission intake did not complete" if job is None else None,
        }
        if job is not None:
            result["body"] = json.loads(job["body"])
            if job["state"] == "failed":
                result["status"] = "failed"
                result["reason"] = job["error"] or "evaluation failed"
            elif job["state"] == "complete":
                result["status"] = "failed"
                result["reason"] = "submission outcome unavailable"
        return result

    def publish(self, topic: Topic, *, epoch: int = 0) -> None:
        with self.transaction() as connection:
            row = connection.execute(
                "SELECT revision,document FROM proof_topics WHERE id=? "
                "ORDER BY revision DESC LIMIT 1",
                (topic.id,),
            ).fetchone()
            if row:
                previous = Topic.model_validate_json(row["document"])
                if topic.revision != previous.revision + 1:
                    raise ServiceError(409, "topic revision must increase by one")
                # A rule revision cannot silently replace the experiment or sealed reference.
                for name in (
                    "statement",
                    "metric",
                    "baseline",
                    "holdout_commitment",
                    "flops_budget",
                    "wall_budget_s",
                    "eval_image_digest",
                    "inference_offer_commitment",
                    "eval_executor",
                ):
                    if previous.status != "draft" and getattr(previous, name) != getattr(
                        topic, name
                    ):
                        raise ServiceError(409, "open topic experiment is immutable; use a new id")
                if previous.status == "closed" and topic.status != "closed":
                    raise ServiceError(409, "closed topic cannot reopen")
            elif topic.revision != 1:
                raise ServiceError(409, "first topic revision must be one")
            connection.execute(
                "INSERT INTO proof_topics VALUES (?,?,?,?)",
                (
                    topic.id,
                    topic.revision,
                    topic.content_digest(),
                    topic.model_dump_json(),
                ),
            )
            connection.execute(
                "INSERT INTO proof_topic_publications VALUES (?,?)", (topic.content_digest(), epoch)
            )

    def topic(self, topic_id: str) -> Topic | None:
        with self._lock:
            row = self._connection.execute(
                "SELECT document FROM proof_topics WHERE id=? ORDER BY revision DESC LIMIT 1",
                (topic_id,),
            ).fetchone()
        return Topic.model_validate_json(row[0]) if row else None

    def topic_by_digest(self, value: str) -> Topic:
        with self._lock:
            row = self._connection.execute(
                "SELECT document FROM proof_topics WHERE digest=?",
                (value,),
            ).fetchone()
        if not row:
            raise ServiceError(503, "stored topic unavailable")
        return Topic.model_validate_json(row[0])

    def topics(self) -> list[Topic]:
        with self._lock:
            rows = self._connection.execute("""
                SELECT document FROM proof_topics t
                WHERE revision=(SELECT max(revision) FROM proof_topics WHERE id=t.id)
                ORDER BY id
            """).fetchall()
        return [Topic.model_validate_json(row[0]) for row in rows]

    def topics_at(self, epoch: int) -> list[Topic]:
        with self._lock:
            rows = self._connection.execute(
                "SELECT t.document FROM proof_topics t "
                "LEFT JOIN proof_topic_publications p ON p.digest=t.digest "
                "WHERE COALESCE(p.epoch,0)<=? "
                "AND json_extract(t.document,'$.valid_from_epoch')<=? ORDER BY t.id,t.revision",
                (epoch, epoch),
            ).fetchall()
        selected = {}
        for row in rows:
            topic = Topic.model_validate_json(row[0])
            selected[topic.id] = topic
        return list(selected.values())

    def holdouts(self, commitment: str) -> tuple[set[str], set[str]]:
        with self._lock:
            row = self._connection.execute(
                "SELECT content_hashes,dataset_ids FROM proof_holdouts WHERE commitment=?",
                (commitment,),
            ).fetchone()
        if not row:
            raise ServiceError(503, "holdout evidence unavailable")
        return set(json.loads(row[0])), set(json.loads(row[1]))

    def register_holdouts(self, content_hashes: list[str], dataset_ids: list[str]) -> str:
        if not content_hashes and not dataset_ids:
            raise ServiceError(400, "holdout evidence required")
        hashes, datasets = sorted(set(content_hashes)), sorted(set(dataset_ids))
        commitment = digest({"content_hashes": hashes, "dataset_ids": datasets})
        with self.transaction() as connection:
            connection.execute(
                "INSERT OR IGNORE INTO proof_holdouts VALUES (?,?,?)",
                (
                    commitment,
                    canonical_json(hashes).decode(),
                    canonical_json(datasets).decode(),
                ),
            )
        return commitment

    def register_evidence(self, topic_id: str, report: dict) -> str:
        commitment = digest(report)
        with self.transaction() as connection:
            connection.execute(
                "INSERT OR IGNORE INTO proof_evidence VALUES (?,?,?)",
                (
                    commitment,
                    topic_id,
                    canonical_json(report).decode(),
                ),
            )
        return commitment

    def evidence(self, commitment: str, topic_id: str) -> dict:
        with self._lock:
            row = self._connection.execute(
                "SELECT report FROM proof_evidence WHERE digest=? AND topic_id=?",
                (commitment, topic_id),
            ).fetchone()
        if not row:
            raise ServiceError(503, "baseline execution evidence unavailable")
        return json.loads(row[0])

    def enqueue(
        self,
        job_id: str,
        topic: Topic,
        submission: Submission,
        epoch: int,
        env_names: list[str],
        *,
        max_pending: int = 4,
    ) -> None:
        with self.transaction() as connection:
            pending = connection.execute(
                "SELECT count(*) FROM proof_jobs WHERE state IN ('queued','running') "
                "AND json_extract(body,'$.miner_hotkey')=?",
                (submission.miner_hotkey,),
            ).fetchone()[0]
            if pending >= max_pending:
                raise ServiceError(429, "miner pending submission quota exceeded")
            connection.execute(
                "INSERT INTO proof_jobs (id,topic_digest,body,env_names,epoch,state) "
                "VALUES (?,?,?,?,?,?)",
                (
                    job_id,
                    topic.content_digest(),
                    submission.model_dump_json(),
                    json.dumps(sorted(env_names)),
                    epoch,
                    "queued",
                ),
            )

    @staticmethod
    def decode_env_names(value: str) -> list[str]:
        try:
            names = json.loads(value)
        except (TypeError, json.JSONDecodeError):
            raise ServiceError(503, "stored credential names are invalid") from None
        if (
            not isinstance(names, list)
            or len(names) > 8
            or any(not valid_env_name(name) for name in names)
            or len(names) != len(set(names))
        ):
            raise ServiceError(503, "stored credential names are invalid")
        return sorted(names)

    def active_vault_entries(self) -> dict[str, list[str]]:
        """Return every nonterminal owner of the shared Proof credential vault."""
        with self._lock:
            rows = list(
                self._connection.execute(
                    "SELECT id,env_names FROM proof_jobs WHERE state IN ('queued','running')"
                ).fetchall()
            )
            setup_exists = self._connection.execute(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name='proof_setup_jobs'"
            ).fetchone()
            if setup_exists is not None:
                rows.extend(
                    self._connection.execute(
                        "SELECT id,env_names FROM proof_setup_jobs WHERE state='pending'"
                    ).fetchall()
                )
        expected: dict[str, list[str]] = {}
        for row in rows:
            if row["id"] in expected:
                raise ServiceError(503, "active credential vault key collision")
            expected[row["id"]] = self.decode_env_names(row["env_names"])
        return expected

    def discard_unstarted(self, job_id: str, *, missing_ok: bool = False) -> None:
        """Remove only an intake row that cannot have been claimed by a worker."""
        with self.transaction() as connection:
            cursor = connection.execute(
                "DELETE FROM proof_jobs WHERE id=? AND state='queued' AND owner IS NULL "
                "AND NOT EXISTS (SELECT 1 FROM proof_submissions WHERE id=?)",
                (job_id, job_id),
            )
            if cursor.rowcount != 1 and not (missing_ok and cursor.rowcount == 0):
                raise ServiceError(503, "queued proof job cleanup failed")

    def protect_finalization(
        self, job_id: str, owner: str, *, now: int, lease_seconds: int
    ) -> None:
        """Confirm worker ownership and hold the lease across vault cleanup plus commit."""
        with self.transaction() as connection:
            cursor = connection.execute(
                "UPDATE proof_jobs SET lease_until=? WHERE id=? AND state='running' AND owner=?",
                (now + lease_seconds, job_id, owner),
            )
            if cursor.rowcount != 1:
                raise ServiceError(503, "job lease lost")

    def job_topic(self, job_id: str) -> Topic:
        with self._lock:
            row = self._connection.execute(
                "SELECT topic_digest FROM proof_jobs WHERE id=?",
                (job_id,),
            ).fetchone()
        if row is None:
            raise ServiceError(404, "job not found")
        return self.topic_by_digest(row[0])

    def claim(self, job_id: str, owner: str, now: int, lease_seconds: int) -> dict | None:
        with self.transaction() as connection:
            row = connection.execute("SELECT * FROM proof_jobs WHERE id=?", (job_id,)).fetchone()
            if not row or row["state"] in {"complete", "failed"}:
                return None
            if row["state"] == "running" and row["lease_until"] > now:
                return None
            connection.execute(
                "UPDATE proof_jobs SET state='running',lease_until=?,owner=? WHERE id=?",
                (now + lease_seconds, owner, job_id),
            )
        result = dict(row)
        result["body"] = json.loads(result["body"])
        result["env_names"] = self.decode_env_names(result["env_names"])
        return result

    def pending(self, now: int) -> list[str]:
        with self._lock:
            rows = self._connection.execute(
                "SELECT id FROM proof_jobs WHERE state='queued' "
                "OR (state='running' AND lease_until<=?) ORDER BY rowid",
                (now,),
            ).fetchall()
        return [row[0] for row in rows]

    def has_unfinished_jobs(self, epoch: int) -> bool:
        """A completed epoch cannot be sealed while accepted work may still score."""
        with self._lock:
            row = self._connection.execute(
                "SELECT 1 FROM proof_jobs WHERE epoch=? AND state IN ('queued','running') LIMIT 1",
                (epoch,),
            ).fetchone()
        return row is not None

    def record(
        self,
        job_id: str,
        topic: Topic,
        submission: Submission,
        epoch: int,
        status: str,
        report: dict | None = None,
        reason: str | None = None,
        *,
        owner: str | None = None,
    ) -> dict:
        with self.transaction() as connection:
            if owner is not None:
                row = connection.execute(
                    "SELECT owner FROM proof_jobs WHERE id=?", (job_id,)
                ).fetchone()
                if not row or row[0] != owner:
                    raise ServiceError(503, "job lease lost")
            connection.execute(
                "INSERT OR REPLACE INTO proof_submissions VALUES (?,?,?,?,?,?,?,?,?)",
                (
                    job_id,
                    submission.miner_hotkey,
                    topic.id,
                    topic.content_digest(),
                    epoch,
                    status,
                    submission.model_dump_json(),
                    canonical_json(report).decode() if report else None,
                    reason,
                ),
            )
            if status != "queued":
                connection.execute("UPDATE proof_jobs SET state='complete' WHERE id=?", (job_id,))
        result = self.submission(job_id)
        if result is None:
            raise ServiceError(503, "submission persistence failed")
        return result

    def fail(self, job_id: str, owner: str, reason: str) -> None:
        with self.transaction() as connection:
            cursor = connection.execute(
                "UPDATE proof_jobs SET state='failed',error=? WHERE id=? AND owner=?",
                (reason, job_id, owner),
            )
            if cursor.rowcount != 1:
                raise ServiceError(503, "job lease lost")

    def submission(self, job_id: str) -> dict | None:
        with self._lock:
            row = self._connection.execute(
                "SELECT * FROM proof_submissions WHERE id=?", (job_id,)
            ).fetchone()
        return self._submission(row) if row else None

    def submissions(self, epoch: int) -> list[dict]:
        with self._lock:
            rows = self._connection.execute(
                "SELECT * FROM proof_submissions WHERE epoch=? ORDER BY id", (epoch,)
            ).fetchall()
        return [self._submission(row) for row in rows]

    def previous_accepted(self, epoch: int) -> list[dict]:
        with self._lock:
            rows = self._connection.execute(
                "SELECT * FROM proof_submissions WHERE epoch<? AND status='accepted' ORDER BY id",
                (epoch,),
            ).fetchall()
        return [self._submission(row) for row in rows]

    @staticmethod
    def _submission(row: sqlite3.Row) -> dict:
        value = dict(row)
        value["body"] = json.loads(value["body"])
        value["report"] = json.loads(value["report"]) if value["report"] else None
        return value
