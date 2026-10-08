from __future__ import annotations

import hashlib
import json
import math
from pathlib import Path

import numpy as np
import pytest
from agg_helpers import (
    COORD,
    SHAPES,
    commit,
    fresh,
    hotkey,
    params,
    raw_delta,
    scenario,
    setup,
    theta0,
)
from test_aggregator import DATASET, _decoded, _ref_hier, _sq

from hypertrain.aggregator.checkpoint import (
    finalize_checkpoint,
    verify_checkpoint,
    write_checkpoint,
)
from hypertrain.aggregator.core import (
    Aggregator,
    FinalityError,
    HashMismatch,
    JournalError,
    ReplayMismatch,
    SequenceError,
    get_object,
    load_state,
    replay_tape,
    sign_tape,
    th,
)
from hypertrain.data.store import LocalFSStore
from hypertrain.protocol.example import example_manifest
from hypertrain.protocol.messages import f32val


def test_apply_round_enforces_sequence(tmp_path: Path) -> None:
    agg, s0 = setup(tmp_path)
    cs = [commit(agg.store, 0, i, raw_delta(i)) for i in range(2)]
    with pytest.raises(SequenceError):
        agg.apply_round(5, s0, [c.model_copy(update={"w": 5}) for c in cs])
    t0 = agg.apply_round(0, s0, cs)
    with pytest.raises(SequenceError):
        agg.apply_round(0, s0, cs)
    c1 = [commit(agg.store, 1, i, raw_delta(10 + i)) for i in range(2)]
    with pytest.raises(SequenceError):
        agg.apply_round(1, s0, c1)
    with pytest.raises(SequenceError):
        agg.apply_round(2, t0["body"]["out_state"], [c.model_copy(update={"w": 2}) for c in c1])
    agg.apply_round(1, t0["body"]["out_state"], c1)
    assert agg.next_round == 2


def test_second_rollback_rejected_state_unchanged(tmp_path: Path) -> None:
    sc = scenario(tmp_path)
    agg, bad = sc["agg"], sc["bad"]
    rb = agg.rollback(sc["t0"], sc["t1"], [bad], [], 10**6)
    journal = agg.journal.path.read_bytes()
    applied = {w: json.dumps(t, sort_keys=True) for w, t in agg.applied.items()}
    with pytest.raises(SequenceError):
        agg.rollback(sc["t0"], sc["t1"], [bad], [], 10**6)
    with pytest.raises(SequenceError):
        agg.rollback(rb.tapes[0], rb.tapes[1], [bad], [], 10**6)
    with pytest.raises(SequenceError):
        agg.rollback(rb.tapes[0], rb.tapes[1], [hotkey(42)], [], 10**6)
    assert agg.journal.path.read_bytes() == journal
    assert {w: json.dumps(t, sort_keys=True) for w, t in agg.applied.items()} == applied


def test_rollback_refused_once_next_round_applied(tmp_path: Path) -> None:
    sc = scenario(tmp_path)
    agg = sc["agg"]
    c2 = [commit(agg.store, 2, i, raw_delta(20 + i)) for i in range(2)]
    agg.apply_round(2, sc["t1"]["body"]["out_state"], c2)
    with pytest.raises(SequenceError):
        agg.rollback(sc["t0"], sc["t1"], [sc["bad"]], [], 10**6)


def test_restart_keeps_finality_and_sequence(tmp_path: Path) -> None:
    sc = scenario(tmp_path)
    sc["agg"].finalize_round(0)
    again = fresh(tmp_path)
    assert again.final == {0} and again.next_round == 2
    assert again.applied[1]["body"]["out_state"] == sc["t1"]["body"]["out_state"]
    with pytest.raises(FinalityError):
        again.rollback(sc["t0"], sc["t1"], [sc["bad"]], [], 10**6)
    with pytest.raises(SequenceError):
        again.apply_round(1, sc["t0"]["body"]["out_state"], sc["c1"])


def test_restart_after_rollback_resumes_rolled_back_head(tmp_path: Path) -> None:
    sc = scenario(tmp_path)
    rb = sc["agg"].rollback(sc["t0"], sc["t1"], [sc["bad"]], [], 10**6)
    again = fresh(tmp_path)
    assert again.applied[1]["body"]["out_state"] == rb.state_key
    with pytest.raises(SequenceError):
        again.rollback(sc["t0"], sc["t1"], [sc["bad"]], [], 10**6)


def test_journal_tamper_detected(tmp_path: Path) -> None:
    sc = scenario(tmp_path)
    path = sc["agg"].journal.path
    lines = path.read_bytes().splitlines()
    e = json.loads(lines[0])
    e["w"] = 7
    path.write_bytes(b"\n".join([json.dumps(e).encode(), *lines[1:]]) + b"\n")
    with pytest.raises(JournalError):
        fresh(tmp_path)
    path.write_bytes(b"\n".join(lines[1:]) + b"\n")
    with pytest.raises(JournalError):
        fresh(tmp_path)


def _hier_round(agg: Aggregator, w: int, prev: str, plan: dict[str, list[list[int]]]):
    chains, raw = {}, {}
    for region, syncs in plan.items():
        cur, tapes, raw[region] = agg.regional_start(prev), [], []
        for k, members in enumerate(syncs, 1):
            cs = [commit(agg.store, w, m, raw_delta(1000 * w + m)) for m in members]
            t = agg.regional_merge(w, region, k, cur, cs)
            tapes.append(t)
            cur = t["body"]["out_state"]
            raw[region].append(_decoded(agg, cs))
        chains[region] = tapes
    return agg.global_from_regions(w, prev, chains, K=len(next(iter(plan.values())))), raw


PLAN_BAD = {"eu": [[0, 9], [1, 9]], "us": [[2, 3], [3, 4]]}
PLAN_W1 = {"eu": [[0, 1], [1, 5]], "us": [[2, 4], [3, 6]]}


def _drop(plan: dict[str, list[list[int]]], bad: int) -> dict[str, list[list[int]]]:
    return {r: [[m for m in s if m != bad] for s in syncs] for r, syncs in plan.items()}


def test_hierarchical_rollback_matches_from_scratch(tmp_path: Path) -> None:
    agg, s0 = setup(tmp_path / "a")
    t0, _ = _hier_round(agg, 0, s0, PLAN_BAD)
    t1, _ = _hier_round(agg, 1, t0["body"]["out_state"], PLAN_W1)
    rb = agg.rollback(t0, t1, [hotkey(9)], [], 10**6)
    ref, r0s = setup(tmp_path / "ref")
    r0, raw0 = _hier_round(ref, 0, r0s, _drop(PLAN_BAD, 9))
    r1, _ = _hier_round(ref, 1, r0["body"]["out_state"], PLAN_W1)
    for got, want in zip(rb.tapes, (r0, r1), strict=True):
        assert got["body"]["out_state"] == want["body"]["out_state"]
    assert rb.tapes[1]["body"]["theta_hash"] != t1["body"]["theta_hash"]
    assert rb.tapes[0]["body"]["theta_hash"] == th(_ref_hier(theta0(), raw0, agg.params)[0])
    assert hotkey(9) not in agg.contributors(rb.tapes[0])
    for t in rb.tapes:
        replay_tape(agg.store, t, COORD.ss58)


def test_replay_detects_resigned_tampered_output(tmp_path: Path) -> None:
    sc = scenario(tmp_path)
    body = json.loads(json.dumps(sc["t1"]["body"]))
    body["theta_hash"] = sc["t0"]["body"]["theta_hash"]
    with pytest.raises(ReplayMismatch, match="theta_hash"):
        replay_tape(sc["agg"].store, sign_tape(COORD, body), COORD.ss58)
    body = json.loads(json.dumps(sc["t1"]["body"]))
    body["out_state"] = sc["t0"]["body"]["out_state"]
    with pytest.raises(ReplayMismatch, match="out_state"):
        replay_tape(sc["agg"].store, sign_tape(COORD, body), COORD.ss58)


def test_global_from_regions_replays_regional_tapes(tmp_path: Path) -> None:
    agg, s0 = setup(tmp_path)
    chains = {}
    for region, members in (("eu", [0, 1]), ("us", [2, 3])):
        cs = [commit(agg.store, 0, m, raw_delta(m)) for m in members]
        chains[region] = [agg.regional_merge(0, region, 1, agg.regional_start(s0), cs)]
    forged_state = load_state(agg.store, chains["us"][0]["body"]["out_state"])
    forged_state = type(forged_state)(
        {n: a * np.float32(3.0) for n, a in forged_state.theta.items()},
        forged_state.u,
        forged_state.center,
    )
    body = json.loads(json.dumps(chains["us"][0]["body"]))
    body["out_state"] = agg.put_state(forged_state)
    body.update(forged_state.hashes())
    forged = dict(chains, us=[sign_tape(COORD, body)])
    with pytest.raises(ReplayMismatch):
        agg.global_from_regions(0, s0, forged, K=1)
    assert 0 not in agg.applied


def test_checkpoint_per_file_sha_pinned(tmp_path: Path) -> None:
    sc = scenario(tmp_path / "s")
    s = load_state(sc["agg"].store, sc["t1"]["body"]["out_state"])
    d = tmp_path / "ckpt"
    write_checkpoint(d, COORD, s.theta, [sc["t0"], sc["t1"]], license="Apache-2.0", dataset=DATASET)
    finalize_checkpoint(d, COORD, ())
    assert verify_checkpoint(d) == []
    lin = d / "lineage.json"
    lin.write_bytes(lin.read_bytes() + b" ")
    assert json.loads(lin.read_bytes())
    assert verify_checkpoint(d) == ["lineage.json: sha256 mismatch"]


class _LyingStore(LocalFSStore):
    def get(self, sha: str) -> bytes:
        return b"not the committed bytes"


def test_get_object_rechecks_hash_on_any_store(tmp_path: Path) -> None:
    store = _LyingStore(tmp_path)
    key = store.put(b"payload")
    with pytest.raises(HashMismatch):
        get_object(store, key)
    agg = Aggregator(store, example_manifest().run_id(), params(), COORD, tmp_path / "st")
    with pytest.raises(HashMismatch):
        agg.regional_start(key)


def test_sparseloco_outer_matches_reference(tmp_path: Path) -> None:
    p = params(opt="sparseloco")
    agg, s0 = setup(tmp_path, p)
    eta = np.float32(f32val(p.lr))
    tau, pc = f32val(p.cclip_tau), f32val(p.preclip_norm)
    theta = theta0()
    center = {n: np.zeros(s, np.float32) for n, s in SHAPES.items()}
    prev, first = s0, ""
    for w in range(2):
        cs = [commit(agg.store, w, i, raw_delta(50 * w + i, scale=0.5)) for i in range(3)]
        t = agg.apply_round(w, prev, cs)
        prev = t["body"]["out_state"]
        ds = _decoded(agg, cs)
        wt = np.float32(1.0) / np.float32(3)
        acc = {n: np.zeros_like(center[n]) for n in center}
        for k in sorted(ds, key=str.encode):
            d = ds[k]
            r = math.sqrt(_sq(d))
            if r > pc:
                d = {n: d[n] * np.float32(pc / r) for n in d}
            diff = {n: d[n] - center[n] for n in d}
            r2 = math.sqrt(_sq(diff))
            sc = np.float32(1.0 if r2 <= tau else tau / r2)
            acc = {n: acc[n] + wt * (diff[n] * sc) for n in d}
        g = {n: center[n] + acc[n] for n in center}
        theta = {n: theta[n] - eta * g[n] for n in theta}
        center = g
        st = load_state(agg.store, prev)
        assert th(st.theta) == th(theta) and th(st.center) == th(center)
        assert all(not st.u[n].any() for n in st.u)
        first = first or th(st.theta)
    nesterov, s0n = setup(tmp_path / "n")
    cs = [commit(nesterov.store, 0, i, raw_delta(i, scale=0.5)) for i in range(3)]
    assert nesterov.apply_round(0, s0n, cs)["body"]["theta_hash"] != first


def test_journal_written_with_hash_chain(tmp_path: Path) -> None:
    sc = scenario(tmp_path)
    lines = [json.loads(x) for x in sc["agg"].journal.path.read_bytes().splitlines()]
    assert [e["event"] for e in lines] == ["apply", "apply"]
    assert lines[1]["prev"] == lines[0]["hash"]
    assert (
        lines[0]["hash"]
        == hashlib.sha256(
            json.dumps(
                {k: v for k, v in lines[0].items() if k != "hash"},
                sort_keys=True,
                separators=(",", ":"),
            ).encode()
        ).hexdigest()
    )
