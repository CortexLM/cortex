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
import tempfile
from collections import deque
from collections.abc import Callable, Iterator, Mapping
from concurrent.futures import Future, ThreadPoolExecutor
from itertools import islice
from pathlib import Path
from typing import Protocol
from urllib.parse import parse_qsl, quote, urlsplit

import httpx
from pydantic import ValidationError

from hypertrain.protocol.envelope import EnvelopeError
from hypertrain.protocol.envelope_v2 import load_json
from hypertrain.protocol.messages_v2 import (
    INDEX_MAGIC,
    MAX_MANIFEST_BYTES,
    MAX_PARTS,
    PART_BYTES,
    PartIndexV1,
)

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
    *,
    headers: Mapping[str, str] | None = None,
) -> str:
    """Query-string SigV4 (UNSIGNED-PAYLOAD). The URL's own query (UploadPart's partNumber and
    uploadId) is signed and kept. Every header in `headers` is signed as well, so the holder must
    send it verbatim: R2 answers 403 without it, and checks a signed x-amz-content-sha256 against
    the body (400 XAmzContentSHA256Mismatch), which binds the URL to one exact payload."""
    if not 1 <= expires <= 604800:
        raise ValueError("expires must be in [1, 604800] seconds")
    u = urlsplit(url)
    date = amz_date[:8]
    scope = f"{date}/{region}/{service}/aws4_request"
    hdrs = {"host": u.netloc}
    hdrs.update({k.lower(): " ".join(v.split()) for k, v in (headers or {}).items()})
    signed = ";".join(sorted(hdrs))
    params = dict(parse_qsl(u.query, keep_blank_values=True))
    params.update(
        {
            "X-Amz-Algorithm": "AWS4-HMAC-SHA256",
            "X-Amz-Credential": f"{creds.access_key_id}/{scope}",
            "X-Amz-Date": amz_date,
            "X-Amz-Expires": str(expires),
            "X-Amz-SignedHeaders": signed,
        }
    )
    q = _canon_query(params)
    canon_h = "".join(f"{k}:{hdrs[k]}\n" for k in sorted(hdrs))
    canon = "\n".join([method, u.path or "/", q, canon_h, signed, "UNSIGNED-PAYLOAD"])
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
    *,
    extra: Mapping[str, str] | None = None,
    content_sha_header: bool = True,
) -> dict[str, str]:
    """Authorization-header SigV4 over host, x-amz-date, x-amz-content-sha256 (unless
    content_sha_header=False) and every header in `extra`; the URL query is signed. Returns the
    headers to add to the request (extra ones are signed, not returned)."""
    u = urlsplit(url)
    date = amz_date[:8]
    scope = f"{date}/{region}/{service}/aws4_request"
    out = {"x-amz-date": amz_date}
    if content_sha_header:
        out["x-amz-content-sha256"] = payload_sha256
    hdrs = {"host": u.netloc, **out}
    hdrs.update({k.lower(): " ".join(v.split()) for k, v in (extra or {}).items()})
    signed = ";".join(sorted(hdrs))
    canon_h = "".join(f"{k}:{hdrs[k]}\n" for k in sorted(hdrs))
    query = _canon_query(dict(parse_qsl(u.query, keep_blank_values=True)))
    canon = "\n".join([method, u.path or "/", query, canon_h, signed, payload_sha256])
    sts = "\n".join(
        ["AWS4-HMAC-SHA256", amz_date, scope, hashlib.sha256(canon.encode()).hexdigest()]
    )
    sig = hmac.new(
        _signing_key(creds.secret_access_key, date, region, service), sts.encode(), hashlib.sha256
    ).hexdigest()
    out["Authorization"] = (
        f"AWS4-HMAC-SHA256 Credential={creds.access_key_id}/{scope}, "
        f"SignedHeaders={signed}, Signature={sig}"
    )
    return out


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


# ---- ht-parts-v1: files above PART_BYTES as 1MiB parts + an index object published last ----

Put = Callable[[bytes], str]
Get = Callable[[str], bytes]


def _put(put: Put, data: bytes) -> str:
    sha = hashlib.sha256(data).hexdigest()
    if put(data) != sha:
        raise StoreError("store returned a key that is not the content sha256")
    return sha


def _get(get: Get, sha: str) -> bytes:
    return _verified(_check_key(sha), get(sha))


def parse_index(raw: bytes) -> PartIndexV1 | None:
    """None if `raw` is a plain object; the index if it carries INDEX_MAGIC; else corrupt."""
    if not raw.startswith(INDEX_MAGIC):
        return None
    if len(raw) > MAX_MANIFEST_BYTES:
        raise CorruptObjectError("part index exceeds manifest byte bound")
    try:
        index = PartIndexV1.model_validate(load_json(raw[len(INDEX_MAGIC) :]))
    except (EnvelopeError, ValidationError) as e:
        raise CorruptObjectError(f"malformed part index ({type(e).__name__})") from None
    if index.encode() != raw:
        raise CorruptObjectError("part index is not canonical")
    return index


def _publish(chunks: Callable[[], bytes], put: Put, workers: int = 1) -> str:
    """workers > 1 PUTs parts concurrently (at most 2 * workers parts = 2 * workers MiB in
    flight, so `put` must be thread-safe); the index is still PUT last, after every part."""
    buf, nxt = chunks(), chunks()
    if not nxt:
        if buf.startswith(INDEX_MAGIC):
            raise StoreError("plain object must not begin with the part-index magic")
        return _put(put, buf)
    whole, size, parts, count = hashlib.sha256(), 0, list[str](), 0
    pool = ThreadPoolExecutor(workers) if workers > 1 else None
    inflight: deque[Future[str]] = deque()
    try:
        while buf:
            if count >= MAX_PARTS:
                raise StoreError("object exceeds MAX_PARTS * PART_BYTES")
            if nxt and len(buf) != PART_BYTES:
                raise StoreError("short read before end of object")
            whole.update(buf)
            size += len(buf)
            count += 1
            if pool is None:
                parts.append(_put(put, buf))
            else:
                if len(inflight) >= 2 * workers:
                    parts.append(inflight.popleft().result())
                inflight.append(pool.submit(_put, put, buf))
            buf, nxt = nxt, (chunks() if nxt else b"")
        while inflight:
            parts.append(inflight.popleft().result())
    finally:  # error/cancel: drop queued PUTs, wait for running ones, never publish the index
        if pool is not None:
            pool.shutdown(wait=True, cancel_futures=True)
    index = PartIndexV1(sha256=whole.hexdigest(), size=size, parts=parts)
    return _put(put, index.encode())  # last: readers never see an index before its parts


def put_file(path: Path, put: Put, workers: int = 1) -> str:
    """Store `path`; returns the key of the plain object (<= PART_BYTES) or of its part index.
    The final put() call's bytes are the referenced object (len <= MAX_MANIFEST_BYTES)."""
    with Path(path).open("rb") as f:
        return _publish(lambda: f.read(PART_BYTES), put, workers)


def put_large(data: bytes, put: Put) -> str:
    view, off = memoryview(data), 0

    def chunks() -> bytes:
        nonlocal off
        off += PART_BYTES
        return bytes(view[off - PART_BYTES : off])

    return _publish(chunks, put)


def _parts(get: Get, shas: list[str], workers: int) -> Iterator[bytes]:
    """Verified parts in index order; workers > 1 prefetches a bounded window concurrently
    (at most 4 * workers parts = 4 * workers MiB in flight)."""
    if workers <= 1:
        yield from (_get(get, s) for s in shas)
        return
    pending = iter(shas)
    with ThreadPoolExecutor(workers) as pool:
        try:
            inflight = deque(pool.submit(_get, get, s) for s in islice(pending, 4 * workers))
            while inflight:
                part = inflight.popleft().result()
                for s in islice(pending, 1):
                    inflight.append(pool.submit(_get, get, s))
                yield part
        finally:  # error/cancel/early close: drop queued GETs instead of draining them
            pool.shutdown(wait=True, cancel_futures=True)


def _stream(sha: str, get: Get, sink: Callable[[bytes], object], workers: int = 1) -> None:
    raw = _get(get, sha)
    index = parse_index(raw)
    if index is None:
        sink(raw)
        return
    whole, size = hashlib.sha256(), 0
    for i, part in enumerate(_parts(get, list(index.parts), workers)):
        want = PART_BYTES if i < len(index.parts) - 1 else index.size - size
        if len(part) != want:
            raise CorruptObjectError(f"part {i} has length {len(part)}, expected {want}")
        whole.update(part)
        size += len(part)
        sink(part)
    if whole.hexdigest() != index.sha256:
        raise CorruptObjectError("reassembled object does not match index sha256")


def get_large(sha: str, get: Get) -> bytes:
    out = bytearray()
    _stream(sha, get, out.extend)
    return bytes(out)


def fetch_to(sha: str, get: Get, dest: Path, workers: int = 1) -> None:
    """Verified reassembly into `dest` via same-directory temp + atomic replace.

    With workers > 1 parts are fetched concurrently (written in order), so `get` must be
    thread-safe."""
    dest = Path(dest)
    fd, tmp = tempfile.mkstemp(dir=dest.parent, prefix=f".{dest.name}.", suffix=".part")
    try:
        with os.fdopen(fd, "wb") as f:
            _stream(sha, get, f.write, workers)
            f.flush()
            os.fsync(f.fileno())
        os.replace(tmp, dest)
    except BaseException:
        Path(tmp).unlink(missing_ok=True)
        raise
