"""Opt-in live test on a `cortex-v3-test-` R2 bucket (docs/adr/0003): a 64 MiB round trip with
its throughput, and the grant binding on the real service. It never runs in CI. It needs
HT_CLOUDFLARE_ENV_FILE, a private env file with R2_ACCESS_KEY_ID, R2_SECRET_ACCESS_KEY and
R2_S3_ENDPOINT, and HT_R2_TEST_BUCKET; every key it writes is deleted at the end."""

from __future__ import annotations

import os
from pathlib import Path
from time import perf_counter

import pytest

from hypertrain.storage.cas import (
    BLOCK_BYTES,
    DEFAULT_PART_BYTES,
    R2ObjectStore,
    plan_parts,
    scan,
)
from hypertrain.storage.offer import finish_upload, offer_upload, send_offer
from hypertrain.storage.r2 import R2Client, R2Config, R2Error, Retry, parse_env_file

pytestmark = [
    pytest.mark.cloudflare,
    pytest.mark.skipif(
        not (os.environ.get("HT_CLOUDFLARE_ENV_FILE") and os.environ.get("HT_R2_TEST_BUCKET")),
        reason="opt-in live R2 test: set HT_CLOUDFLARE_ENV_FILE and HT_R2_TEST_BUCKET",
    ),
]
MiB = 1 << 20
SIZE = 64 * MiB


def _client() -> R2Client:
    env = parse_env_file(Path(os.environ["HT_CLOUDFLARE_ENV_FILE"]))
    bucket = os.environ["HT_R2_TEST_BUCKET"]
    assert bucket.startswith("cortex-v3-test-"), "the live test only writes to test buckets"
    cfg = R2Config(
        account_id=env.get("CLOUDFLARE_ACCOUNT_ID", ""),
        access_key_id=env["R2_ACCESS_KEY_ID"],
        secret_access_key=env["R2_SECRET_ACCESS_KEY"],
        bucket=bucket,
        endpoint=env.get("R2_S3_ENDPOINT", ""),
    )
    return R2Client(cfg, retry=Retry())


def test_r2_64_mib_round_trip_and_bound_part_grant(tmp_path: Path) -> None:
    client = _client()
    store = R2ObjectStore(client, f"live/{os.urandom(8).hex()}/")
    data = os.urandom(SIZE)
    src, out = tmp_path / "src.bin", tmp_path / "out.bin"
    src.write_bytes(data)
    small = data[: DEFAULT_PART_BYTES + MiB]
    small_path = tmp_path / "small.bin"
    small_path.write_bytes(small)
    try:
        t0 = perf_counter()
        sha = store.put_file(src)
        up = perf_counter() - t0
        assert store.exists(sha)
        t0 = perf_counter()
        store.fetch_to(sha, out)
        down = perf_counter() - t0
        assert out.read_bytes() == data

        s = scan(len(small), lambda off, n: small[off : off + n], plan_parts(len(small)))
        offer = offer_upload(store, s.sha256, s.blocks, s.part_sha256, expires=600)
        part, retry = offer.parts[0], Retry()  # R2 answers an occasional 502 to any request
        wrong = retry.run(
            lambda: client.http.put(
                part.url,
                content=bytes(DEFAULT_PART_BYTES),
                headers={"x-amz-content-sha256": part.sha256},
            )
        )
        assert wrong.status_code == 400  # XAmzContentSHA256Mismatch on the real service
        unbound = retry.run(lambda: client.http.put(part.url, content=small[:DEFAULT_PART_BYTES]))
        assert unbound.status_code == 403
        assert send_offer(offer, small_path, client.http) == 2
        assert finish_upload(store, offer) == s.sha256 and store.get(s.sha256) == small

        # the multipart rules tests/storage/fake_r2.py applies hold on the real service
        rules = store.prefix + "rules"
        upload_id = client.create_multipart(rules)
        etags = [client.upload_part(rules, upload_id, n, os.urandom(BLOCK_BYTES)) for n in (1, 2)]
        with pytest.raises(R2Error, match="HTTP 400"):  # EntityTooSmall: a 4 MiB non-final part
            client.complete_multipart(
                rules, upload_id, [(1, etags[0]), (2, etags[1])], immutable=True
            )
        again = client.create_multipart(store.key(sha))
        etag = client.upload_part(store.key(sha), again, 1, b"x")
        assert not client.complete_multipart(store.key(sha), again, [(1, etag)], immutable=True)
        assert store.exists(sha)  # the conditional complete answered 412 and changed nothing

        print(
            f"\nR2 LIVE {SIZE // MiB} MiB, {plan_parts(SIZE).count} parts of "
            f"{DEFAULT_PART_BYTES // MiB} MiB, {store.workers} workers: "
            f"upload {up:.2f} s = {SIZE / up / 1e6:.1f} MB/s, "
            f"download {down:.2f} s = {SIZE / down / 1e6:.1f} MB/s"
        )
    finally:
        for object_key, upload_id in client.list_uploads(store.prefix):
            client.abort_multipart(object_key, upload_id)
        for object_key, _ in client.list_prefix(store.prefix):
            client.delete_object(object_key)
        assert client.list_prefix(store.prefix) == client.list_uploads(store.prefix) == []
