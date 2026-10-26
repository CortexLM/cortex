"""Cloudflare R2 (any S3-compatible endpoint) client: SigV4 on stdlib + httpx, no boto.

Secrets live only in `R2Config.secret_access_key` (repr=False); errors carry method, key and
HTTP status, never headers or credentials.
"""

from __future__ import annotations

import hashlib
import os
import random
import re
import time
import xml.etree.ElementTree as ET
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass, field
from pathlib import Path
from urllib.parse import quote

import httpx

from hypertrain.data.store import S3Credentials, _now, sigv4_headers, sigv4_presign

REQUIRED = ("R2_ACCOUNT_ID", "R2_ACCESS_KEY_ID", "R2_SECRET_ACCESS_KEY", "R2_BUCKET")
_UNRESERVED = "-_.~"
RETRY_STATUS = frozenset({429, 500, 502, 503, 504})


class R2Error(RuntimeError):
    pass


class ConfigError(R2Error):
    pass


class ObjectConflict(R2Error):
    """If-None-Match: * hit an existing object."""


class NoSuchKey(R2Error):
    """GET of a key that does not exist (HTTP 404)."""


class UploadGone(R2Error):
    """NoSuchUpload: the multipart upload was completed, aborted or expired."""


@dataclass(frozen=True)
class Retry:
    """Bounded retry of 429, 5xx and transport errors with full-jitter exponential backoff.

    Attempt n (0-based) waits U(0, min(cap, base * 2**n)) seconds. A numeric Retry-After is a
    floor up to `cap`, and a 429 waits at least 1 s: R2 allows one write per second per key."""

    attempts: int = 5
    base: float = 0.5
    cap: float = 16.0
    sleep: Callable[[float], None] = time.sleep
    rng: random.Random = field(default_factory=random.Random)

    def __post_init__(self) -> None:
        if self.attempts < 1:
            raise ValueError("attempts must be at least 1")

    def run(self, send: Callable[[], httpx.Response]) -> httpx.Response:
        """Call send() until its status is not retryable; the last attempt's reply is returned
        whatever its status, and its transport error propagates."""
        for attempt in range(self.attempts - 1):
            try:
                r = send()
            except httpx.TransportError:
                self.sleep(self.delay(attempt, None))
                continue
            if r.status_code not in RETRY_STATUS:
                return r
            self.sleep(self.delay(attempt, r))
        return send()

    def delay(self, attempt: int, r: httpx.Response | None) -> float:
        wait = self.rng.uniform(0, min(self.cap, self.base * 2**attempt))
        if r is not None and r.status_code == 429:
            wait = max(wait, 1.0)
        after = r.headers.get("retry-after", "") if r is not None else ""
        if after.isdigit():
            wait = max(wait, min(self.cap, float(after)))
        return wait


@dataclass(frozen=True)
class R2Config:
    account_id: str
    access_key_id: str
    secret_access_key: str = field(repr=False)
    bucket: str
    public_base: str = ""
    endpoint: str = ""
    region: str = "auto"  # SigV4 scope: R2 uses "auto"; another S3 endpoint needs its region

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
    meta: Mapping[str, str] = field(default_factory=dict)  # every x-amz-meta-*, prefix removed


@dataclass(frozen=True)
class PartInfo:
    number: int
    size: int
    etag: str  # as R2 sends it: the double-quoted MD5 hex of the part


class R2Client:
    """Path-style S3 client for one bucket. Not thread-safe for key reuse; httpx.Client is.

    With `retry`, every request is re-signed and re-sent on 429, 5xx or a transport error."""

    def __init__(
        self,
        cfg: R2Config,
        *,
        client: httpx.Client | None = None,
        timeout: float = 120.0,
        retry: Retry | None = None,
    ) -> None:
        if not re.fullmatch(r"[a-z0-9][a-z0-9.-]{1,61}[a-z0-9]", cfg.bucket):
            raise ConfigError("invalid bucket name")
        self.cfg = cfg
        self._creds = S3Credentials(cfg.access_key_id, cfg.secret_access_key)
        self.http = client or httpx.Client(timeout=timeout)
        self.retry = retry

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

        def once() -> httpx.Response:
            auth = sigv4_headers(
                method, url, self._creds, self.cfg.region, _now(), sha, extra=extra
            )
            signed = {**(extra or {}), **auth}
            return self.http.request(method, url, headers=signed, content=body or None)

        try:
            return self.retry.run(once) if self.retry else once()
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
        meta = {
            k.removeprefix("x-amz-meta-"): v
            for k, v in r.headers.items()
            if k.startswith("x-amz-meta-")
        }
        return ObjectInfo(
            int(r.headers.get("content-length", "0")), r.headers.get("x-amz-meta-sha256"), meta
        )

    def get_range(self, key: str, start: int, end: int) -> bytes:
        """Bytes [start, end) via the signed S3 endpoint."""
        r = self._send("GET", key, extra={"range": f"bytes={start}-{end - 1}"})
        if r.status_code != 206 or len(r.content) != end - start:
            raise R2Error(f"GET {key}: HTTP {r.status_code}, {len(r.content)} bytes")
        return r.content

    def get_object(self, key: str) -> bytes:
        r = self._send("GET", key)
        if r.status_code == 404:
            raise NoSuchKey(f"GET {key}: HTTP 404")
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

    # ---- multipart (R2 limits: docs/adr/0003-cloudflare-data-plane.md) ----------------------

    def create_multipart(self, key: str, *, metadata: Mapping[str, str] | None = None) -> str:
        """Start an upload; `metadata` becomes x-amz-meta-* of the completed object."""
        extra = {f"x-amz-meta-{k}": v for k, v in (metadata or {}).items()}
        r = self._send("POST", key, query="uploads=", extra=extra)
        if r.status_code >= 300:
            raise self._fail("CreateMultipartUpload", key, r)
        upload_id = _xml(r.content).findtext("UploadId")
        if not upload_id:
            raise R2Error(f"CreateMultipartUpload {key}: no UploadId in the reply")
        return upload_id

    def upload_part(self, key: str, upload_id: str, number: int, data: bytes) -> str:
        """Returns the part ETag. The signed x-amz-content-sha256 makes R2 refuse altered bytes."""
        r = self._send("PUT", key, query=_part_query(upload_id, number), body=data)
        if r.status_code == 404:
            raise UploadGone(f"UploadPart {key}: HTTP 404")
        if r.status_code >= 300 or "etag" not in r.headers:
            raise self._fail("UploadPart", key, r)
        return r.headers["etag"]

    def list_parts(self, key: str, upload_id: str, *, page: int = 1000) -> list[PartInfo] | None:
        """Every uploaded part in number order; None once the upload is gone."""
        out: list[PartInfo] = []
        marker = 0
        while True:
            q = f"max-parts={page}&part-number-marker={marker}&uploadId={_q(upload_id)}"
            r = self._send("GET", key, query=q)
            if r.status_code == 404:
                return None
            if r.status_code >= 300:
                raise self._fail("ListParts", key, r)
            root = _xml(r.content)
            for p in root.iter("Part"):
                out.append(
                    PartInfo(
                        int(p.findtext("PartNumber") or 0),
                        int(p.findtext("Size") or 0),
                        p.findtext("ETag") or "",
                    )
                )
            if root.findtext("IsTruncated") != "true":
                return out
            marker = int(root.findtext("NextPartNumberMarker") or 0)

    def list_uploads(self, prefix: str) -> list[tuple[str, str]]:
        """(key, upload id) of open uploads under `prefix` (first page of ListMultipartUploads)."""
        r = self._send("GET", "", query=f"prefix={_q(prefix)}&uploads=")
        if r.status_code >= 300:
            raise self._fail("ListMultipartUploads", prefix, r)
        return [
            (u.findtext("Key") or "", u.findtext("UploadId") or "")
            for u in _xml(r.content).iter("Upload")
        ]

    def complete_multipart(
        self, key: str, upload_id: str, parts: Sequence[tuple[int, str]], *, immutable: bool
    ) -> bool:
        """True = completed; False = immutable and the key already exists (412). UploadGone if
        the upload no longer exists, e.g. a retried complete whose first reply was lost."""
        body = "".join(
            f"<Part><PartNumber>{n}</PartNumber><ETag>{etag}</ETag></Part>" for n, etag in parts
        )
        xml_body = f"<CompleteMultipartUpload>{body}</CompleteMultipartUpload>".encode()
        extra = {"if-none-match": "*"} if immutable else None
        r = self._send("POST", key, query=f"uploadId={_q(upload_id)}", body=xml_body, extra=extra)
        if r.status_code == 412 and immutable:
            return False
        if r.status_code == 404:
            raise UploadGone(f"CompleteMultipartUpload {key}: HTTP 404")
        # S3 may report a failed complete as 200 with an <Error> body
        if r.status_code >= 300 or (r.content and _xml(r.content).tag == "Error"):
            raise self._fail("CompleteMultipartUpload", key, r)
        return True

    def abort_multipart(self, key: str, upload_id: str) -> None:
        r = self._send("DELETE", key, query=f"uploadId={_q(upload_id)}")
        if r.status_code >= 300 and r.status_code != 404:
            raise self._fail("AbortMultipartUpload", key, r)

    def presign(
        self,
        method: str,
        key: str,
        *,
        query: str = "",
        expires: int = 3600,
        headers: Mapping[str, str] | None = None,
    ) -> str:
        """Query-signed URL for one request; the holder must send `headers` verbatim."""
        url = self._url(key, query)
        return sigv4_presign(
            method, url, self._creds, self.cfg.region, _now(), expires, headers=headers
        )


def _q(value: str) -> str:
    return quote(value, safe=_UNRESERVED)


def _part_query(upload_id: str, number: int) -> str:
    return f"partNumber={number}&uploadId={_q(upload_id)}"


def _xml(raw: bytes) -> ET.Element:
    """Parse an S3 reply and drop the namespace from every tag."""
    root = ET.fromstring(raw)  # noqa: S314 - signed reply from our own bucket
    for el in root.iter():
        el.tag = el.tag.rpartition("}")[2]
    return root
