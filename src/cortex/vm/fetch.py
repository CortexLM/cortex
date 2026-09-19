"""Exact-byte artifact retrieval inside the networked topic guest only."""

from __future__ import annotations

import asyncio
import base64
from urllib.parse import urlsplit

import httpx

from cortex.errors import ServiceError
from cortex.proof.artifacts import verify_artifact
from cortex.rlm.models import VmContext

from .models import MAX_ARTIFACT, VmError


async def fetch_artifact(identity, context: VmContext, uri: str, *, transport=None) -> str:
    if (
        identity.kind != "topic"
        or context.purpose != "evaluate"
        or identity.topic_id != context.topic_id
        or identity.image_digest != context.image_digest
    ):
        raise VmError("artifact fetch topic binding mismatch")
    parsed = urlsplit(uri)
    if (
        parsed.scheme != "https"
        or not parsed.hostname
        or parsed.username
        or parsed.password
        or parsed.fragment
        or len(uri) > 2048
    ):
        raise VmError("artifact fetch requires HTTPS without credentials", 400)
    try:
        async with (
            asyncio.timeout(75),
            httpx.AsyncClient(
                transport=transport,
                trust_env=False,
                follow_redirects=False,
                timeout=httpx.Timeout(60, connect=10),
            ) as client,
        ):
            async with client.stream("GET", uri) as response:
                if response.status_code != 200:
                    raise VmError("artifact fetch failed")
                data = bytearray()
                async for part in response.aiter_raw():
                    if len(data) + len(part) > MAX_ARTIFACT:
                        raise VmError("artifact fetch too large")
                    data.extend(part)
        # HTTP content encoding is not silently decoded; identity is served bytes verbatim.
        verify_artifact(bytes(data), context.artifact_digest or "", limit=MAX_ARTIFACT)
        return base64.b64encode(data).decode()
    except (httpx.HTTPError, ServiceError, TimeoutError):
        raise VmError("artifact fetch or exact-byte verification failed") from None
