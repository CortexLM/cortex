"""Presigned multipart upload offers: the grant path of docs/adr/0003-cloudflare-data-plane.md.

The coordinator keeps the R2 credential. For a grant it has already checked (hotkey, run,
admission, deadline), `offer_upload` opens a multipart upload on the content key and presigns one
UploadPart URL per part, each bound to the declared part sha256 by a signed x-amz-content-sha256:
R2 refuses other bytes (400 XAmzContentSHA256Mismatch) and a request without the header (403).
The host uploads without any credential and resumes from presigned ListParts pages
(`send_offer`). The coordinator completes only when every planned part is held at its planned
size (`finish_upload`), so the object is exactly the declared parts; readers still verify the
whole sha256, which the coordinator never reads.
"""

from __future__ import annotations

import hashlib
import os
from collections.abc import Callable, Sequence
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field
from functools import partial
from pathlib import Path

import httpx

from hypertrain.data.store import StoreError, _check_key
from hypertrain.storage.cas import BlockList, PartPlan, R2ObjectStore, file_reader, plan_parts
from hypertrain.storage.r2 import ObjectConflict, R2Error, Retry, UploadGone, _part_query, _q, _xml

LIST_PAGE = 1000  # parts per ListParts reply


@dataclass(frozen=True)
class PartOffer:
    number: int
    sha256: str
    url: str = field(repr=False)  # presigned UploadPart: a bearer for this one part


@dataclass(frozen=True)
class UploadOffer:
    object_key: str
    upload_id: str
    sha256: str
    size: int
    part_bytes: int
    parts: tuple[PartOffer, ...]
    list_urls: tuple[str, ...] = field(repr=False)  # presigned ListParts, LIST_PAGE parts each

    @property
    def plan(self) -> PartPlan:
        return PartPlan(self.size, self.part_bytes)


def offer_upload(
    store: R2ObjectStore,
    sha256: str,
    blocks: BlockList,
    part_sha256: Sequence[str],
    *,
    expires: int = 3600,
) -> UploadOffer:
    """Open the upload and presign it for a host that declared these hashes of its file."""
    plan = plan_parts(blocks.size, store.part_bytes)
    if len(part_sha256) != plan.count:
        raise ValueError(f"{len(part_sha256)} part hashes for a {plan.count}-part plan")
    for h in part_sha256:  # before any request: a bad declaration costs nothing
        _check_key(h)
    object_key = store.key(sha256)
    store.put_blocks(blocks)
    upload_id = store.client.create_multipart(
        object_key, metadata={"sha256": sha256, "blocks": blocks.digest()}
    )
    parts = tuple(
        PartOffer(
            n,
            h,
            store.client.presign(
                "PUT",
                object_key,
                query=_part_query(upload_id, n),
                expires=expires,
                headers={"x-amz-content-sha256": h},
            ),
        )
        for n, h in enumerate(part_sha256, 1)
    )
    list_urls = tuple(
        store.client.presign(
            "GET",
            object_key,
            query=f"max-parts={LIST_PAGE}&part-number-marker={page * LIST_PAGE}"
            f"&uploadId={_q(upload_id)}",
            expires=expires,
        )
        for page in range(-(-plan.count // LIST_PAGE))
    )
    return UploadOffer(object_key, upload_id, sha256, plan.size, plan.part_bytes, parts, list_urls)


def send_offer(
    offer: UploadOffer,
    path: Path,
    http: httpx.Client,
    *,
    retry: Retry | None = None,
    workers: int = 8,
) -> int:
    """Host side: send every part R2 does not hold yet; returns how many were sent. A held part
    of the planned size is skipped: only this offer's URLs can write the upload, and each URL
    accepts only the bytes of its declared sha256."""
    plan = offer.plan
    held = _held(offer, http, retry)
    todo = [p for p in offer.parts if held.get(p.number) != plan.span(p.number)[1]]
    with Path(path).open("rb") as f:
        if os.fstat(f.fileno()).st_size != offer.size:
            raise StoreError(f"{path}: size differs from the offer")
        read_at = file_reader(f.fileno())

        def send(part: PartOffer) -> None:
            data = read_at(*plan.span(part.number))
            if hashlib.sha256(data).hexdigest() != part.sha256:
                raise StoreError(f"part {part.number} differs from the declared sha256")
            headers = {"x-amz-content-sha256": part.sha256}
            r = _call(retry, partial(http.put, part.url, content=data, headers=headers))
            if r.status_code >= 300:
                raise R2Error(f"UploadPart {offer.object_key} #{part.number}: HTTP {r.status_code}")

        pool = ThreadPoolExecutor(workers)
        try:
            for future in [pool.submit(send, p) for p in todo]:
                future.result()
        finally:  # a failed part cancels the queued ones instead of sending them
            pool.shutdown(wait=True, cancel_futures=True)
    return len(todo)


def finish_upload(store: R2ObjectStore, offer: UploadOffer) -> str:
    """Coordinator side: complete once every planned part is held at its planned size."""
    plan = offer.plan
    listed = store.client.list_parts(offer.object_key, offer.upload_id)
    if listed is None:
        if store.exists(offer.sha256):
            return offer.sha256
        raise UploadGone(f"{offer.object_key}: upload gone and no object")
    held = {p.number: p for p in listed}
    bad = [n for n in range(1, plan.count + 1) if n not in held or held[n].size != plan.span(n)[1]]
    if bad:
        raise StoreError(f"{offer.object_key}: {len(bad)} parts missing or short, first {bad[0]}")
    try:
        store.client.complete_multipart(
            offer.object_key,
            offer.upload_id,
            [(n, held[n].etag) for n in range(1, plan.count + 1)],
            immutable=True,
        )
    except UploadGone:
        if store.exists(offer.sha256):  # a retried complete whose first reply was lost
            return offer.sha256
        raise
    if not store.exists(offer.sha256):  # a 412 on an object without our metadata
        raise ObjectConflict(f"{offer.object_key}: object carries no sha256 metadata")
    return offer.sha256


def _held(offer: UploadOffer, http: httpx.Client, retry: Retry | None) -> dict[int, int]:
    """Part number -> size of every part R2 holds, from the presigned ListParts pages."""
    out: dict[int, int] = {}
    for url in offer.list_urls:
        r = _call(retry, partial(http.get, url))
        if r.status_code >= 300:
            raise R2Error(f"ListParts {offer.object_key}: HTTP {r.status_code}")
        for p in _xml(r.content).iter("Part"):
            out[int(p.findtext("PartNumber") or 0)] = int(p.findtext("Size") or 0)
    return out


def _call(retry: Retry | None, send: Callable[[], httpx.Response]) -> httpx.Response:
    try:
        return retry.run(send) if retry else send()
    except httpx.HTTPError as e:  # type name only: the URL carries a signature
        raise R2Error(f"presigned request: {type(e).__name__}") from None
