"""In-memory fake S3 + public CDN on httpx.MockTransport."""

from __future__ import annotations

import re
from urllib.parse import parse_qs, unquote

import httpx

from hypertrain.storage.r2 import R2Client, R2Config

SECRET = "SECRETSECRETSECRETSECRET1234567890abcd"
CFG = R2Config("acct", "AKIATESTKEY", SECRET, "bkt", "https://pub.test", "https://s3.test")


class FakeS3:
    def __init__(self) -> None:
        self.objs: dict[str, tuple[bytes, dict[str, str]]] = {}
        self.log: list[tuple[str, str]] = []  # (method, key) on the signed endpoint
        self.fail_put_after: int | None = None
        self.puts = 0
        self.public_range = True

    def _range(self, data: bytes, h: httpx.Headers) -> httpx.Response:
        rg = h.get("range")
        if rg and self.public_range:
            a, b = re.fullmatch(r"bytes=(\d+)-(\d+)", rg).groups()  # type: ignore[union-attr]
            return httpx.Response(206, content=data[int(a) : int(b) + 1])
        return httpx.Response(200, content=data)

    def __call__(self, req: httpx.Request) -> httpx.Request | httpx.Response:
        if req.url.host == "pub.test":
            key = unquote(req.url.path[1:])
            return (
                self._range(self.objs[key][0], req.headers)
                if key in self.objs
                else httpx.Response(404)
            )
        assert "AWS4-HMAC-SHA256" in req.headers["authorization"]
        path = unquote(req.url.path)
        assert path.startswith("/bkt/") or path == "/bkt"
        key = path[len("/bkt/") :]
        self.log.append((req.method, key))
        if req.method == "PUT":
            self.puts += 1
            if self.fail_put_after is not None and self.puts > self.fail_put_after:
                return httpx.Response(500)
            if req.headers.get("if-none-match") == "*" and key in self.objs:
                return httpx.Response(412)
            assert req.headers["x-amz-content-sha256"]
            self.objs[key] = (req.content, {"sha256": req.headers.get("x-amz-meta-sha256", "")})
            return httpx.Response(200)
        if req.method == "DELETE":
            self.objs.pop(key, None)
            return httpx.Response(204)
        if req.method == "HEAD":
            if key not in self.objs:
                return httpx.Response(404)
            d, m = self.objs[key]
            return httpx.Response(
                200, headers={"content-length": str(len(d)), "x-amz-meta-sha256": m["sha256"]}
            )
        if key == "":
            pre = parse_qs(req.url.query.decode()).get("prefix", [""])[0]
            body = "".join(
                f"<Contents><Key>{k}</Key><Size>{len(v[0])}</Size></Contents>"
                for k, v in sorted(self.objs.items())
                if k.startswith(pre)
            )
            return httpx.Response(
                200,
                content=f"<ListBucketResult><IsTruncated>false</IsTruncated>{body}</ListBucketResult>",
            )
        if key not in self.objs:
            return httpx.Response(404)
        return self._range(self.objs[key][0], req.headers)


def make() -> tuple[FakeS3, R2Client, httpx.Client]:
    fake = FakeS3()
    http = httpx.Client(transport=httpx.MockTransport(fake))
    return fake, R2Client(CFG, client=http), http
