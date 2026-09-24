"""Gateway sealing boundary: local keys and trust, pinned chain snapshot, durable store."""

import sqlite3
from collections.abc import Callable
from datetime import UTC, datetime
from typing import Protocol

from cortex.errors import ServiceError
from cortex.protocol import Bundle, Leaf, ProtocolError, TrustRoot, aggregate_leaves, build_bundle
from cortex.protocol.crypto import BUNDLE_DOMAIN, RAW_WEIGHT_DOMAIN, encode_hotkey, verify_raw
from cortex.protocol.merkle import merkle_root
from cortex.protocol.scale import uint
from cortex.validator import ChainSnapshot

from .projection import project, timestamp
from .store import GatewayStore, StoredBundle


class SnapshotProvider(Protocol):
    async def current_block(self) -> int: ...

    async def snapshot(self, block: int, netuid: int) -> ChainSnapshot: ...


class GatewayService:
    def __init__(
        self,
        *,
        store: GatewayStore,
        trust: TrustRoot,
        netuid: int,
        chain: SnapshotProvider,
        gateway_seed: Callable[[], bytes],
        clock: Callable[[], datetime] | None = None,
        chain_endpoint: str | Callable[[], str] = "",
        trust_loader: Callable[[int], TrustRoot] | None = None,
    ):
        trust.validate()
        self.store = store
        self.store.bind_netuid(netuid)
        self.trust = trust
        self.netuid = netuid
        self.chain = chain
        self.gateway_seed = gateway_seed
        self.clock = clock or (lambda: datetime.now(UTC))
        self.chain_endpoint = chain_endpoint
        self.trust_loader = trust_loader
        self.store.accept_trust(trust)

    def _chain_endpoint(self) -> str:
        try:
            value = self.chain_endpoint() if callable(self.chain_endpoint) else self.chain_endpoint
        except Exception:
            return ""
        return value if isinstance(value, str) else ""

    def refresh_trust(self, epoch: int) -> None:
        if self.trust_loader is None:
            return
        trust = self.trust_loader(epoch)
        trust.validate()
        self.store.accept_trust(trust)
        self.trust = trust

    def accept_leaf(self, leaf: Leaf) -> dict:
        self.refresh_trust(leaf.epoch)
        entry = next(
            (entry for entry in self.trust.challenges if entry.id == leaf.challenge_id), None
        )
        if entry is None:
            raise ServiceError(404, "challenge not registered")
        try:
            leaf.encode()
        except (ValueError, TypeError):
            raise ServiceError(400, "invalid raw weight") from None
        if not verify_raw(entry.public_key, RAW_WEIGHT_DOMAIN, leaf.payload(), leaf.challenge_sig):
            raise ServiceError(401, "invalid challenge signature")
        try:
            return self.store.put_leaf(leaf)
        except (sqlite3.Error, ProtocolError):
            raise ServiceError(503, "raw weight store unavailable") from None

    def replace_challenge_leaves(
        self,
        challenge_id: bytes,
        epoch: int,
        expected: set[bytes],
        leaves: tuple[Leaf, ...],
    ) -> tuple[dict, ...]:
        """Atomically replace one challenge's complete authoritative epoch snapshot."""
        self.refresh_trust(epoch)
        entry = next((entry for entry in self.trust.challenges if entry.id == challenge_id), None)
        if entry is None:
            raise ServiceError(404, "challenge not registered")
        if any(not isinstance(hotkey, bytes) or len(hotkey) != 32 for hotkey in expected):
            raise ServiceError(400, "invalid challenge participant set")
        if len(leaves) != len(expected) or {leaf.miner_hotkey for leaf in leaves} != expected:
            raise ServiceError(409, "incomplete challenge participant set")
        for leaf in leaves:
            try:
                leaf.encode()
            except (ValueError, TypeError, ProtocolError):
                raise ServiceError(400, "invalid challenge snapshot") from None
            if leaf.challenge_id != challenge_id or leaf.epoch != epoch:
                raise ServiceError(400, "challenge snapshot identity mismatch")
            if not verify_raw(
                entry.public_key, RAW_WEIGHT_DOMAIN, leaf.payload(), leaf.challenge_sig
            ):
                raise ServiceError(401, "invalid challenge signature")
        try:
            return self.store.replace_challenge_leaves(challenge_id, epoch, leaves)
        except ServiceError:
            raise
        except (sqlite3.Error, ProtocolError):
            raise ServiceError(503, "raw weight store unavailable") from None

    def bundle_bytes(self, epoch: int) -> bytes:
        try:
            stored = self.store.bundle(epoch)
        except ProtocolError:
            raise ServiceError(400, "invalid epoch") from None
        except sqlite3.Error:
            raise ServiceError(503, "bundle store unavailable") from None
        if stored is None:
            raise ServiceError(404, "bundle not found")
        return stored.encoded

    def _decode_stored(self, stored: StoredBundle, trust: TrustRoot | None = None) -> Bundle:
        trust = self.trust if trust is None else trust
        bundle = Bundle.decode(stored.encoded)
        if bundle.body.epoch != stored.epoch:
            raise ProtocolError("stored epoch mismatch")
        # The signature protects the chain root and participant coverage checked at
        # sealing. Do not reconstruct chain stakes from a uid_map that omits them.
        body = bundle.body
        if (
            body.protocol_version != 1
            or body.algorithm_version != trust.algorithm_version
            or body.epoch < trust.introduced_epoch
            or body.netuid != self.netuid
            or body.gateway_hotkey != self.trust.gateway_hotkey
            or body.gateway_hotkey != trust.gateway_hotkey
            or body.emission_shares != trust.shares
            or body.measurements_digest != trust.measurements_digest
            or not verify_raw(body.gateway_hotkey, BUNDLE_DOMAIN, body.encode(), bundle.gateway_sig)
        ):
            raise ProtocolError("invalid stored seal")
        if merkle_root(leaf.encode() for leaf in body.leaves) != body.merkle_root:
            raise ProtocolError("invalid stored leaf root")
        keys = {entry.id: entry.public_key for entry in trust.challenges}
        for leaf in body.leaves:
            if (
                leaf.epoch != body.epoch
                or leaf.challenge_id not in keys
                or not verify_raw(
                    keys[leaf.challenge_id], RAW_WEIGHT_DOMAIN, leaf.payload(), leaf.challenge_sig
                )
            ):
                raise ProtocolError("invalid stored challenge signature")
        final = aggregate_leaves(
            body.leaves,
            body.emission_shares,
            body.uid_map,
            algorithm_version=body.algorithm_version,
        )
        if final.final_vector != body.final_vector:
            raise ProtocolError("invalid stored final vector")
        return bundle

    def bundle_by_root(self, root: str) -> bytes:
        try:
            raw = bytes.fromhex(root)
            if len(root) != 64 or root != raw.hex():
                raise ValueError("invalid root")
        except ValueError:
            raise ServiceError(400, "invalid bundle root") from None
        stored = self.store.bundle_by_root(raw)
        if stored is None:
            raise ServiceError(404, "bundle not found")
        try:
            # Archived profiles never become the current profile or lower its watermarks.
            for trust in self.store.trust_profiles():
                try:
                    bundle = self._decode_stored(stored, trust)
                except ProtocolError:
                    continue
                if bundle.body.merkle_root != raw:
                    raise ProtocolError("indexed root mismatch")
                return stored.encoded
        except (sqlite3.Error, ProtocolError):
            raise ServiceError(503, "stored bundle unavailable") from None
        raise ServiceError(503, "stored bundle unavailable")

    def latest(self) -> dict:
        try:
            stored = self.store.latest()
            if stored is not None:
                self.refresh_trust(stored.epoch)
            bundle = self._decode_stored(stored) if stored is not None else None
            if stored is None:
                computed_at = self.clock()
            else:
                computed_at = datetime.fromisoformat(stored.sealed_at.replace("Z", "+00:00"))
                if timestamp(computed_at) != stored.sealed_at:
                    raise ValueError("invalid stored seal timestamp")
            return project(
                bundle, netuid=self.netuid, now=computed_at, chain_endpoint=self._chain_endpoint()
            )
        except (sqlite3.Error, ProtocolError, ValueError, TypeError, ServiceError):
            # Never serve an earlier seal in place of a corrupt latest seal.
            return project(
                None, netuid=self.netuid, now=self.clock(), chain_endpoint=self._chain_endpoint()
            )

    def metagraph(self) -> dict:
        """Hotkeys of the latest verified seal, for challenge intake filters."""
        try:
            stored = self.store.latest()
            if stored is None:
                raise ServiceError(503, "no sealed metagraph")
            self.refresh_trust(stored.epoch)
            body = self._decode_stored(stored).body
        except (sqlite3.Error, ProtocolError, ValueError):
            raise ServiceError(503, "no sealed metagraph") from None
        return {
            "epoch": body.epoch,
            "block": body.block_b,
            "netuid": body.netuid,
            "hotkeys": {encode_hotkey(key): uid for key, uid in body.uid_map},
        }

    async def seal(
        self, epoch: int, *, netuid: int | None = None, block_b: int | None = None
    ) -> Bundle:
        try:
            uint(epoch, 8)
            if netuid is not None:
                uint(netuid, 2)
            if block_b is not None:
                uint(block_b, 8)
        except ProtocolError:
            raise ServiceError(400, "invalid seal parameters") from None
        if netuid is not None and netuid != self.netuid:
            raise ServiceError(400, "seal subnet mismatch")
        self.refresh_trust(epoch)
        try:
            existing = self.store.bundle(epoch)
            if existing is not None:
                return self._existing(existing, block_b)
            chosen_block = await self.chain.current_block() if block_b is None else block_b
            snapshot = await self.chain.snapshot(chosen_block, self.netuid)
            if snapshot.block != chosen_block:
                raise ServiceError(503, "chain snapshot block mismatch")
            seed = self.gateway_seed()

            def build(leaves: tuple[Leaf, ...]) -> Bundle:
                return build_bundle(
                    gateway_seed=seed,
                    epoch=epoch,
                    netuid=self.netuid,
                    block_b=chosen_block,
                    block_hash=snapshot.block_hash,
                    rows=snapshot.rows,
                    leaves=leaves,
                    trust=self.trust,
                )

            stored = self.store.seal(epoch, build, timestamp(self.clock()))
            return self._existing(stored, block_b)
        except ServiceError:
            raise
        except ProtocolError as error:
            if str(error) == "incomplete participant set":
                raise ServiceError(409, "incomplete participant set (D24)") from None
            raise ServiceError(503, "seal verification failed") from None
        except Exception:
            # Chain/key/storage failures are operational and never expose credentials.
            raise ServiceError(503, "seal inputs unavailable") from None

    def _existing(self, stored: StoredBundle, block_b: int | None) -> Bundle:
        bundle = self._decode_stored(stored)
        if block_b is not None and bundle.body.block_b != block_b:
            raise ServiceError(409, "epoch already sealed at another block")
        return bundle
