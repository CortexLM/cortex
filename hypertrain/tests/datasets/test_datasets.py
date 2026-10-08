import copy
import functools
import hashlib
import http.server
import json
import threading
from pathlib import Path
from typing import Any

import numpy as np
import pytest

from hypertrain.data.assignment import assign_round
from hypertrain.data.shards import ShardSetManifest
from hypertrain.data.tokenizer import ByteTokenizer
from hypertrain.datasets.build import BuildError, Row, apply_source_filter, build_mix
from hypertrain.datasets.decontam import Decontaminator, jaccard_estimate, minhash, normalize
from hypertrain.datasets.fetch import FetchError, prefetch, unit_ranges
from hypertrain.datasets.registry import REGISTRY_PATH, RegistryError, load_registry
from hypertrain.datasets.shards16 import (
    ShardSet16Manifest,
    U16ShardSamples,
    shard_name16,
    verify_shards16,
    write_shards16,
)
from hypertrain.miner.cli import shard_sampler
from hypertrain.miner.core import MinerError
from hypertrain.protocol.example import example_manifest
from hypertrain.protocol.messages import DatasetSpec

RUN = bytes(range(32))
SIG = b"sig"
TOK = ByteTokenizer()
UNIT = 4


def _docs(n: int = 200) -> list[Row]:
    return [
        {
            "text": f"document {i} " + " ".join(f"w{(i * 7 + j) % 97}x{i}" for j in range(40)),
            "source": "a",
        }
        for i in range(n)
    ]


def _loader(docs: list[Row], extra: dict[str, list[Row]] | None = None) -> Any:
    def load(src: Any) -> list[Row]:
        if extra and src.id in extra:
            return extra[src.id]
        if src.id == "fineweb-edu-sample10bt":
            return docs
        return [{"question": f"held out question number {src.id}", "text": f"held {src.id}"}]

    return load


def _build(tmp: Path, docs: list[Row], reg: Path | None = None, extra: Any = None) -> Any:
    return build_mix(
        "od-a-proxy-v1",
        tmp,
        cache=tmp,
        registry_path=reg,
        loader=_loader(docs, extra),
        tokenizer=TOK,
        unit=UNIT,
        samples_per_shard=8,
    )


# ---- registry ----


def test_registry_loads_and_ratios_sum() -> None:
    sources, mixes, sha = load_registry()
    assert len(sha) == 64
    assert set(mixes) == {"od-a-v1", "od-a-proxy-v1", "od-b-v1", "od-c-v1"}
    for m in mixes.values():
        assert abs(sum(r for _, r in m.components) - 1) < 1e-9
    assert all(len(s.revision) == 40 for s in sources.values())
    assert sources["fineweb-edu"].revision == "87f09149ef4734204d70ed1d046ddc9ca3f2b8f9"
    assert sources["tulu-3-sft"].revision == "b14afda60f1bbebe55d5d2fa1e4df5042f97f8be"
    b = dict(mixes["od-b-v1"].components)
    assert b["multi-nli"] == 0.3125 and b["tulu-3-sft"] == 0.25


def test_no_ms_marco_anywhere() -> None:
    assert "ms_marco" not in REGISTRY_PATH.read_text()
    sources, mixes, _ = load_registry()
    assert all("ms_marco" not in s.repo for s in sources.values())


def test_registry_rejects_bad_ratio_and_ms_marco(tmp_path: Path) -> None:
    raw = json.loads(REGISTRY_PATH.read_text())
    bad = copy.deepcopy(raw)
    bad["mixes"]["od-b-v1"]["components"][0][1] = 0.5
    p = tmp_path / "r.json"
    p.write_text(json.dumps(bad))
    with pytest.raises(RegistryError, match="ratios"):
        load_registry(p)
    bad = copy.deepcopy(raw)
    bad["sources"]["x"] = {**raw["sources"]["boolq"], "repo": "microsoft/ms_marco"}
    p.write_text(json.dumps(bad))
    with pytest.raises(RegistryError, match="ms_marco"):
        load_registry(p)


# ---- u16 shards ----


def test_manifest_is_superset_of_shardset_manifest() -> None:
    old = set(ShardSetManifest.__dataclass_fields__)
    new = set(ShardSet16Manifest.__dataclass_fields__)
    assert old <= new and new - old == {"unit", "unit_sha256s", "unit_sha256_root"}


def test_write_verify_roundtrip_and_tamper(tmp_path: Path) -> None:
    rng = np.random.default_rng(0)
    rows = [rng.integers(0, 60000, 9, dtype=np.uint32) for _ in range(27)]
    m = write_shards16(rows, tmp_path, 8, UNIT, samples_per_shard=8)
    assert m.n_samples == 24 and m.extra["dropped_tail_samples"] == 3
    assert len(m.unit_sha256s) == 6 and m.n_shards == 3
    assert verify_shards16(tmp_path, m) == []
    s = U16ShardSamples(tmp_path, m.n_shards, m.samples_per_shard, 8)
    assert s.row(5).tolist() == rows[5].tolist() and len(s) == 24
    p = tmp_path / shard_name16(1)
    raw = bytearray(p.read_bytes())
    raw[0] ^= 1
    p.write_bytes(raw)
    assert any("shard-00001" in e for e in verify_shards16(tmp_path, m))
    assert ShardSet16Manifest.from_json(m.to_json()) == m


def test_write_rejects_oversize_token(tmp_path: Path) -> None:
    with pytest.raises(ValueError, match="u16"):
        write_shards16([np.full(9, 70000, dtype=np.uint32)] * 4, tmp_path, 8, UNIT)


# ---- builder ----


def test_build_fixture_200_docs_deterministic(tmp_path: Path) -> None:
    m1, frag = _build(tmp_path / "a", _docs())
    m2, _ = _build(tmp_path / "b", list(reversed(_docs())))
    assert m1.merkle_root == m2.merkle_root  # keyed shuffle: input order is irrelevant
    assert verify_shards16(tmp_path / "a", m1) == []
    assert m1.n_samples % UNIT == 0 and m1.n_samples > 0
    assert frag["assign_unit"] == UNIT and frag["unit_sha256_root"] == m1.unit_sha256_root
    assert frag["source"]["mix_id"] == "od-a-proxy-v1"
    assert (tmp_path / "a" / "build_record.json").is_file()
    s = U16ShardSamples(tmp_path / "a", m1.n_shards, m1.samples_per_shard, m1.seq_len)
    assert int(max(s.row(i).max() for i in range(len(s)))) < 259 + 3


def test_build_decontam_drops_eval_overlap(tmp_path: Path) -> None:
    docs = _docs()
    leak = (
        "the quick brown fox jumps over the lazy dog while "
        "the cat sleeps near the warm fire tonight"
    )
    docs[3] = {"text": "intro " + leak + " outro", "source": "a"}
    base, _ = _build(tmp_path / "a", _docs())
    m, _ = _build(tmp_path / "b", docs, extra={"mmlu-test": [{"question": leak}]})
    rec = json.loads((tmp_path / "b" / "build_record.json").read_text())
    assert len(rec["removed_ids"]) == 1
    assert m.merkle_root != base.merkle_root
    assert m.extra["removed_ids_sha256"] != base.extra["removed_ids_sha256"]


def test_build_refuses_root_mismatch(tmp_path: Path) -> None:
    raw = json.loads(REGISTRY_PATH.read_text())
    raw["build_records"] = {"od-a-proxy-v1": {"merkle_root": "0" * 64}}
    p = tmp_path / "r.json"
    p.write_text(json.dumps(raw))
    with pytest.raises(BuildError, match="build record"):
        _build(tmp_path / "o", _docs(), reg=p)


# ---- Tulu source filter ----


def _filtered_registry(tmp: Path) -> Path:
    raw = json.loads(REGISTRY_PATH.read_text())
    raw["sources"]["fineweb-edu-sample10bt"]["filter"] = {
        "column": "source",
        "allow": {"a": "ODC-BY-1.0"},
        "deny": ["no_robots"],
    }
    p = tmp / "reg.json"
    p.write_text(json.dumps(raw))
    return p


def test_source_filter_allow_drop_and_unclassified_fails(tmp_path: Path) -> None:
    reg = _filtered_registry(tmp_path)
    sources, _, _ = load_registry(reg)
    src = sources["fineweb-edu-sample10bt"]
    rows = [{"source": "a", "n": 1}, {"source": "no_robots", "n": 2}, {"source": "a", "n": 3}]
    assert [r["n"] for r in apply_source_filter(src, rows)] == [1, 3]
    with pytest.raises(BuildError, match="unclassified"):
        list(apply_source_filter(src, [{"source": "mystery"}]))
    docs = _docs()
    docs[0] = {**docs[0], "source": "no_robots"}
    m, _ = _build(tmp_path / "ok", docs, reg=reg)
    assert m.n_samples > 0
    docs[1] = {**docs[1], "source": "mystery"}
    with pytest.raises(BuildError, match="mystery"):
        _build(tmp_path / "bad", docs, reg=reg)


def test_tulu_registry_entry_is_fail_closed() -> None:
    sources, _, _ = load_registry()
    f = sources["tulu-3-sft"].filter
    assert f is not None and f["column"] == "source" and "no_robots" not in f["allow"]


# ---- decontam ----


def test_decontam_13gram_and_minhash() -> None:
    ref = " ".join(f"tok{i}" for i in range(30))
    dc = Decontaminator([ref])
    assert dc.contaminated("x " + " ".join(f"TOK{i}!" for i in range(5, 18)) + " y")
    assert not dc.contaminated(" ".join(f"tok{i}" for i in range(5, 17)))  # 12 words only
    near = ref.replace("tok29", "other")
    assert minhash(normalize(near)).shape == (128,)
    assert jaccard_estimate(minhash(normalize(ref)), minhash(normalize(near))) >= 0.8
    short = "alpha beta gamma delta epsilon zeta eta theta"
    assert Decontaminator([short], minhash_threshold=0.8).contaminated(short + " iota")
    assert not Decontaminator([short], minhash_threshold=0.8).contaminated(
        "completely unrelated words here now"
    )


# ---- Range fetch ----


class _RangeHandler(http.server.SimpleHTTPRequestHandler):
    fail_first: dict[str, int] = {}

    def log_message(self, *a: Any) -> None:
        pass

    def do_GET(self) -> None:
        if self.server.fail_all:  # type: ignore[attr-defined]
            self.send_error(500)
            return
        data = (Path(self.directory) / self.path.lstrip("/")).read_bytes()  # type: ignore[arg-type]
        a, b = self.headers["Range"].removeprefix("bytes=").split("-")
        body = data[int(a) : int(b) + 1]
        if self.server.corrupt:  # type: ignore[attr-defined]
            body = bytes([body[0] ^ 1]) + body[1:]
        self.send_response(206)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


def _serve(directory: Path, *, fail_all: bool = False, corrupt: bool = False) -> tuple[Any, str]:
    h = functools.partial(_RangeHandler, directory=str(directory))
    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), h)
    srv.fail_all = fail_all  # type: ignore[attr-defined]
    srv.corrupt = corrupt  # type: ignore[attr-defined]
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv, f"http://127.0.0.1:{srv.server_address[1]}"


def _published(tmp: Path) -> tuple[ShardSet16Manifest, Path]:
    rng = np.random.default_rng(1)
    rows = [rng.integers(0, 60000, 9, dtype=np.uint32) for _ in range(24)]
    m = write_shards16(rows, tmp / "src", 8, UNIT, samples_per_shard=8)
    pub = tmp / "pub"
    pub.mkdir()
    for i, h in enumerate(m.shard_sha256s):
        (pub / h).write_bytes((tmp / "src" / shard_name16(i)).read_bytes())
    return m, pub


def test_unit_ranges_math(tmp_path: Path) -> None:
    m, _ = _published(tmp_path)
    ub = UNIT * 9 * 2
    r = unit_ranges(m, [0, 3, 5])
    assert r[0] == (m.shard_sha256s[0], 0, ub, m.unit_sha256s[0])
    assert r[1] == (m.shard_sha256s[1], ub, 2 * ub, m.unit_sha256s[3])
    assert r[2][0] == m.shard_sha256s[2]


def test_prefetch_range_fixture_primary_and_fallback(tmp_path: Path) -> None:
    m, pub = _published(tmp_path)
    good, good_url = _serve(pub)
    dead, dead_url = _serve(pub, fail_all=True)
    try:
        order = [4, 1, 5, 0]
        got = list(prefetch(m, order, tmp_path / "c1", [good_url], streams=2, depth=2))
        assert got == order  # step order preserved
        raw = (tmp_path / "src" / shard_name16(0)).read_bytes()
        assert (tmp_path / "c1" / m.unit_sha256s[0]).read_bytes() == raw[: UNIT * 18]
        got = list(prefetch(m, [2], tmp_path / "c2", [dead_url, good_url]))  # mirror fallback
        assert got == [2]
    finally:
        good.shutdown()
        dead.shutdown()


def test_prefetch_rejects_corrupt_range(tmp_path: Path) -> None:
    m, pub = _published(tmp_path)
    bad, url = _serve(pub, corrupt=True)
    try:
        with pytest.raises(FetchError):
            list(prefetch(m, [0], tmp_path / "c", [url]))
    finally:
        bad.shutdown()


# ---- assignment units ----


def _assign(unit: int, **kw: Any) -> Any:
    args: dict[str, Any] = dict(n_samples=64, n_slots=4, batch=8, base_w=0)
    args.update(kw)
    return assign_round(RUN, 1, SIG, unit=unit, **args)


def test_unit1_equals_default_and_old_formula() -> None:
    from hypertrain.data.assignment import FeistelPRP, assign_key, epoch_key

    a = assign_round(RUN, 1, SIG, n_samples=64, n_slots=4, batch=8, base_w=16)
    assert a.slices == _assign(1, base_w=16).slices
    r, e = FeistelPRP(assign_key(RUN, 1, SIG), 32), FeistelPRP(epoch_key(RUN, 0), 64)
    old = tuple(tuple(e(16 + r(s * 8 + j)) for j in range(8)) for s in range(4))
    assert a.slices == old
    assert (
        hashlib.sha256(repr(a.slices).encode()).hexdigest()
        == hashlib.sha256(repr(old).encode()).hexdigest()
    )


def test_unit4_whole_aligned_units() -> None:
    a = _assign(4, n_samples=128, base_w=0)
    seen: set[int] = set()
    for sl in a.slices:
        assert len(sl) == 8
        for k in range(0, 8, 4):
            u = sl[k] // 4
            assert list(sl[k : k + 4]) == [u * 4 + j for j in range(4)]
        seen.update(sl)
    assert len(seen) == 32


@pytest.mark.parametrize("kw", [dict(batch=6), dict(n_samples=66), dict(base_w=2)])
def test_unit4_raises_on_unaligned(kw: dict[str, int]) -> None:
    with pytest.raises(ValueError, match="multiples of unit"):
        _assign(4, **{"n_samples": 128, **kw})


# ---- shard_sampler u16 branch ----


def _manifest_for(m: ShardSet16Manifest, **upd: Any) -> Any:
    base = example_manifest()
    ds = DatasetSpec(
        merkle_root=m.merkle_root,
        depth=m.depth,
        n_samples=m.n_samples,
        sample_format="u16[seq_len+1] token ids",
        shard_uri_template="{shard_sha256}",
        shard_sha256_root=m.shard_sha256_root,
        holdout_commit="0" * 64,
        assign_unit=m.unit,
        unit_sha256_root=m.unit_sha256_root,
    ).model_copy(update=upd)
    return base.model_copy(update={"dataset": ds})


def test_shard_sampler_u16_branch(tmp_path: Path) -> None:
    m, _ = _published(tmp_path)
    get = shard_sampler(tmp_path / "src", _manifest_for(m))
    s = U16ShardSamples(tmp_path / "src", m.n_shards, m.samples_per_shard, m.seq_len)
    assert get(7).tolist() == s.row(7).tolist() and get(7).dtype == np.uint32
    with pytest.raises(MinerError, match="unit root"):
        shard_sampler(tmp_path / "src", _manifest_for(m, unit_sha256_root="1" * 64))
