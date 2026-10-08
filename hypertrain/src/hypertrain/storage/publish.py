"""Idempotent, resumable publisher: shards first, manifest.json and build_record.json LAST."""

from __future__ import annotations

import hashlib
import json
import random
from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path

import httpx

from hypertrain.datasets.fetch import fetch_unit, unit_ranges
from hypertrain.datasets.shards16 import ShardSet16Manifest, shard_name16
from hypertrain.storage.r2 import R2Client, R2Error

LABELS_INDEX = "labels-index.json"


@dataclass
class PublishReport:
    uploaded: list[str] = field(default_factory=list)
    skipped: list[str] = field(default_factory=list)
    verified_units: int = 0


def _key(prefix: str, path: str) -> str:
    return f"{prefix.strip('/')}/{path}" if prefix.strip("/") else path


def _put(
    c: R2Client,
    key: str,
    data: bytes,
    rep: PublishReport,
    *,
    immutable: bool,
    ctype: str | None = None,
) -> None:
    """HEAD first; skip when size and sha256 metadata already match."""
    sha = hashlib.sha256(data).hexdigest()
    info = c.head_object(key)
    if info is not None and info.size == len(data) and info.sha256 == sha:
        rep.skipped.append(key)
        return
    # a same-key object with different content is a conflict for immutable keys (412 path)
    written = c.put_object(key, data, immutable=immutable and info is None, content_type=ctype)
    (rep.uploaded if written else rep.skipped).append(key)


def publish_shardset(
    c: R2Client,
    data_dir: Path,
    *,
    prefix: str = "",
    verify_units: int = 8,
    seed: int = 0,
    on_put: Callable[[str], None] | None = None,
) -> PublishReport:
    data_dir = Path(data_dir)
    m = ShardSet16Manifest.from_json((data_dir / "manifest.json").read_text())
    rep = PublishReport()
    for i, sha in enumerate(m.shard_sha256s):
        raw = (data_dir / shard_name16(i)).read_bytes()
        if hashlib.sha256(raw).hexdigest() != sha:
            raise R2Error(f"{shard_name16(i)}: local sha256 differs from manifest; run verify")
        _put(
            c,
            _key(prefix, m.shard_uri_template.format(shard_sha256=sha)),
            raw,
            rep,
            immutable=True,
            ctype="application/octet-stream",
        )
        if on_put:
            on_put(shard_name16(i))
    # readers key off manifest.json, so it goes last (build_record.json just before it is fine too,
    # but the manifest is the commit point)
    br = data_dir / "build_record.json"
    if br.is_file():
        _put(
            c,
            _key(prefix, "build_record.json"),
            br.read_bytes(),
            rep,
            immutable=False,
            ctype="application/json",
        )
    _put(
        c,
        _key(prefix, "manifest.json"),
        (data_dir / "manifest.json").read_bytes(),
        rep,
        immutable=False,
        ctype="application/json",
    )
    if verify_units and c.cfg.public_base:
        rep.verified_units = verify_public(
            m, _key_base(c.cfg.public_base, prefix), verify_units, seed=seed, client=c.http
        )
    return rep


def _key_base(base: str, prefix: str) -> str:
    p = prefix.strip("/")
    return f"{base.rstrip('/')}/{p}" if p else base.rstrip("/")


def verify_public(
    m: ShardSet16Manifest,
    base: str,
    n: int,
    *,
    seed: int = 0,
    client: httpx.Client | None = None,
) -> int:
    """Range-GET n random units through the public base and compare unit sha256s."""
    ids = random.Random(seed).sample(range(len(m.unit_sha256s)), min(n, len(m.unit_sha256s)))
    own = client is None
    http = client or httpx.Client(timeout=60.0)
    try:
        for rng in unit_ranges(m, ids):
            fetch_unit(http, m, rng, [base])  # raises FetchError on 4xx, short body or bad sha
    finally:
        if own:
            http.close()
    return len(ids)


def publish_dir(c: R2Client, src: Path, *, prefix: str) -> PublishReport:
    """Opaque directory (teacher-label cache). Files first, `labels-index.json` last."""
    src = Path(src)
    rep = PublishReport()
    files = sorted(p for p in src.rglob("*") if p.is_file())
    index: dict[str, dict[str, object]] = {}
    for p in files:
        rel = p.relative_to(src).as_posix()
        data = p.read_bytes()
        index[rel] = {"size": len(data), "sha256": hashlib.sha256(data).hexdigest()}
        _put(c, _key(prefix, rel), data, rep, immutable=False)
    _put(
        c,
        _key(prefix, LABELS_INDEX),
        json.dumps(index, indent=1, sort_keys=True).encode(),
        rep,
        immutable=False,
        ctype="application/json",
    )
    return rep
