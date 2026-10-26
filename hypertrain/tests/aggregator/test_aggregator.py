from __future__ import annotations

import hashlib
import json
import math
import shutil
import subprocess
import sys
from pathlib import Path

import numpy as np
import pytest
from agg_helpers import (
    COORD,
    SHAPES,
    commit,
    fresh,
    hotkey,
    p32,
    params,
    raw_delta,
    scenario,
    setup,
    theta0,
)

from hypertrain.aggregator.checkpoint import (
    CheckpointError,
    finalize_checkpoint,
    verify_checkpoint,
    write_checkpoint,
)
from hypertrain.aggregator.core import (
    FinalityError,
    HashMismatch,
    MalformedDelta,
    OuterState,
    ReplayMismatch,
    TapeError,
    aggregate,
    load_delta,
    load_state,
    load_tape,
    replay_tape,
    sign_tape,
    tape_bytes,
    th,
    verify_tape,
)
from hypertrain.data.store import LocalFSStore
from hypertrain.protocol.envelope import verify_envelope
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages import f32val

DATASET = {
    "name": "HuggingFaceFW/fineweb-edu",
    "license": "ODC-By 1.0",
    "attribution": "FineWeb-Edu by Hugging Face, ODC-By 1.0",
    "merkle_root": "8bd23b4f5772897bf275ce087ef0bc1e84b3ad4b7bce0cb450a73e2a17491d9f",
}


def _sq(d: dict[str, np.ndarray]) -> float:
    return math.fsum(float(x) ** 2 for n in sorted(d) for x in d[n].astype(np.float64).ravel())


def _ref_round(theta, u, center, deltas, p):
    """Independent spec transcription of ultrabrain 3c (preclip, 1-iter CClip, Nesterov)."""
    tau, pc = f32val(p.cclip_tau), f32val(p.preclip_norm)
    eta, mu = np.float32(f32val(p.lr)), np.float32(f32val(p.momentum))
    w = np.float32(1.0) / np.float32(len(deltas))
    v = {n: center[n].copy() for n in center}
    for _ in range(p.cclip_iters):
        acc = {n: np.zeros_like(v[n]) for n in v}
        for k in sorted(deltas, key=lambda s: s.encode()):
            d = deltas[k]
            r = math.sqrt(_sq(d))
            if r > pc:
                d = {n: d[n] * np.float32(pc / r) for n in d}
            diff = {n: d[n] - v[n] for n in v}
            r2 = math.sqrt(_sq(diff))
            s = np.float32(1.0 if r2 <= tau else tau / r2)
            acc = {n: acc[n] + w * (diff[n] * s) for n in v}
        v = {n: v[n] + acc[n] for n in v}
    u2 = {n: mu * u[n] + v[n] for n in v}
    t2 = {n: theta[n] - eta * (v[n] + mu * u2[n]) for n in v}
    return t2, u2, v


def _decoded(agg, commits):
    like = theta0()
    return {c.hotkey: load_delta(agg.store, c.delta_hash, like) for c in commits}


def test_outer_step_matches_independent_spec(tmp_path: Path) -> None:
    sc = scenario(tmp_path)
    agg = sc["agg"]
    z = {n: np.zeros(s, np.float32) for n, s in SHAPES.items()}
    t1, u1, c1 = _ref_round(theta0(), z, z, _decoded(agg, sc["c0"]), agg.params)
    s1 = load_state(agg.store, sc["t0"]["body"]["out_state"])
    assert th(s1.theta) == th(t1) and th(s1.u) == th(u1) and th(s1.center) == th(c1)
    assert sc["t0"]["body"]["theta_hash"] == th(t1)


def test_rollback_equals_from_scratch_without_attacker(tmp_path: Path) -> None:
    sc = scenario(tmp_path / "a")
    agg, bad = sc["agg"], sc["bad"]
    rb = agg.rollback(sc["t0"], sc["t1"], [bad], [hashlib.sha256(b"verdict").hexdigest()], 10**6)
    ref_agg, s0 = setup(tmp_path / "ref")
    honest0 = [commit(ref_agg.store, 0, i, raw_delta(i)) for i in range(3)]
    r0 = ref_agg.apply_round(0, s0, honest0)
    honest1 = [commit(ref_agg.store, 1, i, raw_delta(10 + i)) for i in range(3)]
    r1 = ref_agg.apply_round(1, r0["body"]["out_state"], honest1)
    new = rb.tapes[1]["body"]
    assert new["out_state"] == r1["body"]["out_state"]
    assert new["theta_hash"] == r1["body"]["theta_hash"] != sc["t1"]["body"]["theta_hash"]
    z = {n: np.zeros(s, np.float32) for n, s in SHAPES.items()}
    a = _ref_round(theta0(), z, z, _decoded(agg, sc["c0"][:3]), agg.params)
    b = _ref_round(*a, _decoded(agg, sc["c1"]), agg.params)
    assert new["theta_hash"] == th(b[0]) and new["outer_state_hash"] == th(b[1])
    env = rb.envelope
    assert verify_envelope(env) and env["body"]["excluded"] == [bad]
    assert env["body"]["new_theta_hash_w2"] == new["theta_hash"]
    assert rb.tapes[0]["body"]["excluded"] == [bad]


def test_rollback_deterministic_and_from_stored_artifacts_only(tmp_path: Path) -> None:
    sc = scenario(tmp_path / "s")
    keys = [hashlib.sha256(tape_bytes(sc[t])).hexdigest() for t in ("t0", "t1")]
    bad = sc["bad"]
    del sc  # no miner processes, commits or in-memory deltas from here on
    outs = []
    for run in ("r1", "r2"):
        shutil.copytree(tmp_path / "s", tmp_path / run)
        agg = fresh(tmp_path / run)
        tapes = [load_tape(agg.store, k) for k in keys]
        rb = agg.rollback(tapes[0], tapes[1], [bad], [], 10**6)
        outs.append(
            (
                agg.store.get(rb.state_key),
                [canonicalize(t["body"], allow_float=False) for t in rb.tapes],
                canonicalize(rb.envelope["body"], allow_float=False),
            )
        )
    assert outs[0] == outs[1]
    replayed = replay_tape(LocalFSStore(tmp_path / "r1"), rb.tapes[1], COORD.ss58)
    assert replayed.to_bytes() == outs[1][0]


def test_rollback_refused_after_finality(tmp_path: Path) -> None:
    sc = scenario(tmp_path)
    sc["agg"].finalize_round(0)
    with pytest.raises(FinalityError):
        sc["agg"].rollback(sc["t0"], sc["t1"], [sc["bad"]], [], 10**6)


def test_cclip_bounds_100x_delta(tmp_path: Path) -> None:
    p = params(preclip_norm=p32(1e30))  # disable preclip: isolate CClip
    agg, _ = setup(tmp_path, p)
    like = theta0()
    honest = {hotkey(i): raw_delta(i) for i in range(3)}
    base = math.sqrt(_sq(honest[hotkey(0)]))
    attack = {n: a * np.float32(100.0) for n, a in raw_delta(99).items()}
    center = {n: np.zeros_like(a) for n, a in like.items()}
    with_att = aggregate({**honest, hotkey(9): attack}, center, p).g
    with_copy = aggregate({**honest, hotkey(9): raw_delta(98)}, center, p).g
    tau = f32val(p.cclip_tau)
    infl = math.sqrt(_sq({n: with_att[n] - with_copy[n] for n in like}))
    assert math.sqrt(_sq(attack)) > 90 * base
    assert infl <= 2 * tau / 4 * (1 + 1e-6)
    mean = {n: (sum(honest[k][n] for k in honest) + attack[n]) / 4 for n in like}
    mean_c = {n: (sum(honest[k][n] for k in honest) + raw_delta(98)[n]) / 4 for n in like}
    assert math.sqrt(_sq({n: mean[n] - mean_c[n] for n in like})) > 20 * infl  # control can fail


def test_copy_suspicion_raises_q_never_slashes(tmp_path: Path) -> None:
    agg, s0 = setup(tmp_path)
    cs = [commit(agg.store, 0, i, raw_delta(i)) for i in range(3)]
    cs.append(commit(agg.store, 0, 5, raw_delta(0)))
    tape = agg.apply_round(0, s0, cs)
    screens = tape["body"]["screens"]
    assert screens["raise_q"] == sorted([hotkey(0), hotkey(5)], key=str.encode)
    assert len(tape["body"]["inputs"]) == 4  # flagged deltas still applied (soft screen)


def test_event_tape_replay_reproduces_theta(tmp_path: Path) -> None:
    sc = scenario(tmp_path)
    store = sc["agg"].store
    for t in ("t0", "t1"):
        s = replay_tape(store, sc[t], COORD.ss58)
        assert th(s.theta) == sc[t]["body"]["theta_hash"]
        assert store.get(sc[t]["body"]["out_state"]) == s.to_bytes()
    forged = json.loads(json.dumps(sc["t1"]))
    forged["body"]["inputs"] = forged["body"]["inputs"][:2]
    assert not verify_tape(forged)
    with pytest.raises(TapeError):
        replay_tape(store, forged)
    resigned = sign_tape(COORD, forged["body"])
    with pytest.raises(ReplayMismatch):
        replay_tape(store, resigned, COORD.ss58)


def _ref_hier(theta, deltas_by_region_k, p):
    """Independent hierarchical reference: regional mean chains, then global CClip+Nesterov."""
    pc = f32val(p.preclip_norm)
    finals = {}
    for region, syncs in deltas_by_region_k.items():
        t = {n: theta[n].copy() for n in theta}
        for ds in syncs:
            w = np.float32(1.0) / np.float32(len(ds))
            acc = {n: np.zeros_like(t[n]) for n in t}
            for k in sorted(ds, key=lambda s: s.encode()):
                d = ds[k]
                r = math.sqrt(_sq(d))
                if r > pc:
                    d = {n: d[n] * np.float32(pc / r) for n in d}
                acc = {n: acc[n] + w * d[n] for n in t}
            t = {n: t[n] - acc[n] for n in t}
        finals[f"region:{region}"] = {n: theta[n] - t[n] for n in t}
    z = {n: np.zeros_like(theta[n]) for n in theta}
    return _ref_round(theta, z, z, finals, p)


def test_hierarchical_aggregate_recomputable(tmp_path: Path) -> None:
    agg, s0 = setup(tmp_path)
    chains, raw = {}, {}
    for r_i, region in enumerate(("eu", "us")):
        prev, tapes, raw[region] = agg.regional_start(s0), [], []
        for k in (1, 2):
            cs = [
                commit(agg.store, 0, 10 * r_i + 3 * k + j, raw_delta(100 * r_i + 10 * k + j))
                for j in range(2)
            ]
            t = agg.regional_merge(0, region, k, prev, cs)
            tapes.append(t)
            prev = t["body"]["out_state"]
            raw[region].append(_decoded(agg, cs))
        chains[region] = tapes
    broken = dict(chains)
    broken["us"] = chains["us"][:1]
    with pytest.raises(TapeError):
        agg.global_from_regions(0, s0, broken, K=2)
    tape = agg.global_from_regions(0, s0, chains, K=2)
    ref = _ref_hier(theta0(), raw, agg.params)
    assert tape["body"]["theta_hash"] == th(ref[0])
    assert replay_tape(agg.store, tape, COORD.ss58).to_bytes() == agg.store.get(
        tape["body"]["out_state"]
    )


def test_corrupted_delta_hash_mismatch_blocks_apply(tmp_path: Path) -> None:
    agg, s0 = setup(tmp_path)
    cs = [commit(agg.store, 0, i, raw_delta(i)) for i in range(3)]
    p = tmp_path / cs[1].delta_hash[:2] / cs[1].delta_hash
    data = bytearray(p.read_bytes())
    data[-1] ^= 1
    p.write_bytes(bytes(data))
    with pytest.raises(HashMismatch):
        agg.apply_round(0, s0, cs)
    assert 0 not in agg.applied


def test_malformed_delta_rejected(tmp_path: Path) -> None:
    agg, s0 = setup(tmp_path)
    junk = agg.store.put(b"not a delta")
    c = commit(agg.store, 0, 0, raw_delta(0)).model_copy(update={"delta_hash": junk})
    with pytest.raises(MalformedDelta):
        agg.apply_round(0, s0, [c])
    assert 0 not in agg.applied


def _checkpoint(tmp_path: Path) -> tuple[Path, dict]:
    sc = scenario(tmp_path / "store")
    t1 = sc["t1"]
    s = load_state(sc["agg"].store, t1["body"]["out_state"])
    d = tmp_path / "ckpt"
    write_checkpoint(d, COORD, s.theta, [sc["t0"], t1], license="Apache-2.0", dataset=DATASET)
    return d, sc


def _cli(d: Path) -> subprocess.CompletedProcess[str]:
    exe = Path(sys.executable).parent / "hypertrain"
    return subprocess.run(
        [str(exe), "verify-checkpoint", str(d)], capture_output=True, text=True, timeout=120
    )


def test_finalize_refuses_while_audit_open(tmp_path: Path) -> None:
    d, _ = _checkpoint(tmp_path)
    assert _cli(d).returncode == 1  # provisional is not verifiable as final
    with pytest.raises(CheckpointError):
        finalize_checkpoint(d, COORD, open_audit_rounds={1, 7})
    assert verify_checkpoint(d) != []
    finalize_checkpoint(d, COORD, open_audit_rounds={7})
    assert verify_checkpoint(d, signer=COORD.ss58) == []


def test_verify_checkpoint_cli_and_flipped_byte(tmp_path: Path) -> None:
    d, sc = _checkpoint(tmp_path)
    finalize_checkpoint(d, COORD, open_audit_rounds=())
    ok = _cli(d)
    assert ok.returncode == 0, ok.stderr
    out = json.loads(ok.stdout)
    assert out["result"] == "VERIFIED" and sc["bad"] in out["included"]
    for name in ("model.safetensors", "lineage.json", "MANIFEST.json"):
        bad = tmp_path / f"flip-{name}"
        shutil.copytree(d, bad)
        b = bytearray((bad / name).read_bytes())
        b[len(b) // 2] ^= 1
        (bad / name).write_bytes(bytes(b))
        r = _cli(bad)
        assert r.returncode != 0 and "FAIL" in r.stderr, name
    extra = tmp_path / "extra"
    shutil.copytree(d, extra)
    (extra / "evil.bin").write_bytes(b"x")
    assert _cli(extra).returncode != 0


def test_state_roundtrip_bytes_are_canonical() -> None:
    s = OuterState.init(theta0())
    assert OuterState.from_bytes(s.to_bytes()).to_bytes() == s.to_bytes()
