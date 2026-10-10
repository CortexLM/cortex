"""Miner bounded upload helper, master independent receipt/retrieval verification."""

from __future__ import annotations

import base64
import hashlib
from collections.abc import AsyncIterator
from pathlib import Path
from typing import BinaryIO
from urllib.parse import urlsplit

import anyio
import httpx
from pydantic import JsonValue

from hypertrain.data.stream_store import BLOCK, receive, spool
from hypertrain.protocol import relay_envelope
from hypertrain.protocol.envelope import SignatureError
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.relay_envelope import RelayEnvelope
from hypertrain.protocol.relay_messages import (
    AcceptedUploadAck,
    ChunkCustodyAck,
    MasterAcceptanceV2,
    RelayReceipt,
    RelayRegistryV1,
    RetrievalRequest,
    RetrievalResponse,
    UploadChunk,
    UploadChunkManifest,
    UploadGrant,
)
from hypertrain.relay.core import RelayError


def http_client() -> httpx.AsyncClient:
    """Installed httpx only, bounded pool/timeouts, no capability redirects."""
    return httpx.AsyncClient(
        timeout=httpx.Timeout(connect=5, read=30, write=30, pool=5),
        limits=httpx.Limits(max_connections=8, max_keepalive_connections=4),
        follow_redirects=False,
    )


def endpoint(registry: RelayRegistryV1, relay_id: str) -> str:
    specs = [s for s in registry.specs if s.id == relay_id]
    if len(specs) != 1:
        raise RelayError("endpoint missing from signed registry")
    return specs[0].https_url.rstrip("/")


def receipt_verified(
    raw: RelayEnvelope,
    *,
    registry: RelayRegistryV1,
    relay_id: str,
    run_id: str,
    grant: UploadGrant,
    received_before: int,
) -> RelayReceipt:
    """Historical receipt validity uses issued beacon, not today's rotated key."""
    receipt = RelayReceipt.model_validate(raw.body)
    specs = [s for s in registry.specs if s.id == relay_id]
    valid = len(specs) == 1 and any(
        k.key_id == receipt.key_id
        and k.pubkey == raw.signer
        and k.valid_from_round <= receipt.received_drand < k.valid_until_round
        for k in specs[0].pubkeys
    )
    if (
        not valid
        or raw.run_id != run_id
        or raw.type != "RelayReceipt"
        or not relay_envelope.verify_envelope(raw.model_dump())
        or receipt.grant_hash != grant.digest()
        or receipt.delta_hash != grant.delta_hash
        or receipt.size != grant.size
        or receipt.w != grant.w
        or receipt.hotkey != grant.hotkey
        or receipt.chunk_manifest_hash != grant.chunk_manifest_hash
        or receipt.retain_until < grant.retain_until
        or receipt.received_drand > received_before
    ):
        raise SignatureError("forged/mismatched durability receipt")
    return receipt


def chunk_manifest(path: Path) -> UploadChunkManifest:
    size = 0
    chunks: list[UploadChunk] = []
    with path.open("rb") as file:
        while data := file.read(4 << 20):
            chunks.append(
                UploadChunk(
                    index=len(chunks),
                    off=size,
                    len=len(data),
                    chunk_sha256=hashlib.sha256(data).hexdigest(),
                )
            )
            size += len(data)
    return UploadChunkManifest(size=size, chunks=chunks)


async def _part(file: BinaryIO, size: int) -> AsyncIterator[bytes]:
    remaining = size
    while remaining:
        data = file.read(min(BLOCK, remaining))
        if not data:
            raise RelayError("miner file shortened after manifest")
        remaining -= len(data)
        yield data
        await anyio.lowlevel.checkpoint()


class RelayClient:
    def __init__(self, registry: RelayRegistryV1, client: httpx.AsyncClient) -> None:
        self.registry, self.client = registry, client

    async def upload(
        self,
        path: Path,
        *,
        grant_raw: RelayEnvelope,
        assignment: RelayEnvelope,
        round_open: dict[str, JsonValue],
    ) -> RelayEnvelope:
        """Original bytes only; exact duplicate uploads preserve signed acknowledgments."""
        grant = UploadGrant.model_validate(grant_raw.body)
        capability = base64.b64encode(grant_raw.model_dump_json().encode()).decode()
        manifest = chunk_manifest(path)
        manifest.validate_grant(grant)
        url = endpoint(self.registry, grant.relay_id)
        response = await self.client.post(
            f"{url}/v1/uploads/{grant.digest()}/accept",
            json={
                "grant": grant_raw.model_dump(mode="json"),
                "manifest": manifest.body(),
                "assignment": assignment.model_dump(mode="json"),
                "round_open": round_open,
            },
        )
        response.raise_for_status()
        opportunity_raw = relay_envelope.parse_envelope(response.content)
        opportunity = AcceptedUploadAck.model_validate(opportunity_raw.body)
        keys = next(s.pubkeys for s in self.registry.specs if s.id == grant.relay_id)
        if (
            not relay_envelope.verify_envelope(opportunity_raw.model_dump())
            or opportunity_raw.run_id != grant_raw.run_id
            or opportunity.grant_hash != grant.digest()
            or not any(
                k.pubkey == opportunity_raw.signer
                and k.key_id == opportunity.key_id
                and k.valid_from_round <= opportunity.accepted_beacon < k.valid_until_round
                for k in keys
            )
        ):
            raise SignatureError("invalid opportunity acceptance")
        with path.open("rb") as file:
            for chunk in manifest.chunks:
                response = await self.client.put(
                    f"{url}/v1/uploads/{grant.digest()}/chunks/{chunk.index}",
                    content=_part(file, chunk.len),
                    headers={"Content-Length": str(chunk.len), "X-Upload-Grant": capability},
                )
                response.raise_for_status()
                raw = relay_envelope.parse_envelope(response.content)
                ack = ChunkCustodyAck.model_validate(raw.body)
                if (
                    raw.signer != opportunity_raw.signer
                    or raw.run_id != grant_raw.run_id
                    or not relay_envelope.verify_envelope(raw.model_dump())
                    or ack.upload_ack_hash != opportunity.digest()
                    or ack.grant_hash != grant.digest()
                    or ack.key_id != opportunity.key_id
                    or (ack.index, ack.off, ack.len, ack.chunk_sha256)
                    != (chunk.index, chunk.off, chunk.len, chunk.chunk_sha256)
                ):
                    raise SignatureError("chunk custody mismatch")
        response = await self.client.post(
            f"{url}/v1/uploads/{grant.digest()}/complete", headers={"X-Upload-Grant": capability}
        )
        response.raise_for_status()
        raw = relay_envelope.parse_envelope(response.content)
        receipt_verified(
            raw,
            registry=self.registry,
            relay_id=grant.relay_id,
            run_id=grant_raw.run_id,
            grant=grant,
            received_before=opportunity.service_deadline,
        )
        return raw

    async def retrieval(
        self, raw: RelayEnvelope, destination: BinaryIO, *, relay_public: str
    ) -> RelayEnvelope:
        request = RetrievalRequest.model_validate(raw.body)
        url = endpoint(self.registry, request.relay_id)
        response = await self.client.post(f"{url}/v1/retrievals", content=raw.model_dump_json())
        response.raise_for_status()
        signed = relay_envelope.parse_envelope(response.content)
        body = RetrievalResponse.model_validate(signed.body)
        if (
            signed.run_id != raw.run_id
            or signed.signer != relay_public
            or signed.type != "RetrievalResponse"
            or not relay_envelope.verify_envelope(signed.model_dump())
            or body.request_hash != request.digest()
            or body.nonce != request.nonce
            or body.key_id != request.key_id
            or body.size != request.size
            or body.object_hash != request.object_or_chunk_hash
            or not request.requested_beacon <= body.served_beacon <= request.deadline_beacon
        ):
            raise SignatureError("retrieval response nonce/key/time binding invalid")
        if body.status != "SERVED":
            body.validate_request(request, None)
            return signed
        if body.body_sha256 != request.object_or_chunk_hash:
            raise RelayError("signed SERVED hash conflicts with committed bytes")
        async with self.client.stream(
            "GET",
            f"{url}/v1/objects/{request.object_or_chunk_hash}",
            headers={"X-Retrieval-Hash": request.digest()},
        ) as stream:
            stream.raise_for_status()
            await receive(
                stream.aiter_raw(BLOCK), destination, request.size, request.object_or_chunk_hash
            )
        return signed

    async def accept_receipt(
        self,
        raw: RelayEnvelope,
        *,
        grant: UploadGrant,
        request_raw: RelayEnvelope,
        master: Keypair,
        now: int,
        deadline: int,
    ) -> RelayEnvelope:
        """Master requires independent original-byte retrieval, never receipt-only acceptance."""
        receipt = receipt_verified(
            raw,
            registry=self.registry,
            relay_id=grant.relay_id,
            run_id=request_raw.run_id,
            grant=grant,
            received_before=deadline,
        )
        request = RetrievalRequest.model_validate(request_raw.body)
        if (
            request.receipt_hash != receipt.digest()
            or request.object_or_chunk_hash != grant.delta_hash
            or request.size != grant.size
            or now > deadline
        ):
            raise RelayError("master receipt retrieval/deadline mismatch")
        with spool() as file:
            response_raw = await self.retrieval(request_raw, file, relay_public=raw.signer)
            response = RetrievalResponse.model_validate(response_raw.body)
            if response.status != "SERVED" or max(now, response.served_beacon) > deadline:
                raise RelayError("master cannot accept unreadable/late original object")
        accepted = MasterAcceptanceV2(
            w=grant.w,
            hotkey=grant.hotkey,
            delta_hash=grant.delta_hash,
            receipt_hash=receipt.digest(),
            received_drand=max(now, response.served_beacon),
        )
        return relay_envelope.parse_envelope(
            relay_envelope.seal(
                master, "MasterAcceptanceV2", raw.run_id, accepted, grant.retain_until
            )
        )


async def forward_receipt(
    client: httpx.AsyncClient, master_url: str, raw: RelayEnvelope
) -> RelayEnvelope:
    """Public relay receipt route; relay holds no coordinator/admin credential."""
    parsed = urlsplit(master_url)
    if parsed.scheme != "https" or not parsed.hostname or parsed.username or parsed.password:
        raise RelayError("master endpoint must be pinned credential-free HTTPS")
    response = await client.post(
        master_url.rstrip("/") + "/v2/relay-receipts", content=raw.model_dump_json()
    )
    response.raise_for_status()
    return relay_envelope.parse_envelope(response.content)
