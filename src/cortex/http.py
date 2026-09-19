"""Bounded request parsing and rotating operator bearer files."""

from __future__ import annotations

import hmac
import json
import os
import stat
from pathlib import Path

from fastapi import Request

from cortex.errors import ServiceError


def read_private_file(path: Path, limit: int = 8192) -> str:
    try:
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
        with os.fdopen(fd) as stream:
            metadata = os.fstat(stream.fileno())
            if not stat.S_ISREG(metadata.st_mode) or metadata.st_mode & 0o077:
                raise ServiceError(503, "credential file must be private")
            value = stream.read(limit + 1).strip()
        if not value or len(value) > limit:
            raise ServiceError(503, "credential file unavailable")
        return value
    except (OSError, UnicodeError):
        raise ServiceError(503, "credential file unavailable") from None


class OperatorAuth:
    def __init__(self, token_file: Path):
        self.token_file = token_file

    def require(self, request: Request) -> None:
        expected = read_private_file(self.token_file)
        supplied = request.headers.get("authorization", "")
        if not hmac.compare_digest(supplied.encode(), ("Bearer " + expected).encode()):
            raise ServiceError(401, "operator bearer required")


def decode_json(body: bytes | str) -> dict:
    def unique(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise ValueError("duplicate JSON key")
            result[key] = value
        return result

    def reject_constant(value):
        raise ValueError("non-finite JSON number")

    try:
        value = json.loads(body, object_pairs_hook=unique, parse_constant=reject_constant)
        if not isinstance(value, dict):
            raise ValueError("object required")
        return value
    except (ValueError, UnicodeError, RecursionError):
        raise ServiceError(400, "invalid JSON object") from None


async def bounded_body(request: Request, limit: int) -> bytes:
    body = bytearray()
    async for chunk in request.stream():
        if len(body) + len(chunk) > limit:
            raise ServiceError(413, "request body too large")
        body.extend(chunk)
    return bytes(body)
