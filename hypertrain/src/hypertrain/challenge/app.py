"""FastAPI container: Cortex contract v1, admin lifecycle, public miner routes, auditor queue."""

from __future__ import annotations

import asyncio
import hashlib
import hmac
import json
import os
import time
from collections.abc import Awaitable, Callable, Mapping
from dataclasses import dataclass
from pathlib import Path
from typing import Annotated, Any

import httpx
from fastapi import FastAPI, Header, Query, Request
from fastapi.responses import JSONResponse, Response
from pydantic import BaseModel, ConfigDict, Field, StrictBool, StrictInt, ValidationError

from hypertrain.beacon import BeaconRound, parse_round
from hypertrain.challenge.store import ChallengeError, ChallengeStore
from hypertrain.data.store import LocalFSStore
from hypertrain.data.store import Store as ObjectStore
from hypertrain.ledger import Params, vest_rounds_for_q
from hypertrain.protocol.keys import KeyError_, Keypair, decode_hotkey
from hypertrain.protocol.messages import QUICKNET_GENESIS
from hypertrain.public_api import create_public_app

VERSION = "0.1.0"
BODY_MAX = 1024 * 1024
METAGRAPH_TTL = 30.0
HEX64 = r"^[0-9a-f]{64}$"
SS58 = r"^[1-9A-HJ-NP-Za-km-z]{46,50}$"
Auth = Annotated[str | None, Header()]


class _Strict(BaseModel):
    model_config = ConfigDict(extra="forbid")


class RunConfig(_Strict):
    train_rounds: StrictInt = Field(ge=1, le=10**6)
    final_after_upload: StrictInt = Field(ge=1, le=10**6)
    total_rounds: StrictInt | None = Field(default=None, ge=1, le=10**6)


class Paused(_Strict):
    paused: StrictBool


class RosterEntry(_Strict):
    bond: StrictBool = False
    probation: StrictBool = False
    cluster: str | None = Field(default=None, max_length=64)
    region: str | None = Field(default=None, max_length=64)
    flags: list[str] = Field(default_factory=list, max_length=8)


class Honeypot(_Strict):
    commitment: str = Field(pattern=HEX64)


class HoneypotMember(_Strict):
    hotkey: str = Field(pattern=SS58)
    mode: str = Field(pattern=r"^(honest|fabricate|last_step|noise)$")


class HoneypotReveal(_Strict):
    commitment: str = Field(pattern=HEX64)
    members: list[HoneypotMember] = Field(max_length=4096)
    salt: str = Field(pattern=HEX64)


class LeaseBody(_Strict):
    lease: str = Field(max_length=64)


class CompleteBody(LeaseBody):
    verdict: dict[str, Any]


class FailBody(LeaseBody):
    reason: str = Field(max_length=500)
    retry: StrictBool = True


class RoundStateBody(_Strict):
    theta_start_sha256: str = Field(pattern=HEX64)
    v0_sha256: str | None = Field(default=None, pattern=HEX64)


class LeavesBody(_Strict):
    w: StrictInt = Field(ge=0)
    hotkey: str = Field(pattern=SS58)
    preimages: list[dict[str, Any]] = Field(max_length=4097)
    ef_in_sha256: str | None = Field(default=None, pattern=HEX64)


class RerunBody(_Strict):
    w: StrictInt = Field(ge=0)
    hotkey: str = Field(pattern=SS58)
    leaves_root: str = Field(pattern=HEX64)
    sig: str = Field(pattern=r"^[0-9a-f]{128}$")


class UploadRequest(_Strict):
    w: StrictInt = Field(ge=0)
    hotkey: str = Field(pattern=SS58)
    sha256: str = Field(pattern=HEX64)


@dataclass
class Config:
    slug: str
    state_dir: Path
    master_url: str
    internal_token_file: Path | None
    admin_token_file: Path | None
    worker_token_file: Path | None
    coord_key_file: Path | None
    owner_hotkey: str | None
    params: Params

    @classmethod
    def from_env(cls) -> Config:
        def path(name: str) -> Path | None:
            value = os.environ.get(name)
            return Path(value) if value else None

        env = os.environ.get
        return cls(
            slug=env("CHALLENGE_SLUG", "hypertrain"),
            state_dir=Path(env("CHALLENGE_STATE_DIR", "/data")),
            master_url=env("CHALLENGE_MASTER_URL", "http://cortex-master:8080"),
            internal_token_file=path("CHALLENGE_INTERNAL_TOKEN_FILE"),
            admin_token_file=path("CHALLENGE_ADMIN_TOKEN_FILE"),
            worker_token_file=path("CHALLENGE_WORKER_TOKEN_FILE"),
            coord_key_file=path("HYPERTRAIN_COORD_KEY_FILE"),
            owner_hotkey=env("HYPERTRAIN_OWNER_HOTKEY") or None,
            params=Params(
                challenge_slug=env("CHALLENGE_SLUG", "hypertrain"),
                genesis_unix=int(env("HYPERTRAIN_GENESIS_UNIX", str(QUICKNET_GENESIS))),
                epoch_seconds=int(env("HYPERTRAIN_EPOCH_SECONDS", "4320")),
                epochs_per_round=int(env("HYPERTRAIN_EPOCHS_PER_ROUND", "1")),
                vest_rounds=int(env("HYPERTRAIN_VEST_ROUNDS", str(vest_rounds_for_q("0.1")))),
            ),
        )


def _token(path: Path | None) -> str | None:
    """Read per request: the supervisor canary runs with no secret files at all."""
    if path is None:
        return None
    try:
        token = path.read_text().strip()
    except OSError:
        return None
    return token or None


def _require(path: Path | None, authorization: str | None) -> None:
    expected = _token(path)
    if expected is None:
        raise ChallengeError(503, "this route is not configured")
    if not authorization or not authorization.startswith("Bearer "):
        raise ChallengeError(401, "unauthorized")
    presented = authorization.removeprefix("Bearer ").strip()
    if not hmac.compare_digest(
        hashlib.sha256(presented.encode()).digest(), hashlib.sha256(expected.encode()).digest()
    ):
        raise ChallengeError(401, "unauthorized")


def load_coord_key(path: Path | None) -> Keypair | None:
    """K_coord seed file: 32 raw bytes or 64 hex chars. Never the Cortex leaf seed."""
    if path is None:
        return None
    try:
        raw = path.read_bytes().strip()
    except OSError:
        return None
    try:
        seed = bytes.fromhex(raw.decode()) if len(raw) == 64 else raw
        return Keypair(seed)
    except (ValueError, KeyError_):
        return None


class Metagraph:
    """GET {master}/v1/metagraph/latest -> {"hotkeys": {ss58: uid}}, cached briefly."""

    def __init__(self, url: str, client: httpx.AsyncClient) -> None:
        self.url, self.client = url.rstrip("/") + "/v1/metagraph/latest", client
        self._cache: tuple[float, Mapping[str, Any]] | None = None
        self._lock = asyncio.Lock()

    async def hotkeys(self) -> Mapping[str, Any]:
        async with self._lock:
            if self._cache and time.monotonic() - self._cache[0] < METAGRAPH_TTL:
                return self._cache[1]
            try:
                response = await self.client.get(self.url, timeout=10)
                response.raise_for_status()
                hotkeys = response.json()["hotkeys"]
                if not isinstance(hotkeys, dict):
                    raise TypeError("hotkeys must be an object")
            except (httpx.HTTPError, ValueError, KeyError, TypeError) as error:
                raise ChallengeError(503, "the metagraph is unavailable, retry later") from error
            self._cache = (time.monotonic(), hotkeys)
            return hotkeys


def _no_duplicates(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    out: dict[str, Any] = {}
    for key, value in pairs:
        if key in out:
            raise ChallengeError(400, f"duplicate JSON key {key!r}")
        out[key] = value
    return out


def _reject_constant(value: str) -> Any:
    raise ChallengeError(400, f"non-finite number {value}")


async def read_json(request: Request) -> Any:
    """Raw body (<= 1 MiB, else 413) parsed with duplicate-key and NaN rejection."""
    length = request.headers.get("content-length")
    if length is not None and (not length.isdigit() or int(length) > BODY_MAX):
        raise ChallengeError(413, f"the body exceeds {BODY_MAX} bytes")
    raw = bytearray()
    async for chunk in request.stream():
        raw += chunk
        if len(raw) > BODY_MAX:
            raise ChallengeError(413, f"the body exceeds {BODY_MAX} bytes")
    try:
        return json.loads(raw, object_pairs_hook=_no_duplicates, parse_constant=_reject_constant)
    except ValueError:
        raise ChallengeError(400, "body is not valid JSON") from None


async def read_model[M: BaseModel](request: Request, model: type[M]) -> M:
    data = await read_json(request)
    try:
        return model.model_validate(data)
    except ValidationError as error:
        first = error.errors()[0]
        where = ".".join(str(p) for p in first.get("loc", ()))
        raise ChallengeError(422, f"{where}: {first.get('msg', 'invalid')}") from None


def create_app(
    config: Config,
    *,
    clock: Callable[[], float] = time.time,
    transport: httpx.AsyncBaseTransport | None = None,
    verify_beacon: Callable[[Mapping[str, Any]], BeaconRound] = parse_round,
    objects: ObjectStore | None = None,
) -> FastAPI:
    store = ChallengeStore(
        config.state_dir,
        config.params,
        load_coord_key(config.coord_key_file),
        config.owner_hotkey,
        verify_beacon,
        objects or LocalFSStore(config.state_dir / "objects"),
    )
    client = httpx.AsyncClient(transport=transport)
    metagraph = Metagraph(config.master_url, client)
    app = FastAPI(
        title="hypertrain challenge",
        version=VERSION,
        docs_url=None,
        redoc_url=None,
        openapi_url=None,
    )
    app.state.store = store

    def run(function: Callable[..., Any], *args: Any) -> Awaitable[Any]:
        return asyncio.to_thread(function, *args)

    @app.exception_handler(ChallengeError)
    async def challenge_error(_: Request, error: ChallengeError) -> JSONResponse:
        return JSONResponse({"detail": error.detail}, status_code=error.status)

    def admin(authorization: str | None) -> None:
        _require(config.admin_token_file, authorization)

    def worker(authorization: str | None) -> None:
        _require(config.worker_token_file, authorization)

    async def registered(env: Any) -> None:
        signer = env.get("signer") if isinstance(env, dict) else None
        if not isinstance(signer, str):
            raise ChallengeError(400, "envelope signer missing")
        try:
            decode_hotkey(signer)
        except KeyError_:
            raise ChallengeError(400, "signer is not a valid SS58 hotkey") from None
        if signer not in await metagraph.hotkeys():
            raise ChallengeError(403, "the hotkey is not registered on the subnet")

    @app.get("/health")
    async def health() -> JSONResponse:
        ok = (
            await run(store.healthy)
            and _token(config.internal_token_file) is not None
            and store.coord is not None
        )
        return JSONResponse({"ok": ok}, status_code=200 if ok else 503)

    @app.get("/version")
    async def version() -> dict[str, Any]:
        return {
            "slug": config.slug,
            "version": VERSION,
            "contract": 1,
            "capabilities": ["get_weights", "proxy_routes"],
        }

    @app.get("/internal/v1/get_weights")
    async def get_weights(
        epoch: Annotated[int, Query(ge=0, lt=2**63)],
        authorization: Auth = None,
        x_platform_challenge_slug: Auth = None,
        epoch_at: Annotated[int | None, Query(ge=0, lt=2**63)] = None,
    ) -> Response:
        _require(config.internal_token_file, authorization)
        if x_platform_challenge_slug != config.slug:
            raise ChallengeError(403, "challenge slug mismatch")
        text = await run(store.weights, epoch, epoch_at, int(clock()))
        return Response(text, media_type="application/json")

    @app.post("/v1/admin/beacon")
    async def push_beacon(request: Request, authorization: Auth = None) -> dict[str, Any]:
        admin(authorization)
        payload = await read_json(request)
        if not isinstance(payload, dict):
            raise ChallengeError(400, "beacon payload must be an object")
        result: dict[str, Any] = await run(store.push_beacon, payload)
        return result

    @app.post("/v1/admin/runs", status_code=201)
    async def create_run(request: Request, authorization: Auth = None) -> dict[str, Any]:
        admin(authorization)
        result: dict[str, Any] = await run(store.create_run, await read_json(request))
        return result

    @app.put("/v1/admin/runs/{run_id}/config")
    async def configure(run_id: str, request: Request, authorization: Auth = None) -> Any:
        admin(authorization)
        item = await read_model(request, RunConfig)
        return await run(store.configure, run_id, item.model_dump(exclude_none=True))

    @app.put("/v1/admin/runs/{run_id}/paused")
    async def paused(run_id: str, request: Request, authorization: Auth = None) -> Any:
        admin(authorization)
        item = await read_model(request, Paused)
        return await run(store.set_paused, run_id, item.paused)

    @app.put("/v1/admin/runs/{run_id}/roster/{hotkey}")
    async def roster_put(
        run_id: str, hotkey: str, request: Request, authorization: Auth = None
    ) -> Any:
        admin(authorization)
        item = await read_model(request, RosterEntry)
        try:
            decode_hotkey(hotkey)
        except KeyError_:
            raise ChallengeError(400, "invalid hotkey") from None
        return await run(store.set_roster, run_id, hotkey, item.model_dump())

    @app.delete("/v1/admin/runs/{run_id}/roster/{hotkey}")
    async def roster_delete(run_id: str, hotkey: str, authorization: Auth = None) -> Any:
        admin(authorization)
        return await run(store.set_roster, run_id, hotkey, None)

    @app.put("/v1/admin/runs/{run_id}/honeypot")
    async def honeypot(run_id: str, request: Request, authorization: Auth = None) -> Any:
        admin(authorization)
        item = await read_model(request, Honeypot)
        return await run(store.set_honeypot, run_id, item.commitment)

    @app.post("/v1/admin/runs/{run_id}/honeypot/reveal")
    async def honeypot_reveal(run_id: str, request: Request, authorization: Auth = None) -> Any:
        admin(authorization)
        item = await read_model(request, HoneypotReveal)
        members = [m.model_dump() for m in item.members]
        return await run(store.reveal_honeypot, run_id, item.commitment, members, item.salt)

    @app.get("/v1/runs")
    async def runs() -> dict[str, Any]:
        return {"runs": await run(store.runs)}

    @app.get("/v1/beacon/latest")
    async def beacon_latest() -> Any:
        return await run(store.latest_beacon)

    @app.get("/v1/runs/{run_id}")
    async def run_status(run_id: str) -> Any:
        return await run(store.run_status, run_id)

    @app.get("/v1/runs/{run_id}/rounds/{w}")
    async def round_view(run_id: str, w: int) -> Any:
        return await run(store.round_view, run_id, w)

    @app.get("/v1/runs/{run_id}/auditor-stats")
    @app.get("/run/{run_id}/auditor-stats")
    async def auditor_stats(run_id: str) -> Any:
        return await run(store.auditor_stats, run_id)

    def miner_route(path: str, method: Callable[[str, Any], dict[str, Any]]) -> None:
        async def handler(run_id: str, request: Request) -> Any:
            env = await read_json(request)
            await registered(env)
            return await run(method, run_id, env)

        app.post(path, name=method.__name__)(handler)

    miner_route("/v1/runs/{run_id}/accept", store.accept)
    miner_route("/v1/runs/{run_id}/commit", store.commit)
    miner_route("/v1/runs/{run_id}/delta", store.delta)
    miner_route("/v1/runs/{run_id}/state", store.state_serve)
    miner_route("/v1/runs/{run_id}/dispute", store.dispute)

    @app.post("/v1/runs/{run_id}/bisect")
    async def bisect(run_id: str, request: Request) -> Any:
        return await run(store.bisect, run_id, await read_json(request))

    @app.post("/v1/runs/{run_id}/resolution")
    async def resolution(run_id: str, request: Request, authorization: Auth = None) -> Any:
        worker(authorization)
        return await run(store.resolution, run_id, await read_json(request))

    @app.post("/v1/runs/{run_id}/uploads")
    async def upload_url(run_id: str, request: Request) -> Any:
        item = await read_model(request, UploadRequest)
        await registered({"signer": item.hotkey})
        return await run(store.upload_url, run_id, item.w, item.hotkey, item.sha256)

    @app.post("/v1/worker/lease")
    async def lease(authorization: Auth = None) -> Response:
        worker(authorization)
        job = await run(store.lease)
        if job is None:
            return Response(status_code=204)
        return JSONResponse(job)

    @app.get("/v1/worker/jobs/{job_id}/serves")
    async def serves(job_id: str, lease: str, authorization: Auth = None) -> Any:
        worker(authorization)
        return await run(store.serves, job_id, lease)

    @app.get("/v1/objects/{sha256}")
    async def get_object(sha256: str, authorization: Auth = None) -> Response:
        worker(authorization)
        data = await run(store.get_object, sha256)
        return Response(data, media_type="application/octet-stream")

    @app.post("/v1/runs/{run_id}/leaves")
    async def leaves(run_id: str, request: Request) -> Any:
        item = await read_model(request, LeavesBody)
        await registered({"signer": item.hotkey})
        return await run(
            store.leaves, run_id, item.w, item.hotkey, item.preimages, item.ef_in_sha256
        )

    @app.post("/v1/runs/{run_id}/rerun")
    async def rerun(run_id: str, request: Request) -> Any:
        item = await read_model(request, RerunBody)
        await registered({"signer": item.hotkey})
        return await run(store.rerun, run_id, item.w, item.hotkey, item.leaves_root, item.sig)

    @app.put("/v1/aggregator/runs/{run_id}/rounds/{w}/state")
    async def round_state(run_id: str, w: int, request: Request, authorization: Auth = None) -> Any:
        admin(authorization)
        item = await read_model(request, RoundStateBody)
        return await run(store.set_round_state, run_id, w, item.theta_start_sha256, item.v0_sha256)

    @app.post("/v1/worker/jobs/{job_id}/heartbeat")
    async def heartbeat(job_id: str, request: Request, authorization: Auth = None) -> Any:
        worker(authorization)
        item = await read_model(request, LeaseBody)
        return await run(store.heartbeat, job_id, item.lease)

    @app.post("/v1/worker/jobs/{job_id}/complete")
    async def complete(job_id: str, request: Request, authorization: Auth = None) -> Any:
        worker(authorization)
        item = await read_model(request, CompleteBody)
        return await run(store.complete, job_id, item.lease, item.verdict)

    @app.post("/v1/worker/jobs/{job_id}/fail")
    async def fail(job_id: str, request: Request, authorization: Auth = None) -> Any:
        worker(authorization)
        item = await read_model(request, FailBody)
        return await run(store.fail, job_id, item.lease, item.reason, item.retry)

    @app.get("/v1/aggregator/runs/{run_id}/rounds/{w}/inputs")
    async def inputs(
        run_id: str,
        w: int,
        authorization: Auth = None,
        d_open: Annotated[int | None, Query(ge=1)] = None,
    ) -> Any:
        admin(authorization)
        return await run(store.inputs, run_id, w, d_open)

    def aggregator_route(path: str, method: Callable[[str, int, Any], dict[str, Any]]) -> None:
        async def handler(run_id: str, w: int, request: Request, authorization: Auth = None) -> Any:
            admin(authorization)
            return await run(method, run_id, w, await read_json(request))

        app.post(path, name=method.__name__)(handler)

    aggregator_route("/v1/aggregator/runs/{run_id}/rounds/{w}/aggregate", store.aggregate)
    aggregator_route("/v1/aggregator/runs/{run_id}/rounds/{w}/rollback", store.rollback)
    aggregator_route("/v1/aggregator/runs/{run_id}/rounds/{w}/finalize", store.finalize)

    app.mount(
        "/public",
        create_public_app(
            config.state_dir / "challenge.db", config.state_dir / "public" / "metrics.jsonl"
        ),
    )
    return app


def __getattr__(name: str) -> FastAPI:
    """`uvicorn hypertrain.challenge.app:app` builds the app from the environment on import."""
    if name == "app":
        return create_app(Config.from_env())
    raise AttributeError(name)
