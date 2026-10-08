"""python -m hypertrain.teacher estimate|label|build-records."""

from __future__ import annotations

import argparse
import json
import sys
from collections import Counter
from collections.abc import Iterator
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

from hypertrain.datasets.build import Loader, _key, apply_source_filter, parquet_loader
from hypertrain.datasets.registry import Source, load_registry
from hypertrain.teacher.adapters import RecordBuilder, to_question
from hypertrain.teacher.client import BudgetExceeded, TeacherClient
from hypertrain.teacher.config import TeacherConfig, TeacherError, load_config
from hypertrain.teacher.labeller import (
    LabelCache,
    Question,
    estimate_question,
    label_question,
)


def iter_questions(
    mix_id: str,
    loader: Loader,
    limit: int | None,
    only: set[str] | None = None,
    registry_path: Path | None = None,
) -> Iterator[tuple[Source, Question]]:
    """Same per-source order and limit as datasets.build (keyed sort, then first `limit`)."""
    sources, mixes, _ = load_registry(registry_path)
    if mix_id not in mixes:
        raise TeacherError(f"unknown mix {mix_id}")
    for cid, _ in mixes[mix_id].components:
        if only and cid not in only:
            continue
        src = sources[cid]
        rows = sorted(
            apply_source_filter(src, loader(src)),
            key=lambda r: _key(mix_id, cid, str(r[src.text_field])),
        )
        for r in rows[:limit] if limit else rows:
            yield src, to_question(src, r)


def estimate(
    qs: list[tuple[Source, Question]], cfg: TeacherConfig, cache: LabelCache
) -> dict[str, float | int]:
    calls, usd = 0, 0.0
    for _, q in qs:
        c, u = estimate_question(q, cfg, cache)
        calls, usd = calls + c, usd + u
    return {"questions": len(qs), "uncached_calls": calls, "est_usd": round(usd, 6)}


def run_label(
    qs: list[tuple[Source, Question]],
    cfg: TeacherConfig,
    cache: LabelCache,
    client: TeacherClient,
) -> dict[str, float | int | bool]:
    counts: Counter[str] = Counter()
    cost = 0.0
    stopped = False

    def one(item: tuple[Source, Question]) -> tuple[str, int, int, float] | None:
        try:
            r = label_question(item[1], cfg, cache, client)
        except BudgetExceeded:
            return None
        return r.status, r.calls, r.cached, r.cost

    with ThreadPoolExecutor(cfg.max_concurrency) as ex:
        for res in ex.map(one, qs):
            if res is None:
                stopped = True
                continue
            st, calls, cached, c = res
            counts[st] += 1
            counts["calls"] += calls
            counts["cached"] += cached
            cost += c
    return {
        "ok": counts["ok"],
        "failed": counts["failed"],
        "calls": counts["calls"],
        "cached_rotations": counts["cached"],
        "budget_stopped": stopped,
        "unlabelled": len(qs) - counts["ok"] - counts["failed"],
        "actual_usd": round(cost, 6),
    }


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(prog="python -m hypertrain.teacher")
    sub = p.add_subparsers(dest="cmd", required=True)
    for name in ("estimate", "label", "build-records"):
        s = sub.add_parser(name)
        s.add_argument("--mix", default="od-b-v1")
        s.add_argument("--config", type=Path)
        s.add_argument("--data-cache", type=Path, default=Path("cache"), help="parquet cache dir")
        s.add_argument("--limit", type=int, help="rows per source")
        if name != "build-records":
            s.add_argument("--sources", nargs="*", help="restrict to these source ids")
        if name == "label":
            s.add_argument("--max-usd", type=float)
            s.add_argument("--out", type=Path, help="write the JSON report here")
        if name == "build-records":
            s.add_argument("--out", type=Path, required=True)
    a = p.parse_args(argv)
    try:
        cfg = load_config(a.config, max_usd=getattr(a, "max_usd", None))
        cache = LabelCache(Path(cfg.cache_dir))
        if a.cmd == "build-records":
            from hypertrain.datasets.build import build_mix

            sources, mixes, _ = load_registry()
            rec = mixes[a.mix].record
            if rec is None:
                raise TeacherError(f"{a.mix} is not a record mix")
            from hypertrain.data.tokenizer import HFTokenizer
            from hypertrain.datasets.registry import tokenizer_pin

            pin = tokenizer_pin(mixes[a.mix].tokenizer)
            tok = HFTokenizer(a.data_cache / "tokenizer.json", pin.sha256, mixes[a.mix].tokenizer)
            rb = RecordBuilder(tok, rec, cfg, cache)
            m, _ = build_mix(
                a.mix, a.out, cache=a.data_cache, limit_docs=a.limit, tokenizer=tok, to_record=rb
            )
            print(json.dumps({**vars(rb.stats), "merkle_root": m.merkle_root}))
            return 0
        qs = list(
            iter_questions(a.mix, parquet_loader(a.data_cache), a.limit, set(a.sources or ()))
        )
        est = estimate(qs, cfg, cache)
        print(f"estimate: {json.dumps(est)}")
        if a.cmd == "estimate":
            return 0
        if est["est_usd"] > cfg.max_usd:
            print(f"estimate exceeds max_usd {cfg.max_usd}; will stop at the cap", file=sys.stderr)
        client = TeacherClient(cfg, cfg.api_key())
        rep = run_label(qs, cfg, cache, client)
        print(f"actual: {json.dumps(rep)}")
        if a.out:
            a.out.write_text(json.dumps({"estimate": est, "actual": rep}, indent=1))
        return 1 if rep["failed"] or rep["unlabelled"] else 0
    except (TeacherError, NotImplementedError, ValueError) as e:
        print(f"error: {e}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
