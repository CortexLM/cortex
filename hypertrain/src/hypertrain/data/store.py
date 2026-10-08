"""Content-addressed object stores: objects are keyed by their sha256 hex and verified on get.

LocalFSStore for tests/local runs; S3Store for any S3-compatible endpoint incl. Cloudflare R2
(region "auto", path-style URLs). AWS SigV4 implemented on stdlib + httpx (no boto dependency).
Credentials only from a 0600 JSON secret file; never logged or repr'd.
"""

from __future__ import annotations

import datetime as dt
import hashlib
import hmac
import json
import os
import re
import stat
from pathlib import Path
from typing import Protocol
from urllib.parse import quote, urlsplit

import httpx

_HEX64 = re.compile(r"[0-9a-f]{64}")


class StoreError(RuntimeError):
    pass


class CorruptObjectError(StoreError):
    """Fetched bytes do not hash to the requested key."""


class ObjectNotFound(StoreError):
    pass


def _check_key(sha: str) -> str:
    if not _HEX64.fullmatch(sha):
        raise ValueError("object key must be 64 lowercase hex chars")
    return sha


def _verified(sha: str, data: bytes) -> bytes:
    got = hashlib.sha256(data).hexdigest()
    if got != sha:
        raise CorruptObjectError(f"object {sha}: content hashes to {got}")
    return data


class Store(Protocol):
    def put(self, data: bytes) -> str: ...
    def get(self, sha: str) -> bytes: ...
    def presign(self, sha: str, method: str = "GET", expires: int = 3600) -> str: ...


class LocalFSStore:
    def __init__(self, root: Path) -> None:
        self.root = Path(root)
        self.root.mkdir(parents=True, exist_ok=True)

    def _path(self, sha: str) -> Path:
        return self.root / _check_key(sha)[:2] / sha

    def put(self, data: bytes) -> str:
        sha = hashlib.sha256(data).hexdigest()
        p = self._path(sha)
        if not p.exists():
            p.parent.mkdir(exist_ok=True)
            tmp = p.with_suffix(f".tmp{os.getpid()}")
            tmp.write_bytes(data)
            tmp.replace(p)
        return sha

    def get(self, sha: str) -> bytes:
        p = self._path(sha)
        if not p.is_file():
            raise ObjectNotFound(sha)
        return _verified(sha, p.read_bytes())

    def presign(self, sha: str, method: str = "GET", expires: int = 3600) -> str:
        return self._path(sha).resolve().as_uri()


class S3Credentials:
    __slots__ = ("access_key_id", "_secret")

    def __init__(self, access_key_id: str, secret_access_key: str) -> None:
        self.access_key_id = access_key_id
        self._secret = secret_access_key

    @property
    def secret_access_key(self) -> str:
        return self._secret

    def __repr__(self) -> str:
        return f"S3Credentials(access_key_id={self.access_key_id[:4]}..., secret=<redacted>)"

    @classmethod
    def from_secret_file(cls, path: Path) -> S3Credentials:
        """JSON {"access_key_id", "secret_access_key"}; file must be a regular file with mode 0600
        (or stricter) and owned by the current user."""
        st = os.stat(path, follow_symlinks=False)
        if not stat.S_ISREG(st.st_mode):
            raise StoreError(f"{path}: secret must be a regular file")
        if st.st_mode & 0o077 or st.st_uid != os.getuid():
            raise StoreError(
                f"{path}: secret file must be mode 0600 and owned by uid {os.getuid()}"
            )
        try:
            obj = json.loads(Path(path).read_text())
            return cls(str(obj["access_key_id"]), str(obj["secret_access_key"]))
        except (ValueError, KeyError, TypeError) as e:  # never echo file content
            raise StoreError(f"{path}: malformed secret file ({type(e).__name__})") from None


def _hmac(key: bytes, msg: str) -> bytes:
    return hmac.new(key, msg.encode(), hashlib.sha256).digest()


def _signing_key(secret: str, date: str, region: str, service: str) -> bytes:
    k = _hmac(("AWS4" + secret).encode(), date)
    k = _hmac(k, region)
    k = _hmac(k, service)
    return _hmac(k, "aws4_request")


def _canon_query(params: dict[str, str]) -> str:
    return "&".join(
        f"{quote(k, safe='-_.~')}={quote(v, safe='-_.~')}" for k, v in sorted(params.items())
    )


def sigv4_presign(
    method: str,
    url: str,
    creds: S3Credentials,
    region: str,
    amz_date: str,
    expires: int,
    service: str = "s3",
) -> str:
    """Query-string SigV4 (UNSIGNED-PAYLOAD), host-only signed headers."""
    if not 1 <= expires <= 604800:
        raise ValueError("expires must be in [1, 604800] seconds")
    u = urlsplit(url)
    date = amz_date[:8]
    scope = f"{date}/{region}/{service}/aws4_request"
    params = {
        "X-Amz-Algorithm": "AWS4-HMAC-SHA256",
        "X-Amz-Credential": f"{creds.access_key_id}/{scope}",
        "X-Amz-Date": amz_date,
        "X-Amz-Expires": str(expires),
        "X-Amz-SignedHeaders": "host",
    }
    q = _canon_query(params)
    canon = "\n".join([method, u.path or "/", q, f"host:{u.netloc}\n", "host", "UNSIGNED-PAYLOAD"])
    sts = "\n".join(
        ["AWS4-HMAC-SHA256", amz_date, scope, hashlib.sha256(canon.encode()).hexdigest()]
    )
    sig = hmac.new(
        _signing_key(creds.secret_access_key, date, region, service), sts.encode(), hashlib.sha256
    ).hexdigest()
    return f"{u.scheme}://{u.netloc}{u.path}?{q}&X-Amz-Signature={sig}"


def sigv4_headers(
    method: str,
    url: str,
    creds: S3Credentials,
    region: str,
    amz_date: str,
    payload_sha256: str,
    service: str = "s3",
) -> dict[str, str]:
    """Authorization-header SigV4 over host, x-amz-content-sha256, x-amz-date (no query)."""
    u = urlsplit(url)
    date = amz_date[:8]
    scope = f"{date}/{region}/{service}/aws4_request"
    hdrs = {"host": u.netloc, "x-amz-content-sha256": payload_sha256, "x-amz-date": amz_date}
    signed = ";".join(sorted(hdrs))
    canon_h = "".join(f"{k}:{hdrs[k]}\n" for k in sorted(hdrs))
    canon = "\n".join([method, u.path or "/", "", canon_h, signed, payload_sha256])
    sts = "\n".join(
        ["AWS4-HMAC-SHA256", amz_date, scope, hashlib.sha256(canon.encode()).hexdigest()]
    )
    sig = hmac.new(
        _signing_key(creds.secret_access_key, date, region, service), sts.encode(), hashlib.sha256
    ).hexdigest()
    auth = (
        f"AWS4-HMAC-SHA256 Credential={creds.access_key_id}/{scope}, "
        f"SignedHeaders={signed}, Signature={sig}"
    )
    return {"x-amz-content-sha256": payload_sha256, "x-amz-date": amz_date, "Authorization": auth}


def _now() -> str:
    return dt.datetime.now(dt.UTC).strftime("%Y%m%dT%H%M%SZ")


class S3Store:
    """Path-style S3-compatible store. R2: endpoint https://<account>.r2.cloudflarestorage.com,
    region "auto". Use your own bucket; never third-party published keys."""

    def __init__(
        self,
        endpoint: str,
        bucket: str,
        creds: S3Credentials,
        *,
        region: str = "auto",
        prefix: str = "",
        client: httpx.Client | None = None,
        timeout: float = 60.0,
    ) -> None:
        if not re.fullmatch(r"[a-z0-9][a-z0-9.-]{1,61}[a-z0-9]", bucket):
            raise ValueError("invalid bucket name")
        self.endpoint = endpoint.rstrip("/")
        self.bucket = bucket
        self.prefix = prefix
        self.region = region
        self._creds = creds
        self._client = client or httpx.Client(timeout=timeout)

    def _url(self, sha: str) -> str:
        return f"{self.endpoint}/{self.bucket}/{quote(self.prefix + _check_key(sha), safe='/-_.~')}"

    def _send(self, method: str, sha: str, body: bytes = b"") -> httpx.Response:
        url = self._url(sha)
        h = sigv4_headers(
            method, url, self._creds, self.region, _now(), hashlib.sha256(body).hexdigest()
        )
        r = self._client.request(method, url, headers=h, content=body)
        if r.status_code == 404:
            raise ObjectNotFound(sha)
        if r.status_code >= 300:
            raise StoreError(f"{method} {sha}: HTTP {r.status_code}")
        return r

    def put(self, data: bytes) -> str:
        sha = hashlib.sha256(data).hexdigest()
        self._send("PUT", sha, data)
        return sha

    def get(self, sha: str) -> bytes:
        return _verified(sha, self._send("GET", sha).content)

    def presign(self, sha: str, method: str = "GET", expires: int = 3600) -> str:
        if method not in ("GET", "PUT"):
            raise ValueError("method must be GET or PUT")
        return sigv4_presign(method, self._url(sha), self._creds, self.region, _now(), expires)
