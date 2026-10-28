"""Relay HTTP boundary; runtime keys/config mounted separately from coordinator secrets."""

from __future__ import annotations

import base64
import binascii
import hashlib
import hmac
import logging
import os
import tempfile
import time
from collections.abc import AsyncIterator, Callable
from contextlib import asynccontextmanager
from pathlib import Path

import anyio
import httpx
from fastapi import FastAPI, HTTPException, Request
from fastapi.responses import JSONResponse, StreamingResponse
from pydantic import BaseModel, ConfigDict, Field, ValidationError

from hypertrain.beacon.core import BeaconError, BeaconRound, round_at_or_after
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
from hypertrain.relay.client import forward_receipt, http_client, settlement_from_master
from hypertrain.relay.core import Relay, RelayError

# Seconds between two passes that delete released or lapsed custody and prune its records.
COLLECT_INTERVAL = 300
# Storage faults only: disk I/O or timeout, store refusal, S3 transport, an unparseable journal.
_STORAGE_DOWN = (OSError, StoreError, httpx.HTTPError, ValidationError)


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
    # Omitted when unset, so a relay.json that does not use it still loads in an older image.
    max_uploads: int | None = Field(None, ge=1, exclude_if=lambda v: v is None)


class _Fetch:
    """One drand fetch in flight, shared by every caller that arrives while it runs."""

    def __init__(self) -> None:
        self.done = anyio.Event()
        self.beacon: BeaconRound | None = None
        self.error: BaseException | None = None


class BeaconCache:
    """Verified drand beacon shared by concurrent callers and held for one drand period.

    The staleness bound is applied on every call, against the wall clock, to the beacon that
    call would use: a held beacon that no longer passes it is replaced, not served, and a fetch
    that does not pass it is refused.
    """

    def __init__(
        self,
        latest: Callable[[], BeaconRound],
        wall: Callable[[], float],
        monotonic: Callable[[], float],
    ) -> None:
        self._latest = latest
        self._wall = wall
        self._monotonic = monotonic
        self._held: tuple[BeaconRound, float] | None = None
        self._fetch: _Fetch | None = None

    def _usable(self, beacon: BeaconRound) -> bool:
        expected = round_at_or_after(int(self._wall()), QUICKNET_GENESIS, QUICKNET_PERIOD) - 1
        return beacon.bls_verified and abs(expected - beacon.round) <= 2

    async def _fetched(self) -> BeaconRound:
        while (fetch := self._fetch) is not None:
            await fetch.done.wait()
            if fetch.error is not None:
                raise fetch.error
            if fetch.beacon is not None:
                return fetch.beacon
            # The fetching caller was interrupted without an outcome: fetch again.
        fetch = self._fetch = _Fetch()
        try:
            beacon = await anyio.to_thread.run_sync(self._latest)
            fetch.beacon = beacon
            # Timed from the end of the fetch: BLS verification can take seconds on a busy host.
            self._held = (beacon, self._monotonic())
            return beacon
        except Exception as error:
            fetch.error = error
            raise
        finally:
            self._fetch = None
            fetch.done.set()

    async def now(self) -> int:
        held = self._held
        if (
            held is not None
            and self._monotonic() - held[1] < QUICKNET_PERIOD
            and self._usable(held[0])
        ):
            return held[0].round
        beacon = await self._fetched()
        if not self._usable(beacon):
            raise RelayError("verified beacon stale; eligibility/deadlines frozen")
        return beacon.round


def create_app(relay: Relay, drain_token: str) -> FastAPI:
    if len(drain_token) < 32:
        raise RelayError("drain credential must contain >=32 characters")
    app = FastAPI(docs_url=None, redoc_url=None, openapi_url=None)

    @app.exception_handler(RelayError)
    async def refused(request: Request, error: Exception) -> JSONResponse:
        # Every RelayError message is a fixed string; ValidationError and EnvelopeError texts can
        # echo request content, so those two stay name-only.
        return JSONResponse({"error": type(error).__name__, "detail": str(error)}, status_code=400)

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
        log = logging.getLogger(__name__)
        try:
            value = await relay.ready()
        except RelayError:
            value = False
        except _STORAGE_DOWN as error:
            log.warning("relay readiness probe failed: %s", type(error).__name__)
            value = False
        # The fill fields stay in the answer, null when the journal cannot be read, so a client
        # that parses them sees one shape whether or not storage is up.
        fill: dict[str, object]
        try:
            report = await relay.journal_fill()
        except _STORAGE_DOWN as error:
            log.warning("relay journal fill unreadable: %s", type(error).__name__)
            value = False
            fill = dict.fromkeys(("journal_bytes", "journal_budget", "journal_fill", "alarm"))
        else:
            fill = {
                "journal_bytes": report.used,
                "journal_budget": report.budget,
                "journal_fill": round(report.ratio, 4),
                "alarm": report.alarm,
            }
        return JSONResponse({"ready": value, **fill}, status_code=200 if value else 503)

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


async def collect_forever(relay: Relay) -> None:
    """Rotate custody out of the journal without an operator; one failed pass ends nothing."""
    while True:
        await anyio.sleep(COLLECT_INTERVAL)
        try:
            await relay.collect()
        except (RelayError, StoreError, httpx.HTTPError, OSError) as error:
            logging.getLogger(__name__).warning("relay collect deferred: %s", type(error).__name__)


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
        now = BeaconCache(DrandQuicknet().latest, time.time, time.monotonic).now

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
            max_uploads=config.max_uploads,
        )
        relay.settlement = settlement_from_master(relay, client, config.master_url)
        if min(config.registry.exp_drand, config.network_manifest.exp_drand) < await now():
            raise RelayError("pinned registry/network manifest expired")
        live_app = create_app(relay, drain_token)
        app.mount("/", live_app)
        # Recovery is real and fail-closed; missing L0 receipt route cannot become MASTER_ACCEPTED.
        await relay.recover()
        async with anyio.create_task_group() as tasks:
            tasks.start_soon(collect_forever, relay)
            yield
            tasks.cancel_scope.cancel()
        relay.draining = True


app = FastAPI(lifespan=runtime, docs_url=None, redoc_url=None, openapi_url=None)
