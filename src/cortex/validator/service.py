"""Fetch-only validator: independently verify a current seal before chain submission."""

import asyncio
import re
import sqlite3
from collections.abc import Callable
from dataclasses import dataclass
from hashlib import sha256
from pathlib import Path
from typing import Protocol, cast
from urllib.parse import urlsplit
from uuid import uuid4

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


class DispatchNotBroadcast(ProtocolError):
    """Dispatch failed before the SDK could submit an extrinsic."""


class DispatchUncertain(ProtocolError):
    """The SDK may have submitted an extrinsic; reconciliation is required."""

    def __init__(
        self,
        message: str,
        *,
        extrinsic_hash: str | None = None,
        nonce: int | None = None,
    ):
        super().__init__(message)
        self.extrinsic_hash = extrinsic_hash
        self.nonce = nonce


@dataclass(frozen=True)
class ChainSnapshot:
    block: int
    block_hash: bytes
    rows: tuple[MetagraphRow, ...]
    owner_hotkey: bytes
    validator_permits: frozenset[int]
    epoch: int | None = None


class Chain(Protocol):
    async def current_epoch(self, netuid: int) -> int: ...

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
        self.connection.execute(
            "CREATE TABLE IF NOT EXISTS weight_submission_attempts ("
            "attempt_id TEXT PRIMARY KEY, netuid INTEGER NOT NULL, epoch INTEGER NOT NULL, "
            "digest TEXT NOT NULL, state TEXT NOT NULL CHECK(state IN "
            "('pending','dispatching','uncertain','submitted','not_broadcast','reconciled_submitted',"
            "'reconciled_not_broadcast')), extrinsic_hash TEXT, nonce TEXT, "
            "evidence_digest TEXT)"
        )
        self.connection.execute(
            "CREATE INDEX IF NOT EXISTS weight_submission_attempt_epoch "
            "ON weight_submission_attempts(netuid, epoch, state)"
        )
        for netuid, epoch, digest, state in self.connection.execute(
            "SELECT s.netuid,s.epoch,s.digest,s.state FROM weight_submissions s "
            "WHERE NOT EXISTS (SELECT 1 FROM weight_submission_attempts a "
            "WHERE a.netuid=s.netuid AND a.epoch=s.epoch)"
        ).fetchall():
            self.connection.execute(
                "INSERT INTO weight_submission_attempts "
                "(attempt_id,netuid,epoch,digest,state) VALUES (?,?,?,?,?)",
                (f"legacy-{netuid}-{epoch}", netuid, epoch, digest, state),
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

    def claim(self, netuid: int, epoch: int, digest: str) -> str | None:
        attempt_id = str(uuid4())
        self.connection.execute("BEGIN IMMEDIATE")
        try:
            cursor = self.connection.execute(
                "INSERT OR IGNORE INTO weight_submissions VALUES (?, ?, ?, 'pending')",
                (netuid, epoch, digest),
            )
            if cursor.rowcount != 1:
                self.connection.execute("ROLLBACK")
                return None
            self.connection.execute(
                "INSERT INTO weight_submission_attempts "
                "(attempt_id,netuid,epoch,digest,state) VALUES (?,?,?,?, 'dispatching')",
                (attempt_id, netuid, epoch, digest),
            )
            self.connection.execute("COMMIT")
            return attempt_id
        except BaseException:
            if self.connection.in_transaction:
                self.connection.execute("ROLLBACK")
            raise

    def complete(self, netuid: int, epoch: int, attempt_id: str) -> None:
        self.connection.execute("BEGIN IMMEDIATE")
        try:
            attempt = self.connection.execute(
                "UPDATE weight_submission_attempts SET state='submitted' "
                "WHERE attempt_id=? AND netuid=? AND epoch=? AND state='dispatching'",
                (attempt_id, netuid, epoch),
            )
            if attempt.rowcount != 1:
                raise ProtocolError("dispatch attempt is not active")
            submission = self.connection.execute(
                "UPDATE weight_submissions SET state='submitted' "
                "WHERE netuid=? AND epoch=? AND state='pending'",
                (netuid, epoch),
            )
            if submission.rowcount != 1:
                raise ProtocolError("pending weight submission unavailable")
            self.connection.execute("COMMIT")
        except BaseException:
            if self.connection.in_transaction:
                self.connection.execute("ROLLBACK")
            raise

    def release_failed(self, netuid: int, epoch: int, attempt_id: str) -> None:
        self.connection.execute("BEGIN IMMEDIATE")
        try:
            attempt = self.connection.execute(
                "UPDATE weight_submission_attempts SET state='not_broadcast' "
                "WHERE attempt_id=? AND netuid=? AND epoch=? AND state='dispatching'",
                (attempt_id, netuid, epoch),
            )
            if attempt.rowcount != 1:
                raise ProtocolError("dispatch attempt is not active")
            submission = self.connection.execute(
                "DELETE FROM weight_submissions WHERE netuid=? AND epoch=? AND state='pending'",
                (netuid, epoch),
            )
            if submission.rowcount != 1:
                raise ProtocolError("pending weight submission unavailable")
            self.connection.execute("COMMIT")
        except BaseException:
            if self.connection.in_transaction:
                self.connection.execute("ROLLBACK")
            raise

    def record_uncertain(
        self,
        netuid: int,
        epoch: int,
        attempt_id: str,
        *,
        extrinsic_hash: str | None = None,
        nonce: int | None = None,
    ) -> None:
        if extrinsic_hash is not None and not re.fullmatch(r"0x[0-9a-f]{64}", extrinsic_hash):
            raise ProtocolError("invalid extrinsic hash")
        if nonce is not None:
            uint(nonce, 8)
        cursor = self.connection.execute(
            "UPDATE weight_submission_attempts "
            "SET state='uncertain',extrinsic_hash=?,nonce=? "
            "WHERE attempt_id=? AND netuid=? AND epoch=? AND state='dispatching'",
            (extrinsic_hash, str(nonce) if nonce is not None else None, attempt_id, netuid, epoch),
        )
        if cursor.rowcount != 1:
            raise ProtocolError("active dispatch attempt unavailable")

    def recover_interrupted(self, netuid: int) -> int:
        """Make attempts left by a stopped validator explicitly reconcilable."""
        cursor = self.connection.execute(
            "UPDATE weight_submission_attempts SET state='uncertain' "
            "WHERE netuid=? AND state='dispatching' AND EXISTS ("
            "SELECT 1 FROM weight_submissions s "
            "WHERE s.netuid=weight_submission_attempts.netuid "
            "AND s.epoch=weight_submission_attempts.epoch AND s.state='pending')",
            (netuid,),
        )
        return cursor.rowcount

    def pending(self, netuid: int, epoch: int) -> dict[str, object] | None:
        row = self.connection.execute(
            "SELECT a.attempt_id,a.digest,a.extrinsic_hash,a.nonce,a.state "
            "FROM weight_submission_attempts a JOIN weight_submissions s "
            "ON s.netuid=a.netuid AND s.epoch=a.epoch "
            "WHERE a.netuid=? AND a.epoch=? "
            "AND a.state IN ('pending','dispatching','uncertain') AND s.state='pending'",
            (netuid, epoch),
        ).fetchone()
        if row is None:
            return None
        return {
            "attempt_id": row[0],
            "digest": row[1],
            "extrinsic_hash": row[2],
            "nonce": int(row[3]) if row[3] is not None else None,
            "state": row[4],
        }

    def reconcile(
        self,
        *,
        netuid: int,
        epoch: int,
        digest: str,
        attempt_id: str,
        result: str,
        evidence_digest: str,
    ) -> dict[str, object]:
        uint(netuid, 2)
        uint(epoch, 8)
        if not re.fullmatch(r"[0-9a-f]{64}", digest):
            raise ProtocolError("invalid bundle digest")
        if not attempt_id or len(attempt_id) > 128:
            raise ProtocolError("invalid dispatch attempt id")
        if result not in {"submitted", "not_broadcast"}:
            raise ProtocolError("invalid reconciliation result")
        if not re.fullmatch(r"[0-9a-f]{64}", evidence_digest):
            raise ProtocolError("invalid reconciliation evidence digest")
        self.connection.execute("BEGIN IMMEDIATE")
        try:
            row = self.connection.execute(
                "SELECT a.digest,a.extrinsic_hash,a.nonce,a.state,s.state "
                "FROM weight_submission_attempts a JOIN weight_submissions s "
                "ON s.netuid=a.netuid AND s.epoch=a.epoch "
                "WHERE a.attempt_id=? AND a.netuid=? AND a.epoch=?",
                (attempt_id, netuid, epoch),
            ).fetchone()
            if (
                row is None
                or row[0] != digest
                or row[3] not in {"pending", "uncertain"}
                or row[4] != "pending"
            ):
                raise ProtocolError("reconciliation identity does not match pending dispatch")
            state = f"reconciled_{result}"
            attempt = self.connection.execute(
                "UPDATE weight_submission_attempts SET state=?,evidence_digest=? "
                "WHERE attempt_id=? AND state=?",
                (state, evidence_digest, attempt_id, row[3]),
            )
            if attempt.rowcount != 1:
                raise ProtocolError("pending dispatch changed during reconciliation")
            if result == "submitted":
                submission = self.connection.execute(
                    "UPDATE weight_submissions SET state='submitted' "
                    "WHERE netuid=? AND epoch=? AND state='pending'",
                    (netuid, epoch),
                )
            else:
                submission = self.connection.execute(
                    "DELETE FROM weight_submissions WHERE netuid=? AND epoch=? AND state='pending'",
                    (netuid, epoch),
                )
            if submission.rowcount != 1:
                raise ProtocolError("pending weight submission unavailable")
            self.connection.execute("COMMIT")
        except BaseException:
            if self.connection.in_transaction:
                self.connection.execute("ROLLBACK")
            raise
        return {
            "attempt_id": attempt_id,
            "digest": digest,
            "evidence_digest": evidence_digest,
            "extrinsic_hash": row[1],
            "nonce": int(row[2]) if row[2] is not None else None,
            "state": state,
        }

    def close(self) -> None:
        self.connection.close()


@dataclass(frozen=True)
class TickResult:
    outcome: str
    epoch: int | None = None
    attempt_id: str | None = None
    extrinsic_hash: str | None = None
    nonce: int | None = None


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
        peer_consensus: bool = False,
        min_peer_sample: int = 1,
        trust_loader: Callable[[int], TrustRoot] | None = None,
        verify_only: bool = False,
    ):
        """Verify the gateway seal and submit weights.

        ``peer_consensus`` selects the deployment model. Cortex runs a centralized
        authoritative gateway: the master gateway is the authority for weights and a
        validator consumes ``/v1/weights/latest``, so peer-root cross-check is off by
        default and ``peers``/``min_peer_sample`` are inert. Enable it only for an
        independently-operated multi-validator deployment, where a peer-root sample
        of at least ``min_peer_sample`` becomes a precondition for submission. The
        local equivocation guard runs in both models.

        A seal names the epoch it closes: ``block_B`` is that epoch's inclusive
        last block (BUNDLE_SPEC.md §4.2), so the only seal on offer between epoch
        boundaries is the last closed epoch's. Freshness is therefore measured in
        epochs, not blocks: the seal must name the current epoch or the one
        immediately before it. A fixed block window cannot bound staleness here,
        because ``tempo`` is larger than any window that still rejects a replay,
        and a closed epoch's bundle carries a fixed ``block_B`` that never becomes
        fresh again.
        """
        uint(netuid, 2)
        uint(version_key, 8)
        trust.validate()
        gateway = urlsplit(gateway_url)
        if (
            gateway_url != gateway_url.strip()
            or gateway.scheme != "https"
            or not gateway.hostname
            or gateway.username
            or gateway.password
            or gateway.path not in {"", "/"}
            or gateway.query
            or gateway.fragment
        ):
            raise ValueError("gateway URL requires HTTPS without credentials or path")
        self.gateway_url = gateway_url.rstrip("/")
        self.netuid = netuid
        self.trust = trust
        self.chain = chain
        self.journal = journal
        self.journal.bind_netuid(netuid)
        self.journal.recover_interrupted(netuid)
        self.http = http
        self.version_key = version_key
        if min_peer_sample < 0:
            raise ValueError("invalid peer sample limit")
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
        self.peer_consensus = peer_consensus
        self.min_peer_sample = min_peer_sample
        self.trust_loader = trust_loader
        self.verify_only = verify_only
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
        # The gateway is authoritative for weights, so a peer-root sample is required
        # only in an opt-in multi-validator deployment. The local equivocation guard
        # above is self-consistency and runs in both models.
        if not self.peer_consensus or not others:
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

    async def _require_current_epoch(self, bundle: Bundle) -> None:
        """Refuse a seal older than the one the previous epoch boundary produced.

        ``block_B`` names the epoch it closes, so age is an epoch count, not a
        block count. The master seals one bundle per completed epoch, and that
        seal is the only one on offer until the next boundary, so the accepted
        window is exactly ``{current, current - 1}``. An older epoch is a replay
        and is refused; a block-height window cannot express this, because it
        would have to exceed ``tempo`` to stay live and would then also admit a
        stale closed epoch.
        """
        current = await self.chain.current_epoch(self.netuid)
        if bundle.body.epoch not in (current, current - 1):
            raise ProtocolError("bundle block is stale or in the future")

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
        await self._require_current_epoch(bundle)
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
        preflight_outcome = "verified"
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
            preflight_outcome = "dissent_vector_mismatch"
        if quarantined:
            if self.consensus_seed is None:
                raise ProtocolError("quarantine requires signed dissent")
            self._dissent(DissentReason.LEAF_SIGNATURE_INVALID, local_vector)
            preflight_outcome = (
                "dissent_vector_mismatch_and_quarantine"
                if vector_mismatch
                else "dissent_quarantine"
            )
        final_snapshot = await self.chain.snapshot(body.block_b, self.netuid)
        if final_snapshot != snapshot:
            raise ProtocolError("chain block changed before dispatch")
        await self._require_current_epoch(bundle)
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
        if self.verify_only:
            preflight = getattr(self.chain, "preflight", None)
            if preflight is not None:
                try:
                    await preflight(self.netuid, local_vector, self.version_key)
                except DispatchNotBroadcast:
                    return TickResult("chain_preflight_failed", epoch)
            return TickResult(preflight_outcome, epoch)
        attempt_id = self.journal.claim(self.netuid, epoch, bundle_digest(bundle))
        if attempt_id is None:
            pending = self.journal.pending(self.netuid, epoch)
            return TickResult(
                "already_submitted_or_pending",
                epoch,
                cast(str | None, pending["attempt_id"]) if pending else None,
                cast(str | None, pending["extrinsic_hash"]) if pending else None,
                cast(int | None, pending["nonce"]) if pending else None,
            )
        try:
            success = await self.chain.submit(self.netuid, local_vector, self.version_key)
        except DispatchNotBroadcast:
            self.journal.release_failed(self.netuid, epoch, attempt_id)
            return TickResult("dispatch_failed", epoch)
        except DispatchUncertain as error:
            self.journal.record_uncertain(
                self.netuid,
                epoch,
                attempt_id,
                extrinsic_hash=error.extrinsic_hash,
                nonce=error.nonce,
            )
            return TickResult(
                "dispatch_pending",
                epoch,
                attempt_id,
                error.extrinsic_hash,
                error.nonce,
            )
        except Exception:
            self.journal.record_uncertain(self.netuid, epoch, attempt_id)
            raise
        if not success:
            self.journal.release_failed(self.netuid, epoch, attempt_id)
            return TickResult("dispatch_failed", epoch)
        self.journal.complete(self.netuid, epoch, attempt_id)
        return TickResult("submitted", epoch)
