"""Cloudflare R2 (any S3-compatible endpoint) client: SigV4 on stdlib + httpx, no boto.

Secrets live only in `R2Config.secret_access_key` (repr=False); errors carry method, key and
HTTP status, never headers or credentials.
"""

from __future__ import annotations

import hashlib
import os
import re
import xml.etree.ElementTree as ET
from collections.abc import Mapping
from dataclasses import dataclass, field
from pathlib import Path
from urllib.parse import quote

import httpx

from hypertrain.data.store import S3Credentials, _now, sigv4_headers

REQUIRED = ("R2_ACCOUNT_ID", "R2_ACCESS_KEY_ID", "R2_SECRET_ACCESS_KEY", "R2_BUCKET")
_UNRESERVED = "-_.~"


class R2Error(RuntimeError):
    pass


class ConfigError(R2Error):
    pass


class ObjectConflict(R2Error):
    """If-None-Match: * hit an existing object."""


@dataclass(frozen=True)
class R2Config:
    account_id: str
    access_key_id: str
    secret_access_key: str = field(repr=False)
    bucket: str
    public_base: str = ""
    endpoint: str = ""

    @property
    def url(self) -> str:
        return (self.endpoint or f"https://{self.account_id}.r2.cloudflarestorage.com").rstrip("/")

    def __repr__(self) -> str:
        return f"R2Config(bucket={self.bucket!r}, endpoint={self.url!r}, secret=<redacted>)"

    __str__ = __repr__


def parse_env_file(path: Path) -> dict[str, str]:
    out: dict[str, str] = {}
    for line in Path(path).read_text().splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        k, v = line.split("=", 1)
        k = k.strip().removeprefix("export ").strip()
        out[k] = v.strip().strip("\"'")
    return out


def load_config(env: Mapping[str, str] | None = None, env_file: Path | None = None) -> R2Config:
    """Env-file values are the base; real environment variables override them."""
    merged: dict[str, str] = parse_env_file(env_file) if env_file else {}
    merged.update(os.environ if env is None else env)
    missing = [k for k in REQUIRED if not merged.get(k)]
    if missing:
        raise ConfigError("missing R2 settings: " + ", ".join(missing))
    return R2Config(
        account_id=merged["R2_ACCOUNT_ID"],
        access_key_id=merged["R2_ACCESS_KEY_ID"],
        secret_access_key=merged["R2_SECRET_ACCESS_KEY"],
        bucket=merged["R2_BUCKET"],
        public_base=merged.get("R2_PUBLIC_BASE", "").rstrip("/"),
        endpoint=merged.get("R2_ENDPOINT", ""),
    )


# ---- client ----------------------------------------------------------------------------------


@dataclass(frozen=True)
class ObjectInfo:
    size: int
    sha256: str | None  # from x-amz-meta-sha256


class R2Client:
    """Path-style S3 client for one bucket. Not thread-safe for key reuse; httpx.Client is."""

    def __init__(
        self, cfg: R2Config, *, client: httpx.Client | None = None, timeout: float = 120.0
    ) -> None:
        if not re.fullmatch(r"[a-z0-9][a-z0-9.-]{1,61}[a-z0-9]", cfg.bucket):
            raise ConfigError("invalid bucket name")
        self.cfg = cfg
        self._creds = S3Credentials(cfg.access_key_id, cfg.secret_access_key)
        self.http = client or httpx.Client(timeout=timeout)

    def __repr__(self) -> str:
        return f"R2Client({self.cfg!r})"

    def _url(self, key: str, query: str = "") -> str:
        u = f"{self.cfg.url}/{self.cfg.bucket}/{quote(key, safe='/' + _UNRESERVED)}"
        return f"{u}?{query}" if query else u

    def _send(
        self,
        method: str,
        key: str,
        *,
        query: str = "",
        body: bytes = b"",
        extra: Mapping[str, str] | None = None,
    ) -> httpx.Response:
        sha = hashlib.sha256(body).hexdigest()
        url = self._url(key, query)
        amz = _now()
        auth = sigv4_headers(method, url, self._creds, "auto", amz, sha, extra=extra)
        signed = {**(extra or {}), **auth}
        try:
            return self.http.request(method, url, headers=signed, content=body or None)
        except httpx.HTTPError as e:  # type name only: the request object carries auth headers
            raise R2Error(f"{method} {key}: {type(e).__name__}") from None

    @staticmethod
    def _fail(method: str, key: str, r: httpx.Response) -> R2Error:
        return R2Error(f"{method} {key}: HTTP {r.status_code}")

    # ponytail: single PUT only. Shards are <= 64 MiB (shards16.MAX_SHARD_BYTES) and the 5 GiB
    # single-PUT ceiling is far away; add S3 multipart when a teacher-label file can exceed it.
    def put_object(
        self, key: str, data: bytes, *, immutable: bool = False, content_type: str | None = None
    ) -> bool:
        """True = written, False = already existed (immutable only; sha256 must match, else
        ObjectConflict)."""
        if len(data) > 5 << 30:
            raise R2Error(f"PUT {key}: object over 5 GiB needs multipart (not implemented)")
        sha = hashlib.sha256(data).hexdigest()
        extra = {"x-amz-meta-sha256": sha}
        if immutable:
            extra["if-none-match"] = "*"
        if content_type:
            extra["content-type"] = content_type
        r = self._send("PUT", key, body=data, extra=extra)
        if r.status_code == 412:
            info = self.head_object(key)
            if info is None or info.sha256 != sha or info.size != len(data):
                raise ObjectConflict(f"{key}: exists with different content")
            return False
        if r.status_code >= 300:
            raise self._fail("PUT", key, r)
        return True

    def head_object(self, key: str) -> ObjectInfo | None:
        r = self._send("HEAD", key)
        if r.status_code == 404:
            return None
        if r.status_code >= 300:
            raise self._fail("HEAD", key, r)
        return ObjectInfo(
            int(r.headers.get("content-length", "0")), r.headers.get("x-amz-meta-sha256")
        )

    def get_range(self, key: str, start: int, end: int) -> bytes:
        """Bytes [start, end) via the signed S3 endpoint."""
        r = self._send("GET", key, extra={"range": f"bytes={start}-{end - 1}"})
        if r.status_code != 206 or len(r.content) != end - start:
            raise R2Error(f"GET {key}: HTTP {r.status_code}, {len(r.content)} bytes")
        return r.content

    def get_object(self, key: str) -> bytes:
        r = self._send("GET", key)
        if r.status_code >= 300:
            raise self._fail("GET", key, r)
        return r.content

    def delete_object(self, key: str) -> None:
        r = self._send("DELETE", key)
        if r.status_code >= 300 and r.status_code != 404:
            raise self._fail("DELETE", key, r)

    def list_prefix(self, prefix: str) -> list[tuple[str, int]]:
        out: list[tuple[str, int]] = []
        token = ""
        while True:
            q = f"list-type=2&prefix={quote(prefix, safe=_UNRESERVED)}"
            if token:
                q += f"&continuation-token={quote(token, safe=_UNRESERVED)}"
            r = self._send("GET", "", query=q)
            if r.status_code >= 300:
                raise self._fail("LIST", prefix, r)
            root = ET.fromstring(r.content)  # noqa: S314 - signed reply from our own bucket
            ns = root.tag.partition("}")[0] + "}" if root.tag.startswith("{") else ""
            for c in root.findall(f"{ns}Contents"):
                out.append((c.findtext(f"{ns}Key") or "", int(c.findtext(f"{ns}Size") or 0)))
            if root.findtext(f"{ns}IsTruncated") != "true":
                return out
            token = root.findtext(f"{ns}NextContinuationToken") or ""
