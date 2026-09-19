"""Internal Bounty HTTP routes; no public leaderboard or report API."""

from typing import Annotated

from fastapi import APIRouter, Header, Request
from fastapi.responses import JSONResponse
from pydantic import BaseModel, ConfigDict, Field, ValidationError

from .backend import BackendUnavailable, Severity, Verdict
from .scoring import MAX_TRIAGE_NOISE_BPS, MIN_PRECISION_BPS, SCORE_MAX, SEVERITY_BPS
from .service import PAIR_GRANT_MAX_TTL_SECONDS, TERMS_TEXT, BountyService
from .store import StoreError

PAIR_GRANT_MAX_BODY_BYTES = 4096


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


async def _read_pair_grant(request: Request) -> PairGrantBody:
    encoded = bytearray()
    async for chunk in request.stream():
        if len(encoded) + len(chunk) > PAIR_GRANT_MAX_BODY_BYTES:
            raise StoreError(413, "pair grant request too large")
        encoded.extend(chunk)
    try:
        return PairGrantBody.model_validate_json(bytes(encoded))
    except ValidationError:
        raise StoreError(422, "invalid pair grant request") from None


def create_router(service: BountyService) -> APIRouter:
    router = APIRouter(tags=["bounty"])

    @router.get("/health")
    async def health():
        return {"ok": True, "challenge_id": "bounty", "scoring_version": 1}

    @router.get("/v1/status")
    async def status():
        reason = None
        try:
            await service.backend.fetch()
            can_score = True
        except BackendUnavailable as error:
            can_score = False
            reason = str(error)
        return {
            "challenge_id": "bounty",
            "scoring_version": 1,
            "score_max": SCORE_MAX,
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
                "paid_on": ["precision", "severity_impact"],
                "off_score_gates": ["triage_noise"],
                "min_precision_bps": MIN_PRECISION_BPS,
                "max_triage_noise_bps": MAX_TRIAGE_NOISE_BPS,
                "severities": list(SEVERITY_BPS),
            },
            "quotas": {
                "max_pending_reports_per_hotkey": 5,
                "min_report_interval_secs": 60,
                "min_report_body_chars": 80,
                "min_repro_chars": 20,
            },
            "terms": TERMS_TEXT,
        }

    @router.post("/v1/pair", status_code=201)
    async def pair(body: PairBody):
        try:
            return service.pair(body)
        except StoreError as exc:
            return _error(exc)

    @router.post("/v1/admin/pair-grants", status_code=201)
    async def grant_pair(request: Request, authorization: Annotated[str | None, Header()] = None):
        try:
            service.require_operator(authorization)
            body = await _read_pair_grant(request)
            return service.grant_pair(body)
        except StoreError as exc:
            return _error(exc)

    @router.post("/v1/reports", status_code=201)
    async def submit(body: ReportBody):
        try:
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

    @router.post("/v1/admin/adjudicate")
    async def adjudicate(
        body: AdjudicateBody, authorization: Annotated[str | None, Header()] = None
    ):
        try:
            service.require_operator(authorization)
            return service.store.adjudicate(
                body.report_id, body.verdict, body.severity, body.duplicate_of
            )
        except StoreError as exc:
            return _error(exc)

    return router
