"""python -m hypertrain.datasets build|verify|publish|sources."""

from __future__ import annotations

import argparse
import json
import shutil
import sys
from collections import Counter
from pathlib import Path

from hypertrain.datasets.build import build_mix
from hypertrain.datasets.shards16 import ShardSet16Manifest, shard_name16, verify_shards16


def _sources(path: Path, column: str) -> Counter[str]:
    import pyarrow.parquet as pq

    files = [path] if path.is_file() else sorted(path.rglob("*.parquet"))
    c: Counter[str] = Counter()
    for f in files:
        c.update(pq.read_table(f, columns=[column]).column(column).to_pylist())
    return c


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(prog="python -m hypertrain.datasets")
    sub = p.add_subparsers(dest="cmd", required=True)
    b = sub.add_parser("build")
    b.add_argument("--mix", required=True)
    b.add_argument("--out", type=Path, required=True)
    b.add_argument("--cache", type=Path, required=True)
    b.add_argument("--limit-docs", type=int)
    v = sub.add_parser("verify")
    v.add_argument("data_dir", type=Path)
    pb = sub.add_parser("publish")
    pb.add_argument("data_dir", type=Path)
    pb.add_argument("--r2", type=Path, help="local target dir (content-addressed copy, tests)")
    pb.add_argument("--hf-mirror", type=Path, help="mirror target dir")
    pb.add_argument("--to", choices=["r2"], help="upload to Cloudflare R2 (R2_* env / --env-file)")
    pb.add_argument("--prefix", default="", help="key prefix inside the bucket")
    pb.add_argument("--env-file", type=Path)
    pb.add_argument(
        "--verify-units", type=int, default=8, help="random units re-read via R2_PUBLIC_BASE"
    )
    pl = sub.add_parser("publish-labels", help="upload an opaque teacher-label cache dir to R2")
    pl.add_argument("label_dir", type=Path)
    pl.add_argument("--prefix", required=True)
    pl.add_argument("--env-file", type=Path)
    s = sub.add_parser("sources")
    s.add_argument("repo", type=Path, help="pinned parquet file or directory")
    s.add_argument("--column", default="source")
    a = p.parse_args(argv)
    if a.cmd == "build":
        m, frag = build_mix(a.mix, a.out, cache=a.cache, limit_docs=a.limit_docs)
        print(json.dumps({"manifest": json.loads(m.to_json()), "dataset": frag}, indent=1))
    elif a.cmd == "verify":
        m = ShardSet16Manifest.from_json((a.data_dir / "manifest.json").read_text())
        errs = verify_shards16(a.data_dir, m)
        print("\n".join(errs) or "OK")
        return 1 if errs else 0
    elif a.cmd == "publish-labels":
        from hypertrain.storage.publish import publish_dir
        from hypertrain.storage.r2 import R2Client, load_config

        rep = publish_dir(R2Client(load_config(env_file=a.env_file)), a.label_dir, prefix=a.prefix)
        print(f"uploaded {len(rep.uploaded)}, skipped {len(rep.skipped)}")
    elif a.cmd == "publish":
        if a.to == "r2":
            from hypertrain.storage.publish import publish_shardset
            from hypertrain.storage.r2 import R2Client, load_config

            rep = publish_shardset(
                R2Client(load_config(env_file=a.env_file)),
                a.data_dir,
                prefix=a.prefix,
                verify_units=a.verify_units,
            )
            print(
                f"uploaded {len(rep.uploaded)}, skipped {len(rep.skipped)}, "
                f"verified units {rep.verified_units}"
            )
            return 0
        if not (a.r2 or a.hf_mirror):
            p.error("publish needs --to r2, --r2 DIR or --hf-mirror DIR")
        # HF mirror stays a local dir export (secondary, not the hot path).
        m = ShardSet16Manifest.from_json((a.data_dir / "manifest.json").read_text())
        for dest in filter(None, [a.r2, a.hf_mirror]):
            dest.mkdir(parents=True, exist_ok=True)
            for i, h in enumerate(m.shard_sha256s):
                shutil.copyfile(a.data_dir / shard_name16(i), dest / h)
            shutil.copyfile(a.data_dir / "manifest.json", dest / "manifest.json")
    else:
        for k, n in _sources(a.repo, a.column).most_common():
            print(f"{n}\t{k}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
