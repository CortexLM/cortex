"""Authenticated VM-host API. TLS and a reread private token file are mandatory."""

import hashlib
import hmac
import os
import re
import stat
from contextlib import asynccontextmanager
from pathlib import Path

from fastapi import FastAPI, Request
from fastapi.exceptions import RequestValidationError
from fastapi.responses import JSONResponse
from pydantic import BaseModel, ConfigDict, ValidationError

from cortex.errors import ServiceError
from cortex.http import bounded_body, decode_json
from cortex.rlm.errors import ToolRejected
from cortex.rlm.knowledge import KnowledgeApproval, Observation
from cortex.rlm.offer import InferenceOffer

from .models import API_VERSION, ExecuteRequest, VmError, VmSpec
from .research import ResearchHost, ResearchRequest
from .runtime import Orchestrator


def read_token(path: Path) -> str:
    fd = None
    try:
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        info = os.fstat(fd)
        if not stat.S_ISREG(info.st_mode) or info.st_mode & 0o077 or info.st_size > 4096:
            raise VmError("private bearer token file required")
        with os.fdopen(fd) as stream:
            fd = None
            token = stream.read(4097).strip()
        if not token or len(token) > 4096 or any(char.isspace() for char in token):
            raise VmError("bearer token file unavailable")
        return token
    except (OSError, UnicodeError):
        raise VmError("bearer token file unavailable") from None
    finally:
        if fd is not None:
            os.close(fd)


class TeardownRequest(BaseModel):
    model_config = ConfigDict(extra="forbid", strict=True)
    topic_id: str
    retain: bool = False


def create_app(
    orchestrator: Orchestrator,
    token_file: Path,
    research: ResearchHost | None = None,
    *,
    inference_offer_commitment: str = "",
    inference_offer: InferenceOffer | None = None,
    custom_ids: tuple[str, ...] = (),
) -> FastAPI:
    if inference_offer_commitment and not re.fullmatch(r"[0-9a-f]{64}", inference_offer_commitment):
        raise ValueError("inference offer requires a sha256 commitment")

    def require_offer() -> None:
        if research is None or inference_offer is None:
            raise VmError("signed inference offer unavailable")
        try:
            inference_offer.verify_runtime(
                research.provider.model, research.limits, inference_offer_commitment
            )
        except ValueError:
            raise VmError("inference offer invalid, closed or runtime mismatched") from None

    @asynccontextmanager
    async def lifespan(app):
        await orchestrator.recover()
        try:
            yield
        finally:
            if research is not None:
                await research.close()
                if research.knowledge is not None:
                    research.knowledge.close()
            await orchestrator.close()

    app = FastAPI(docs_url=None, redoc_url=None, openapi_url=None, lifespan=lifespan)

    @app.get("/v1/knowledge/pending")
    async def pending_knowledge():
        if research is None or research.knowledge is None:
            raise VmError("shared knowledge is not configured")
        return {"observations": [item.model_dump() for item in research.knowledge.pending()]}

    @app.post("/v1/knowledge/approve")
    async def approve_knowledge(request: Request):
        if research is None or research.knowledge is None:
            raise VmError("shared knowledge is not configured")
        try:
            body = decode_json(await bounded_body(request, 32768))
            if set(body) != {"observation", "approval"}:
                raise ValueError("invalid approval envelope")
            observation = Observation.model_validate(body["observation"])
            approval = KnowledgeApproval.model_validate(body["approval"])
            # Owner signature authorizes exact content and public/private visibility.
            research.knowledge.approve_observation(observation, approval)
        except (ValidationError, ValueError, ToolRejected, ServiceError):
            raise VmError("invalid signed knowledge approval", 400) from None
        return {"approved": approval.observation_digest}

    @app.exception_handler(RequestValidationError)
    async def invalid_request(request: Request, exc: RequestValidationError):
        return JSONResponse({"error": "invalid VM request"}, status_code=400)

    @app.middleware("http")
    async def authenticate(request: Request, call_next):
        try:
            if request.url.scheme != "https":
                raise VmError("HTTPS required", 400)
            expected = read_token(token_file)
            header = request.headers.get("authorization", "")
            scheme, _, presented = header.partition(" ")
            if scheme.lower() != "bearer" or not hmac.compare_digest(
                hashlib.sha256(expected.encode()).digest(),
                hashlib.sha256(presented.strip().encode()).digest(),
            ):
                raise VmError("unauthorized", 401)
        except VmError as exc:
            return JSONResponse({"error": exc.reason}, status_code=exc.status)
        return await call_next(request)

    @app.exception_handler(VmError)
    async def vm_error(request: Request, exc: VmError):
        return JSONResponse({"error": exc.reason}, status_code=exc.status)

    @app.get("/v1/health")
    async def health():
        config = getattr(orchestrator.hypervisor, "config", None)
        details = {
            "api_version": API_VERSION,
            "image_digests": sorted(getattr(config, "images", {})),
            "resource_caps": orchestrator.caps.model_dump(mode="json"),
            "inference_offer_commitment": inference_offer_commitment,
            "research_ready": research is not None and bool(inference_offer_commitment),
            "custom_ids": list(custom_ids),
            "research_limits": research.limits.model_dump(mode="json") if research else None,
            "inference_offer": inference_offer.model_dump(mode="json") if inference_offer else None,
            "live_harvest_wired": False,
        }
        try:
            if research is not None:
                require_offer()
            await orchestrator.hypervisor.ready()
            key_file = getattr(research.provider, "api_key_file", None) if research else None
            if key_file is not None:
                read_token(Path(key_file))
            return {**details, "ready": True, "reason": ""}
        except VmError as exc:
            return {**details, "ready": False, "reason": exc.reason}

    @app.post("/v1/vms", status_code=201)
    async def create(spec: VmSpec):
        # Accepted creates must be harvested too: a dropped HTTP task may not orphan a boot.
        import asyncio

        return await asyncio.shield(orchestrator.create(spec))

    @app.get("/v1/vms/by-topic/{topic_id}")
    async def attach(topic_id: str):
        return orchestrator.by_topic(topic_id)

    @app.post("/v1/vms/{vm_id}/execute")
    async def execute(vm_id: str, request: ExecuteRequest):
        return await orchestrator.execute(vm_id, request)

    @app.post("/v1/vms/{vm_id}/agent")
    async def agent(vm_id: str, request: ResearchRequest):
        if research is None:
            raise VmError("topic RLM provider is not configured")
        require_offer()
        return await research.run(vm_id, request)

    @app.delete("/v1/vms/{vm_id}")
    async def teardown(vm_id: str, request: TeardownRequest):
        record = orchestrator.get(vm_id)
        if record.spec.topic_id != request.topic_id:
            raise VmError("topic_mismatch", 409)
        if research is not None and research.is_busy(vm_id):
            raise VmError("topic agent busy", 409)
        if orchestrator.db.execute(
            "SELECT 1 FROM jobs WHERE vm_id=? AND state='running'", (vm_id,)
        ).fetchone():
            raise VmError("VM busy", 409)
        return {
            "vm_id": vm_id,
            "confirmed": await orchestrator.teardown(vm_id, retain=request.retain),
        }

    return app
