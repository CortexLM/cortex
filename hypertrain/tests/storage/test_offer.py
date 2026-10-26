"""Presigned multipart offers (docs/adr/0003): a host with no credential uploads parts bound to
their declared sha256, resumes from presigned ListParts, and the coordinator completes only a
fully held upload."""

from __future__ import annotations

import hashlib
import random
from pathlib import Path
from urllib.parse import parse_qs, urlsplit

import httpx
import pytest

from hypertrain.data.store import CorruptObjectError, StoreError
from hypertrain.storage import offer as offer_mod
from hypertrain.storage.cas import DEFAULT_PART_BYTES, R2ObjectStore, plan_parts, scan
from hypertrain.storage.offer import UploadOffer, finish_upload, offer_upload, send_offer
from hypertrain.storage.r2 import R2Error, Retry

from .fake_r2 import FakeR2, Sleeps, make

Setup = tuple[FakeR2, R2ObjectStore, UploadOffer, httpx.Client, Path, bytes]


def _setup(tmp_path: Path, size: int) -> Setup:
    """The coordinator's store and offer, and the host's bare httpx client (no R2 key)."""
    fake, client, host = make()
    store = R2ObjectStore(client, "runs/r1/up/5Hotkey/")
    data = random.Random(size).randbytes(size)
    path = tmp_path / "delta.bin"
    path.write_bytes(data)
    s = scan(size, lambda off, n: data[off : off + n], plan_parts(size))
    offer = offer_upload(store, s.sha256, s.blocks, s.part_sha256, expires=600)
    return fake, store, offer, host, path, data


def test_offer_round_trip_without_a_credential(tmp_path: Path) -> None:
    fake, store, offer, host, path, data = _setup(tmp_path, 2 * DEFAULT_PART_BYTES + 9)
    assert send_offer(offer, path, host) == 3
    assert finish_upload(store, offer) == hashlib.sha256(data).hexdigest()
    assert store.get(offer.sha256) == data
    out = tmp_path / "out.bin"
    store.fetch_to(offer.sha256, out)
    assert out.read_bytes() == data


def test_part_url_accepts_only_the_declared_bytes(tmp_path: Path) -> None:
    fake, store, offer, http, path, data = _setup(tmp_path, DEFAULT_PART_BYTES + 1)
    part = offer.parts[0]
    wrong = bytes(DEFAULT_PART_BYTES)
    bound = {"x-amz-content-sha256": part.sha256}
    assert http.put(part.url, content=wrong, headers=bound).status_code == 400
    assert http.put(part.url, content=data[:DEFAULT_PART_BYTES]).status_code == 403
    path.write_bytes(wrong + data[DEFAULT_PART_BYTES:])  # the host's file no longer matches
    with pytest.raises(StoreError, match="differs from the declared sha256"):
        send_offer(offer, path, http, workers=1)
    assert 1 not in fake.uploads[offer.upload_id][2]  # R2 holds no first part at all


def test_finish_refuses_an_upload_with_a_missing_part(tmp_path: Path) -> None:
    fake, store, offer, host, path, data = _setup(tmp_path, 2 * DEFAULT_PART_BYTES + 9)
    fake.fault("UploadPart", None, 500, None)
    with pytest.raises(R2Error, match="HTTP 500"):
        send_offer(offer, path, host, workers=1)
    with pytest.raises(StoreError, match="missing or short"):
        finish_upload(store, offer)
    assert not store.exists(offer.sha256) and fake.count("CompleteMultipartUpload") == 0


def test_send_offer_resumes_from_every_presigned_part_list_page(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(offer_mod, "LIST_PAGE", 2)
    fake, store, offer, host, path, data = _setup(tmp_path, 3 * DEFAULT_PART_BYTES + 9)
    assert len(offer.list_urls) == 2
    fake.fault("UploadPart", None, None, None, 503)  # parts 1-3 land, part 4 fails
    with pytest.raises(R2Error):
        send_offer(offer, path, host, workers=1)
    ((_, _, parts),) = fake.uploads.values()
    assert set(parts) == {1, 2, 3}  # part 3 is only on the second page
    assert send_offer(offer, path, host) == 1
    assert finish_upload(store, offer) == offer.sha256
    assert store.get(offer.sha256) == data


def test_offer_refuses_part_hashes_that_do_not_fit_the_plan(tmp_path: Path) -> None:
    fake, store, offer, host, path, data = _setup(tmp_path, 2 * DEFAULT_PART_BYTES + 9)
    s = scan(len(data), lambda off, n: data[off : off + n], plan_parts(len(data)))
    with pytest.raises(ValueError, match="2 part hashes for a 3-part plan"):
        offer_upload(store, s.sha256, s.blocks, s.part_sha256[:-1])
    before = len(fake.ops)
    with pytest.raises(ValueError, match="64 lowercase hex"):
        offer_upload(store, s.sha256, s.blocks, (s.part_sha256[0], "Z" * 64, s.part_sha256[2]))
    assert len(fake.ops) == before  # refused before any request


def test_offer_urls_expire_as_asked_and_stay_out_of_repr(tmp_path: Path) -> None:
    fake, store, offer, host, path, data = _setup(tmp_path, DEFAULT_PART_BYTES + 1)
    for url in (offer.parts[0].url, offer.list_urls[0]):
        assert parse_qs(urlsplit(url).query)["X-Amz-Expires"] == ["600"]
    assert "X-Amz-Signature" not in repr(offer)


def test_finish_refuses_a_part_shorter_than_planned(tmp_path: Path) -> None:
    fake, client, host = make()
    store = R2ObjectStore(client, "up/")
    data = random.Random(1).randbytes(DEFAULT_PART_BYTES + 9)
    s = scan(len(data), lambda off, n: data[off : off + n], plan_parts(len(data)))
    short = data[DEFAULT_PART_BYTES : DEFAULT_PART_BYTES + 4]
    declared = (s.part_sha256[0], hashlib.sha256(short).hexdigest())
    offer = offer_upload(store, s.sha256, s.blocks, declared)
    for part, body in zip(offer.parts, (data[:DEFAULT_PART_BYTES], short), strict=True):
        bound = {"x-amz-content-sha256": part.sha256}
        assert host.put(part.url, content=body, headers=bound).status_code == 200
    with pytest.raises(StoreError, match="missing or short"):
        finish_upload(store, offer)
    assert fake.count("CompleteMultipartUpload") == 0


def test_finish_survives_a_lost_reply_and_a_second_call(tmp_path: Path) -> None:
    fake, client, host = make(Retry(sleep=Sleeps()))
    store = R2ObjectStore(client, "up/")
    data = random.Random(2).randbytes(DEFAULT_PART_BYTES + 9)
    path = tmp_path / "f.bin"
    path.write_bytes(data)
    s = scan(len(data), lambda off, n: data[off : off + n], plan_parts(len(data)))
    offer = offer_upload(store, s.sha256, s.blocks, s.part_sha256)
    send_offer(offer, path, host)
    fake.fault("CompleteMultipartUpload", 503, after=True)
    assert finish_upload(store, offer) == s.sha256  # the retry meets NoSuchUpload
    assert finish_upload(store, offer) == s.sha256  # upload gone, object present


def test_host_declaring_other_bytes_gets_an_object_no_reader_accepts(tmp_path: Path) -> None:
    fake, client, host = make()
    store = R2ObjectStore(client, "up/")
    claimed = random.Random(3).randbytes(DEFAULT_PART_BYTES + 9)
    sent = random.Random(4).randbytes(len(claimed))
    path = tmp_path / "sent.bin"
    path.write_bytes(sent)
    s = scan(len(sent), lambda off, n: sent[off : off + n], plan_parts(len(sent)))
    sha = hashlib.sha256(claimed).hexdigest()
    offer = offer_upload(store, sha, s.blocks, s.part_sha256)  # parts and blocks of `sent`
    send_offer(offer, path, host)
    assert finish_upload(store, offer) == sha  # the coordinator never reads the bytes
    with pytest.raises(CorruptObjectError):
        store.get(sha)
    with pytest.raises(CorruptObjectError, match="content hashes to"):
        store.fetch_to(sha, tmp_path / "out.bin")


def test_send_offer_retries_a_throttled_part(tmp_path: Path) -> None:
    fake, store, offer, host, path, data = _setup(tmp_path, DEFAULT_PART_BYTES + 1)
    fake.fault("UploadPart", 503)
    assert send_offer(offer, path, host, retry=Retry(sleep=Sleeps())) == 2
    assert finish_upload(store, offer) == offer.sha256
