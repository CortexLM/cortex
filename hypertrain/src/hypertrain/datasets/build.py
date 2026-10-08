"""Deterministic offline builder (A7-A15, A20). No network: rows come from `loader`.

Default loader reads pinned parquet files the curator already fetched into
`cache/<source id>/**/*.parquet`; tests inject an in-memory loader.
"""

from __future__ import annotations

import hashlib
import json
from collections.abc import Callable, Iterable, Iterator
from pathlib import Path
from typing import Any

import numpy as np
import numpy.typing as npt

from hypertrain.data.shards import pack_samples
from hypertrain.data.tokenizer import HFTokenizer, Tokenizer
from hypertrain.datasets.decontam import Decontaminator
from hypertrain.datasets.registry import (
    OFFSET,
    Mix,
    Source,
    load_registry,
    tokenizer_pin,
)
from hypertrain.datasets.shards16 import ShardSet16Manifest, write_shards16
from hypertrain.protocol.hashing import sha256_hex

Row = dict[str, Any]
Loader = Callable[[Source], Iterable[Row]]
ToRecord = Callable[[Source, Row], npt.NDArray[np.uint32] | None]
UNIT = 512


class BuildError(ValueError):
    pass


def apply_source_filter(src: Source, rows: Iterable[Row]) -> Iterator[Row]:
    """Fail-closed allow-list (A9): allowed passes, denied dropped, else raises."""
    f = src.filter
    if not f:
        yield from rows
        return
    col, allow, deny = f["column"], f.get("allow", {}), set(f.get("deny", ()))
    for r in rows:
        v = r.get(col)
        if v in allow:
            yield r
        elif v in deny:
            continue
        else:
            raise BuildError(
                f"{src.id}: unclassified {col} value {v!r}; classify it in the registry"
            )


def parquet_loader(cache: Path) -> Loader:
    def load(src: Source) -> Iterable[Row]:
        import pyarrow.parquet as pq

        files = sorted((cache / src.id).rglob("*.parquet"))
        if not files:
            raise BuildError(f"{src.id}: no parquet files in {cache / src.id}")
        for p in files:
            yield from pq.read_table(p).to_pylist()

    return load


def _key(mix_id: str, src_id: str, text: str) -> bytes:
    return hashlib.sha256(f"od-shuffle-v1|{mix_id}|{src_id}|".encode() + text.encode()).digest()


def _eval_text(r: Row) -> str:
    return " ".join(str(v) for v in r.values() if isinstance(v, str))


def build_mix(
    mix_id: str,
    out: Path,
    *,
    cache: Path,
    limit_docs: int | None = None,
    registry_path: Path | None = None,
    loader: Loader | None = None,
    tokenizer: Tokenizer | None = None,
    to_record: ToRecord | None = None,
    unit: int = UNIT,
    samples_per_shard: int | None = None,
) -> tuple[ShardSet16Manifest, dict[str, Any]]:
    """Returns (manifest, DatasetSpec fragment). Writes shards, manifest.json, build_record.json."""
    sources, mixes, reg_sha = load_registry(registry_path)
    if mix_id not in mixes:
        raise BuildError(f"unknown mix {mix_id}")
    mix: Mix = mixes[mix_id]
    load = loader or parquet_loader(cache)
    if tokenizer is None:
        pin = tokenizer_pin(mix.tokenizer, registry_path)
        tokenizer = HFTokenizer(cache / "tokenizer.json", pin.sha256, mix.tokenizer)
    if tokenizer.vocab_size + OFFSET > 0xFFFF:
        raise BuildError("vocab does not fit u16")
    # decontam reference set: eval sources and the text holdout
    eval_texts = [_eval_text(r) for sid in mix.decontam_against for r in load(sources[sid])]
    dc = Decontaminator(eval_texts, minhash_threshold=0.8 if mix.record else None)
    removed: list[str] = []
    per_comp: list[list[tuple[bytes, Row]]] = []
    for cid, _ in mix.components:
        src = sources[cid]
        kept: list[tuple[bytes, Row]] = []
        for r in apply_source_filter(src, load(src)):
            text = str(r[src.text_field]) if mix.seq_len else _eval_text(r)
            k = _key(mix_id, cid, text)
            if dc.contaminated(text):
                removed.append(k.hex())
                continue
            kept.append((k, r))
        kept.sort(key=lambda t: t[0])
        per_comp.append(kept[:limit_docs] if limit_docs else kept)
    tok_meta = {"name": tokenizer.name, "sha256": tokenizer.sha256}
    extra = {
        "mix_id": mix_id,
        "removed_ids_sha256": sha256_hex("\n".join(sorted(removed)).encode()),
    }
    if mix.seq_len is not None:
        m = _build_text(
            mix, sources, per_comp, tokenizer, out, tok_meta, extra, unit, samples_per_shard
        )
    else:
        if to_record is None:
            raise BuildError(
                f"{mix_id}: record mixes need to_record (teacher labels are built offline)"
            )
        recs = (
            rec
            for (cid, _), kept in zip(mix.components, per_comp, strict=True)
            for _, r in kept
            if (rec := to_record(sources[cid], r)) is not None
        )
        assert mix.record is not None
        width = _record_len(mix.record)
        m = write_shards16(
            recs,
            out,
            width - 1,
            unit,
            samples_per_shard=samples_per_shard,
            tokenizer=tok_meta,
            extra=extra,
        )
    if mix.expected_merkle_root and mix.expected_merkle_root != m.merkle_root:
        raise BuildError(f"{mix_id}: built root {m.merkle_root} != registry build record")
    fragment = {
        "merkle_root": m.merkle_root,
        "depth": m.depth,
        "n_samples": m.n_samples,
        "sample_format": m.sample_format,
        "shard_uri_template": m.shard_uri_template,
        "shard_sha256_root": m.shard_sha256_root,
        "assign_unit": m.unit,
        "unit_sha256_root": m.unit_sha256_root,
        "source": {"mix_id": mix_id, "registry_sha256": reg_sha},
    }
    (out / "build_record.json").write_text(
        json.dumps({"fragment": fragment, "removed_ids": sorted(removed)}, indent=1, sort_keys=True)
    )
    return m, fragment


def _record_len(r: dict[str, Any]) -> int:
    q, k = r["n_questions"], r["n_options"]
    return r["state_len"] + q * r["instr_len"] + q * k * r["opt_len"] + 2 * q + q * (k + 1)


def _build_text(
    mix: Mix,
    sources: dict[str, Source],
    per_comp: list[list[tuple[bytes, Row]]],
    tok: Tokenizer,
    out: Path,
    tok_meta: dict[str, str],
    extra: dict[str, Any],
    unit: int,
    sps: int | None,
) -> ShardSet16Manifest:
    assert mix.seq_len is not None
    toks: list[list[tuple[bytes, list[int]]]] = []
    for (cid, _), kept in zip(mix.components, per_comp, strict=True):
        toks.append(
            [
                (k, [t + OFFSET for t in tok.encode(str(r[sources[cid].text_field]))])
                for k, r in kept
            ]
        )
    # ratios by tokens: total T is the largest budget every component can fill
    avail = [sum(len(t) + 1 for _, t in c) for c in toks]
    total = min(a / r for a, (_, r) in zip(avail, mix.components, strict=True))
    docs: list[tuple[bytes, list[int]]] = []
    for c, (_, r) in zip(toks, mix.components, strict=True):
        quota, used = r * total, 0
        for k, t in c:
            if used >= quota:
                break
            docs.append((k, t))
            used += len(t) + 1
    docs.sort(key=lambda d: d[0])  # keyed global shuffle
    eos = tok.eos_id + OFFSET
    samples = pack_samples((t for _, t in docs), mix.seq_len, eos)
    return write_shards16(
        samples, out, mix.seq_len, unit, samples_per_shard=sps, tokenizer=tok_meta, extra=extra
    )
