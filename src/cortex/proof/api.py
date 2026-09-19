"""Public miner API; topic-defined endpoints dispatch through the same intake gates."""

from __future__ import annotations

from contextlib import asynccontextmanager

from fastapi import APIRouter, FastAPI, Request
from fastapi.responses import JSONResponse, PlainTextResponse
from pydantic import ValidationError
from starlette.datastructures import UploadFile
from starlette.formparsers import MultiPartException, MultiPartParser

from cortex.errors import ServiceError
from cortex.http import OperatorAuth, bounded_body, decode_json
from cortex.proof.artifacts import MAX_ARTIFACT_BYTES
from cortex.proof.models import Submission, SubmissionLookup, Topic
from cortex.proof.service import ProofService


async def parse_submission(request: Request) -> tuple[Submission, bytes | None]:
    body = await bounded_body(request, MAX_ARTIFACT_BYTES + 1024 * 1024)
    artifact = None
    if request.headers.get("content-type", "").startswith("multipart/form-data"):

        async def chunks():
            yield body

        parser = MultiPartParser(
            request.headers, chunks(), max_files=1, max_fields=24, max_part_size=MAX_ARTIFACT_BYTES
        )
        parser.spool_max_size = MAX_ARTIFACT_BYTES + 1024 * 1024
        try:
            form = await parser.parse()
            fields: dict = {}
            seen = set()
            try:
                for name, part in form.multi_items():
                    if name in seen:
                        raise ServiceError(400, "duplicate multipart field")
                    seen.add(name)
                    if name == "artifact" and isinstance(part, UploadFile):
                        artifact = await part.read(MAX_ARTIFACT_BYTES + 1)
                    elif isinstance(part, str):
                        fields[name] = part
                    else:
                        raise ServiceError(400, "invalid multipart field")
            finally:
                await form.close()
            if "json" in fields:
                if len(fields) != 1:
                    raise ServiceError(400, "ambiguous multipart metadata")
                value = decode_json(fields["json"])
            else:
                value = fields
                for name in ("manifest", "env"):
                    if name in value:
                        value[name] = decode_json(value[name])
                if "declared_flops" in value:
                    try:
                        value["declared_flops"] = int(value["declared_flops"])
                    except ValueError:
                        raise ServiceError(400, "invalid declared_flops") from None
        except MultiPartException:
            raise ServiceError(400, "invalid multipart body") from None
    else:
        value = decode_json(body)
    try:
        submission = Submission.model_validate(value)
    except ValidationError as error:
        # Pydantic's full errors include inputs; env values must never be echoed.
        invalid_fields = {str(item["loc"][0]) for item in error.errors()}
        if invalid_fields.intersection({"miner_hotkey", "submit_nonce", "hotkey_signature"}):
            raise ServiceError(401, "missing or invalid signature, hotkey or nonce") from None
        raise ServiceError(400, "invalid submission fields") from None
    return submission, artifact


def public_submission(row: dict) -> dict:
    report = row.get("report")
    return {
        "id": row["id"],
        "topic_id": row["topic_id"],
        "miner_hotkey": row["hotkey"],
        "epoch": row["epoch"],
        "status": row["status"],
        "artifact_digest": row["body"]["artifact_digest"],
        "reason": row["reason"],
        "metrics": report["metrics"] if report else None,
        "evidence_digest": report["evidence_digest"] if report else None,
    }


async def parse_lookup(request: Request) -> SubmissionLookup:
    value = decode_json(await bounded_body(request, 128 * 1024))
    if "env" in value or "artifact_uri" in value:
        raise ServiceError(400, "lookup accepts only the original signed envelope")
    try:
        return SubmissionLookup.model_validate(value)
    except ValidationError as error:
        invalid_fields = {str(item["loc"][0]) for item in error.errors()}
        if invalid_fields.intersection({"miner_hotkey", "submit_nonce", "hotkey_signature"}):
            raise ServiceError(401, "missing or invalid signature, hotkey or nonce") from None
        raise ServiceError(400, "invalid lookup fields") from None


def create_router(service: ProofService, operator: OperatorAuth, topic_setup=None) -> APIRouter:
    router = APIRouter()
    if topic_setup is not None:
        from cortex.proof.setup import create_setup_router

        router.include_router(create_setup_router(topic_setup, operator))
    else:

        @router.post("/v1/admin/proof/setup")
        async def unwired_setup(request: Request):
            operator.require(request)
            raise ServiceError(503, "topic setup is not wired")

    @router.get("/healthz")
    async def health():
        return {"ok": True}

    @router.get("/v1/status")
    async def status():
        result = await service.status()
        executor_status = getattr(service.backend, "executor_status", None)
        if executor_status is not None:
            try:
                details = await executor_status()
                result["eval_executor"] = details.get("eval_executor")
                result["executor_pin"] = details.get("pin")
                result["harvest_executor_ready"] = details.get("ready", False)
                if result.get("harvest_reason") is None and not details.get("ready", False):
                    reason = details.get("reason")
                    if isinstance(reason, str):
                        result["harvest_reason"] = reason.encode()[:512].decode(errors="ignore")
            except Exception:
                result["eval_executor"] = None
                result["executor_pin"] = None
                result["harvest_executor_ready"] = False
                if result.get("harvest_reason") is None:
                    result["harvest_reason"] = "harvest executor status unavailable"
        return result

    @router.get("/v1/proof/executor")
    async def executor():
        executor_status = getattr(service.backend, "executor_status", None)
        if executor_status is not None:
            return await executor_status()
        try:
            ready = await service.backend.readiness()
            return {
                "ready": ready.live_harvest_wired,
                "reason": None if ready.live_harvest_wired else "harvest executor unavailable",
            }
        except ServiceError as error:
            return {"ready": False, "reason": error.reason}

    @router.post("/v1/admin/proof/executor")
    async def rotate_executor(request: Request):
        operator.require(request)
        rotate = getattr(service.backend, "rotate_executor", None)
        if rotate is None:
            raise ServiceError(503, "harvest executor is not wired")
        from cortex.proof.executor import EvalExecutorOffer

        try:
            offer = EvalExecutorOffer.model_validate(
                decode_json(await bounded_body(request, 32 * 1024))
            )
        except ValidationError:
            raise ServiceError(400, "invalid eval executor offer") from None
        return {"eval_executor": rotate(offer).public_view()}

    @router.get("/v1/proof/topics")
    async def topics():
        return {"topics": [topic.model_dump() for topic in service.store.topics()]}

    @router.get("/v1/proof/topics/{topic_id}")
    async def topic(topic_id: str):
        found = service.store.topic(topic_id)
        if found is None:
            raise ServiceError(404, "topic not found")
        return found.model_dump()

    @router.post("/v1/submissions", status_code=201)
    async def submit(request: Request):
        submission, artifact = await parse_submission(request)
        return public_submission(await service.submit(submission, artifact))

    @router.post("/v1/submissions/lookup")
    async def lookup(request: Request):
        return public_submission(service.lookup(await parse_lookup(request)))

    @router.get("/v1/submissions/{submission_id}")
    async def get_submission(submission_id: str):
        row = service.store.submission(submission_id)
        if row is None:
            raise ServiceError(404, "submission not found")
        return public_submission(row)

    @router.post("/v1/admin/proof/topics", status_code=201)
    async def publish(request: Request):
        operator.require(request)
        try:
            document = Topic.model_validate(decode_json(await bounded_body(request, 128 * 1024)))
        except ValidationError:
            raise ServiceError(400, "invalid topic document") from None
        return (await service.publish(document)).model_dump()

    @router.post("/v1/admin/proof/drain")
    async def drain(request: Request):
        operator.require(request)
        await service.resume()
        return {"scheduled": True}

    @router.api_route("/v1/proof/topics/{topic_id}/{suffix:path}", methods=["GET", "POST"])
    async def dynamic_endpoint(topic_id: str, suffix: str, request: Request):
        topic = service.store.topic(topic_id)
        if topic is None:
            raise ServiceError(404, "topic not found")
        endpoint = next(
            (
                item
                for item in topic.endpoints
                if item.path == "/" + suffix and item.method == request.method
            ),
            None,
        )
        if endpoint is None:
            raise ServiceError(404, "topic endpoint not found")
        if endpoint.purpose == "documentation" and request.method == "GET":
            return PlainTextResponse(topic.documentation, media_type="text/markdown")
        if endpoint.purpose == "submission" and request.method == "POST":
            submission, artifact = await parse_submission(request)
            if submission.topic_id != topic_id:
                raise ServiceError(400, "submission topic mismatch")
            return JSONResponse(
                public_submission(await service.submit(submission, artifact)), status_code=201
            )
        if endpoint.purpose == "results" and request.method == "GET":
            return {
                "submissions": [
                    public_submission(row)
                    for row in service.store.submissions(service.epoch())
                    if row["topic_id"] == topic_id
                ]
            }
        raise ServiceError(405, "endpoint method does not match its purpose")

    return router


def create_app(service: ProofService, operator: OperatorAuth) -> FastAPI:
    @asynccontextmanager
    async def lifespan(app):
        await service.resume()
        yield
        await service.drain()

    app = FastAPI(title="Cortex Proof", lifespan=lifespan)

    @app.exception_handler(ServiceError)
    async def service_error(request, error: ServiceError):
        return JSONResponse({"error": error.reason}, status_code=error.status)

    app.include_router(create_router(service, operator))
    return app
