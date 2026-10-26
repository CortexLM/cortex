"""Relay HTTP boundary; runtime keys/config mounted separately from coordinator secrets."""

from __future__ import annotations

import base64
import binascii
import hashlib
import hmac
import os
import tempfile
import time
from collections.abc import AsyncIterator
from contextlib import asynccontextmanager
from pathlib import Path

import anyio
import httpx
from fastapi import FastAPI, HTTPException, Request
from fastapi.responses import JSONResponse, StreamingResponse
from pydantic import BaseModel, ConfigDict, ValidationError

from hypertrain.beacon.core import BeaconError, round_at_or_after
from hypertrain.beacon.drand import QUICKNET_GENESIS, QUICKNET_PERIOD, DrandQuicknet
from hypertrain.data.store import LocalFSStore, S3Credentials, S3Store, StoreError
from hypertrain.data.stream_store import StreamStore, blocks, spool
from hypertrain.protocol import envelope_v2, relay_envelope
from hypertrain.protocol.envelope import EnvelopeError
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.relay_envelope import RelayEnvelope
from hypertrain.protocol.relay_messages import (
    RelayNetworkManifest,
    RelayRegistryV1,
    UploadChunkManifest,
    UploadGrant,
)
from hypertrain.relay.client import forward_receipt, http_client
from hypertrain.relay.core import Relay, RelayError


class AcceptRequest(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)
    grant: RelayEnvelope
    manifest: UploadChunkManifest
    assignment: RelayEnvelope
    round_open: envelope_v2.EnvelopeV2


class RelayConfig(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)
    run_id: str
    master_public: str
    master_url: str
    registry: RelayEnvelope
    network_manifest: RelayEnvelope
    relay_id: str
    region: str
    active_key: str
    signing_files: dict[str, str]
    observer_public_keys: list[str]
    s3_endpoint: str | None = None
    s3_bucket: str | None = None
    s3_prefix: str = "relay/"
    s3_region: str = "auto"
    local_backing: str | None = None


def create_app(relay: Relay, drain_token: str) -> FastAPI:
    if len(drain_token) < 32:
        raise RelayError("drain credential must contain >=32 characters")
    app = FastAPI(docs_url=None, redoc_url=None, openapi_url=None)

    @app.exception_handler(RelayError)
    @app.exception_handler(ValidationError)
    @app.exception_handler(EnvelopeError)
    async def rejected(request: Request, error: Exception) -> JSONResponse:
        return JSONResponse({"error": type(error).__name__}, status_code=400)

    @app.exception_handler(StoreError)
    @app.exception_handler(httpx.HTTPError)
    @app.exception_handler(BeaconError)
    async def unavailable(request: Request, error: Exception) -> JSONResponse:
        return JSONResponse({"error": type(error).__name__}, status_code=503)

    async def body(request: Request) -> bytes:
        data = bytearray()
        async for part in request.stream():
            data.extend(part)
            if len(data) > (1 << 20) + 4096:
                raise HTTPException(413, "control message too large")
        return bytes(data)

    async def upload_capability(request: Request, grant_hash: str) -> None:
        value = request.headers.get("X-Upload-Grant", "")
        if len(value) > 8192:
            raise RelayError("grant header exceeds bound")
        try:
            raw = relay_envelope.parse_envelope(base64.b64decode(value, validate=True))
        except binascii.Error:
            raise RelayError("malformed signed upload capability") from None
        grant = relay.master_message(raw, UploadGrant, await relay.now())
        if grant.digest() != grant_hash:
            raise RelayError("upload capability path mismatch")

    @app.get("/livez")
    async def live() -> JSONResponse:
        return JSONResponse({"live": True})

    @app.get("/readyz")
    async def ready() -> JSONResponse:
        try:
            value = await relay.ready()
        except RelayError:
            value = False
        return JSONResponse({"ready": value}, status_code=200 if value else 503)

    @app.get("/probe")
    async def probe() -> JSONResponse:
        return JSONResponse(
            {"relay_id": relay.spec.id, "region": relay.spec.region, "beacon": await relay.now()}
        )

    @app.post("/v1/uploads/{grant_hash}/accept")
    async def accept(grant_hash: str, request: Request) -> JSONResponse:
        parsed = AcceptRequest.model_validate(envelope_v2.load_json(await body(request)))
        if UploadGrant.model_validate(parsed.grant.body).digest() != grant_hash:
            raise RelayError("path grant differs from signed capability")
        ack = await relay.accept(
            parsed.grant, parsed.manifest, parsed.assignment, parsed.round_open
        )
        return JSONResponse(ack.model_dump(mode="json"))

    @app.put("/v1/uploads/{grant_hash}/chunks/{index}")
    async def chunk(grant_hash: str, index: int, request: Request) -> JSONResponse:
        await upload_capability(request, grant_hash)
        ack = await relay.chunk(grant_hash, index, request.stream())
        return JSONResponse(ack.model_dump(mode="json"))

    @app.post("/v1/uploads/{grant_hash}/complete")
    async def complete(grant_hash: str, request: Request) -> JSONResponse:
        await upload_capability(request, grant_hash)
        receipt = await relay.complete(grant_hash)
        return JSONResponse(receipt.model_dump(mode="json"))

    @app.get("/v1/uploads/{grant_hash}/acks")
    async def acks(grant_hash: str, request: Request, cursor: int = 0) -> JSONResponse:
        await upload_capability(request, grant_hash)
        if not 0 <= cursor <= 512:
            raise RelayError("ack cursor exceeds bounded chunk list")
        return JSONResponse(
            [e.model_dump(mode="json") for e in await relay.acks(grant_hash, cursor)]
        )

    @app.post("/v1/uploads/{grant_hash}/retention")
    async def retention(grant_hash: str, request: Request) -> JSONResponse:
        raw = relay_envelope.parse_envelope(await body(request))
        if raw.body.get("grant_hash") != grant_hash:
            raise RelayError("retention path mismatch")
        ack = await relay.retention(raw)
        return JSONResponse(ack.model_dump(mode="json"))

    @app.post("/v1/uploads/{grant_hash}/release")
    async def release(grant_hash: str, request: Request) -> JSONResponse:
        raw = relay_envelope.parse_envelope(await body(request))
        if raw.body.get("grant_hash") != grant_hash:
            raise RelayError("release path mismatch")
        result = await relay.release(raw)
        return JSONResponse(result.model_dump(mode="json"))

    @app.post("/v1/retrievals")
    async def retrieval(request: Request) -> JSONResponse:
        result = await relay.retrieve(relay_envelope.parse_envelope(await body(request)))
        return JSONResponse(result.model_dump(mode="json"))

    @app.get("/v1/objects/{sha}")
    async def object_stream(sha: str, request: Request) -> StreamingResponse:
        size = await relay.authorize_object(sha, request.headers.get("X-Retrieval-Hash", ""))

        async def stream() -> AsyncIterator[bytes]:
            with spool() as file:
                await relay.backing.get(sha, size, file)
                async for data in blocks(file):
                    yield data

        return StreamingResponse(
            stream(), media_type="application/octet-stream", headers={"Content-Length": str(size)}
        )

    @app.post("/internal/drain")
    async def drain(request: Request) -> JSONResponse:
        token = request.headers.get("Authorization", "").removeprefix("Bearer ")
        if not hmac.compare_digest(token, drain_token):
            raise HTTPException(403, "drain denied")
        relay.draining = True
        await relay.recover()
        return JSONResponse({"draining": True})

    return app


@asynccontextmanager
async def runtime(app: FastAPI) -> AsyncIterator[None]:
    config_bytes = await anyio.to_thread.run_sync(
        Path(os.environ.get("RELAY_CONFIG", "/config/relay.json")).read_bytes
    )
    config = RelayConfig.model_validate(envelope_v2.load_json(config_bytes))
    if (
        config.registry.signer != config.master_public
        or config.registry.run_id != config.run_id
        or config.registry.type != "RelayRegistryV1"
        or not relay_envelope.verify_envelope(config.registry.model_dump())
    ):
        raise RelayError("registry lacks pinned master signature")
    registry = RelayRegistryV1.model_validate(config.registry.body)
    if (
        config.network_manifest.signer != config.master_public
        or config.network_manifest.run_id != config.run_id
        or config.network_manifest.type != "RelayNetworkManifest"
        or not relay_envelope.verify_envelope(config.network_manifest.model_dump())
    ):
        raise RelayError("network manifest lacks pinned master signature")
    network = RelayNetworkManifest.model_validate(config.network_manifest.body)
    if (
        network.run_id != config.run_id
        or network.registry_hash != registry.digest()
        or network.specs_hash
        != hashlib.sha256(
            canonicalize([s.body() for s in registry.specs], allow_float=False)
        ).hexdigest()
    ):
        raise RelayError("network manifest registry linkage mismatch")
    secret_root = Path(os.environ.get("RELAY_SECRETS", "/run/relay-secrets"))
    keys: dict[str, Keypair] = {}
    for key_id, filename in config.signing_files.items():
        if Path(filename).name != filename:
            raise RelayError("signing file must remain in mounted secret directory")
        keys[key_id] = Keypair(bytes.fromhex((secret_root / filename).read_text().strip()))
    drain_token = (secret_root / "drain.token").read_text().strip()
    async with http_client() as client:
        if config.local_backing is not None:
            store: LocalFSStore | S3Store = LocalFSStore(Path(config.local_backing))
        elif config.s3_endpoint is not None and config.s3_bucket is not None:
            with tempfile.NamedTemporaryFile() as credentials:
                credentials.write((secret_root / "object.json").read_bytes())
                credentials.flush()
                os.fchmod(credentials.fileno(), 0o600)
                object_credentials = S3Credentials.from_secret_file(Path(credentials.name))
            store = S3Store(
                config.s3_endpoint,
                config.s3_bucket,
                object_credentials,
                region=config.s3_region,
                prefix=config.s3_prefix,
            )
        else:
            raise RelayError("durable external object backing not configured")
        drand = DrandQuicknet()

        async def now() -> int:
            beacon = await anyio.to_thread.run_sync(drand.latest)
            expected = round_at_or_after(int(time.time()), QUICKNET_GENESIS, QUICKNET_PERIOD) - 1
            if not beacon.bls_verified or abs(expected - beacon.round) > 2:
                raise RelayError("verified beacon stale; eligibility/deadlines frozen")
            return beacon.round

        async def forward(raw: RelayEnvelope) -> RelayEnvelope:
            return await forward_receipt(client, config.master_url, raw)

        relay = Relay(
            run_id=config.run_id,
            master=config.master_public,
            registry=registry,
            network_manifest_hash=network.digest(),
            observers=set(config.observer_public_keys),
            relay_id=config.relay_id,
            region=config.region,
            keys=keys,
            active_key=config.active_key,
            backing=StreamStore(store, client),
            now=now,
            forward=forward,
        )
        if min(config.registry.exp_drand, config.network_manifest.exp_drand) < await now():
            raise RelayError("pinned registry/network manifest expired")
        live_app = create_app(relay, drain_token)
        app.mount("/", live_app)
        # Recovery is real and fail-closed; missing L0 receipt route cannot become MASTER_ACCEPTED.
        await relay.recover()
        yield
        relay.draining = True


app = FastAPI(lifespan=runtime, docs_url=None, redoc_url=None, openapi_url=None)
