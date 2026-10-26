from __future__ import annotations

import copy
import json
import sqlite3
import threading
from pathlib import Path
from typing import Any

import pytest
from fastapi.testclient import TestClient

from hypertrain.challenge.store import _SCHEMA
from hypertrain.protocol.example import example_manifest
from hypertrain.protocol.messages import f32hex
from hypertrain.public_api import create_public_app
from hypertrain.public_api.views import (
    default_snapshot,
    list_runs,
    model_description,
    od_param_count,
    prune_none,
    run_snapshot,
)

from .frontend_validators import is_encoder_arch, is_run, is_snapshot

HK = ["5" + c * 47 for c in "ABCD"]  # 48-char stand-in hotkeys
GEN, PERIOD = example_manifest().beacon.genesis_time, example_manifest().beacon.period


def _round_body(w: int, roster: list[str], base: int) -> dict[str, Any]:
    o = base + w * 100
    return {
        "w": w,
        "roster": [{"hotkey": h, "slot": i, "q_i": f32hex(0.1)} for i, h in enumerate(roster)],
        "d_open": o + 1,
        "d_assign": o + 10,
        "d_commit": o + 40,
        "d_audit": o + 50,
        "d_upload": o + 60,
        "d_final": o + 70,
    }


def _commit(hk: str, w: int, tokens: int) -> str:
    return json.dumps({"body": {"w": w, "hotkey": hk, "tokens": tokens, "delta_bytes": 1000}})


def seed(
    path: Path,
    *,
    manifest: dict[str, Any] | None = None,
    config: dict[str, Any] | None = None,
    finalize: bool = True,
    rounds: int = 2,
    commits: bool = True,
    status: str = "running",
    base: int = 1000,
    beacon_max: int = 1020,
) -> str:
    m = manifest or json.loads(example_manifest().model_dump_json())
    run_id = f"run-{m['outer']['opt']}-{base}-{rounds}-{int(finalize)}-{status}"
    db = sqlite3.connect(path)
    db.executescript(_SCHEMA)
    db.execute("INSERT OR REPLACE INTO beacon VALUES(?, 'sig', 'rand', 1)", (beacon_max,))
    cfg = {"train_rounds": 10, "final_after_upload": 5} if config is None else config
    db.execute(
        "INSERT INTO runs VALUES(?, ?, '{}', ?, ?)",
        (run_id, json.dumps(m), status, json.dumps(cfg)),
    )
    roster = HK[:3]
    for hk, cl, reg in ((HK[0], "c1", "eu"), (HK[1], "c1", "eu"), (HK[2], "c2", None)):
        db.execute(
            "INSERT INTO roster VALUES(?, ?, 1, 0, ?, ?, '[]', 0, 0, 0)", (run_id, hk, cl, reg)
        )
    for w in range(rounds):
        body = _round_body(w, roster, base)
        done = finalize and w < rounds - 1
        final_at = GEN + (body["d_final"] - 1) * PERIOD if done else None
        db.execute(
            "INSERT INTO rounds VALUES(?, ?, ?, '{}', 0, '{}', 0, NULL, NULL, NULL, ?, NULL, NULL)",
            (run_id, w, json.dumps(body), final_at),
        )
        # HK[2] is rostered but has no miners row in any round (rostered-only).
        for hk, st, tok in ((HK[0], "COMMITTED", 100), (HK[1], "MISMATCH", 50)):
            if commits:
                accept = json.dumps({"body": {"n_gpus": 2}})
                db.execute(
                    "INSERT INTO miners(run_id, w, hotkey, status, accept, commit_env) "
                    "VALUES(?,?,?,?,?,?)",
                    (run_id, w, hk, st, accept, _commit(hk, w, tok)),
                )
            else:
                db.execute(
                    "INSERT INTO miners(run_id, w, hotkey, status) VALUES(?,?,?, 'ASSIGNED')",
                    (run_id, w, hk),
                )
    db.commit()
    db.close()
    return run_id


def od_manifest(**over: Any) -> dict[str, Any]:
    m = json.loads(example_manifest().model_dump_json())
    m["model"].update(
        arch="od-encoder",
        n_layers=12,
        d_model=768,
        n_heads=12,
        d_ff=3072,
        vocab=50432,
        seq_len=1024,
        param_count=123_753_984,
        compute_dtype="bf16",
        od={
            "preset": "od-base",
            "head_layers": 2,
            "objective": "mlm",
            "mask_ratio": f32hex(0.3),
            "mask_seed": 1,
            "tokenizer_offset": 3,
            "warm_start": False,
            "record": None,
            "decision_rule": "log",
            "rps_weight": f32hex(1.0),
            "distill_temp": f32hex(1.0),
        },
    )
    m["model"].update(over)
    m["dataset"]["source"] = {"mix_id": "od-a-v1", "registry_sha256": "0" * 64}
    m["reference_spec"]["profile"] = "od-bf16-det-eager-v1"
    return m


def ro(path: Path) -> sqlite3.Connection:
    db = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
    db.row_factory = sqlite3.Row
    return db


def walk(x: Any) -> None:
    assert x is not None
    if isinstance(x, dict):
        for v in x.values():
            walk(v)
    elif isinstance(x, list):
        for v in x:
            walk(v)


@pytest.fixture
def db_path(tmp_path: Path) -> Path:
    return tmp_path / "challenge.db"


def snap_of(path: Path, run_id: str, metrics: list[dict[str, Any]] | None = None) -> dict[str, Any]:
    s = run_snapshot(ro(path), run_id, metrics or [])
    assert s is not None
    return s


def test_snapshot_required_keys_typed(db_path: Path) -> None:
    rid = seed(db_path)
    s = snap_of(db_path, rid, [{"run_id": rid, "w": 0, "value": 2.5}])
    assert is_snapshot(s)
    walk(s)
    assert s["metrics"]["loss"] == [
        {"t": s["rounds"][0]["syncedAt"], "round": 0, "step": 30, "value": 2.5}
    ]
    assert s["architecture"]["family"] == "decoder-only transformer"
    assert "source" not in s


def test_rostered_only_and_never_synced_cluster(db_path: Path) -> None:
    rid = seed(db_path)
    s = snap_of(db_path, rid)
    assert HK[2] not in {p["id"] for p in s["participants"]}
    c2 = next(c for c in s["clusters"] if c["id"] == "c2")
    assert (c2["nodes"], c2["lastSyncRound"], c2["tokensPerSec"], c2["share"]) == (1, 0, 0, 0)
    assert c2["lastSyncAt"] == s["run"]["startedAt"]
    assert c2["region"] == "unknown"
    assert is_snapshot(s)


def test_faulted_commits_excluded(db_path: Path) -> None:
    rid = seed(db_path)
    s = snap_of(db_path, rid)
    assert s["run"]["tokensTrained"] == 100  # only finalized w0, HK[0]; MISMATCH dropped
    p = {x["id"]: x for x in s["participants"]}
    assert p[HK[1]]["tokensContributed"] == 0 and p[HK[1]]["roundsContributed"] == 0
    assert p[HK[0]]["tokensContributed"] == 100 and p[HK[0]]["share"] == 1.0
    assert p[HK[0]]["gpus"] == 2 and p[HK[0]]["label"] == HK[0][:6] + "..." + HK[0][-4:]
    assert p[HK[0]]["tokensPerSec"] == 100 / (30 * PERIOD)


def test_no_finalized_round(db_path: Path) -> None:
    rid = seed(db_path, finalize=False, commits=False)
    s = snap_of(db_path, rid)
    assert is_snapshot(s)
    assert s["run"]["tokensTrained"] == 0 and "endedAt" not in s["run"]
    assert all("syncedAt" not in r for r in s["rounds"])
    assert all(c["tokensPerSec"] == 0 and c["lastSyncRound"] == 0 for c in s["clusters"])
    assert s["metrics"]["loss"] == []
    walk(s)


def test_total_rounds_absent_fallbacks(db_path: Path) -> None:
    rid = seed(db_path, rounds=3)
    run = snap_of(db_path, rid)["run"]
    assert run["currentRound"] == 2 and run["totalRounds"] == 3
    assert run["targetTokens"] == run["tokensTrained"] and run["status"] == "running"


def test_total_rounds_set_and_completed(db_path: Path) -> None:
    cfg = {"train_rounds": 10, "final_after_upload": 5, "total_rounds": 1}
    rid = seed(db_path, config=cfg)
    run = snap_of(db_path, rid)["run"]
    m = example_manifest()
    assert run["status"] == "completed" and "endedAt" in run
    assert run["totalRounds"] == 1
    assert (
        run["targetTokens"]
        == 3 * m.inner.H * m.inner.micro_batch * m.inner.grad_accum * m.model.seq_len
    )
    s = snap_of(db_path, rid)
    assert {p["status"] for p in s["participants"]} == {"finished"}
    cfg2 = {**cfg, "total_rounds": 5}
    assert snap_of(db_path, seed(db_path, config=cfg2, base=2000))["run"]["status"] == "running"


def test_status_mapping(db_path: Path) -> None:
    assert snap_of(db_path, seed(db_path, status="created"))["run"]["status"] == "scheduled"
    s = snap_of(db_path, seed(db_path, status="paused", base=3000))
    assert s["run"]["status"] == "stopped" and "endedAt" in s["run"]
    assert is_run(s["run"])


def test_participant_status_current_round(db_path: Path) -> None:
    rid = seed(db_path)
    p = {x["id"]: x for x in snap_of(db_path, rid)["participants"]}
    assert p[HK[0]]["status"] == "syncing"  # committed, round not final
    assert p[HK[1]]["status"] == "offline"  # faulted


def test_exclusions_and_order(db_path: Path) -> None:
    a = seed(db_path, base=1000)
    b = seed(db_path, base=5000)
    sp = json.loads(example_manifest().model_dump_json())
    sp["outer"]["opt"] = "sparseloco"
    seed(db_path, manifest=sp)  # excluded: sparseloco
    seed(db_path, rounds=0, base=7000)  # excluded: zero rounds
    seed(db_path, config={}, base=8000)  # config {} is set, kept
    db = sqlite3.connect(db_path)
    db.execute("UPDATE runs SET config=NULL WHERE run_id=?", (seed(db_path, base=9000),))
    db.commit()
    db.close()
    ids = [r["id"] for r in list_runs(ro(db_path))]
    assert ids.index(b) < ids.index(a)
    assert not any("sparseloco" in i or "-0-" in i for i in ids)
    assert all(is_run(r) for r in list_runs(ro(db_path)))
    assert run_snapshot(ro(db_path), "nope", []) is None


def test_default_snapshot_prefers_running(db_path: Path) -> None:
    seed(db_path, status="paused", base=9000)
    run = seed(db_path, base=1000)
    s = default_snapshot(ro(db_path), [])
    assert s is not None and s["run"]["id"] == run


def test_participant_cap(db_path: Path) -> None:
    rid = seed(db_path)
    db = sqlite3.connect(db_path)
    for i in range(600):
        hk = f"5{i:047d}"
        db.execute("INSERT INTO roster VALUES(?, ?, 1, 0, 'c9', 'eu', '[]', 0, 0, 0)", (rid, hk))
        db.execute(
            "INSERT INTO miners(run_id, w, hotkey, status) VALUES(?, 0, ?, 'ASSIGNED')", (rid, hk)
        )
    db.commit()
    db.close()
    s = snap_of(db_path, rid)
    assert len(s["participants"]) == 500 and is_snapshot(s)
    assert s["participants"][0]["id"] == HK[0]  # sorted by tokens desc


def test_od_snapshot_encoder_architecture(db_path: Path) -> None:
    rid = seed(db_path, manifest=od_manifest())
    s = snap_of(db_path, rid)
    a = s["architecture"]
    assert is_encoder_arch(a) and is_snapshot(s)
    assert a["parameters"] == 143_241_220 and a["contextLength"] == 1024
    assert a["decisionHead"] == {
        "layers": 2,
        "questionTypes": ["choice", "score", "noul"],
        "unknownSlot": True,
        "trainedInThisRun": False,
    }
    assert s["run"]["model"].startswith("OpenDecision od-base") and s["run"]["name"].endswith(
        "/ od-a-v1"
    )


def test_od_decision_record_context(db_path: Path) -> None:
    m = od_manifest()
    m["model"]["od"].update(objective="decision", record={"state_len": 512})
    a = snap_of(db_path, seed(db_path, manifest=m))["architecture"]
    assert a["contextLength"] == 512 and a["decisionHead"]["trainedInThisRun"] is True


def test_validator_port_rejects() -> None:
    good = {
        "family": "encoder + decision head",
        "parameters": 1,
        "layers": 1,
        "dModel": 1,
        "heads": 1,
        "mlpHidden": 1,
        "contextLength": 1,
        "vocabSize": 1,
        "norm": "LayerNorm",
        "activation": "GELU",
        "positional": "RoPE",
        "pretrainObjective": "mlm",
        "calibration": "temperature",
        "decisionHead": {
            "layers": 1,
            "questionTypes": ["choice"],
            "unknownSlot": True,
            "trainedInThisRun": False,
        },
    }
    assert is_encoder_arch(good)
    bad = copy.deepcopy(good)
    bad["norm"] = "RMSNorm"
    assert not is_encoder_arch(bad)
    bad = copy.deepcopy(good)
    bad["decisionHead"]["questionTypes"] = ["x"]
    assert not is_encoder_arch(bad)


@pytest.mark.parametrize(
    ("args", "want"),
    [
        ((768, 12, 12, 2, 50432), 143_241_220),
        ((1024, 24, 16, 4, 50432), 422_072_324),
        ((64, 2, 4, 1, 259), 187_140),
    ],
)
def test_od_param_count_golden(args: tuple[int, ...], want: int) -> None:
    assert od_param_count(*args) == want


def test_model_description(db_path: Path) -> None:
    m = od_manifest()
    d = model_description(m)
    assert d["arch"] == "od-encoder" and d["parametersFull"] == 143_241_220
    assert d["parameters"] == 123_753_984 and d["encoder"]["headDim"] == 64
    assert d["embedding"]["tokenizer"]["offset"] == 3
    assert d["decisionHead"]["presentInRun"] is False
    assert d["training"]["profile"] == "od-bf16-det-eager-v1"
    walk(d)
    dec = model_description(json.loads(example_manifest().model_dump_json()))
    assert dec["family"] == "decoder-only transformer"


def test_prune_none() -> None:
    assert prune_none({"a": None, "b": [{"c": None, "d": 1}], "e": {"f": None}}) == {
        "b": [{"d": 1}],
        "e": {},
    }


# ---- HTTP layer ----


def client(path: Path, **kw: Any) -> TestClient:
    return TestClient(create_public_app(path, path.parent / "metrics.jsonl", **kw))


def test_routes_bare_array_cache_etag(db_path: Path) -> None:
    rid = seed(db_path)
    c = client(db_path)
    r = c.get("/v1/runs")
    assert r.status_code == 200 and isinstance(r.json(), list) and r.json()[0]["id"] == rid
    assert (
        r.headers["cache-control"] == "public, max-age=15, s-maxage=60, stale-while-revalidate=300"
    )
    etag = r.headers["etag"]
    assert etag.startswith('"') and len(etag) == 66
    r2 = c.get("/v1/runs", headers={"If-None-Match": etag})
    assert r2.status_code == 304 and r2.content == b""
    assert is_snapshot(c.get(f"/v1/runs/{rid}").json())
    assert is_snapshot(c.get("/v1/snapshot").json())
    walk(c.get(f"/v1/runs/{rid}").json())


def test_metrics_file_feeds_loss(db_path: Path) -> None:
    rid = seed(db_path)
    (db_path.parent / "metrics.jsonl").write_text(
        json.dumps({"run_id": rid, "w": 0, "value": 1.5})
        + "\nnot json\n"
        + json.dumps({"run_id": "other", "w": 0, "value": 9.0})
        + "\n"
    )
    loss = client(db_path).get(f"/v1/runs/{rid}").json()["metrics"]["loss"]
    assert [p["value"] for p in loss] == [1.5]


def test_404s_and_empty(db_path: Path) -> None:
    c = client(db_path)
    seed(db_path, rounds=0)
    assert c.get("/v1/runs").json() == []
    r = c.get("/v1/snapshot")
    assert r.status_code == 404 and r.json() == {"detail": "no runs"}
    assert r.headers["cache-control"] == "public, max-age=10"
    r = c.get("/v1/runs/zzz")
    assert r.status_code == 404 and r.headers["cache-control"] == "public, max-age=10"
    assert c.get("/v1/runs/zzz/model").status_code == 404
    assert c.post("/v1/runs").status_code == 405


def test_model_route(db_path: Path) -> None:
    rid = seed(db_path, manifest=od_manifest())
    body = client(db_path).get(f"/v1/runs/{rid}/model").json()
    assert body["arch"] == "od-encoder" and body["manifest"]["runId"] == rid
    assert body["dataset"]["mixId"] == "od-a-v1"


def test_ttl_cache_and_503(db_path: Path, tmp_path: Path) -> None:
    now = [0.0]
    c = client(db_path, clock=lambda: now[0], ttl=15.0)
    assert c.get("/v1/runs").status_code == 503  # file missing, mode=ro cannot open
    now[0] = 1.0
    seed(db_path)
    first = c.get("/v1/runs").json()  # the earlier 503 was not cached
    assert len(first) == 1
    seed(db_path, base=5000)
    assert len(c.get("/v1/runs").json()) == 1  # served from cache
    now[0] = 30.0
    assert len(c.get("/v1/runs").json()) == 2


def test_concurrent_writer(db_path: Path) -> None:
    rid = seed(db_path)
    stop = threading.Event()

    def write() -> None:
        w = sqlite3.connect(db_path, isolation_level=None)
        w.execute("PRAGMA journal_mode=WAL")
        i = 0
        while not stop.is_set():
            i += 1
            w.execute("INSERT OR REPLACE INTO meta VALUES('x', ?)", (str(i),))
        w.close()

    t = threading.Thread(target=write)
    t.start()
    try:
        for _ in range(20):
            assert is_snapshot(snap_of(db_path, rid))
    finally:
        stop.set()
        t.join()
