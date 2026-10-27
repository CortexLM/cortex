"""Content-addressed R2 store (docs/adr/0003): part plan, relay-equal block list, resumable
multipart, verified reads, bound presigned PUT and retry. The fake applies the R2 rules probed
on the real service on 2026-10-26."""

from __future__ import annotations

import hashlib
import os
import random
from pathlib import Path

import pytest

from hypertrain.data.store import CorruptObjectError, ObjectNotFound, StoreError
from hypertrain.relay.client import chunk_manifest
from hypertrain.storage.cas import (
    BLOCK_BYTES,
    DEFAULT_PART_BYTES,
    R2_MAX_PARTS,
    BlockList,
    R2ObjectStore,
    plan_parts,
    put_headers,
    scan,
)
from hypertrain.storage.r2 import ObjectConflict, R2Error, Retry, UploadGone

from .fake_r2 import Sleeps, make

MiB = 1 << 20


def _data(size: int, seed: int = 0) -> bytes:
    return random.Random(seed).randbytes(size)


def _file(tmp_path: Path, size: int, seed: int = 0) -> tuple[Path, bytes]:
    data = _data(size, seed)
    path = tmp_path / f"obj-{seed}.bin"
    path.write_bytes(data)
    return path, data


# ---- part plan: R2 refuses parts under 5 MiB except the last, and more than 10,000 parts -------


def test_plan_refuses_non_final_parts_under_5_mib() -> None:
    with pytest.raises(ValueError, match="outside the R2 range"):
        plan_parts(20 * MiB, BLOCK_BYTES)  # today's 4 MiB chunk as a part: EntityTooSmall on R2
    with pytest.raises(ValueError, match="outside the R2 range"):
        R2ObjectStore(make()[1], part_bytes=BLOCK_BYTES)
    plan = plan_parts(8 * MiB + 1)
    assert (plan.part_bytes, plan.count) == (DEFAULT_PART_BYTES, 2)
    assert plan.span(2) == (8 * MiB, 1)  # only the last part may be under 5 MiB


def test_plan_refuses_an_object_needing_more_than_10000_parts_at_8_mib() -> None:
    assert plan_parts(R2_MAX_PARTS * 8 * MiB, 8 * MiB).count == R2_MAX_PARTS
    assert plan_parts(R2_MAX_PARTS * 8 * MiB).part_bytes == 8 * MiB  # no growth at exactly 10,000
    with pytest.raises(ValueError, match="need 10001 parts"):
        plan_parts(R2_MAX_PARTS * 8 * MiB + 1, 8 * MiB)
    grown = plan_parts(R2_MAX_PARTS * 8 * MiB + 1)  # no size given: grow by one block
    assert (grown.part_bytes, grown.count) == (12 * MiB, 6667)
    with pytest.raises(ValueError, match="outside the R2 range 1 B"):
        plan_parts((5 << 40) - (5 << 30) + 1)  # past the 4.995 TiB object cap


def test_plan_parts_are_whole_4_mib_blocks() -> None:
    with pytest.raises(ValueError, match="whole number of 4 MiB blocks"):
        plan_parts(20 * MiB, 6 * MiB)
    assert plan_parts(20 * MiB, 12 * MiB).count == 2


# ---- block list ---------------------------------------------------------------------------------


@pytest.mark.parametrize("size", [2 * BLOCK_BYTES + 3, 3 * BLOCK_BYTES])
def test_block_list_is_the_relay_chunk_manifest(tmp_path: Path, size: int) -> None:
    path, data = _file(tmp_path, size)
    relay = chunk_manifest(path)
    s = scan(len(data), lambda off, n: data[off : off + n], plan_parts(len(data)))
    assert s.blocks.body() == [c.body() for c in relay.chunks]
    assert s.blocks.digest() == relay.chunk_manifest_hash()
    assert s.sha256 == hashlib.sha256(data).hexdigest()
    assert BlockList.decode(s.blocks.encode()) == s.blocks
    with pytest.raises(CorruptObjectError):
        BlockList.decode(s.blocks.encode().replace(b'"len"', b'"len" ', 1))


# ---- multipart writes ---------------------------------------------------------------------------


def test_put_file_multipart_round_trip(tmp_path: Path) -> None:
    fake, client, _ = make()
    store = R2ObjectStore(client, "runs/r1/objects/")
    path, data = _file(tmp_path, 2 * DEFAULT_PART_BYTES + 5)
    sha = store.put_file(path)
    assert sha == hashlib.sha256(data).hexdigest()
    assert fake.count("UploadPart") == 3 and fake.count("CompleteMultipartUpload") == 1
    stored, meta = fake.objects["runs/r1/objects/" + sha]
    assert stored == data and meta["x-amz-meta-sha256"] == sha
    blocks = BlockList.decode(store.get(meta["x-amz-meta-blocks"]))
    assert blocks.digest() == chunk_manifest(path).chunk_manifest_hash()
    assert store.exists(sha) and store.get(sha) == data
    ops = [op for op, _ in fake.ops]
    assert ops.index("PutObject") < ops.index("CompleteMultipartUpload")  # block list first
    for workers in (8, 1):  # one worker keeps a 2-range window, so the window refills
        out = tmp_path / f"out-{workers}.bin"
        R2ObjectStore(client, "runs/r1/objects/", workers=workers).fetch_to(sha, out)
        assert out.read_bytes() == data


def test_crashed_upload_is_invisible_then_resumes_from_r2(tmp_path: Path) -> None:
    fake, client, _ = make()
    store = R2ObjectStore(client, workers=1)
    path, data = _file(tmp_path, 3 * DEFAULT_PART_BYTES + 7)
    fake.fault("UploadPart", None, None, 500)  # the third part fails, e.g. the host is evicted
    with pytest.raises(R2Error, match="UploadPart"):
        store.put_file(path)
    sha = hashlib.sha256(data).hexdigest()
    assert not store.exists(sha) and sha not in fake.objects
    with pytest.raises(ObjectNotFound):
        store.get(sha)
    ((_, _, parts),) = fake.uploads.values()
    held = set(parts)
    assert 3 not in held and {1, 2} <= held
    sent = fake.count("UploadPart")
    assert store.put_file(path) == sha
    assert fake.count("UploadPart") - sent == 4 - len(held)  # only what R2 does not hold
    assert fake.objects[sha][0] == data


def test_resume_resends_a_held_part_whose_bytes_differ(tmp_path: Path) -> None:
    fake, client, _ = make()
    store = R2ObjectStore(client)
    path, data = _file(tmp_path, 2 * DEFAULT_PART_BYTES + 1)
    sha = hashlib.sha256(data).hexdigest()
    s = scan(len(data), lambda off, n: data[off : off + n], plan_parts(len(data)))
    upload_id = client.create_multipart(sha, metadata={"sha256": sha, "blocks": s.blocks.digest()})
    client.upload_part(sha, upload_id, 1, _data(DEFAULT_PART_BYTES, seed=9))  # right size only
    client.upload_part(sha, upload_id, 2, data[DEFAULT_PART_BYTES : 2 * DEFAULT_PART_BYTES])
    sent = fake.count("UploadPart")
    assert store.put_file(path) == sha
    assert fake.count("UploadPart") - sent == 2  # parts 1 (bytes differ) and 3 (missing)
    assert fake.objects[sha][0] == data


def test_put_is_write_once_and_idempotent(tmp_path: Path) -> None:
    fake, client, _ = make()
    store = R2ObjectStore(client)
    big = _data(DEFAULT_PART_BYTES + 1)
    small = b"manifest"
    assert store.put(big) == store.put(big) == hashlib.sha256(big).hexdigest()
    assert fake.count("CreateMultipartUpload") == 1
    sha = store.put(small)
    assert store.put(small) == sha and fake.count("PutObject") == 3  # sidecar + 2 small PUTs
    assert fake.objects[sha][0] == small


@pytest.mark.parametrize("size", [100, DEFAULT_PART_BYTES + 1])
def test_put_refuses_a_key_held_by_other_bytes(size: int) -> None:
    fake, client, _ = make()
    store = R2ObjectStore(client)
    data = _data(size)
    sha = hashlib.sha256(data).hexdigest()
    fake.objects[sha] = (b"garbage", {})
    with pytest.raises(ObjectConflict):
        store.put(data)
    assert fake.objects[sha] == (b"garbage", {})


def test_put_succeeds_when_another_writer_completes_first(tmp_path: Path) -> None:
    fake, client, _ = make()
    store = R2ObjectStore(client, workers=1)
    path, data = _file(tmp_path, 2 * DEFAULT_PART_BYTES + 1)
    sha = hashlib.sha256(data).hexdigest()

    def other_writer_completes() -> None:
        if fake.count("UploadPart") == 2:
            fake.uploads.clear()
            fake.objects[sha] = (data, {"x-amz-meta-sha256": sha})

    fake.hooks["UploadPart"] = other_writer_completes
    assert store.put_file(path) == sha and fake.count("CompleteMultipartUpload") == 0


def test_aborted_upload_is_reported_and_a_rerun_starts_over(tmp_path: Path) -> None:
    fake, client, _ = make()
    store = R2ObjectStore(client, workers=1)
    path, data = _file(tmp_path, 2 * DEFAULT_PART_BYTES + 1)
    fake.hooks["UploadPart"] = lambda: (
        fake.uploads.clear() if fake.count("UploadPart") == 2 else None
    )
    with pytest.raises(UploadGone):  # aborted by a lifecycle rule mid-upload: no object exists
        store.put_file(path)
    del fake.hooks["UploadPart"]
    assert store.put_file(path) == hashlib.sha256(data).hexdigest()


def test_complete_whose_reply_was_lost_is_a_success(tmp_path: Path) -> None:
    fake, client, _ = make(Retry(sleep=Sleeps()))
    store = R2ObjectStore(client)
    path, data = _file(tmp_path, DEFAULT_PART_BYTES + 1)
    fake.fault("CompleteMultipartUpload", 503, after=True)  # done, but the reply is a 503
    assert store.put_file(path) == hashlib.sha256(data).hexdigest()
    assert fake.count("CompleteMultipartUpload") == 2  # the retry meets NoSuchUpload


# ---- verified reads -----------------------------------------------------------------------------


def test_reads_refuse_a_corrupt_object(tmp_path: Path) -> None:
    fake, client, _ = make()
    store = R2ObjectStore(client)
    data = _data(2 * DEFAULT_PART_BYTES + 3)
    sha = store.put(data)
    body, meta = fake.objects[sha]
    flipped = bytearray(body)
    flipped[BLOCK_BYTES * 3 + 10] ^= 1
    fake.objects[sha] = (bytes(flipped), meta)
    with pytest.raises(CorruptObjectError):
        store.get(sha)
    out = tmp_path / "out.bin"
    with pytest.raises(CorruptObjectError, match="block 3 "):
        store.fetch_to(sha, out)
    assert not out.exists() and os.listdir(tmp_path) == []


def test_fetch_without_a_block_list_still_checks_the_whole_sha256(tmp_path: Path) -> None:
    fake, client, _ = make()
    store = R2ObjectStore(client)
    sha = store.put(b"a single-request object")
    fake.objects[sha] = (b"a single-request objecT", fake.objects[sha][1])
    with pytest.raises(CorruptObjectError, match="content hashes to"):
        store.fetch_to(sha, tmp_path / "out.bin")
    assert os.listdir(tmp_path) == []


def test_fetch_refuses_a_missing_or_wrong_block_list(tmp_path: Path) -> None:
    fake, client, _ = make()
    store = R2ObjectStore(client)
    data = _data(DEFAULT_PART_BYTES + 1)
    sha = store.put(data)
    digest = fake.objects[sha][1]["x-amz-meta-blocks"]
    sidecar = fake.objects.pop(digest)
    with pytest.raises(CorruptObjectError, match="is missing"):
        store.fetch_to(sha, tmp_path / "out.bin")
    other = BlockList(BLOCK_BYTES, (hashlib.sha256(b"x").hexdigest(),))
    fake.objects[digest] = (other.encode(), sidecar[1])
    with pytest.raises(CorruptObjectError):  # the sidecar no longer hashes to its key
        store.fetch_to(sha, tmp_path / "out.bin")
    wrong = store.put_blocks(other)
    fake.objects[sha][1]["x-amz-meta-blocks"] = wrong
    with pytest.raises(CorruptObjectError, match="size differs"):
        store.fetch_to(sha, tmp_path / "out.bin")
    assert os.listdir(tmp_path) == []


class _Stop(Exception):
    pass


def test_fetch_stops_between_ranges_when_the_caller_says_so(tmp_path: Path) -> None:
    fake, client, _ = make()
    store = R2ObjectStore(client, workers=1)
    data = _data(5 * DEFAULT_PART_BYTES + 1)  # six ranges of the default window
    sha = store.put(data)
    full, before = tmp_path / "full.bin", fake.count("GetObject")
    store.fetch_to(sha, full, check=lambda: None)
    assert full.read_bytes() == data
    whole = fake.count("GetObject") - before

    calls: list[int] = []

    def stop_at_the_second_range() -> None:
        calls.append(1)
        if len(calls) == 2:
            raise _Stop

    out, before = tmp_path / "out.bin", fake.count("GetObject")
    with pytest.raises(_Stop):
        store.fetch_to(sha, out, check=stop_at_the_second_range)
    assert len(calls) == 2  # called once per range received, never again after it raised
    assert fake.count("GetObject") - before < whole  # the rest of the object was not requested
    assert sorted(os.listdir(tmp_path)) == ["full.bin"]  # no target, no temporary file


def test_missing_object_is_object_not_found(tmp_path: Path) -> None:
    store = R2ObjectStore(make()[1])
    with pytest.raises(ObjectNotFound):
        store.get("0" * 64)
    with pytest.raises(ObjectNotFound):
        store.fetch_to("0" * 64, tmp_path / "x")
    assert not store.exists("0" * 64)


def test_exists_is_false_for_an_object_without_its_sha256_metadata() -> None:
    fake, client, _ = make()
    store = R2ObjectStore(client)
    sha = hashlib.sha256(b"x").hexdigest()
    fake.objects[sha] = (b"x", {})  # e.g. written through an unbound presigned PUT
    assert not store.exists(sha)
    fake.objects[sha] = (b"x", {"x-amz-meta-sha256": sha})
    assert store.exists(sha)


def test_file_changed_after_hashing_is_refused(tmp_path: Path) -> None:
    fake, client, _ = make()
    store = R2ObjectStore(client)
    path, data = _file(tmp_path, DEFAULT_PART_BYTES + 1)

    def edit() -> None:  # the file changes between the hashing pass and the part uploads
        with path.open("r+b") as f:
            f.write(b"\xff")

    fake.hooks["CreateMultipartUpload"] = edit
    with pytest.raises(StoreError, match="changed since the source was hashed"):
        store.put_file(path)
    assert fake.count("CompleteMultipartUpload") == 0


# ---- presigned single PUT -----------------------------------------------------------------------


def test_presigned_put_accepts_only_the_keyed_bytes_once() -> None:
    fake, client, http = make()
    store = R2ObjectStore(client, "up/")
    data = b"delta bytes"
    sha = hashlib.sha256(data).hexdigest()
    url = store.presign(sha, "PUT", 600)
    assert http.put(url, content=b"other bytes", headers=put_headers(sha)).status_code == 400
    assert http.put(url, content=data).status_code == 403  # the bound headers are mandatory
    assert http.put(url, content=data, headers=put_headers(sha)).status_code == 200
    assert http.put(url, content=data, headers=put_headers(sha)).status_code == 412
    assert store.exists(sha) and store.get(sha) == data
    got = http.get(store.presign(sha))
    assert got.status_code == 200 and got.content == data
