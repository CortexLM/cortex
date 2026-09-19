"""Durable research observations with an explicit owner approval boundary.

Miner observations are never promoted by a model. Approval binds the exact
content, visibility and evidence to an owner signature. Rule revisions use a
separate signed, append-only chain; learning cannot silently change scoring.
"""

from __future__ import annotations

import sqlite3
from collections.abc import Awaitable
from pathlib import Path
from typing import Literal, Protocol

from cryptography.exceptions import InvalidSignature
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey
from pydantic import Field

from cortex.state import secure_sqlite_path

from .errors import ToolRejected
from .models import Digest, Identifier, Rule, StrictModel, canonical_bytes, digest_of


class Observation(StrictModel):
    topic_id: Identifier
    content: str = Field(min_length=1, max_length=4096)
    evidence_digest: Digest
    source_artifact_digest: Digest | None = None
    visibility: Literal["public", "topic_private"] = "topic_private"


class KnowledgeApproval(StrictModel):
    observation_digest: Digest
    verification_report_digest: Digest
    owner_signature: str = Field(pattern=r"^[0-9a-f]{128}$")

    def signing_bytes(self) -> bytes:
        return b"cortex-knowledge-approval-v1\x00" + canonical_bytes(
            {
                "observation_digest": self.observation_digest,
                "verification_report_digest": self.verification_report_digest,
            }
        )


class RuleRevision(StrictModel):
    topic_id: Identifier
    revision: int = Field(ge=1)
    previous_digest: Digest | None = None
    rules: list[Rule] = Field(min_length=1, max_length=64)
    owner_signature: str = Field(pattern=r"^[0-9a-f]{128}$")

    def signing_bytes(self) -> bytes:
        data = self.model_dump(mode="json", exclude={"owner_signature"})
        return b"cortex-rule-revision-v1\x00" + canonical_bytes(data)


class KnowledgeAccess(Protocol):
    def read_verified(
        self, topic_id: str, *, limit: int = 8
    ) -> list[Observation] | Awaitable[list[Observation]]: ...

    def propose(self, observation: Observation) -> str | Awaitable[str]: ...


class KnowledgeStore:
    def __init__(self, path: str | Path, *, owner_public_key: bytes) -> None:
        self._owner = Ed25519PublicKey.from_public_bytes(owner_public_key)
        path = secure_sqlite_path(path)
        self._connection = sqlite3.connect(str(path), timeout=10)
        self._connection.execute("PRAGMA journal_mode=WAL")
        self._connection.executescript(
            """
            CREATE TABLE IF NOT EXISTS observations (
                digest TEXT PRIMARY KEY,
                topic_id TEXT NOT NULL,
                visibility TEXT NOT NULL,
                body TEXT NOT NULL,
                approval TEXT
            );
            CREATE TABLE IF NOT EXISTS rule_revisions (
                topic_id TEXT NOT NULL,
                revision INTEGER NOT NULL,
                digest TEXT NOT NULL UNIQUE,
                body TEXT NOT NULL,
                PRIMARY KEY (topic_id, revision)
            );
            """
        )

    def close(self) -> None:
        self._connection.close()

    def pending(self, *, limit: int = 32) -> list[Observation]:
        if not 1 <= limit <= 128:
            raise ValueError("invalid knowledge page limit")
        rows = self._connection.execute(
            "SELECT body FROM observations WHERE approval IS NULL ORDER BY rowid LIMIT ?", (limit,)
        ).fetchall()
        return [Observation.model_validate_json(row[0]) for row in rows]

    def propose(self, observation: Observation) -> str:
        digest = digest_of(observation)
        with self._connection:
            self._connection.execute(
                "INSERT OR IGNORE INTO observations VALUES (?, ?, ?, ?, NULL)",
                (
                    digest,
                    observation.topic_id,
                    observation.visibility,
                    observation.model_dump_json(),
                ),
            )
        return digest

    def approve(self, approval: KnowledgeApproval) -> None:
        self._verify(approval.owner_signature, approval.signing_bytes())
        with self._connection:
            result = self._connection.execute(
                "UPDATE observations SET approval = ? WHERE digest = ? AND approval IS NULL",
                (approval.model_dump_json(), approval.observation_digest),
            )
            if result.rowcount != 1:
                raise ToolRejected("observation missing or already approved")

    def approve_observation(self, observation: Observation, approval: KnowledgeApproval) -> str:
        """Verify an owner approval before atomically storing its observation."""

        digest = digest_of(observation)
        if digest != approval.observation_digest:
            raise ToolRejected("approval content mismatch")
        self._verify(approval.owner_signature, approval.signing_bytes())
        body = observation.model_dump_json()
        with self._connection:
            self._connection.execute(
                "INSERT OR IGNORE INTO observations VALUES (?, ?, ?, ?, NULL)",
                (digest, observation.topic_id, observation.visibility, body),
            )
            row = self._connection.execute(
                "SELECT topic_id, visibility, body, approval FROM observations WHERE digest = ?",
                (digest,),
            ).fetchone()
            if row != (observation.topic_id, observation.visibility, body, None):
                raise ToolRejected("observation missing, changed, or already approved")
            self._connection.execute(
                "UPDATE observations SET approval = ? WHERE digest = ?",
                (approval.model_dump_json(), digest),
            )
        return digest

    def read_verified(self, topic_id: str, *, limit: int = 8) -> list[Observation]:
        if not 1 <= limit <= 16:
            raise ValueError("knowledge limit must be between 1 and 16")
        rows = self._connection.execute(
            """SELECT body, approval, digest, topic_id, visibility FROM observations
            WHERE approval IS NOT NULL
            AND (visibility = 'public' OR topic_id = ?) ORDER BY digest LIMIT ?""",
            (topic_id, limit),
        ).fetchall()
        result: list[Observation] = []
        for body, approval_text, digest, stored_topic, stored_visibility in rows:
            observation = Observation.model_validate_json(body)
            approval = KnowledgeApproval.model_validate_json(approval_text)
            self._verify(approval.owner_signature, approval.signing_bytes())
            if (
                digest_of(observation) != digest
                or approval.observation_digest != digest
                or observation.topic_id != stored_topic
                or observation.visibility != stored_visibility
                or (observation.visibility != "public" and observation.topic_id != topic_id)
            ):
                raise ToolRejected("stored knowledge integrity check failed")
            result.append(observation)
        return result

    def publish_rules(self, revision: RuleRevision) -> str:
        self._verify(revision.owner_signature, revision.signing_bytes())
        if len({rule.id for rule in revision.rules}) != len(revision.rules):
            raise ToolRejected("duplicate rule id")
        digest = digest_of(revision)
        try:
            self._connection.execute("BEGIN IMMEDIATE")
            row = self._connection.execute(
                "SELECT revision, digest FROM rule_revisions WHERE topic_id = ? "
                "ORDER BY revision DESC LIMIT 1",
                (revision.topic_id,),
            ).fetchone()
            expected_revision, expected_previous = (row[0] + 1, row[1]) if row else (1, None)
            if (revision.revision, revision.previous_digest) != (
                expected_revision,
                expected_previous,
            ):
                raise ToolRejected("rule revision must extend the current signed revision")
            self._connection.execute(
                "INSERT INTO rule_revisions VALUES (?, ?, ?, ?)",
                (revision.topic_id, revision.revision, digest, revision.model_dump_json()),
            )
            self._connection.commit()
        except BaseException:
            self._connection.rollback()
            raise
        return digest

    def latest_rules(self, topic_id: str) -> RuleRevision | None:
        row = self._connection.execute(
            "SELECT body, digest FROM rule_revisions WHERE topic_id = ? "
            "ORDER BY revision DESC LIMIT 1",
            (topic_id,),
        ).fetchone()
        if row is None:
            return None
        revision = RuleRevision.model_validate_json(row[0])
        self._verify(revision.owner_signature, revision.signing_bytes())
        if digest_of(revision) != row[1]:
            raise ToolRejected("stored rule integrity check failed")
        return revision

    def _verify(self, signature: str, payload: bytes) -> None:
        try:
            self._owner.verify(bytes.fromhex(signature), payload)
        except (ValueError, InvalidSignature):
            raise ToolRejected("owner signature invalid") from None
