"""Master gateway routes; only sealing requires the rotating operator bearer."""

from fastapi import APIRouter, Request
from fastapi.responses import JSONResponse, Response
from pydantic import ValidationError

from cortex.errors import ServiceError
from cortex.http import OperatorAuth, bounded_body, decode_json

from .models import RawWeightRequest, SealRequest
from .service import GatewayService
from .store import RawWeightConflict


def _error(error: ServiceError) -> JSONResponse:
    body: dict = {"error": error.reason}
    if isinstance(error, RawWeightConflict):
        body["original"] = error.original
    if error.reason == "incomplete participant set (D24)":
        body["code"] = "incomplete_participant_set"
    return JSONResponse(body, status_code=error.status)


def create_router(service: GatewayService, operator_auth: OperatorAuth) -> APIRouter:
    router = APIRouter(tags=["gateway"])

    @router.post("/v1/weights/raw", status_code=202)
    async def raw_weight(request: Request):
        try:
            body = RawWeightRequest.model_validate(decode_json(await bounded_body(request, 4096)))
            return service.accept_leaf(body.to_leaf())
        except ValidationError:
            return _error(ServiceError(400, "invalid raw weight body"))
        except ServiceError as error:
            return _error(error)

    @router.post("/v1/admin/seal")
    async def seal(request: Request):
        try:
            operator_auth.require(request)
            body = SealRequest.model_validate(decode_json(await bounded_body(request, 1024)))
            bundle = await service.seal(body.epoch, netuid=body.netuid, block_b=body.block_b)
            return dict(
                epoch=bundle.body.epoch,
                merkle_root=bundle.body.merkle_root.hex(),
                final_vector_len=len(bundle.body.final_vector),
            )
        except ValidationError:
            return _error(ServiceError(400, "invalid seal body"))
        except ServiceError as error:
            return _error(error)

    @router.get("/v1/weights/latest")
    async def latest():
        return service.latest()

    @router.get("/v1/bundle/{epoch}")
    async def bundle(epoch: str):
        try:
            if len(epoch) > 20 or not epoch.isascii() or not epoch.isdecimal():
                raise ServiceError(400, "invalid epoch")
            return Response(service.bundle_bytes(int(epoch)), media_type="application/octet-stream")
        except ServiceError as error:
            return _error(error)

    @router.get("/v1/bundle/root/{root}")
    async def bundle_root(root: str):
        try:
            return Response(service.bundle_by_root(root), media_type="application/octet-stream")
        except ServiceError as error:
            return _error(error)

    return router
