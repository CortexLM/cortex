"""In-memory R2 on httpx.MockTransport: SigV4 (header and presigned) checked against the request
as sent, and the multipart rules the real service applies (probed 2026-10-26, docs/adr/0003)."""

from __future__ import annotations

import hashlib
import hmac
import re
from collections import defaultdict, deque
from collections.abc import Callable, Mapping
from urllib.parse import parse_qsl, unquote

import httpx

from hypertrain.data.store import _canon_query, _signing_key
from hypertrain.storage.r2 import R2Client, R2Config, Retry

from .fakes import SECRET

CFG = R2Config("acct", "AKIATESTKEY", SECRET, "bkt", endpoint="https://r2.test")
NS = "http://s3.amazonaws.com/doc/2006-03-01/"
MIN_PART = 5 << 20
AUTH = re.compile(
    r"AWS4-HMAC-SHA256 Credential=AKIATESTKEY/(\d{8})/auto/s3/aws4_request, "
    r"SignedHeaders=([a-z0-9;-]+), Signature=([0-9a-f]{64})"
)


def _xml(tag: str, inner: str) -> str:
    return f'<?xml version="1.0" encoding="UTF-8"?><{tag} xmlns="{NS}">{inner}</{tag}>'


def _error(status: int, code: str, headers: Mapping[str, str] | None = None) -> httpx.Response:
    return httpx.Response(status, headers=headers, content=_xml("Error", f"<Code>{code}</Code>"))


def _md5(data: bytes) -> str:
    return '"' + hashlib.md5(data, usedforsecurity=False).hexdigest() + '"'


class FakeR2:
    def __init__(self) -> None:
        self.objects: dict[str, tuple[bytes, dict[str, str]]] = {}
        self.uploads: dict[str, tuple[str, dict[str, str], dict[int, bytes]]] = {}
        self.ops: list[tuple[str, str]] = []  # (operation, key) of every authenticated request
        self.hooks: dict[str, Callable[[], None]] = {}  # run before serving an operation
        self._next = 0
        self._before: dict[str, deque[int | None]] = defaultdict(deque)
        self._after: dict[str, deque[int | None]] = defaultdict(deque)
        self._headers: dict[str, Mapping[str, str]] = {}

    def fault(
        self,
        op: str,
        *statuses: int | None,
        after: bool = False,
        headers: Mapping[str, str] | None = None,
    ) -> None:
        """Answer the next calls of `op` with these statuses (None = serve normally). With
        after=True the call is served first and only its reply is replaced (a lost reply)."""
        (self._after if after else self._before)[op].extend(statuses)
        if headers:
            self._headers[op] = headers

    def count(self, op: str) -> int:
        return sum(1 for o, _ in self.ops if o == op)

    def __call__(self, req: httpx.Request) -> httpx.Response:
        raw_path, _, raw_query = req.url.raw_path.decode().partition("?")
        params = dict(parse_qsl(raw_query, keep_blank_values=True))
        denied = self._auth(req, raw_path, params)
        if denied is not None:
            return denied
        assert raw_path == "/bkt" or raw_path.startswith("/bkt/")
        key = unquote(raw_path[len("/bkt/") :])
        op = self._op(req.method, key, params)
        self.ops.append((op, key))
        if op in self.hooks:
            self.hooks[op]()
        queued = self._before[op]
        if queued and (status := queued.popleft()) is not None:
            return _error(status, "Injected", self._headers.get(op))
        r = getattr(self, "_" + op)(req, key, params)
        queued = self._after[op]
        if queued and (status := queued.popleft()) is not None:
            return _error(status, "InjectedAfter", self._headers.get(op))
        return r

    # ---- authentication ---------------------------------------------------------------------

    def _auth(self, req: httpx.Request, path: str, params: dict[str, str]) -> httpx.Response | None:
        if "X-Amz-Signature" in params:
            params = dict(params)
            sig = params.pop("X-Amz-Signature")
            if not params.get("X-Amz-Credential", "").startswith("AKIATESTKEY/"):
                return _error(403, "InvalidAccessKeyId")
            signed, amz, payload = (
                params["X-Amz-SignedHeaders"].split(";"),
                params["X-Amz-Date"],
                ("UNSIGNED-PAYLOAD"),
            )
            query = {k: v for k, v in params.items()}
        else:
            m = AUTH.fullmatch(req.headers.get("authorization", ""))
            if m is None:
                return _error(403, "AccessDenied")
            signed, sig, amz = m.group(2).split(";"), m.group(3), req.headers["x-amz-date"]
            payload, query = req.headers["x-amz-content-sha256"], params
        if any(h not in req.headers for h in signed):
            return _error(403, "SignatureDoesNotMatch")
        canon_h = "".join(f"{h}:{' '.join(req.headers[h].split())}\n" for h in signed)
        canon = "\n".join(
            [req.method, path, _canon_query(query), canon_h, ";".join(signed), payload]
        )
        sts = "\n".join(
            [
                "AWS4-HMAC-SHA256",
                amz,
                f"{amz[:8]}/auto/s3/aws4_request",
                hashlib.sha256(canon.encode()).hexdigest(),
            ]
        )
        want = hmac.new(
            _signing_key(SECRET, amz[:8], "auto", "s3"), sts.encode(), hashlib.sha256
        ).hexdigest()
        if not hmac.compare_digest(want, sig):
            return _error(403, "SignatureDoesNotMatch")
        declared = req.headers.get("x-amz-content-sha256", "UNSIGNED-PAYLOAD")
        if declared != "UNSIGNED-PAYLOAD" and declared != hashlib.sha256(req.content).hexdigest():
            return _error(400, "XAmzContentSHA256Mismatch")
        return None

    @staticmethod
    def _op(method: str, key: str, params: dict[str, str]) -> str:
        if key == "":
            return "ListMultipartUploads" if "uploads" in params else "ListObjects"
        if "uploads" in params:
            return "CreateMultipartUpload"
        if "uploadId" in params:
            return {
                "PUT": "UploadPart",
                "GET": "ListParts",
                "POST": "CompleteMultipartUpload",
                "DELETE": "AbortMultipartUpload",
            }[method]
        return {
            "PUT": "PutObject",
            "GET": "GetObject",
            "HEAD": "HeadObject",
            "DELETE": "DeleteObject",
        }[method]

    # ---- objects ------------------------------------------------------------------------------

    @staticmethod
    def _meta(req: httpx.Request) -> dict[str, str]:
        return {k: v for k, v in req.headers.items() if k.startswith("x-amz-meta-")}

    def _PutObject(self, req: httpx.Request, key: str, _: dict[str, str]) -> httpx.Response:
        if req.headers.get("if-none-match") == "*" and key in self.objects:
            return _error(412, "PreconditionFailed")
        self.objects[key] = (req.content, self._meta(req))
        return httpx.Response(200, headers={"etag": _md5(req.content)})

    def _GetObject(self, req: httpx.Request, key: str, _: dict[str, str]) -> httpx.Response:
        if key not in self.objects:
            return _error(404, "NoSuchKey")
        data = self.objects[key][0]
        rg = re.fullmatch(r"bytes=(\d+)-(\d+)", req.headers.get("range", ""))
        if rg is None:
            return httpx.Response(200, content=data)
        return httpx.Response(206, content=data[int(rg.group(1)) : int(rg.group(2)) + 1])

    def _HeadObject(self, req: httpx.Request, key: str, _: dict[str, str]) -> httpx.Response:
        if key not in self.objects:
            return httpx.Response(404)
        data, meta = self.objects[key]
        return httpx.Response(200, headers={"content-length": str(len(data)), **meta})

    def _DeleteObject(self, req: httpx.Request, key: str, _: dict[str, str]) -> httpx.Response:
        self.objects.pop(key, None)
        return httpx.Response(204)

    # ---- multipart ----------------------------------------------------------------------------

    def _CreateMultipartUpload(
        self, req: httpx.Request, key: str, _: dict[str, str]
    ) -> httpx.Response:
        self._next += 1
        upload_id = f"upload-{self._next:04d}"
        self.uploads[upload_id] = (key, self._meta(req), {})
        return httpx.Response(
            200, content=_xml("InitiateMultipartUploadResult", f"<UploadId>{upload_id}</UploadId>")
        )

    def _upload(self, key: str, params: dict[str, str]) -> dict[int, bytes] | None:
        found = self.uploads.get(params["uploadId"])
        return found[2] if found is not None and found[0] == key else None

    def _UploadPart(self, req: httpx.Request, key: str, params: dict[str, str]) -> httpx.Response:
        parts = self._upload(key, params)
        if parts is None:
            return _error(404, "NoSuchUpload")
        number = int(params["partNumber"])
        if not 1 <= number <= 10_000:
            return _error(400, "InvalidArgument")
        parts[number] = req.content
        return httpx.Response(200, headers={"etag": _md5(req.content)})

    def _ListParts(self, req: httpx.Request, key: str, params: dict[str, str]) -> httpx.Response:
        parts = self._upload(key, params)
        if parts is None:
            return _error(404, "NoSuchUpload")
        page, marker = (
            int(params.get("max-parts", "1000")),
            int(params.get("part-number-marker", "0")),
        )
        numbers = sorted(n for n in parts if n > marker)
        shown, more = numbers[:page], len(numbers) > page
        body = "".join(
            f"<Part><PartNumber>{n}</PartNumber><Size>{len(parts[n])}</Size>"
            f"<ETag>{_md5(parts[n])}</ETag></Part>"
            for n in shown
        )
        tail = f"<NextPartNumberMarker>{shown[-1]}</NextPartNumberMarker>" if more else ""
        trunc = "true" if more else "false"
        return httpx.Response(
            200, content=_xml("ListPartsResult", f"<IsTruncated>{trunc}</IsTruncated>{tail}{body}")
        )

    def _ListMultipartUploads(
        self, req: httpx.Request, key: str, params: dict[str, str]
    ) -> httpx.Response:
        prefix = params.get("prefix", "")
        body = "".join(
            f"<Upload><Key>{k}</Key><UploadId>{u}</UploadId></Upload>"
            for u, (k, _, _) in sorted(self.uploads.items())
            if k.startswith(prefix)
        )
        return httpx.Response(200, content=_xml("ListMultipartUploadsResult", body))

    def _CompleteMultipartUpload(
        self, req: httpx.Request, key: str, params: dict[str, str]
    ) -> httpx.Response:
        parts = self._upload(key, params)
        if parts is None:
            return _error(404, "NoSuchUpload")
        if req.headers.get("if-none-match") == "*" and key in self.objects:
            del self.uploads[params["uploadId"]]  # R2 treats the upload as aborted
            return _error(412, "PreconditionFailed")
        chosen = [
            (int(n), etag.replace("&quot;", '"'))
            for n, etag in re.findall(
                r"<Part><PartNumber>(\d+)</PartNumber><ETag>([^<]+)</ETag></Part>",
                req.content.decode(),
            )
        ]
        numbers = [n for n, _ in chosen]
        if numbers != sorted(set(numbers)) or not chosen:
            return _error(400, "InvalidPartOrder")
        if any(n not in parts or _md5(parts[n]) != etag for n, etag in chosen):
            return _error(400, "InvalidPart")
        sizes = [len(parts[n]) for n in numbers]
        if any(s < MIN_PART for s in sizes[:-1]):
            return _error(400, "EntityTooSmall")
        if len(set(sizes[:-1])) > 1 or (len(sizes) > 1 and sizes[-1] > sizes[0]):
            return _error(400, "InvalidPart")
        _, meta, _ = self.uploads.pop(params["uploadId"])
        self.objects[key] = (b"".join(parts[n] for n in numbers), meta)
        return httpx.Response(200, content=_xml("CompleteMultipartUploadResult", ""))

    def _AbortMultipartUpload(
        self, req: httpx.Request, key: str, params: dict[str, str]
    ) -> httpx.Response:
        if self._upload(key, params) is None:
            return _error(404, "NoSuchUpload")
        del self.uploads[params["uploadId"]]
        return httpx.Response(204)


class Sleeps:
    def __init__(self) -> None:
        self.waits: list[float] = []

    def __call__(self, seconds: float) -> None:
        self.waits.append(seconds)


def make(retry: Retry | None = None) -> tuple[FakeR2, R2Client, httpx.Client]:
    """The fake, a credentialed client on it, and a bare httpx client (a host with no key)."""
    fake = FakeR2()
    http = httpx.Client(transport=httpx.MockTransport(fake))
    return fake, R2Client(CFG, client=http, retry=retry), http
