"""Per-unit HTTP Range fetch with sha256 check, base-URL fallback and ordered prefetch (A15)."""

from __future__ import annotations

import hashlib
from collections import deque
from collections.abc import Iterator, Sequence
from concurrent.futures import Future, ThreadPoolExecutor
from pathlib import Path

import httpx

from hypertrain.datasets.shards16 import ShardSet16Manifest

RETRIES = 3  # failures on one base before falling back to the next (A15)


class FetchError(RuntimeError):
    pass


def unit_ranges(m: ShardSet16Manifest, unit_ids: Sequence[int]) -> list[tuple[str, int, int, str]]:
    """(shard_sha, start, end_exclusive, unit_sha) for each unit id."""
    ub = m.unit * (m.seq_len + 1) * 2
    per = m.samples_per_shard // m.unit
    out = []
    for u in unit_ids:
        if not 0 <= u < len(m.unit_sha256s):
            raise IndexError(u)
        shard, k = divmod(u, per)
        out.append((m.shard_sha256s[shard], k * ub, (k + 1) * ub, m.unit_sha256s[u]))
    return out


def fetch_unit(
    client: httpx.Client,
    m: ShardSet16Manifest,
    rng: tuple[str, int, int, str],
    bases: Sequence[str],
) -> bytes:
    shard, start, end, want = rng
    path = m.shard_uri_template.format(shard_sha256=shard)
    errs: list[str] = []
    for base in bases:
        for _ in range(RETRIES):
            try:
                r = client.get(
                    f"{base.rstrip('/')}/{path}", headers={"Range": f"bytes={start}-{end - 1}"}
                )
                if r.status_code != 206 or len(r.content) != end - start:
                    raise FetchError(f"status {r.status_code}, {len(r.content)} bytes")
                if hashlib.sha256(r.content).hexdigest() != want:
                    raise FetchError("unit sha256 mismatch")
                return r.content
            except (httpx.HTTPError, FetchError) as e:
                errs.append(f"{base}: {e}")
    raise FetchError(f"unit {want}: all bases failed: {errs[-3:]}")


def prefetch(
    m: ShardSet16Manifest,
    unit_ids: Sequence[int],
    cache: Path,
    bases: Sequence[str],
    streams: int = 8,
    depth: int = 8,
    *,
    client: httpx.Client | None = None,
) -> Iterator[int]:
    """Yield unit ids in the given (step) order once cached at cache/<unit_sha256>; at most
    `depth` units in flight ahead of the consumer."""
    cache.mkdir(parents=True, exist_ok=True)
    ranges = unit_ranges(m, unit_ids)
    own = client is None
    c = client or httpx.Client()

    def work(i: int) -> int:
        dest = cache / ranges[i][3]
        if not (dest.is_file() and hashlib.sha256(dest.read_bytes()).hexdigest() == ranges[i][3]):
            tmp = dest.with_suffix(".tmp")
            tmp.write_bytes(fetch_unit(c, m, ranges[i], bases))
            tmp.replace(dest)
        return unit_ids[i]

    try:
        with ThreadPoolExecutor(max_workers=streams) as ex:
            q: deque[Future[int]] = deque()
            nxt = 0
            while nxt < len(ranges) or q:
                while nxt < len(ranges) and len(q) < depth:
                    q.append(ex.submit(work, nxt))
                    nxt += 1
                yield q.popleft().result()
    finally:
        if own:
            c.close()
