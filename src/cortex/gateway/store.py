"""SQLite durability for raw leaves and immutable epoch seals."""

import sqlite3
import threading
from collections.abc import Callable
from contextlib import contextmanager
from dataclasses import dataclass
from hashlib import sha256
from pathlib import Path
from uuid import uuid4

from cortex.errors import ServiceError
from cortex.protocol import Bundle, Leaf, NoScore, ProtocolError, Score
from cortex.protocol.scale import Reader, uint
from cortex.state import secure_sqlite_path


class RawWeightConflict(ServiceError):
    def __init__(self, original: dict):
        super().__init__(409, "raw weight already stored")
        self.original = original


@dataclass(frozen=True)
class StoredBundle:
    epoch: int
    encoded: bytes
    sealed_at: str


def _epoch(epoch: int) -> str:
    uint(epoch, 8)
    # TEXT preserves the full wire u64 range and sorts in numeric order.
    return f"{epoch:020d}"


def _leaf(encoded: bytes) -> Leaf:
    reader = Reader(encoded)
    leaf = Leaf.read(reader)
    reader.finish()
    return leaf


def _ack(row_id: str, leaf: Leaf, superseded: bool = False) -> dict:
    value = dict(
        id=row_id,
        challenge_id=leaf.challenge_id.decode(),
        epoch=leaf.epoch,
        miner_hotkey=leaf.miner_hotkey.hex(),
    )
    if isinstance(leaf.score, Score):
        value.update(kind="score", score=leaf.score.value)
    else:
        value.update(kind="no_score", absence_reason=str(int(leaf.score.reason)))
    if superseded:
        value["superseded"] = True
    return value


class GatewayStore:
    def __init__(self, path: Path | str):
        path = secure_sqlite_path(path)
        self._connection = sqlite3.connect(path, isolation_level=None, check_same_thread=False)
        self._connection.execute("PRAGMA journal_mode=WAL")
        self._connection.execute("PRAGMA synchronous=FULL")
        self._connection.execute("PRAGMA busy_timeout=5000")
        self._lock = threading.RLock()
        self._connection.executescript("""
            CREATE TABLE IF NOT EXISTS gateway_metadata (
                name TEXT PRIMARY KEY, value TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS gateway_raw_leaves (
                challenge_id BLOB NOT NULL, epoch TEXT NOT NULL, miner_hotkey BLOB NOT NULL,
                row_id TEXT NOT NULL UNIQUE, payload_digest BLOB NOT NULL, leaf_scale BLOB NOT NULL,
                PRIMARY KEY(challenge_id, epoch, miner_hotkey)
            );
            CREATE INDEX IF NOT EXISTS gateway_raw_epoch ON gateway_raw_leaves(epoch);
            CREATE TABLE IF NOT EXISTS gateway_bundles (
                epoch TEXT PRIMARY KEY, bundle_scale BLOB NOT NULL, sealed_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS gateway_bundle_roots (
                root BLOB PRIMARY KEY, epoch TEXT NOT NULL
            );
        """)
        for epoch, encoded in self._connection.execute(
            "SELECT epoch,bundle_scale FROM gateway_bundles "
            "WHERE epoch NOT IN (SELECT epoch FROM gateway_bundle_roots)"
        ).fetchall():
            try:
                root = Bundle.decode(encoded).body.merkle_root
            except ProtocolError:
                continue
            self._connection.execute(
                "INSERT OR IGNORE INTO gateway_bundle_roots VALUES (?,?)", (root, epoch)
            )

    @contextmanager
    def _transaction(self):
        with self._lock:
            try:
                self._connection.execute("BEGIN IMMEDIATE")
                yield self._connection
                self._connection.execute("COMMIT")
            except BaseException:
                if self._connection.in_transaction:
                    self._connection.execute("ROLLBACK")
                raise

    def bind_netuid(self, netuid: int) -> None:
        uint(netuid, 2)
        with self._transaction() as connection:
            connection.execute(
                "INSERT OR IGNORE INTO gateway_metadata VALUES ('netuid', ?)", (str(netuid),)
            )
            row = connection.execute(
                "SELECT value FROM gateway_metadata WHERE name='netuid'"
            ).fetchone()
            if row[0] != str(netuid):
                raise ServiceError(503, "gateway database belongs to another subnet")

    def trust_versions(self, challenges: int, measurements: int) -> None:
        with self._transaction() as connection:
            for name, value in (
                ("challenges_version", challenges),
                ("measurements_version", measurements),
            ):
                row = connection.execute(
                    "SELECT value FROM gateway_metadata WHERE name=?", (name,)
                ).fetchone()
                if row is not None and value < int(row[0]):
                    raise ServiceError(503, "gateway trust version rollback")
                connection.execute(
                    "INSERT INTO gateway_metadata VALUES (?,?) "
                    "ON CONFLICT(name) DO UPDATE SET value=excluded.value",
                    (name, str(value)),
                )

    def put_leaf(self, leaf: Leaf) -> dict:
        encoded = leaf.encode()
        digest = sha256(leaf.payload()).digest()
        identity = (leaf.challenge_id, _epoch(leaf.epoch), leaf.miner_hotkey)
        with self._transaction() as connection:
            previous = connection.execute(
                "SELECT row_id, payload_digest, leaf_scale FROM gateway_raw_leaves "
                "WHERE challenge_id=? AND epoch=? AND miner_hotkey=?",
                identity,
            ).fetchone()
            if previous:
                original = _leaf(previous[2])
                protected = (
                    isinstance(original.score, Score)
                    and original.score.value > 0
                    and (
                        isinstance(leaf.score, NoScore)
                        or isinstance(leaf.score, Score)
                        and leaf.score.value == 0
                    )
                )
                if previous[1] == digest or protected:
                    raise RawWeightConflict(_ack(previous[0], original))
            row_id = str(uuid4())
            connection.execute(
                "INSERT INTO gateway_raw_leaves VALUES (?, ?, ?, ?, ?, ?) "
                "ON CONFLICT(challenge_id, epoch, miner_hotkey) DO UPDATE SET "
                "row_id=excluded.row_id, payload_digest=excluded.payload_digest, "
                "leaf_scale=excluded.leaf_scale",
                (*identity, row_id, digest, encoded),
            )
        return _ack(row_id, leaf, previous is not None)

    def replace_challenge_leaves(
        self, challenge_id: bytes, epoch: int, leaves: tuple[Leaf, ...]
    ) -> tuple[dict, ...]:
        encoded = []
        hotkeys = set()
        for leaf in sorted(leaves, key=lambda item: item.miner_hotkey):
            if leaf.challenge_id != challenge_id or leaf.epoch != epoch:
                raise ProtocolError("challenge snapshot identity mismatch")
            if leaf.miner_hotkey in hotkeys:
                raise ProtocolError("duplicate challenge snapshot participant")
            hotkeys.add(leaf.miner_hotkey)
            raw = leaf.encode()
            encoded.append((leaf, raw, sha256(leaf.payload()).digest()))

        epoch_key = _epoch(epoch)
        with self._transaction() as connection:
            if connection.execute(
                "SELECT 1 FROM gateway_bundles WHERE epoch=?", (epoch_key,)
            ).fetchone():
                raise ServiceError(409, "sealed epoch is immutable")
            existing = connection.execute(
                "SELECT row_id,leaf_scale FROM gateway_raw_leaves "
                "WHERE challenge_id=? AND epoch=? ORDER BY miner_hotkey",
                (challenge_id, epoch_key),
            ).fetchall()
            if len(existing) == len(encoded) and all(
                row[1] == item[1] for row, item in zip(existing, encoded, strict=True)
            ):
                return tuple(
                    _ack(row[0], item[0]) for row, item in zip(existing, encoded, strict=True)
                )
            connection.execute(
                "DELETE FROM gateway_raw_leaves WHERE challenge_id=? AND epoch=?",
                (challenge_id, epoch_key),
            )
            result = []
            for leaf, raw, digest in encoded:
                row_id = str(uuid4())
                connection.execute(
                    "INSERT INTO gateway_raw_leaves VALUES (?, ?, ?, ?, ?, ?)",
                    (challenge_id, epoch_key, leaf.miner_hotkey, row_id, digest, raw),
                )
                result.append(_ack(row_id, leaf))
        return tuple(result)

    def leaves(self, epoch: int) -> tuple[Leaf, ...]:
        with self._lock:
            rows = self._connection.execute(
                "SELECT leaf_scale FROM gateway_raw_leaves WHERE epoch=?", (_epoch(epoch),)
            ).fetchall()
        return tuple(_leaf(row[0]) for row in rows)

    def bundle(self, epoch: int) -> StoredBundle | None:
        with self._lock:
            row = self._connection.execute(
                "SELECT bundle_scale, sealed_at FROM gateway_bundles WHERE epoch=?",
                (_epoch(epoch),),
            ).fetchone()
        return None if row is None else StoredBundle(epoch, row[0], row[1])

    def latest(self) -> StoredBundle | None:
        with self._lock:
            row = self._connection.execute(
                "SELECT epoch, bundle_scale, sealed_at FROM gateway_bundles "
                "ORDER BY epoch DESC LIMIT 1"
            ).fetchone()
        return None if row is None else StoredBundle(int(row[0]), row[1], row[2])

    def bundle_by_root(self, root: bytes) -> StoredBundle | None:
        with self._lock:
            row = self._connection.execute(
                "SELECT b.epoch,b.bundle_scale,b.sealed_at FROM gateway_bundle_roots r "
                "JOIN gateway_bundles b ON b.epoch=r.epoch WHERE r.root=?",
                (root,),
            ).fetchone()
        return None if row is None else StoredBundle(int(row[0]), row[1], row[2])

    def seal(
        self, epoch: int, build: Callable[[tuple[Leaf, ...]], Bundle], sealed_at: str
    ) -> StoredBundle:
        # Serialize selection, signing and insertion against competing emitters/sealers.
        with self._transaction() as connection:
            existing = self.bundle(epoch)
            if existing is not None:
                return existing
            bundle = build(self.leaves(epoch))
            if bundle.body.epoch != epoch:
                raise ServiceError(503, "seal epoch mismatch")
            encoded = bundle.encode()
            connection.execute(
                "INSERT INTO gateway_bundles VALUES (?, ?, ?)", (_epoch(epoch), encoded, sealed_at)
            )
            connection.execute(
                "INSERT OR IGNORE INTO gateway_bundle_roots VALUES (?,?)",
                (bundle.body.merkle_root, _epoch(epoch)),
            )
        return StoredBundle(epoch, encoded, sealed_at)

    def close(self) -> None:
        with self._lock:
            self._connection.close()
