"""Fetch-only validator: independently verify a current seal before chain submission."""

import asyncio
import sqlite3
from collections.abc import Callable
from dataclasses import dataclass
from hashlib import sha256
from pathlib import Path
from typing import Protocol
from urllib.parse import urlsplit

import httpx

from cortex.errors import ServiceError
from cortex.http import decode_json
from cortex.protocol import Bundle, MetagraphRow, ProtocolError, TrustRoot
from cortex.protocol.bundle import bundle_digest, recompute, verify_inputs
from cortex.protocol.consensus import Dissent, DissentReason, RootStatement
from cortex.protocol.crypto import public_key
from cortex.protocol.models import encode_final_vector
from cortex.protocol.scale import fixed, uint

from .evidence import EvidenceStore


@dataclass(frozen=True)
class ChainSnapshot:
    block: int
    block_hash: bytes
    rows: tuple[MetagraphRow, ...]
    owner_hotkey: bytes
    validator_permits: frozenset[int]
    epoch: int | None = None


class Chain(Protocol):
    async def current_block(self) -> int: ...

    async def snapshot(self, block: int, netuid: int) -> ChainSnapshot: ...

    async def submit(
        self, netuid: int, vector: tuple[tuple[int, int], ...], version_key: int
    ) -> bool: ...


class SubmissionJournal:
    """Durable dedupe. A crash during dispatch leaves a pending reconciliation record."""

    def __init__(self, path: Path | str):
        self.connection = sqlite3.connect(path, isolation_level=None)
        self.connection.execute("PRAGMA journal_mode=WAL")
        self.connection.execute("PRAGMA synchronous=FULL")
        self.connection.execute(
            "CREATE TABLE IF NOT EXISTS weight_submissions ("
            "netuid INTEGER NOT NULL, epoch INTEGER NOT NULL, digest TEXT NOT NULL, "
            "state TEXT NOT NULL CHECK(state IN ('pending','submitted')), "
            "PRIMARY KEY(netuid, epoch))"
        )
        self.evidence = EvidenceStore(self.connection)

    def bind_netuid(self, netuid: int) -> None:
        self.connection.execute(
            "INSERT OR IGNORE INTO consensus_watermarks VALUES ('netuid',?)", (str(netuid),)
        )
        row = self.connection.execute(
            "SELECT value FROM consensus_watermarks WHERE name='netuid'"
        ).fetchone()
        if row[0] != str(netuid):
            raise ProtocolError("validator journal belongs to another subnet")

    def claim(self, netuid: int, epoch: int, digest: str) -> bool:
        cursor = self.connection.execute(
            "INSERT OR IGNORE INTO weight_submissions VALUES (?, ?, ?, 'pending')",
            (netuid, epoch, digest),
        )
        return cursor.rowcount == 1

    def complete(self, netuid: int, epoch: int) -> None:
        self.connection.execute(
            "UPDATE weight_submissions SET state='submitted' WHERE netuid=? AND epoch=?",
            (netuid, epoch),
        )

    def release_failed(self, netuid: int, epoch: int) -> None:
        self.connection.execute(
            "DELETE FROM weight_submissions WHERE netuid=? AND epoch=? AND state='pending'",
            (netuid, epoch),
        )

    def close(self) -> None:
        self.connection.close()


@dataclass(frozen=True)
class TickResult:
    outcome: str
    epoch: int | None = None


def forbidden_monopoly(vector: tuple[tuple[int, int], ...], snapshot: ChainSnapshot) -> bool:
    paid = [uid for uid, weight in vector if weight > 0]
    if len(paid) != 1 or paid[0] == 0:
        return False
    uid = paid[0]
    owner_uids = {row.uid for row in snapshot.rows if row.hotkey == snapshot.owner_hotkey}
    return uid in owner_uids or uid in snapshot.validator_permits


class Validator:
    def __init__(
        self,
        *,
        gateway_url: str,
        netuid: int,
        trust: TrustRoot,
        chain: Chain,
        journal: SubmissionJournal,
        http: httpx.AsyncClient,
        version_key: int = 1,
        consensus_seed: Callable[[], bytes] | None = None,
        peers: dict[bytes, str] | None = None,
        min_peer_sample: int = 1,
        max_block_lag: int = 256,
        trust_loader: Callable[[int], TrustRoot] | None = None,
    ):
        uint(netuid, 2)
        uint(version_key, 8)
        trust.validate()
        self.gateway_url = gateway_url.rstrip("/")
        self.netuid = netuid
        self.trust = trust
        self.chain = chain
        self.journal = journal
        self.journal.bind_netuid(netuid)
        self.http = http
        self.version_key = version_key
        if min_peer_sample < 0 or max_block_lag < 1:
            raise ValueError("invalid peer sample or block freshness limit")
        self.consensus_seed = consensus_seed
        self.peers = peers or {}
        if len(self.peers) > 64:
            raise ValueError("too many peer endpoints")
        for hotkey, url in self.peers.items():
            fixed(hotkey, 32)
            parsed = urlsplit(url)
            if (
                parsed.scheme != "https"
                or not parsed.hostname
                or parsed.username
                or parsed.password
                or parsed.path not in {"", "/"}
                or parsed.query
                or parsed.fragment
            ):
                raise ValueError("peer URLs require HTTPS without credentials")
        self.min_peer_sample, self.max_block_lag = min_peer_sample, max_block_lag
        self.trust_loader = trust_loader
        self._observed: Bundle | None = None
        self._lock = asyncio.Lock()

    async def _read(self, path: str, limit: int, *, base_url: str | None = None) -> bytes:
        async with self.http.stream(
            "GET",
            (base_url or self.gateway_url).rstrip("/") + path,
            follow_redirects=False,
            timeout=15,
        ) as reply:
            reply.raise_for_status()
            chunks = bytearray()
            async for chunk in reply.aiter_bytes():
                if len(chunks) + len(chunk) > limit:
                    raise ProtocolError("gateway response too large")
                chunks.extend(chunk)
        return bytes(chunks)

    async def run_once(self) -> TickResult:
        async with self._lock:
            self._observed = None
            try:
                return await self._run_once()
            except ProtocolError as error:
                reasons = {
                    "signature": DissentReason.BUNDLE_SIGNATURE_INVALID,
                    "challenge signature": DissentReason.LEAF_SIGNATURE_INVALID,
                    "unknown challenge": DissentReason.LEAF_CHALLENGE_KEY_UNKNOWN,
                    "participant": DissentReason.INCOMPLETE_PARTICIPANT_SET,
                    "Merkle": DissentReason.MERKLE_ROOT_MISMATCH,
                    "emission": DissentReason.EMISSION_SHARE_MISMATCH,
                    "metagraph": DissentReason.METAGRAPH_ROOT_MISMATCH,
                    "block": DissentReason.BLOCK_HASH_MISMATCH,
                    "version": DissentReason.PROTOCOL_VERSION_UNSUPPORTED,
                    "uid map": DissentReason.UID_MAP_MISMATCH,
                    "measurements": DissentReason.MEASUREMENTS_DIGEST_MISMATCH,
                    "share mass": DissentReason.SHARE_MASS_BELOW_THRESHOLD,
                }
                reason = DissentReason.AGGREGATION_OVERFLOW
                for text, candidate in reasons.items():
                    if text in str(error):
                        reason = candidate
                self._dissent(reason)
                raise

    def _dissent(self, reason: DissentReason, vector=()) -> None:
        if self.consensus_seed is None:
            return
        body = self._observed.body if self._observed else None
        self.journal.evidence.dissent(
            Dissent.sign(
                self.consensus_seed(),
                body.epoch if body else 0,
                body.merkle_root if body else bytes(32),
                sha256(encode_final_vector(vector)).digest() if vector else bytes(32),
                sha256(encode_final_vector(body.final_vector)).digest() if body else bytes(32),
                reason,
            )
        )

    async def _crosscheck(self, bundle: Bundle, snapshot: ChainSnapshot) -> bool:
        body = bundle.body
        own = public_key(self.consensus_seed()) if self.consensus_seed else None
        validators = {row.hotkey for row in snapshot.rows if row.uid in snapshot.validator_permits}
        others = validators - {own}
        if own is not None and self.consensus_seed is not None:
            old = self.journal.evidence.local_root(body.epoch)
            if old and (old.hotkey != own or old.merkle_root != body.merkle_root):
                self._dissent(DissentReason.PEER_ROOT_CONFLICT)
                return False
            self.journal.evidence.root(
                RootStatement.sign(self.consensus_seed(), body.epoch, body.merkle_root), local=True
            )
        if not others:
            return True
        if self.min_peer_sample == 0 or own is None:
            self._dissent(DissentReason.PEER_SAMPLE_INSUFFICIENT)
            return False

        async def fetch(hotkey, endpoint):
            try:
                value = decode_json(
                    await self._read(f"/v1/consensus/root/{body.epoch}", 4096, base_url=endpoint)
                )
                statement = RootStatement.from_json(value)
                if statement.hotkey != hotkey or statement.epoch != body.epoch:
                    return None
                return statement
            except (httpx.HTTPError, ValueError, ServiceError):
                return None

        statements = await asyncio.gather(
            *(fetch(key, endpoint) for key, endpoint in self.peers.items() if key in others)
        )
        valid = [statement for statement in statements if statement is not None]
        conflict = False
        for statement in valid:
            if (
                not self.journal.evidence.root(statement)
                or statement.merkle_root != body.merkle_root
            ):
                conflict = True
        if conflict:
            self._dissent(DissentReason.PEER_ROOT_CONFLICT)
            return False
        if len(valid) < self.min_peer_sample:
            self._dissent(DissentReason.PEER_SAMPLE_INSUFFICIENT)
            return False
        return True

    async def _run_once(self) -> TickResult:
        latest = decode_json(await self._read("/v1/weights/latest", 1024 * 1024))
        # Never read an LKG on disk, even when this process previously verified a seal.
        if latest.get("sealed") is not True:
            return TickResult("unsealed")
        epoch = latest.get("epoch")
        if not isinstance(epoch, int):
            raise ProtocolError("invalid epoch")
        uint(epoch, 8)
        if latest.get("netuid", self.netuid) != self.netuid:
            raise ProtocolError("gateway subnet mismatch")
        data = await self._read(f"/v1/bundle/{epoch}", 32 * 1024 * 1024)
        bundle = Bundle.decode(data)
        self._observed = bundle
        tip = await self.chain.current_block()
        if not 0 <= tip - bundle.body.block_b <= self.max_block_lag:
            raise ProtocolError("bundle block is stale or in the future")
        snapshot = await self.chain.snapshot(bundle.body.block_b, self.netuid)
        if snapshot.block != bundle.body.block_b:
            raise ProtocolError("chain snapshot block mismatch")
        if snapshot.epoch is not None and snapshot.epoch != epoch:
            raise ProtocolError("bundle epoch does not match the pinned chain block")
        if self.trust_loader:
            self.trust = self.trust_loader(epoch)
        self.journal.evidence.watermark("challenges_version", self.trust.challenges_version)
        self.journal.evidence.watermark("measurements_version", self.trust.measurements_version)
        body, quarantined = verify_inputs(
            bundle,
            rows=snapshot.rows,
            block_hash=snapshot.block_hash,
            trust=self.trust,
            netuid=self.netuid,
            epoch=epoch,
            allow_quarantine=True,
        )
        self.journal.evidence.watermark("epoch", epoch)
        self.journal.evidence.bundle(bundle)
        if latest.get("merkle_root") != body.merkle_root.hex():
            raise ProtocolError("latest Merkle root does not match bundle")
        if "bundle_digest" in latest and latest["bundle_digest"] != bundle_digest(bundle):
            raise ProtocolError("latest bundle digest mismatch")
        if (
            "vector_digest" in latest
            and latest["vector_digest"] != sha256(body.encode()).hexdigest()
        ):
            raise ProtocolError("latest vector digest mismatch")
        if "final_vector" in latest and latest["final_vector"] != [
            list(p) for p in body.final_vector
        ]:
            raise ProtocolError("latest vector does not match bundle")
        floats = recompute(body, quarantined)
        local_vector = floats.final_vector
        vector_mismatch = body.final_vector != local_vector
        if not vector_mismatch and "uids" in latest and latest["uids"] != list(floats.uids):
            raise ProtocolError("latest uid vector mismatch")
        if (
            not vector_mismatch
            and "weights" in latest
            and latest["weights"] != list(floats.weights)
        ):
            raise ProtocolError("latest float vector mismatch")
        if forbidden_monopoly(local_vector, snapshot):
            return TickResult("owner_or_validator_monopoly", epoch)
        if not await self._crosscheck(bundle, snapshot):
            return TickResult("peer_consensus_unavailable", epoch)
        if vector_mismatch:
            if self.consensus_seed is None:
                raise ProtocolError("final vector mismatch requires signed dissent")
            self._dissent(DissentReason.VECTOR_MISMATCH, local_vector)
        if quarantined:
            if self.consensus_seed is None:
                raise ProtocolError("quarantine requires signed dissent")
            self._dissent(DissentReason.LEAF_SIGNATURE_INVALID, local_vector)
        final_snapshot = await self.chain.snapshot(body.block_b, self.netuid)
        if final_snapshot != snapshot:
            raise ProtocolError("chain block changed before dispatch")
        if not 0 <= await self.chain.current_block() - body.block_b <= self.max_block_lag:
            raise ProtocolError("bundle block expired before dispatch")
        # Verification may perform slow historical RPCs. A seal that was current
        # before those reads is not a submission path after latest becomes unsealed.
        fresh = decode_json(await self._read("/v1/weights/latest", 1024 * 1024))
        if fresh.get("sealed") is not True:
            return TickResult("unsealed")
        identity_fields = (
            "epoch",
            "netuid",
            "merkle_root",
            "bundle_digest",
            "vector_digest",
            "final_vector",
            "uids",
            "weights",
        )
        if any(fresh.get(name) != latest.get(name) for name in identity_fields):
            return TickResult("latest_changed", epoch)
        if not self.journal.claim(self.netuid, epoch, bundle_digest(bundle)):
            return TickResult("already_submitted_or_pending", epoch)
        # An exception is ambiguous: retain pending rather than double-spend on retry.
        success = await self.chain.submit(self.netuid, local_vector, self.version_key)
        if not success:
            self.journal.release_failed(self.netuid, epoch)
            return TickResult("dispatch_failed", epoch)
        self.journal.complete(self.netuid, epoch)
        return TickResult("submitted", epoch)
