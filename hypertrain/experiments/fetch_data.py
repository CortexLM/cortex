"""Fetch a pinned FineWeb-Edu parquet and build byte-level u32 shards + Merkle manifest under data/.

Source: HuggingFaceFW/fineweb-edu (ODC-By 1.0), sample/10BT/000_00000.parquet at a pinned
revision and sha256. Nothing unpinned is downloaded. data/ is excluded from publish.

    uv run --frozen python experiments/fetch_data.py               # download (if needed) + build
    uv run --frozen python experiments/fetch_data.py --verify-only # re-hash everything, no network
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import secrets
import sys
from collections.abc import Iterator
from pathlib import Path

import httpx
import numpy as np
import numpy.typing as npt

from hypertrain.data.shards import (
    ShardSetManifest,
    ShardWriter,
    finalize,
    holdout_commit,
    pack_samples,
    verify_shards,
)
from hypertrain.data.tokenizer import ByteTokenizer

REPO = "HuggingFaceFW/fineweb-edu"
REVISION = "87f09149ef4734204d70ed1d046ddc9ca3f2b8f9"
FILE = "sample/10BT/000_00000.parquet"
SHA256 = "b1ba7b2ce4cb5ea6ef42dca40263eabb85f37700d01693a68e9b30a31d78e871"
SIZE = 2152819114
LICENSE = "ODC-By 1.0"
ATTRIBUTION = (
    "FineWeb-Edu by Hugging Face (HuggingFaceFW/fineweb-edu), licensed under ODC-By 1.0; "
    "subject to Common Crawl terms of use."
)
URL = f"https://huggingface.co/datasets/{REPO}/resolve/{REVISION}/{FILE}"

SEQ_LEN = 1024
SAMPLES_PER_SHARD = 4096
TRAIN_SHARDS = 48  # 48*4096*1025 = 201.5M byte tokens >= 200M
HOLDOUT_SHARDS = 2
HOLDOUT_MOD = 64  # doc goes to holdout iff sha256(doc id)[0:8] % 64 == 0 (disjoint by doc)

ROOT = Path(__file__).resolve().parent.parent / "data"
RAW = ROOT / "raw" / "fineweb-edu-000_00000.parquet"
MANIFEST = ROOT / "manifest.json"
SALT = ROOT / "holdout.salt"  # secret until reveal; 0600


def sha256_file(p: Path) -> str:
    h = hashlib.sha256()
    with p.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 22), b""):
            h.update(chunk)
    return h.hexdigest()


def download() -> None:
    if RAW.is_file() and RAW.stat().st_size == SIZE and sha256_file(RAW) == SHA256:
        print(f"raw: cached {RAW} sha256 ok")
        return
    RAW.parent.mkdir(parents=True, exist_ok=True)
    part = RAW.with_suffix(".part")
    h = hashlib.sha256()
    timeout = httpx.Timeout(60.0, connect=30.0)
    with (
        httpx.stream("GET", URL, follow_redirects=True, timeout=timeout) as r,
        part.open("wb") as f,
    ):
        r.raise_for_status()
        for chunk in r.iter_bytes(1 << 22):
            h.update(chunk)
            f.write(chunk)
    if part.stat().st_size != SIZE or h.hexdigest() != SHA256:
        part.unlink()
        sys.exit(f"raw: sha256/size mismatch for {URL}; refusing unpinned content")
    part.replace(RAW)
    print(f"raw: downloaded {SIZE} bytes sha256 ok")


def build() -> dict[str, object]:
    import pyarrow.parquet as pq  # type: ignore[import-untyped]  # pyarrow ships no stubs

    tok = ByteTokenizer()
    train = ShardWriter(ROOT / "train", SEQ_LEN, SAMPLES_PER_SHARD, TRAIN_SHARDS)
    hold = ShardWriter(ROOT / "holdout", SEQ_LEN, SAMPLES_PER_SHARD, HOLDOUT_SHARDS)
    n_docs = [0, 0]
    pf = pq.ParquetFile(RAW)

    def docs(split: int) -> Iterator[npt.NDArray[np.uint32]]:
        for batch in pf.iter_batches(batch_size=4096, columns=["id", "text"]):
            ids = batch.column(0).to_pylist()
            texts = batch.column(1).to_pylist()
            for doc_id, text in zip(ids, texts, strict=True):
                h = int.from_bytes(hashlib.sha256(doc_id.encode()).digest()[:8], "big")
                if (h % HOLDOUT_MOD == 0) == bool(split):
                    n_docs[split] += 1
                    yield np.frombuffer(text.encode("utf-8"), dtype=np.uint8).astype(np.uint32)

    for split, w in ((0, train), (1, hold)):
        for row in pack_samples(docs(split), SEQ_LEN, tok.eos_id):
            w.add(row)
            if w.full:
                break
        if not w.full:
            sys.exit(f"split {split}: not enough data for {w.max_shards} shards")

    tinfo = {"name": tok.name, "sha256": tok.sha256}
    source = {
        "repo": REPO,
        "revision": REVISION,
        "file": FILE,
        "sha256": SHA256,
        "size": SIZE,
        "license": LICENSE,
        "attribution": ATTRIBUTION,
        "holdout_rule": f"sha256(doc id)[:8] big-endian % {HOLDOUT_MOD} == 0",
    }
    tm, _ = finalize(train, tinfo, {"split": "train", "source": source, "docs_read": n_docs[0]})
    hm, _ = finalize(hold, tinfo, {"split": "holdout", "source": source, "docs_read": n_docs[1]})
    salt = secrets.token_bytes(32)
    fd = os.open(SALT, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "wb") as f:
        f.write(salt)
    commit = holdout_commit(bytes.fromhex(hm.merkle_root), salt)
    (ROOT / "train" / "manifest.json").write_text(tm.to_json())
    (ROOT / "holdout" / "manifest.json").write_text(hm.to_json())
    out = {
        "source": source,
        "tokenizer": tinfo,
        "dataset": {
            "merkle_root": tm.merkle_root,
            "depth": tm.depth,
            "n_samples": tm.n_samples,
            "sample_format": tm.sample_format,
            "seq_len": SEQ_LEN,
            "shard_uri_template": tm.shard_uri_template,
            "shard_sha256_root": tm.shard_sha256_root,
            "n_shards": tm.n_shards,
            "tokens": tm.n_samples * (SEQ_LEN + 1),
            "holdout_commit": commit,
        },
        "holdout": {"n_samples": hm.n_samples, "shard_sha256_root": hm.shard_sha256_root},
    }
    MANIFEST.write_text(json.dumps(out, indent=2, sort_keys=True))
    return out


def verify_only() -> int:
    if not MANIFEST.is_file():
        print(f"FAIL: {MANIFEST} missing; run without --verify-only first", file=sys.stderr)
        return 2
    top = json.loads(MANIFEST.read_text())
    errors: list[str] = []
    if top["source"]["revision"] != REVISION or top["source"]["sha256"] != SHA256:
        errors.append("manifest: source pin differs from script pin")
    if RAW.is_file():
        got = sha256_file(RAW)
        if got != SHA256:
            errors.append(f"{RAW.name}: sha256 {got} != pinned {SHA256}")
    else:
        print(f"note: raw parquet not present at {RAW}; shards verified from manifest only")
    for split in ("train", "holdout"):
        m = ShardSetManifest.from_json((ROOT / split / "manifest.json").read_text())
        errors += [f"{split}/{e}" for e in verify_shards(ROOT / split, m)]
        if split == "train" and m.merkle_root != top["dataset"]["merkle_root"]:
            errors.append("train/manifest: merkle_root differs from data/manifest.json")
    if SALT.is_file():
        hroot = ShardSetManifest.from_json((ROOT / "holdout" / "manifest.json").read_text())
        if (
            holdout_commit(bytes.fromhex(hroot.merkle_root), SALT.read_bytes())
            != top["dataset"]["holdout_commit"]
        ):
            errors.append("holdout: commit does not open with local salt")
    print(f"revision={REVISION}")
    print(f"sha256={SHA256}")
    print(f"merkle_root={top['dataset']['merkle_root']}")
    print(f"n_samples={top['dataset']['n_samples']} tokens={top['dataset']['tokens']}")
    print(f"holdout_commit={top['dataset']['holdout_commit']}")
    for e in errors:
        print(f"FAIL: {e}", file=sys.stderr)
    print("VERIFY " + ("FAIL" if errors else "OK"))
    return 1 if errors else 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--verify-only", action="store_true")
    a = ap.parse_args()
    if a.verify_only:
        return verify_only()
    ROOT.mkdir(exist_ok=True)
    download()
    print(json.dumps(build()["dataset"], indent=2))
    return verify_only()


if __name__ == "__main__":
    raise SystemExit(main())
