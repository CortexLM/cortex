"""Public reverse proxy from /challenge/<id>/<path> to a registered container."""

from __future__ import annotations

from collections.abc import Callable

import httpx
from fastapi import APIRouter, Request
from fastapi.responses import JSONResponse, Response

from .registry import RegistryEntry

MAX_RESPONSE_BYTES = 8 * 1024 * 1024
FORWARDED_HEADERS = ("content-type", "accept", "authorization")
METHODS = ["GET", "POST", "PUT", "PATCH", "DELETE"]


def _error(status: int, reason: str) -> JSONResponse:
    return JSONResponse({"error": reason}, status_code=status)


def _safe(path: str, raw_path: bytes) -> bool:
    segments = path.split("/")
    return (
        b"%" not in raw_path
        and "\\" not in path
        and segments[0] != "internal"
        and all(segment not in {"", ".", ".."} for segment in segments)
    )


def create_router(
    registry: Callable[[], dict[str, RegistryEntry]], http: httpx.AsyncClient
) -> APIRouter:
    router = APIRouter(tags=["challenges"])

    @router.api_route("/challenge/{challenge_id}/{path:path}", methods=METHODS)
    async def proxy(challenge_id: str, path: str, request: Request):
        entry = registry().get(challenge_id)
        if entry is None or not _safe(path, request.scope.get("raw_path", b"")):
            return _error(404, "not found")
        body = bytearray()
        async for chunk in request.stream():
            body.extend(chunk)
            if len(body) > entry.proxy_body_limit:
                return _error(413, "request body too large")
        headers = {
            name: request.headers[name] for name in FORWARDED_HEADERS if name in request.headers
        }
        if request.client is not None:
            headers["x-forwarded-for"] = request.client.host
        try:
            async with http.stream(
                request.method,
                f"{entry.url}/{path}",
                params=request.url.query or None,
                content=bytes(body) if body else None,
                headers=headers,
                timeout=entry.proxy_timeout_seconds,
            ) as upstream:
                content = bytearray()
                async for chunk in upstream.aiter_bytes():
                    content.extend(chunk)
                    if len(content) > MAX_RESPONSE_BYTES:
                        return _error(502, "challenge response too large")
                return Response(
                    bytes(content),
                    status_code=upstream.status_code,
                    media_type=upstream.headers.get("content-type"),
                )
        except httpx.HTTPError:
            return _error(502, "challenge unavailable")

    return router
