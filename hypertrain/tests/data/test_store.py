import hashlib
import json
import os
import re
from pathlib import Path

import httpx
import pytest

from hypertrain.data.store import (
    CorruptObjectError,
    LocalFSStore,
    ObjectNotFound,
    S3Credentials,
    S3Store,
    StoreError,
    _canon_query,
    sigv4_presign,
)

AUTH = re.compile(
    r"AWS4-HMAC-SHA256 Credential=AKIDTEST/\d{8}/auto/s3/aws4_request, "
    r"SignedHeaders=host;x-amz-content-sha256;x-amz-date, Signature=[0-9a-f]{64}"
)


class MockS3:
    """In-memory path-style S3 stub; rejects unsigned requests and payload-hash mismatches."""

    def __init__(self) -> None:
        self.objects: dict[str, bytes] = {}
        self.seen_headers: list[str] = []

    def __call__(self, req: httpx.Request) -> httpx.Response:
        self.seen_headers.append(str(dict(req.headers)))
        if not AUTH.fullmatch(req.headers.get("authorization", "")):
            return httpx.Response(403)
        key = req.url.path
        if req.method == "PUT":
            body = req.content
            if hashlib.sha256(body).hexdigest() != req.headers["x-amz-content-sha256"]:
                return httpx.Response(400)
            self.objects[key] = body
            return httpx.Response(200)
        if req.method == "GET":
            return (
                httpx.Response(200, content=self.objects[key])
                if key in self.objects
                else httpx.Response(404)
            )
        return httpx.Response(405)


CREDS = S3Credentials("AKIDTEST", "SUPERSECRETVALUE")


def _s3(mock: MockS3) -> S3Store:
    return S3Store(
        "https://acct.r2.cloudflarestorage.com",
        "ht-bucket",
        CREDS,
        client=httpx.Client(transport=httpx.MockTransport(mock)),
        prefix="shards/",
    )


def test_localfs_roundtrip_and_corruption(tmp_path: Path) -> None:
    s = LocalFSStore(tmp_path)
    data = b"shard bytes" * 100
    sha = s.put(data)
    assert sha == hashlib.sha256(data).hexdigest()
    assert s.get(sha) == data and s.put(data) == sha
    p = tmp_path / sha[:2] / sha
    p.write_bytes(data[:-1] + b"X")
    with pytest.raises(CorruptObjectError):
        s.get(sha)
    with pytest.raises(ObjectNotFound):
        s.get("0" * 64)
    with pytest.raises(ValueError):
        s.get("../etc/passwd")
    assert s.presign(sha).startswith("file://")


def test_s3_mock_roundtrip_and_corruption() -> None:
    mock = MockS3()
    s = _s3(mock)
    data = os.urandom(4096)
    sha = s.put(data)
    assert mock.objects == {f"/ht-bucket/shards/{sha}": data}
    assert s.get(sha) == data
    mock.objects[f"/ht-bucket/shards/{sha}"] = data[::-1]
    with pytest.raises(CorruptObjectError):
        s.get(sha)
    with pytest.raises(ObjectNotFound):
        s.get("1" * 64)
    assert all("SUPERSECRETVALUE" not in h for h in mock.seen_headers)


def test_s3_presign_shape_and_no_secret() -> None:
    url = _s3(MockS3()).presign("a" * 64, "PUT", 600)
    assert url.startswith(
        "https://acct.r2.cloudflarestorage.com/ht-bucket/shards/" + "a" * 64 + "?"
    )
    assert "X-Amz-Signature=" in url and "SUPERSECRETVALUE" not in url
    with pytest.raises(ValueError):
        _s3(MockS3()).presign("a" * 64, "DELETE")


def test_sigv4_presign_matches_aws_published_vector() -> None:
    # AWS S3 docs "Authenticating Requests: Using Query Parameters" example.
    creds = S3Credentials("AKIAIOSFODNN7EXAMPLE", "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY")
    url = sigv4_presign(
        "GET",
        "https://examplebucket.s3.amazonaws.com/test.txt",
        creds,
        "us-east-1",
        "20130524T000000Z",
        86400,
    )
    assert url.endswith(
        "X-Amz-Signature=aeeed9bbccd4d02ee5c0109b86d86835f995330da4c265957d157751f604d404"
    )


def test_canonical_query_quotes_keys_and_values() -> None:
    assert _canon_query({"b k/=": "x y/+", "X-Amz-A": "a~b.c_d-e", "a": ""}) == (
        "X-Amz-A=a~b.c_d-e&a=&b%20k%2F%3D=x%20y%2F%2B"
    )


@pytest.mark.parametrize("mode", [0o640, 0o604, 0o644, 0o660, 0o606, 0o610, 0o601])
def test_secret_file_rejects_group_or_other_bits(tmp_path: Path, mode: int) -> None:
    p = tmp_path / "r2.json"
    p.write_text(json.dumps({"access_key_id": "AKIDTEST", "secret_access_key": "S3CR3T"}))
    p.chmod(mode)
    if mode & 0o077:
        with pytest.raises(StoreError, match="0600"):
            S3Credentials.from_secret_file(p)
    else:
        assert S3Credentials.from_secret_file(p).secret_access_key == "S3CR3T"


@pytest.mark.parametrize("mode", [0o600, 0o400])
def test_secret_file_accepts_owner_only(tmp_path: Path, mode: int) -> None:
    p = tmp_path / "r2.json"
    p.write_text(json.dumps({"access_key_id": "AKIDTEST", "secret_access_key": "S3CR3T"}))
    p.chmod(mode)
    assert S3Credentials.from_secret_file(p).access_key_id == "AKIDTEST"


def test_secret_file_mode_enforced(tmp_path: Path) -> None:
    p = tmp_path / "r2.json"
    p.write_text(json.dumps({"access_key_id": "AKIDTEST", "secret_access_key": "S3CR3T"}))
    p.chmod(0o644)
    with pytest.raises(StoreError, match="0600"):
        S3Credentials.from_secret_file(p)
    p.chmod(0o600)
    c = S3Credentials.from_secret_file(p)
    assert c.secret_access_key == "S3CR3T" and "S3CR3T" not in repr(c)
    p.write_text("not json S3CR3T")
    with pytest.raises(StoreError) as e:
        S3Credentials.from_secret_file(p)
    assert "S3CR3T" not in str(e.value)
