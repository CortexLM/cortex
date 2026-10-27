"""Content-addressed objects on Cloudflare R2 (docs/adr/0003-cloudflare-data-plane.md).

A key is `prefix + sha256(content)`. Every read verifies the sha256, so a wrong or hostile object
can fail a read but is never accepted.

The 4 MiB block of the relay upload manifest (`relay_messages.CHUNK_BYTES`) stays the unit of
hashing, verification and resume: the block list of an object is the relay's chunk list, and its
canonical bytes hash to the relay `chunk_manifest_hash`. R2 refuses a non-final multipart part
under 5 MiB (EntityTooSmall at CompleteMultipartUpload), unequal non-final parts and more than
10,000 parts. A part is therefore a whole number of blocks: two (8 MiB) by default, more only when
an object would otherwise need over 10,000 parts.

A multipart object carries `x-amz-meta-sha256` and `x-amz-meta-blocks`. The second names a sidecar
object under the same prefix that holds the canonical block list, so a reader can check each block
as it arrives. An interrupted upload resumes from R2 itself: the key fixes the content, so an open
upload on the key keeps every part whose size and MD5 ETag match the local bytes.
"""

from __future__ import annotations

import hashlib
import json
import os
import tempfile
from collections import deque
from collections.abc import Callable, Iterator
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
from itertools import islice
from pathlib import Path

from hypertrain.data.store import (
    CorruptObjectError,
    ObjectNotFound,
    StoreError,
    _check_key,
    _verified,
)
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.relay_messages import CHUNK_BYTES
from hypertrain.storage.r2 import NoSuchKey, ObjectConflict, ObjectInfo, R2Client, UploadGone

BLOCK_BYTES = CHUNK_BYTES
# R2 limits read 2026-10-26: https://developers.cloudflare.com/r2/objects/upload-objects/
# (updated 2026-07-29) and https://developers.cloudflare.com/r2/platform/limits/ (updated
# 2026-06-08), whose footnotes cap a part at 5 MiB under 5 GiB and an object at 5 GiB under 5 TiB
R2_MIN_PART_BYTES = 5 << 20
R2_MAX_PART_BYTES = (5 << 30) - (5 << 20)
R2_MAX_PARTS = 10_000
R2_MAX_OBJECT_BYTES = (5 << 40) - (5 << 30)
DEFAULT_PART_BYTES = 2 * BLOCK_BYTES

ReadAt = Callable[[int, int], bytes]  # (offset, length) -> exactly `length` bytes


@dataclass(frozen=True)
class PartPlan:
    size: int
    part_bytes: int

    @property
    def count(self) -> int:
        return -(-self.size // self.part_bytes)

    def span(self, number: int) -> tuple[int, int]:
        """(offset, length) of part `number`, counted from 1 as in S3."""
        if not 1 <= number <= self.count:
            raise ValueError(f"part {number} is outside 1..{self.count}")
        off = (number - 1) * self.part_bytes
        return off, min(self.part_bytes, self.size - off)


def plan_parts(size: int, part_bytes: int | None = None) -> PartPlan:
    """Multipart layout R2 accepts, in whole 4 MiB blocks. Without `part_bytes` the part is the
    smallest multiple of the block, at least 8 MiB, that keeps the object within 10,000 parts."""
    if not 1 <= size <= R2_MAX_OBJECT_BYTES:
        raise ValueError(f"object size {size} is outside the R2 range 1 B..4.995 TiB")
    if part_bytes is None:
        blocks = -(-size // (R2_MAX_PARTS * BLOCK_BYTES))
        part_bytes = max(DEFAULT_PART_BYTES, blocks * BLOCK_BYTES)
    if part_bytes % BLOCK_BYTES:
        raise ValueError(f"part size {part_bytes} is not a whole number of 4 MiB blocks")
    if not R2_MIN_PART_BYTES <= part_bytes <= R2_MAX_PART_BYTES:
        raise ValueError(f"part size {part_bytes} is outside the R2 range 5 MiB..4.995 GiB")
    plan = PartPlan(size, part_bytes)
    if plan.count > R2_MAX_PARTS:
        raise ValueError(f"{size} bytes need {plan.count} parts of {part_bytes}; R2 allows 10,000")
    return plan


@dataclass(frozen=True)
class BlockList:
    """sha256 of every 4 MiB block; `body()` is the relay UploadChunkManifest chunk list."""

    size: int
    blocks: tuple[str, ...]

    def __post_init__(self) -> None:
        if self.size < 1 or len(self.blocks) != -(-self.size // BLOCK_BYTES):
            raise ValueError("block list does not cover the object size")
        for sha in self.blocks:
            _check_key(sha)

    def body(self) -> list[dict[str, int | str]]:
        return [
            {
                "index": i,
                "off": i * BLOCK_BYTES,
                "len": min(BLOCK_BYTES, self.size - i * BLOCK_BYTES),
                "chunk_sha256": sha,
            }
            for i, sha in enumerate(self.blocks)
        ]

    def encode(self) -> bytes:
        return canonicalize(self.body(), allow_float=False)

    def digest(self) -> str:
        """Equals UploadChunkManifest.chunk_manifest_hash() of the same bytes."""
        return sha256_hex(self.encode())

    @classmethod
    def decode(cls, raw: bytes) -> BlockList:
        try:
            rows = json.loads(raw)
            out = cls(sum(int(r["len"]) for r in rows), tuple(str(r["chunk_sha256"]) for r in rows))
        except (ValueError, TypeError, KeyError) as e:
            raise CorruptObjectError(f"malformed block list ({type(e).__name__})") from None
        if out.encode() != raw:
            raise CorruptObjectError("block list is not the canonical encoding")
        return out


@dataclass(frozen=True)
class Scan:
    """One read of a source: the object sha256, its blocks, and the sha256 and MD5 of each part
    (R2 checks the first on upload and reports the second as the part ETag)."""

    sha256: str
    blocks: BlockList
    plan: PartPlan
    part_sha256: tuple[str, ...]
    part_md5: tuple[str, ...]


def scan(size: int, read_at: ReadAt, plan: PartPlan) -> Scan:
    whole, blocks, part_sha, part_md5 = hashlib.sha256(), [], [], []
    for number in range(1, plan.count + 1):
        off, length = plan.span(number)
        ps, pm = hashlib.sha256(), hashlib.md5(usedforsecurity=False)
        for b_off in range(off, off + length, BLOCK_BYTES):
            block = read_at(b_off, min(BLOCK_BYTES, off + length - b_off))
            whole.update(block)
            ps.update(block)
            pm.update(block)
            blocks.append(hashlib.sha256(block).hexdigest())
        part_sha.append(ps.hexdigest())
        part_md5.append(pm.hexdigest())
    return Scan(
        whole.hexdigest(), BlockList(size, tuple(blocks)), plan, tuple(part_sha), tuple(part_md5)
    )


def file_reader(fd: int) -> ReadAt:
    def read_at(off: int, length: int) -> bytes:
        data = os.pread(fd, length, off)
        if len(data) != length:
            raise StoreError(f"source shrank: {len(data)} of {length} bytes at offset {off}")
        return data

    return read_at


class R2ObjectStore:
    """Checkpoint-store contract (put/get/exists, v3-core-eviction) and data.store.Store presign
    on content-addressed keys under one prefix of one bucket. Parts move `workers` at a time, so
    at most `workers` parts are in memory; the client's httpx.Client must be thread-safe."""

    def __init__(
        self,
        client: R2Client,
        prefix: str = "",
        *,
        part_bytes: int | None = None,
        workers: int = 8,
    ) -> None:
        if part_bytes is not None:
            plan_parts(part_bytes, part_bytes)  # refuse a part size R2 rejects before any request
        if workers < 1:
            raise ValueError("workers must be at least 1")
        self.client = client
        self.prefix = prefix
        self.part_bytes = part_bytes
        self.workers = workers

    def key(self, sha: str) -> str:
        return self.prefix + _check_key(sha)

    def put(self, data: bytes) -> str:
        view = memoryview(data)
        return self._put(len(data), lambda off, n: bytes(view[off : off + n]))

    def put_file(self, path: Path) -> str:
        """Store a file of any size; re-running it after a crash resumes the upload."""
        with Path(path).open("rb") as f:
            return self._put(os.fstat(f.fileno()).st_size, file_reader(f.fileno()))

    def get(self, sha: str) -> bytes:
        try:
            return _verified(sha, self.client.get_object(self.key(sha)))
        except NoSuchKey:
            raise ObjectNotFound(sha) from None

    def exists(self, sha: str) -> bool:
        """True only for a completed object written by this store (its sha256 metadata)."""
        info = self.client.head_object(self.key(sha))
        return info is not None and info.sha256 == sha

    def presign(self, sha: str, method: str = "GET", expires: int = 3600) -> str:
        """GET: any holder may read. PUT: a single-request upload that R2 accepts only for the
        bytes hashing to `sha`, once; the uploader sends `put_headers(sha)` verbatim."""
        if method == "GET":
            return self.client.presign("GET", self.key(sha), expires=expires)
        if method == "PUT":
            return self.client.presign(
                "PUT", self.key(sha), expires=expires, headers=put_headers(sha)
            )
        raise ValueError("method must be GET or PUT")

    def fetch_to(self, sha: str, dest: Path, *, check: Callable[[], None] | None = None) -> None:
        """Ranged, parallel, verified download into `dest` via same-directory temp + atomic
        replace. A multipart object is checked block by block against its sidecar block list.
        `check` runs after each range arrives; whatever it raises stops the download (the queued
        ranges are dropped, the temporary file is removed)."""
        key = self.key(sha)
        info = self.client.head_object(key)
        if info is None:
            raise ObjectNotFound(sha)
        blocks = self._blocks(info)
        window = self.part_bytes or DEFAULT_PART_BYTES
        spans = [(off, min(window, info.size - off)) for off in range(0, info.size, window)]
        dest = Path(dest)
        fd, tmp = tempfile.mkstemp(dir=dest.parent, prefix=f".{dest.name}.", suffix=".part")
        try:
            whole = hashlib.sha256()
            with os.fdopen(fd, "wb") as f:
                for off, data in self._ranges(key, spans):
                    if check is not None:
                        check()
                    if blocks is not None:
                        _check_blocks(blocks, off, data)
                    whole.update(data)
                    f.write(data)
                f.flush()
                os.fsync(f.fileno())
            if whole.hexdigest() != sha:
                raise CorruptObjectError(f"object {sha}: content hashes to {whole.hexdigest()}")
            os.replace(tmp, dest)
        except BaseException:
            Path(tmp).unlink(missing_ok=True)
            raise

    def put_blocks(self, blocks: BlockList) -> str:
        """Store the sidecar block list; its key is the relay chunk_manifest_hash."""
        return self.put(blocks.encode())

    # ---- internals ------------------------------------------------------------------------

    def _put(self, size: int, read_at: ReadAt) -> str:
        if size <= (self.part_bytes or DEFAULT_PART_BYTES):
            data = read_at(0, size)
            sha = hashlib.sha256(data).hexdigest()
            self.client.put_object(self.key(sha), data, immutable=True)
            return sha
        s = scan(size, read_at, plan_parts(size, self.part_bytes))
        key = self.key(s.sha256)
        if self.exists(s.sha256):
            return s.sha256
        # the block list lands first, so a reader of the object always finds it
        self.put_blocks(s.blocks)
        try:
            upload_id, etags = self._resume(key, s)
            missing = [n for n in range(1, s.plan.count + 1) if n not in etags]
            pool = ThreadPoolExecutor(self.workers)
            try:
                sent = [
                    pool.submit(self._send_part, key, upload_id, s, read_at, n) for n in missing
                ]
                for n, future in zip(missing, sent, strict=True):
                    etags[n] = future.result()
            finally:  # a failed part cancels the queued ones instead of sending them
                pool.shutdown(wait=True, cancel_futures=True)
            self.client.complete_multipart(key, upload_id, sorted(etags.items()), immutable=True)
        except UploadGone:
            # done by a concurrent writer or by a retried complete whose reply was lost; else the
            # upload was aborted or expired, and a re-run starts a new one
            if self.exists(s.sha256):
                return s.sha256
            raise
        if not self.exists(s.sha256):  # a 412, or an adopted upload that lacked our metadata
            raise ObjectConflict(f"{key}: object carries no sha256 metadata for its content")
        return s.sha256

    def _send_part(self, key: str, upload_id: str, s: Scan, read_at: ReadAt, n: int) -> str:
        data = read_at(*s.plan.span(n))
        if hashlib.sha256(data).hexdigest() != s.part_sha256[n - 1]:
            raise StoreError(f"part {n} changed since the source was hashed")
        return self.client.upload_part(key, upload_id, n, data)

    def _resume(self, key: str, s: Scan) -> tuple[str, dict[int, str]]:
        """The open upload on `key` with the most parts matching the local bytes (same size and
        MD5 ETag), else a new upload."""
        best: tuple[str, dict[int, str]] | None = None
        for upload_key, upload_id in self.client.list_uploads(key):
            if upload_key != key:
                continue
            listed = self.client.list_parts(key, upload_id)
            good = {
                p.number: p.etag
                for p in listed or ()
                if 1 <= p.number <= s.plan.count
                and p.size == s.plan.span(p.number)[1]
                and p.etag.strip('"') == s.part_md5[p.number - 1]
            }
            if good and (best is None or len(good) > len(best[1])):
                best = upload_id, good
        if best is not None:
            return best
        meta = {"sha256": s.sha256, "blocks": s.blocks.digest()}
        return self.client.create_multipart(key, metadata=meta), {}

    def _blocks(self, info: ObjectInfo) -> BlockList | None:
        digest = info.meta.get("blocks")
        if digest is None:
            return None
        try:
            blocks = BlockList.decode(self.get(digest))
        except ObjectNotFound:
            raise CorruptObjectError(f"block list {digest} is missing") from None
        if blocks.size != info.size:
            raise CorruptObjectError("block list size differs from the object size")
        return blocks

    def _ranges(self, key: str, spans: list[tuple[int, int]]) -> Iterator[tuple[int, bytes]]:
        """(offset, bytes) in order, at most 2 * workers ranges in flight."""

        def get(span: tuple[int, int]) -> tuple[int, bytes]:
            off, length = span
            return off, self.client.get_range(key, off, off + length)

        pending = iter(spans)
        with ThreadPoolExecutor(self.workers) as pool:
            try:
                inflight = deque(
                    pool.submit(get, span) for span in islice(pending, 2 * self.workers)
                )
                while inflight:
                    item = inflight.popleft().result()
                    for span in islice(pending, 1):
                        inflight.append(pool.submit(get, span))
                    yield item
            finally:  # error or early close: drop queued GETs instead of draining them
                pool.shutdown(wait=True, cancel_futures=True)


def put_headers(sha: str) -> dict[str, str]:
    """Headers a presigned single-request PUT signs: R2 rejects other bytes (400) or a second
    write of the key (412), and stores the sha256 that exists() checks."""
    return {"x-amz-content-sha256": sha, "x-amz-meta-sha256": sha, "if-none-match": "*"}


def _check_blocks(blocks: BlockList, off: int, data: bytes) -> None:
    for start in range(0, len(data), BLOCK_BYTES):
        index = (off + start) // BLOCK_BYTES
        if hashlib.sha256(data[start : start + BLOCK_BYTES]).hexdigest() != blocks.blocks[index]:
            raise CorruptObjectError(f"block {index} does not match the block list")
