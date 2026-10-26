"""R2Client against the fake R2 (docs/adr/0003): multipart primitives, write-once completion,
presigned URLs bound to their query and headers, and bounded retry."""

from __future__ import annotations

import dataclasses
import hashlib
import random

import httpx
import pytest

from hypertrain.storage.r2 import R2Client, R2Error, Retry, UploadGone, _part_query

from .fake_r2 import CFG, Sleeps, make

MiB = 1 << 20


def test_list_parts_follows_pages() -> None:
    _, client, _ = make()
    upload_id = client.create_multipart("k")
    for n in range(1, 6):
        client.upload_part("k", upload_id, n, bytes([n]))
    parts = client.list_parts("k", upload_id, page=2)
    assert parts is not None and [p.number for p in parts] == [1, 2, 3, 4, 5]


def test_complete_is_write_once_and_reports_a_gone_upload() -> None:
    fake, client, _ = make()
    first = client.create_multipart("k")
    etag = client.upload_part("k", first, 1, b"a")
    assert client.complete_multipart("k", first, [(1, etag)], immutable=True)
    second = client.create_multipart("k")
    etag = client.upload_part("k", second, 1, b"b")
    assert not client.complete_multipart("k", second, [(1, etag)], immutable=True)
    assert fake.objects["k"][0] == b"a"
    with pytest.raises(UploadGone):
        client.complete_multipart("k", first, [(1, etag)], immutable=True)


def test_presigned_part_url_is_bound_to_its_query_and_headers() -> None:
    fake, client, http = make()
    upload_id = client.create_multipart("k")
    part = bytes(range(256)) * 4
    sha = hashlib.sha256(part).hexdigest()
    bound = {"x-amz-content-sha256": sha}
    url = client.presign("PUT", "k", query=_part_query(upload_id, 1), expires=60, headers=bound)
    assert http.put(url, content=part[::-1], headers=bound).status_code == 400
    assert http.put(url, content=part).status_code == 403
    assert (
        http.put(
            url.replace("partNumber=1", "partNumber=2"), content=part, headers=bound
        ).status_code
        == 403
    )
    assert http.put(url, content=part, headers=bound).status_code == 200
    assert fake.uploads[upload_id][2] == {1: part}


def test_retry_backs_off_on_throttling_and_server_errors() -> None:
    sleeps = Sleeps()
    fake, client, _ = make(Retry(attempts=4, sleep=sleeps, rng=random.Random(1)))
    fake.fault("PutObject", 429, 503)
    assert client.put_object("k", b"x")
    assert fake.count("PutObject") == 3 and len(sleeps.waits) == 2
    assert sleeps.waits[0] >= 1.0  # R2 allows one write per second per key


def test_retry_honours_retry_after() -> None:
    sleeps = Sleeps()
    fake, client, _ = make(Retry(sleep=sleeps))
    fake.fault("PutObject", 503, headers={"retry-after": "7"})
    assert client.put_object("k", b"x")
    assert sleeps.waits == [7.0]


def test_retry_gives_up_after_its_attempts_and_without_policy_never_retries() -> None:
    sleeps = Sleeps()
    fake, client, _ = make(Retry(attempts=3, sleep=sleeps))
    fake.fault("PutObject", 500, 500, 500, 500)
    with pytest.raises(R2Error, match="HTTP 500"):
        client.put_object("k", b"x")
    assert fake.count("PutObject") == 3 and len(sleeps.waits) == 2
    fake, client, _ = make()
    fake.fault("PutObject", 503)
    with pytest.raises(R2Error, match="HTTP 503"):
        client.put_object("k", b"x")
    assert fake.count("PutObject") == 1


@pytest.mark.parametrize("status", [429, 500, 502, 503, 504, "transport"])
def test_retry_covers_throttling_server_errors_and_transport_errors(status: int | str) -> None:
    sleeps = Sleeps()
    fake, client, _ = make(Retry(sleep=sleeps))
    if status == "transport":
        calls = []

        def drop_once() -> None:
            calls.append(1)
            if len(calls) == 1:
                raise httpx.ConnectError("connection reset")

        fake.hooks["PutObject"] = drop_once
    else:
        fake.fault("PutObject", status)
    assert client.put_object("k", b"x") and len(sleeps.waits) == 1


def test_retry_backoff_doubles_up_to_its_cap() -> None:
    class Top(random.Random):
        def uniform(self, a: float, b: float) -> float:
            return b

    sleeps = Sleeps()
    fake, client, _ = make(Retry(attempts=7, base=0.5, cap=4.0, sleep=sleeps, rng=Top()))
    fake.fault("PutObject", *[500] * 6)
    assert client.put_object("k", b"x")
    assert sleeps.waits == [0.5, 1.0, 2.0, 4.0, 4.0, 4.0]


def test_region_is_the_sigv4_scope_of_requests_and_presigned_urls() -> None:
    seen: list[httpx.Request] = []

    def capture(req: httpx.Request) -> httpx.Response:
        seen.append(req)
        return httpx.Response(200)

    cfg = dataclasses.replace(CFG, region="eu-central-1")
    client = R2Client(cfg, client=httpx.Client(transport=httpx.MockTransport(capture)))
    client.put_object("k", b"x")
    assert "/eu-central-1/s3/aws4_request" in seen[0].headers["authorization"]
    assert "%2Feu-central-1%2Fs3%2Faws4_request" in client.presign("GET", "k")
