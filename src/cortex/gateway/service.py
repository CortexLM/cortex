"""Gateway sealing boundary: local keys and trust, pinned chain snapshot, durable store."""

import sqlite3
from collections.abc import Callable
from datetime import UTC, datetime
from typing import Protocol

from cortex.errors import ServiceError
from cortex.protocol import Bundle, Leaf, ProtocolError, TrustRoot, aggregate_leaves, build_bundle
from cortex.protocol.crypto import BUNDLE_DOMAIN, RAW_WEIGHT_DOMAIN, verify_raw
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
        chain_endpoint: str = "",
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
        self.store.trust_versions(trust.challenges_version, trust.measurements_version)

    def refresh_trust(self, epoch: int) -> None:
        if self.trust_loader is None:
            return
        trust = self.trust_loader(epoch)
        trust.validate()
        self.store.trust_versions(trust.challenges_version, trust.measurements_version)
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

    def _decode_stored(self, stored: StoredBundle) -> Bundle:
        bundle = Bundle.decode(stored.encoded)
        if bundle.body.epoch != stored.epoch:
            raise ProtocolError("stored epoch mismatch")
        # The signature protects the chain root and participant coverage checked at
        # sealing. Do not reconstruct chain stakes from a uid_map that omits them.
        body = bundle.body
        if (
            body.protocol_version != 1
            or body.algorithm_version != 1
            or body.netuid != self.netuid
            or body.gateway_hotkey != self.trust.gateway_hotkey
            or body.emission_shares != self.trust.shares
            or body.measurements_digest != self.trust.measurements_digest
            or not verify_raw(body.gateway_hotkey, BUNDLE_DOMAIN, body.encode(), bundle.gateway_sig)
        ):
            raise ProtocolError("invalid stored seal")
        if merkle_root(leaf.encode() for leaf in body.leaves) != body.merkle_root:
            raise ProtocolError("invalid stored leaf root")
        final = aggregate_leaves(body.leaves, body.emission_shares, body.uid_map)
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
            if self._decode_stored(stored).body.merkle_root != raw:
                raise ProtocolError("indexed root mismatch")
        except ProtocolError:
            raise ServiceError(503, "stored bundle unavailable") from None
        return stored.encoded

    def latest(self) -> dict:
        try:
            stored = self.store.latest()
            if stored is not None:
                self.refresh_trust(stored.epoch)
            bundle = self._decode_stored(stored) if stored is not None else None
            return project(
                bundle, netuid=self.netuid, now=self.clock(), chain_endpoint=self.chain_endpoint
            )
        except (sqlite3.Error, ProtocolError, ValueError, TypeError, ServiceError):
            # Never serve an earlier seal in place of a corrupt latest seal.
            return project(
                None, netuid=self.netuid, now=self.clock(), chain_endpoint=self.chain_endpoint
            )

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
