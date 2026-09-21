"""Internal Bounty HTTP routes; no public leaderboard or report API."""

from typing import Annotated

from fastapi import APIRouter, Header, Request
from fastapi.responses import JSONResponse
from pydantic import BaseModel, ConfigDict, Field, ValidationError

from cortex.protocol.models import BOUNTY_FULL_SHARE_REPORTS

from .backend import BackendUnavailable, Severity, Verdict
from .scoring import MAX_TRIAGE_NOISE_BPS, MIN_PRECISION_BPS, SCORE_MAX, SEVERITY_BPS
from .service import PAIR_GRANT_MAX_TTL_SECONDS, TERMS_TEXT, BountyService
from .store import StoreError

SMALL_WRITE_MAX_BODY_BYTES = 4096
REPORT_MAX_BODY_BYTES = 256 * 1024


class RequestBody(BaseModel):
    model_config = ConfigDict(extra="forbid", strict=True)


class PairBody(RequestBody):
    account_id: str = Field(max_length=128)
    hotkey: str = Field(max_length=128)
    nonce: str = Field(max_length=64)
    exp: int
    signature: str = Field(max_length=130)
    terms_accepted: bool


class PairGrantBody(RequestBody):
    account_id: str = Field(max_length=128)
    hotkey: str = Field(max_length=128)
    expires_at: int


class ReportBody(RequestBody):
    session: str = Field(max_length=128)
    hotkey: str | None = Field(default=None, max_length=128)
    title: str = Field(max_length=512)
    body: str = Field(max_length=100_000)
    repro_steps: str | None = Field(default=None, max_length=100_000)


class AdjudicateBody(RequestBody):
    report_id: str = Field(max_length=128)
    verdict: Verdict
    severity: Severity | None = None
    duplicate_of: str | None = Field(default=None, max_length=128)


def _error(exc: StoreError) -> JSONResponse:
    return JSONResponse({"error": str(exc)}, status_code=exc.status)


def _request_openapi(model: type[RequestBody]) -> dict[str, object]:
    return {
        "requestBody": {
            "required": True,
            "content": {"application/json": {"schema": model.model_json_schema()}},
        }
    }


async def _read_request[RequestModel: RequestBody](
    request: Request,
    model: type[RequestModel],
    *,
    limit: int,
    label: str,
) -> RequestModel:
    encoded = bytearray()
    async for chunk in request.stream():
        if len(encoded) + len(chunk) > limit:
            raise StoreError(413, f"{label} request too large")
        encoded.extend(chunk)
    try:
        return model.model_validate_json(bytes(encoded))
    except ValidationError:
        raise StoreError(422, f"invalid {label} request") from None


def create_router(service: BountyService) -> APIRouter:
    router = APIRouter(tags=["bounty"])

    @router.get("/health")
    async def health():
        return {"ok": True, "challenge_id": "bounty", "scoring_version": service.scoring_version()}

    @router.get("/v1/status")
    async def status():
        version = service.scoring_version()
        reason = None
        try:
            await service.backend.probe()
            can_score = True
        except BackendUnavailable as error:
            can_score = False
            reason = str(error)
        return {
            "challenge_id": "bounty",
            "scoring_version": version,
            "score_max": SCORE_MAX if version == 1 else 2**64 - 1,
            "champion_hotkey": None,
            "scoring_backend": "backend_public" if service.backend.configured else "unconfigured",
            "can_score": can_score,
            "reason": reason,
            "backend_public_configured": service.backend.configured,
            "pairing": {
                "requires_operator_grant": True,
                "grant_max_ttl_secs": PAIR_GRANT_MAX_TTL_SECONDS,
            },
            "scoring": {
                "paid_on": ["valid_report_count"],
                "points_per_valid_report": 1,
                "full_share_reports": BOUNTY_FULL_SHARE_REPORTS,
                "emission_share_bps": 3000,
                "population": "expected_metagraph_hotkeys",
                "window": "cumulative_published_history",
                "off_score_gates": [],
                "severities": list(SEVERITY_BPS),
            }
            if version == 2
            else {
                "paid_on": ["precision", "severity_impact"],
                "off_score_gates": ["triage_noise"],
                "min_precision_bps": MIN_PRECISION_BPS,
                "max_triage_noise_bps": MAX_TRIAGE_NOISE_BPS,
                "severities": list(SEVERITY_BPS),
            },
            "quotas": {
                "max_pending_reports_per_hotkey": 5,
                "max_concurrent_feed_validations_per_hotkey": 1,
                "min_report_interval_secs": 60,
                "min_report_body_chars": 80,
                "min_repro_chars": 20,
                "max_report_request_bytes": REPORT_MAX_BODY_BYTES,
            },
            "terms": TERMS_TEXT,
        }

    @router.post("/v1/pair", status_code=201, openapi_extra=_request_openapi(PairBody))
    async def pair(request: Request):
        try:
            body = await _read_request(
                request,
                PairBody,
                limit=SMALL_WRITE_MAX_BODY_BYTES,
                label="pair",
            )
            return service.pair(body)
        except StoreError as exc:
            return _error(exc)

    @router.post(
        "/v1/admin/pair-grants",
        status_code=201,
        openapi_extra=_request_openapi(PairGrantBody),
    )
    async def grant_pair(request: Request, authorization: Annotated[str | None, Header()] = None):
        try:
            service.require_operator(authorization)
            body = await _read_request(
                request,
                PairGrantBody,
                limit=SMALL_WRITE_MAX_BODY_BYTES,
                label="pair grant",
            )
            return service.grant_pair(body)
        except StoreError as exc:
            return _error(exc)

    @router.post("/v1/reports", status_code=201, openapi_extra=_request_openapi(ReportBody))
    async def submit(request: Request):
        try:
            body = await _read_request(
                request,
                ReportBody,
                limit=REPORT_MAX_BODY_BYTES,
                label="report",
            )
            row = await service.submit(body)
            return {key: row[key] for key in ("id", "miner_hotkey", "state", "fingerprint")}
        except StoreError as exc:
            return _error(exc)

    @router.get("/v1/reports")
    async def list_reports(authorization: Annotated[str | None, Header()] = None):
        try:
            service.require_operator(authorization)
            return {"items": service.store.list_reports()}
        except StoreError as exc:
            return _error(exc)

    @router.get("/v1/reports/{report_id}")
    async def get_report(report_id: str, authorization: Annotated[str | None, Header()] = None):
        try:
            service.require_operator(authorization)
            return service.store.get_report(report_id)
        except StoreError as exc:
            return _error(exc)

    @router.post("/v1/admin/adjudicate", openapi_extra=_request_openapi(AdjudicateBody))
    async def adjudicate(
        request: Request,
        authorization: Annotated[str | None, Header()] = None,
    ):
        try:
            service.require_operator(authorization)
            body = await _read_request(
                request,
                AdjudicateBody,
                limit=SMALL_WRITE_MAX_BODY_BYTES,
                label="adjudication",
            )
            return service.store.adjudicate(
                body.report_id, body.verdict, body.severity, body.duplicate_of
            )
        except StoreError as exc:
            return _error(exc)

    return router
