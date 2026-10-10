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
from pydantic import (
    BaseModel,
    ConfigDict,
    Field,
    StrictBool,
    StrictInt,
    ValidationError,
)

from hypertrain.beacon import BeaconRound, parse_round
from hypertrain.challenge.admission_store import AdmissionError
from hypertrain.challenge.disputes_v2 import DisputeError, EventAck
from hypertrain.challenge.store import ChallengeError, ChallengeStore
from hypertrain.data.store import LocalFSStore
from hypertrain.data.store import Store as ObjectStore
from hypertrain.ledger import Params, vest_rounds_for_q
from hypertrain.ledger.escrow_v2 import EscrowError
from hypertrain.protocol.envelope import EnvelopeError
from hypertrain.protocol.envelope_v2 import load_json
from hypertrain.protocol.jcs import canonicalize
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
    netuid: int = 100

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
            netuid=int(env("CHALLENGE_NETUID", "100")),
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
        hashlib.sha256(presented.encode()).digest(),
        hashlib.sha256(expected.encode()).digest(),
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
    """GET {master}/v1/metagraph/latest?netuid=N -> {"netuid": N, "hotkeys": {ss58: uid}}.

    The response must name the configured subnet; any other or missing netuid is 503.
    """

    def __init__(self, url: str, client: httpx.AsyncClient, netuid: int) -> None:
        self.url = url.rstrip("/") + f"/v1/metagraph/latest?netuid={netuid}"
        self.client, self.netuid = client, netuid
        self._cache: tuple[float, Mapping[str, Any]] | None = None
        self._lock = asyncio.Lock()

    async def hotkeys(self) -> Mapping[str, Any]:
        async with self._lock:
            if self._cache and time.monotonic() - self._cache[0] < METAGRAPH_TTL:
                return self._cache[1]
            try:
                response = await self.client.get(self.url, timeout=10)
                response.raise_for_status()
                data = response.json()
                hotkeys = data["hotkeys"]
                if not isinstance(hotkeys, dict):
                    raise TypeError("hotkeys must be an object")
            except (httpx.HTTPError, ValueError, KeyError, TypeError) as error:
                raise ChallengeError(503, "the metagraph is unavailable, retry later") from error
            if type(data.get("netuid")) is not int or data["netuid"] != self.netuid:
                raise ChallengeError(503, f"the metagraph is not for subnet netuid {self.netuid}")
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
    _store: ChallengeStore | None = None,
) -> FastAPI:
    if _store is not None and (
        _store.state_dir.resolve() != config.state_dir.resolve()
        or _store.owner_hotkey != config.owner_hotkey
        or _store.params != config.params
        or _store.coord is None
        or config.coord_key_file is None
        or (configured_coord := load_coord_key(config.coord_key_file)) is None
        or _store.coord.ss58 != configured_coord.ss58
        or objects is not None
        and _store.objects is not objects
    ):
        raise ValueError("trusted continuation store/config differs")
    store = (
        _store
        if _store is not None
        else ChallengeStore(
            config.state_dir,
            config.params,
            load_coord_key(config.coord_key_file),
            config.owner_hotkey,
            verify_beacon,
            objects or LocalFSStore(config.state_dir / "objects"),
        )
    )
    store.clock = clock
    client = httpx.AsyncClient(transport=transport)
    metagraph = Metagraph(config.master_url, client, config.netuid)
    app = FastAPI(
        title="hypertrain challenge",
        version=VERSION,
        docs_url=None,
        redoc_url=None,
        openapi_url=None,
    )
    app.state.store = store
    app.state.metagraph = metagraph

    async def subnet_member(hotkey: object) -> None:
        """Permissionless admission: any hotkey currently registered on the subnet."""
        if not isinstance(hotkey, str) or hotkey not in await metagraph.hotkeys():
            raise ChallengeError(403, "the hotkey is not registered on the subnet")

    def protocol_roles(run_id: str) -> set[str]:
        from hypertrain.protocol.messages_v2 import DisputePolicyV2

        hosted = store.for_run(run_id)
        manifest = hosted._run_v2(run_id)
        policy = DisputePolicyV2.model_validate_json(
            hosted.objects.get(manifest.network.dispute_policy_hash)
        )
        return {*manifest.training.auditors, manifest.training.coord_pubkey, *policy.referees}

    async def party_member(run_id: str, party: str) -> None:
        """Shared dispute routes: pinned auditors/referees/coordinator, else subnet miners."""
        if party not in await run(protocol_roles, run_id):
            await subnet_member(party)

    def locked_now() -> int:
        with store._lock:
            return store._now(store._db)

    def signer(raw: bytes) -> str:
        from hypertrain.protocol.envelope_v2 import parse_envelope

        return parse_envelope(raw).signer

    @app.on_event("shutdown")
    def close_notifications() -> None:
        store.close_v2_notifications()

    async def v2_raw(request: Request) -> bytes:
        data = bytearray()
        async for chunk in request.stream():
            data.extend(chunk)
            if len(data) > BODY_MAX + 1024:
                raise ChallengeError(413, "v2 body exceeds limit")
        load_json(bytes(data), max_bytes=BODY_MAX + 1024)
        return bytes(data)

    async def v2_error(_: Request, error: Exception) -> JSONResponse:
        return JSONResponse({"detail": str(error)}, status_code=409)

    for cls in (
        AdmissionError,
        DisputeError,
        EscrowError,
        EnvelopeError,
        ValidationError,
    ):
        app.add_exception_handler(cls, v2_error)

    @app.post("/v2/admin/runs", status_code=201)
    async def create_v2(request: Request, authorization: Auth = None) -> Any:
        admin(authorization)
        return await run(store.create_run_v2, await v2_raw(request))

    @app.get("/v2/runs/{run_id}")
    async def status_v2(run_id: str) -> Any:
        return await run(store.for_run(run_id).run_status_v2, run_id)

    @app.post("/v2/runs/{run_id}/join")
    async def join_v2(run_id: str, request: Request) -> Any:
        import ipaddress

        address = request.client.host if request.client else "127.0.0.1"
        try:
            ip = ipaddress.ip_address(address)
            prefix = str(
                ipaddress.ip_network(f"{ip}/{24 if ip.version == 4 else 64}", strict=False)
            )
        except ValueError:
            prefix = "local-transport"
        raw = await v2_raw(request)
        data = load_json(raw)
        await subnet_member(data.get("hotkey") if isinstance(data, dict) else None)
        return await run(store.for_run(run_id).admission_v2, run_id, "join", raw, "", prefix)

    @app.get("/v2/runs/{run_id}/join/{admission_id}/challenge")
    async def challenge_v2(run_id: str, admission_id: str) -> Any:
        # Opening a trial consumes replay capacity: only for currently registered hotkeys.
        _, admission, _ = await run(store.for_run(run_id)._services, run_id)
        await subnet_member((await run(admission.store.by_id, admission_id)).hotkey)
        return await run(store.for_run(run_id).admission_v2, run_id, "challenge", b"", admission_id)

    @app.get("/v2/runs/{run_id}/admission/{hotkey}")
    async def admission_status_v2(run_id: str, hotkey: str) -> Any:
        return await run(store.for_run(run_id).admission_v2, run_id, "status", b"", hotkey)

    @app.post("/v2/runs/{run_id}/join/proof")
    async def proof_v2(run_id: str, request: Request) -> Any:
        data = load_json(await v2_raw(request))
        if not isinstance(data, dict) or set(data) != {"proof", "screen"}:
            raise ChallengeError(422, "expected original signed proof and screen")
        proof, screen = canonicalize(data["proof"]), canonicalize(data["screen"])
        await subnet_member(signer(proof))
        _, admission, _ = await run(store.for_run(run_id)._services, run_id)
        # No operator in the loop: the service runs the trusted reference itself (committed
        # before the proof is compared), then signs trial finality through the same checks
        # as the admin routes, which remain available as overrides.
        due = await run(lambda: admission.reference_due(proof, now=locked_now()))
        if due is not None:
            await run(store.for_run(run_id).trial_reference_v2, run_id, due)
        record = await run(store.for_run(run_id).trial_proof_v2, run_id, proof, screen)
        final = await run(
            lambda: admission.trial_finality(record["admission_id"], now=locked_now())
        )
        if final is None:
            return record
        status = await run(
            store.for_run(run_id).trial_finalize_v2, run_id, record["admission_id"], final
        )
        return status["record"]

    @app.post("/v2/runs/{run_id}/admin/join/{admission_id}/reference")
    async def reference_v2(run_id: str, admission_id: str, authorization: Auth = None) -> Any:
        admin(authorization)
        return await run(store.for_run(run_id).trial_reference_v2, run_id, admission_id)

    @app.post("/v2/runs/{run_id}/admin/join/{admission_id}/finalize")
    async def trial_final_v2(
        run_id: str, admission_id: str, request: Request, authorization: Auth = None
    ) -> Any:
        admin(authorization)
        return await run(
            store.for_run(run_id).trial_finalize_v2, run_id, admission_id, await v2_raw(request)
        )

    @app.post("/v2/runs/{run_id}/admin/join/{admission_id}/shadow-reservation")
    async def shadow_reservation_v2(
        run_id: str, admission_id: str, request: Request, authorization: Auth = None
    ) -> Any:
        admin(authorization)
        return await run(
            store.for_run(run_id).shadow_reservation_v2, run_id, admission_id, await v2_raw(request)
        )

    @app.post("/v2/runs/{run_id}/admin/join/{admission_id}/shadow-reward")
    async def shadow_reward_v2(
        run_id: str, admission_id: str, request: Request, authorization: Auth = None
    ) -> Any:
        admin(authorization)
        return await run(
            store.for_run(run_id).shadow_reward_v2, run_id, admission_id, await v2_raw(request)
        )

    @app.post("/v2/runs/{run_id}/admission/{hotkey}/rotate")
    async def rotate_v2(run_id: str, hotkey: str, request: Request) -> Any:
        raw = await v2_raw(request)
        from hypertrain.protocol.envelope_v2 import parse_envelope

        body = parse_envelope(raw).body
        if body.get("hotkey") != hotkey:
            raise ChallengeError(422, "rotation path and body differ")
        await subnet_member(body.get("new_hotkey"))
        return await run(store.for_run(run_id).admission_v2, run_id, "rotate", raw)

    # Miner-hotkey signers are gated; escrow routes are coldkey-signed (not subnet hotkeys).
    miner_signed = {"recover"}
    party_signed = {"dispute", "bisect", "state-serve"}

    def v2_operation(path: str, action: str) -> None:
        async def handler(run_id: str, request: Request) -> Any:
            raw = await v2_raw(request)
            if action in miner_signed:
                await subnet_member(signer(raw))
            elif action in party_signed:
                await party_member(run_id, signer(raw))
            return await run(store.for_run(run_id).admission_v2, run_id, action, raw)

        app.post("/v2/runs/{run_id}/" + path, name="v2_" + action)(handler)

    for path, action in (
        ("escrow/lock", "lock"),
        ("escrow/transfer", "transfer"),
        ("escrow/release", "release"),
        ("dispute", "dispute"),
        ("bisect", "bisect"),
        ("resolution", "resolution"),
        ("state-serve", "state-serve"),
        ("admission/recover", "recover"),
    ):
        v2_operation(path, action)

    @app.put("/v2/runs/{run_id}/admin/dataset")
    async def dataset_v2(run_id: str, request: Request, authorization: Auth = None) -> Any:
        admin(authorization)
        data = load_json(await v2_raw(request))
        if not isinstance(data, dict):
            raise ChallengeError(422, "expected dataset object linkage")
        return await run(store.for_run(run_id).configure_inputs_v2, run_id, data)

    @app.post("/v2/runs/{run_id}/admin/genesis")
    async def genesis_v2(run_id: str, request: Request, authorization: Auth = None) -> Any:
        admin(authorization)
        return await run(store.for_run(run_id).genesis_v2, run_id, await v2_raw(request))

    @app.post("/v2/runs/{run_id}/admin/rounds")
    async def open_v2(run_id: str, request: Request, authorization: Auth = None) -> Any:
        admin(authorization)
        return await run(store.for_run(run_id).open_round_v2, run_id, await v2_raw(request))

    @app.put("/v2/runs/{run_id}/objects/{sha256}")
    async def put_object_v2(
        run_id: str,
        sha256: str,
        request: Request,
        x_hotkey: Annotated[str, Header()],
        x_object_signature: Annotated[str, Header()],
    ) -> Any:
        from hypertrain.protocol.keys import verify

        await subnet_member(x_hotkey)
        hosted = store.for_run(run_id)
        with hosted._lock:
            hosted._run_v2(run_id)
            hosted._owner_v2(run_id, x_hotkey)
        try:
            valid = verify(
                decode_hotkey(x_hotkey),
                f"hypertrain/object/2|{run_id}|{sha256}".encode(),
                bytes.fromhex(x_object_signature),
            )
        except (ValueError, KeyError_):
            valid = False
        if not valid:
            raise ChallengeError(403, "object signature mismatch")
        # Admission objects are separately bounded by the pinned policy.
        raw = bytearray()
        limit = hosted._services(run_id)[1].policy.artifact_limits.max_object_bytes
        async for chunk in request.stream():
            raw.extend(chunk)
            if len(raw) > limit:
                raise ChallengeError(413, "object exceeds pinned admission limit")
        if hashlib.sha256(raw).hexdigest() != sha256:
            raise ChallengeError(422, "object hash differs")
        key = await run(hosted.objects.put, bytes(raw))
        return {"sha256": key}

    @app.get("/v2/runs/{run_id}/rounds/{w}")
    async def round_v2(run_id: str, w: int) -> Any:
        return await run(store.for_run(run_id).round_view_v2, run_id, w)

    @app.get("/v2/runs/{run_id}/rounds/{w}/job/{hotkey}")
    async def job_v2(run_id: str, w: int, hotkey: str) -> Any:
        return await run(store.for_run(run_id).island_job_v2, run_id, w, hotkey)

    @app.get("/v2/runs/{run_id}/objects/{sha256}")
    async def object_v2(run_id: str, sha256: str) -> Response:
        hosted = store.for_run(run_id)
        hosted._run_v2(run_id)
        # Published run datasets/state are public, content identity still verifies.
        return Response(
            await run(hosted.objects.get, sha256), media_type="application/octet-stream"
        )

    for action in ("accept", "commit", "delta"):

        def add_training(action: str) -> None:
            async def handler(run_id: str, request: Request) -> Any:
                raw = await v2_raw(request)
                await subnet_member(signer(raw))
                return await run(store.for_run(run_id).training_v2, run_id, action, raw)

            app.post("/v2/runs/{run_id}/" + action, name="v2_" + action)(handler)

        add_training(action)

    @app.post("/v2/runs/{run_id}/leaves")
    async def leaves_v2(run_id: str, request: Request) -> Any:
        raw = await v2_raw(request)
        await subnet_member(signer(raw))
        return await run(store.for_run(run_id).leaves_v2, run_id, raw)

    @app.post("/v2/runs/{run_id}/admin/rounds/{w}/audits")
    async def audits_v2(run_id: str, w: int, authorization: Auth = None) -> Any:
        admin(authorization)
        return {"jobs": await run(store.for_run(run_id).schedule_audits_v2, run_id, w)}

    @app.post("/v2/runs/{run_id}/worker/lease")
    async def lease_v2(run_id: str, request: Request) -> Any:
        job = await run(store.for_run(run_id).lease_v2, run_id, await v2_raw(request))
        return Response(status_code=204) if job is None else job

    @app.post("/v2/runs/{run_id}/worker/jobs/{job_id}/execute")
    async def execute_v2(run_id: str, job_id: str, request: Request) -> Any:
        return await run(
            store.for_run(run_id).execute_audit_v2, run_id, job_id, await v2_raw(request)
        )

    @app.post("/v2/runs/{run_id}/worker/jobs/{job_id}/complete")
    async def complete_v2(run_id: str, job_id: str, request: Request) -> Any:
        return await run(
            store.for_run(run_id).complete_audit_v2, run_id, job_id, await v2_raw(request)
        )

    @app.post("/v2/runs/{run_id}/admin/rounds/{w}/aggregate")
    async def aggregate_v2(run_id: str, w: int, authorization: Auth = None) -> Any:
        admin(authorization)
        return await run(store.for_run(run_id).aggregate_v2, run_id, w)

    @app.post("/v2/runs/{run_id}/admin/rounds/{w}/finalize")
    async def final_v2(run_id: str, w: int, request: Request, authorization: Auth = None) -> Any:
        admin(authorization)
        return await run(store.for_run(run_id).finalize_v2, run_id, w, await v2_raw(request))

    @app.get("/v2/runs/{run_id}/rounds/{w}/relay/{hotkey}")
    async def assignment_v2(run_id: str, w: int, hotkey: str) -> Any:
        return await run(store.for_run(run_id).relay_assignment_v2, run_id, w, hotkey)

    @app.post("/v2/runs/{run_id}/upload-grant")
    async def grant_v2(run_id: str, request: Request, commit_hash: str | None = None) -> Any:
        from hypertrain.protocol import relay_envelope

        raw = await v2_raw(request)
        await subnet_member(relay_envelope.parse_envelope(raw).signer)
        return await run(store.for_run(run_id).upload_grant_v2, run_id, raw, commit_hash)

    @app.post("/v2/relay-receipts")
    async def receipt_v2(request: Request) -> Any:
        async with httpx.AsyncClient(follow_redirects=False, timeout=30) as transport:
            from hypertrain.protocol import relay_envelope

            raw = await v2_raw(request)
            hosted = store.for_run(relay_envelope.parse_envelope(raw).run_id)
            return await hosted.relay_receipt_v2(raw, transport)

    @app.get("/v2/runs/{run_id}/admin/relay-settlement/{grant_hash}")
    async def relay_settlement_v2(run_id: str, grant_hash: str, authorization: Auth = None) -> Any:
        admin(authorization)
        return await run(store.for_run(run_id).relay_settlement_v2, run_id, grant_hash)

    @app.post("/v2/runs/{run_id}/admin/disputes/{dispute_id}/state/{checkpoint}")
    async def dispute_state_v2(
        run_id: str, dispute_id: str, checkpoint: int, authorization: Auth = None
    ) -> Any:
        admin(authorization)
        return await run(store.for_run(run_id).dispute_state_v2, run_id, dispute_id, checkpoint)

    @app.post("/v2/runs/{run_id}/admin/rollback")
    async def rollback_v2(run_id: str, request: Request, authorization: Auth = None) -> Any:
        from hypertrain.aggregator.core import TapeError

        admin(authorization)
        try:
            return await run(store.for_run(run_id).rollback_v2, run_id, await v2_raw(request))
        except TapeError as error:
            raise ChallengeError(409, str(error)) from error

    @app.post("/v2/runs/{run_id}/admin/rollback-preview")
    async def rollback_preview_v2(run_id: str, request: Request, authorization: Auth = None) -> Any:
        from hypertrain.aggregator.core import TapeError

        admin(authorization)
        try:
            return await run(
                store.for_run(run_id).rollback_preview_v2, run_id, await v2_raw(request)
            )
        except TapeError as error:
            raise ChallengeError(409, str(error)) from error

    @app.post("/v2/runs/{run_id}/admin/disputes/{dispute_id}/referee")
    async def referee_v2(run_id: str, dispute_id: str, authorization: Auth = None) -> Any:
        admin(authorization)
        return await run(store.for_run(run_id).referee_v2, run_id, dispute_id)

    @app.post("/v2/runs/{run_id}/admin/disputes/{dispute_id}/timeout")
    async def timeout_v2(
        run_id: str, dispute_id: str, request: Request, authorization: Auth = None
    ) -> Any:
        admin(authorization)
        return await run(
            store.for_run(run_id).dispute_timeout_v2, run_id, dispute_id, await v2_raw(request)
        )

    @app.post("/v2/runs/{run_id}/admin/relay-registry")
    async def registry_v2(run_id: str, request: Request, authorization: Auth = None) -> Any:
        admin(authorization)
        return await run(store.for_run(run_id).rotate_registry_v2, run_id, await v2_raw(request))

    @app.post("/v2/runs/{run_id}/admin/relay/{grant_hash}/{action}")
    async def relay_control_v2(
        run_id: str, grant_hash: str, action: str, authorization: Auth = None
    ) -> Any:
        admin(authorization)
        async with httpx.AsyncClient(follow_redirects=False, timeout=30) as client:
            return await store.for_run(run_id).relay_control_v2(run_id, grant_hash, action, client)

    @app.post("/v2/runs/{run_id}/admin/relay-failure")
    async def failure_v2(run_id: str, request: Request, authorization: Auth = None) -> Any:
        admin(authorization)
        return await run(store.for_run(run_id).relay_failure_v2, run_id, await v2_raw(request))

    @app.get("/v2/runs/{run_id}/relay-observers/{grant_hash}/{observer}")
    async def observer_challenge_v2(run_id: str, grant_hash: str, observer: str) -> Any:
        return await run(store.for_run(run_id).relay_observer_v2, run_id, grant_hash, observer)

    @app.post("/v2/runs/{run_id}/relay-observers/{grant_hash}/{observer}")
    async def observer_response_v2(
        run_id: str, grant_hash: str, observer: str, request: Request
    ) -> Any:
        return await run(
            store.for_run(run_id).relay_observer_v2,
            run_id,
            grant_hash,
            observer,
            await v2_raw(request),
        )

    @app.post("/v2/runs/{run_id}/admin/relay-fallback/{failure_hash}")
    async def fallback_v2(run_id: str, failure_hash: str, authorization: Auth = None) -> Any:
        admin(authorization)
        async with httpx.AsyncClient(follow_redirects=False, timeout=30) as client:
            return await store.for_run(run_id).relay_fallback_v2(run_id, failure_hash, client)

    @app.post("/v2/runs/{run_id}/admin/relay-drain/{relay_id}")
    async def drain_v2(run_id: str, relay_id: str, authorization: Auth = None) -> Any:
        admin(authorization)
        return await run(store.for_run(run_id).relay_drain_v2, run_id, relay_id)

    @app.get("/v2/runs/{run_id}/disputes")
    async def events_v2(
        run_id: str,
        party: str,
        cursor: int = 0,
        wait_seconds: Annotated[float, Query(alias="timeout")] = 0,
        x_dispute_signature: Annotated[str | None, Header()] = None,
    ) -> Any:
        from hypertrain.protocol.keys import verify

        if cursor < 0 or not 0 <= wait_seconds <= 30:
            raise ChallengeError(422, "invalid event cursor or wait")
        try:
            valid = verify(
                decode_hotkey(party),
                f"hypertrain/watch/2|{run_id}|{party}".encode(),
                bytes.fromhex(x_dispute_signature or ""),
            )
        except (ValueError, KeyError_):
            valid = False
        if not valid:
            raise ChallengeError(403, "party event access requires signature")
        await party_member(run_id, party)
        _, _, disputes = await run(store.for_run(run_id)._services, run_id)
        events = await run(lambda: disputes.events(party, cursor, timeout=wait_seconds))
        return [e.model_dump(mode="json") for e in events]

    @app.post("/v2/runs/{run_id}/disputes/ack")
    async def ack_v2(run_id: str, request: Request) -> Any:
        ack = EventAck.model_validate(load_json(await v2_raw(request)))
        await party_member(run_id, ack.party)
        hosted = store.for_run(run_id)
        _, _, disputes = await run(hosted._services, run_id)

        def accepted_ack() -> None:
            with hosted._tx():
                hosted._snapshot_current_v2(hosted._run_v2(run_id))
                disputes.acknowledge(ack, now=hosted._now(hosted._db))

        await run(accepted_ack)
        return {"accepted": ack.cursor}

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
        hotkeys = await metagraph.hotkeys()  # fail closed before the ledger journals an answer
        text = await run(store.weights, epoch, epoch_at, int(clock()))
        return Response(
            await run(registered_weights, epoch, text, hotkeys), media_type="application/json"
        )

    def registered_weights(epoch: int, text: bytes, hotkeys: Mapping[str, Any]) -> bytes:
        """Pay only hotkeys registered at first answer; the filtered answer is final too.

        ponytail: the ledger still books the dropped units as paid (they burn to UID0 at
        the master); a ledger-level freeze for deregistered hotkeys would keep them owed.
        """
        from hypertrain.ledger.journal import canonical

        with store._tx() as db:
            db.execute(
                "CREATE TABLE IF NOT EXISTS weights_registered("
                "epoch INTEGER PRIMARY KEY, answer BLOB NOT NULL)"
            )
            row = db.execute(
                "SELECT answer FROM weights_registered WHERE epoch=?", (epoch,)
            ).fetchone()
            if row is not None:
                return bytes(row[0])
            answer = json.loads(text)
            dropped = {h: w for h, w in answer["weights"].items() if h not in hotkeys}
            if dropped:
                units = int(sum(dropped.values()))
                answer["weights"] = {h: w for h, w in answer["weights"].items() if h not in dropped}
                meta = answer["metadata"]
                meta["units_paid"] -= units
                meta["units_burned_this_epoch"] += units
                meta["units_unregistered"] = units
            out = canonical(answer)
            db.execute("INSERT INTO weights_registered VALUES(?,?)", (epoch, out))
            return out

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
        return await run(
            store.for_run(run_id).configure, run_id, item.model_dump(exclude_none=True)
        )

    @app.put("/v1/admin/runs/{run_id}/paused")
    async def paused(run_id: str, request: Request, authorization: Auth = None) -> Any:
        admin(authorization)
        item = await read_model(request, Paused)
        return await run(store.for_run(run_id).set_paused, run_id, item.paused)

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
        return await run(store.for_run(run_id).set_roster, run_id, hotkey, item.model_dump())

    @app.delete("/v1/admin/runs/{run_id}/roster/{hotkey}")
    async def roster_delete(run_id: str, hotkey: str, authorization: Auth = None) -> Any:
        admin(authorization)
        return await run(store.for_run(run_id).set_roster, run_id, hotkey, None)

    @app.put("/v1/admin/runs/{run_id}/honeypot")
    async def honeypot(run_id: str, request: Request, authorization: Auth = None) -> Any:
        admin(authorization)
        item = await read_model(request, Honeypot)
        return await run(store.for_run(run_id).set_honeypot, run_id, item.commitment)

    @app.post("/v1/admin/runs/{run_id}/honeypot/reveal")
    async def honeypot_reveal(run_id: str, request: Request, authorization: Auth = None) -> Any:
        admin(authorization)
        item = await read_model(request, HoneypotReveal)
        members = [m.model_dump() for m in item.members]
        return await run(
            store.for_run(run_id).reveal_honeypot, run_id, item.commitment, members, item.salt
        )

    @app.get("/v1/runs")
    async def runs() -> dict[str, Any]:
        return {"runs": await run(store.runs)}

    @app.get("/v1/beacon/latest")
    async def beacon_latest() -> Any:
        return await run(store.latest_beacon)

    @app.get("/v1/runs/{run_id}")
    async def run_status(run_id: str) -> Any:
        return await run(store.for_run(run_id).run_status, run_id)

    @app.get("/v1/runs/{run_id}/rounds/{w}")
    async def round_view(run_id: str, w: int) -> Any:
        return await run(store.for_run(run_id).round_view, run_id, w)

    @app.get("/v1/runs/{run_id}/auditor-stats")
    @app.get("/run/{run_id}/auditor-stats")
    async def auditor_stats(run_id: str) -> Any:
        return await run(store.for_run(run_id).auditor_stats, run_id)

    def miner_route(path: str, name: str) -> None:
        async def handler(run_id: str, request: Request) -> Any:
            env = await read_json(request)
            await registered(env)
            return await run(getattr(store.for_run(run_id), name), run_id, env)

        app.post(path, name=name)(handler)

    miner_route("/v1/runs/{run_id}/accept", "accept")
    miner_route("/v1/runs/{run_id}/commit", "commit")
    miner_route("/v1/runs/{run_id}/delta", "delta")
    miner_route("/v1/runs/{run_id}/state", "state_serve")
    miner_route("/v1/runs/{run_id}/dispute", "dispute")

    @app.post("/v1/runs/{run_id}/bisect")
    async def bisect(run_id: str, request: Request) -> Any:
        return await run(store.for_run(run_id).bisect, run_id, await read_json(request))

    @app.post("/v1/runs/{run_id}/resolution")
    async def resolution(run_id: str, request: Request, authorization: Auth = None) -> Any:
        worker(authorization)
        return await run(store.for_run(run_id).resolution, run_id, await read_json(request))

    @app.post("/v1/runs/{run_id}/uploads")
    async def upload_url(run_id: str, request: Request) -> Any:
        item = await read_model(request, UploadRequest)
        await registered({"signer": item.hotkey})
        return await run(store.for_run(run_id).upload_url, run_id, item.w, item.hotkey, item.sha256)

    @app.post("/v1/worker/lease")
    async def lease(authorization: Auth = None) -> Response:
        worker(authorization)
        job = await run(store.lease_any)
        if job is None:
            return Response(status_code=204)
        return JSONResponse(job)

    @app.get("/v1/worker/jobs/{job_id}/serves")
    async def serves(job_id: str, lease: str, authorization: Auth = None) -> Any:
        worker(authorization)
        return await run(store.for_job(job_id).serves, job_id, lease)

    @app.get("/v1/objects/{sha256}")
    async def get_object(sha256: str, authorization: Auth = None) -> Response:
        worker(authorization)
        data = await run(store.get_object_any, sha256)
        return Response(data, media_type="application/octet-stream")

    @app.post("/v1/runs/{run_id}/leaves")
    async def leaves(run_id: str, request: Request) -> Any:
        item = await read_model(request, LeavesBody)
        await registered({"signer": item.hotkey})
        return await run(
            store.for_run(run_id).leaves,
            run_id,
            item.w,
            item.hotkey,
            item.preimages,
            item.ef_in_sha256,
        )

    @app.post("/v1/runs/{run_id}/rerun")
    async def rerun(run_id: str, request: Request) -> Any:
        item = await read_model(request, RerunBody)
        await registered({"signer": item.hotkey})
        return await run(
            store.for_run(run_id).rerun, run_id, item.w, item.hotkey, item.leaves_root, item.sig
        )

    @app.put("/v1/aggregator/runs/{run_id}/rounds/{w}/state")
    async def round_state(run_id: str, w: int, request: Request, authorization: Auth = None) -> Any:
        admin(authorization)
        item = await read_model(request, RoundStateBody)
        return await run(
            store.for_run(run_id).set_round_state,
            run_id,
            w,
            item.theta_start_sha256,
            item.v0_sha256,
        )

    @app.post("/v1/worker/jobs/{job_id}/heartbeat")
    async def heartbeat(job_id: str, request: Request, authorization: Auth = None) -> Any:
        worker(authorization)
        item = await read_model(request, LeaseBody)
        return await run(store.for_job(job_id).heartbeat, job_id, item.lease)

    @app.post("/v1/worker/jobs/{job_id}/complete")
    async def complete(job_id: str, request: Request, authorization: Auth = None) -> Any:
        worker(authorization)
        item = await read_model(request, CompleteBody)
        return await run(store.for_job(job_id).complete, job_id, item.lease, item.verdict)

    @app.post("/v1/worker/jobs/{job_id}/fail")
    async def fail(job_id: str, request: Request, authorization: Auth = None) -> Any:
        worker(authorization)
        item = await read_model(request, FailBody)
        return await run(store.for_job(job_id).fail, job_id, item.lease, item.reason, item.retry)

    @app.get("/v1/aggregator/runs/{run_id}/rounds/{w}/inputs")
    async def inputs(
        run_id: str,
        w: int,
        authorization: Auth = None,
        d_open: Annotated[int | None, Query(ge=1)] = None,
    ) -> Any:
        admin(authorization)
        return await run(store.for_run(run_id).inputs, run_id, w, d_open)

    def aggregator_route(path: str, name: str) -> None:
        async def handler(run_id: str, w: int, request: Request, authorization: Auth = None) -> Any:
            admin(authorization)
            return await run(
                getattr(store.for_run(run_id), name), run_id, w, await read_json(request)
            )

        app.post(path, name=name)(handler)

    aggregator_route("/v1/aggregator/runs/{run_id}/rounds/{w}/aggregate", "aggregate")
    aggregator_route("/v1/aggregator/runs/{run_id}/rounds/{w}/rollback", "rollback")
    aggregator_route("/v1/aggregator/runs/{run_id}/rounds/{w}/finalize", "finalize")

    app.mount(
        "/public",
        create_public_app(
            config.state_dir / "challenge.db",
            config.state_dir / "public" / "metrics.jsonl",
        ),
    )
    return app


def __getattr__(name: str) -> FastAPI:
    """`uvicorn hypertrain.challenge.app:app` builds the app from the environment on import."""
    if name == "app":
        return create_app(Config.from_env())
    raise AttributeError(name)
