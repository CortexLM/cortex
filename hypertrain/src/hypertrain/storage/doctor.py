"""Credential, bucket and public-read checks against a live (or fake) endpoint."""

from __future__ import annotations

import os
import secrets

import httpx

from hypertrain.storage.r2 import R2Client, R2Config, R2Error


def doctor(
    cfg: R2Config, *, client: httpx.Client | None = None, cors_origin: str = ""
) -> list[tuple[str, bool, str]]:
    """[(check, ok, detail)]. Writes and deletes one temp key under `_doctor/`."""
    out: list[tuple[str, bool, str]] = []
    http = client or httpx.Client(timeout=30.0)
    c = R2Client(cfg, client=http)
    key = f"_doctor/{secrets.token_hex(8)}"
    body = os.urandom(64)
    created = False

    def check(name: str, fn: object) -> bool:
        try:
            detail = fn()  # type: ignore[operator]
            out.append((name, True, str(detail or "")))
            return True
        except (R2Error, httpx.HTTPError, AssertionError) as e:
            out.append((name, False, str(e) or type(e).__name__))
            return False

    def put() -> None:
        nonlocal created
        c.put_object(key, body, immutable=True)
        created = True

    def head() -> str:
        info = c.head_object(key)
        assert info is not None and info.size == len(body), "HEAD size mismatch"
        assert info.sha256, "x-amz-meta-sha256 not stored"
        return f"size={info.size}"

    def rng() -> None:
        assert c.get_range(key, 8, 24) == body[8:24], "Range body mismatch"

    def public() -> str:
        url = f"{cfg.public_base}/{key}"
        r = http.get(
            url, headers={"Range": "bytes=8-23", **({"Origin": cors_origin} if cors_origin else {})}
        )
        assert r.status_code == 206, f"public Range GET gave HTTP {r.status_code}, want 206"
        assert r.content == body[8:24], "public Range body mismatch"
        if cors_origin:
            allow = r.headers.get("access-control-allow-origin")
            assert allow in ("*", cors_origin), "no access-control-allow-origin for the origin"
            return "206 + CORS"
        return "206"

    try:
        if check("put", put):
            check("head", head)
            check("range-get", rng)
            if cfg.public_base:
                check("public-range", public)
            else:
                out.append(("public-range", False, "R2_PUBLIC_BASE not set"))
    finally:
        if created:
            check("delete", lambda: c.delete_object(key))
        if client is None:
            http.close()
    return out
