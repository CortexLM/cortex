"""Todo-17 acceptance scenarios. Every actor (challenge app, aggregator, auditor, each miner) is
its own OS process; this module is the owner/admin + drand relay and asserts on HTTP state."""

from __future__ import annotations

import hashlib
import json
import math
from dataclasses import replace
from pathlib import Path
from typing import Any

import numpy as np
import pytest
from common import COORD, DELTA, manifest
from harness import NO_REPLAY, ActorError, key

from hypertrain.aggregator.core import COPY_OVERLAP, COPY_TOPK_FRAC, load_delta, load_state
from hypertrain.data.store import LocalFSStore
from hypertrain.ledger import FULL_SHARE_MASS
from hypertrain.protocol.messages import QUICKNET_GENESIS, Commit
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.loop import Assignment, train_round
from hypertrain.trainer.model import init_params

H3 = [key(0x41), key(0x42), key(0x43)]
PEERS = [key(0x41), key(0x42), key(0x43), key(0x44)]
BAD = key(0x50)
PROBATION = {"probation": True}
ADMIN_H = {"authorization": "Bearer e2e-admin-token"}


def epoch_at(epoch: int) -> int:
    return QUICKNET_GENESIS + epoch * 86_400 + 1


def final_epoch(w: Any, rnd: int) -> int:
    t = w.m.beacon.genesis_time + (rnd - 1) * w.m.beacon.period
    return (t - QUICKNET_GENESIS) // 86_400


def verdict(w: Any, wr: int, hk: str) -> dict[str, Any]:
    m = next(m for m in w.view(wr)["miners"] if m["hotkey"] == hk)
    out: dict[str, Any] = m["verdict"]["body"]
    return out


def binom_interval(n: int, p: float, z: float = 3.29) -> tuple[int, int]:
    """99.9% normal-approximation band (fixed seeds make the draw deterministic)."""
    sd = math.sqrt(n * p * (1 - p))
    return max(0, math.floor(n * p - z * sd)), math.ceil(n * p + z * sd)


def topk_overlap(a: dict[str, np.ndarray], b: dict[str, np.ndarray]) -> float:
    fa = np.concatenate([a[n].ravel() for n in sorted(a)])
    fb = np.concatenate([b[n].ravel() for n in sorted(b)])
    k = max(1, int(np.ceil(COPY_TOPK_FRAC * fa.size)))
    ta = set(np.lexsort((np.arange(fa.size), -np.abs(fa)))[:k].tolist())
    tb = set(np.lexsort((np.arange(fb.size), -np.abs(fb)))[:k].tolist())
    return len(ta & tb) / k


def bump_at(step: int) -> Any:
    import torch

    def hook(t: int, theta: Any) -> None:
        if t == step:
            with torch.no_grad():
                theta[sorted(theta)[0]].view(-1)[0] += DELTA

    return hook


def test_01_honest_paid_after_vesting(world: Any, record: dict[str, Any]) -> None:
    w = world(manifest(policy="carry", q=0.5, E=3), {k: {"probation": False} for k in H3})
    miners = [w.miner(k) for k in H3]
    assert w.run_all(miners, 0) == ["UPLOADED"] * 3
    w.aggregate(0)
    record["audits"] = w.audit(0, miners)
    r = w.finalize(0)
    assert r["status"] == 200, r
    verdicts = r["json"]["verdicts"]
    assert set(verdicts.values()) <= {"MATCH", "UNSAMPLED"} and len(verdicts) == 3
    assert "MATCH" in verdicts.values()
    fe = final_epoch(w, w.body(0)["d_final"])
    E = w.m.verify.E_vest_rounds
    before = w.weights(fe + E - 1, epoch_at(fe + E - 1))
    assert before["weights"] == {}  # still vesting one epoch before E: nothing paid
    assert before["metadata"]["units_burned_this_epoch"] == FULL_SHARE_MASS
    after = w.weights(fe + E, epoch_at(fe + E))
    share = FULL_SHARE_MASS // 3
    assert after["weights"] == {k.ss58: float(share) for k in H3}
    assert after["metadata"]["units_burned_this_epoch"] == FULL_SHARE_MASS - 3 * share
    assert w.view(0)["forfeits"] == []
    states = {"at_finalize": w.view(0)["state"]}
    per_epoch = 86_400 // w.m.beacon.period
    d_final = w.body(0)["d_final"]
    for k in range(1, E + 1):  # store round_state over drand time: FINAL -> VESTING -> RELEASED
        w.jump(d_final + k * per_epoch)
        states[f"final+{k}"] = w.view(0)["state"]
    expected = {"at_finalize": "FINAL", **{f"final+{k}": "VESTING" for k in range(1, E)}}
    assert states == {**expected, f"final+{E}": "RELEASED"}, states
    record.update(verdicts=verdicts, before=before, after=after, round_states=states)


def test_02_averaged_delta_fault(world: Any, record: dict[str, Any]) -> None:
    w = world(manifest(policy="carry"), {k: PROBATION for k in [*PEERS, BAD]})
    honest = [w.miner(k) for k in PEERS]
    assert w.run_all(honest, 0) == ["UPLOADED"] * 4
    cheat = w.miner(BAD, "averager")
    assert cheat.call("run_round", w=0) == "UPLOADED"
    tape = w.aggregate(0)
    objects = LocalFSStore(w.srv.state / "objects")
    like = {n: np.asarray(a) for n, a in load_state(objects, w.state_key[0]).theta.items()}
    commits = [Commit.model_validate(c) for c in w.agg.call("commit_list", w=0)]
    ds = {c.hotkey: load_delta(objects, c.delta_hash, like) for c in commits}
    overlaps = {k.ss58: topk_overlap(ds[BAD.ss58], ds[k.ss58]) for k in PEERS}
    flags = tape["body"]["screens"]["copy_flags"]
    flagged = {f[s] for f in flags for s in ("a", "b")}
    record.update(overlaps=overlaps, copy_flags=flags)
    assert max(overlaps.values()) <= COPY_OVERLAP and BAD.ss58 not in flagged
    record["audits"] = w.audit(0, [*honest, cheat])
    st = w.status(0)
    assert st == {**{k.ss58: "MATCH" for k in PEERS}, BAD.ss58: "MISMATCH"}, st
    assert verdict(w, 0, BAD.ss58)["first_bad_leaf"] == 1
    w.aggregate(1)
    rb = w.agg.call("rollback", w=0, bad=[BAD.ss58])
    assert w.view(0)["rollback"]["body"]["excluded"] == [BAD.ss58]
    r = w.finalize(0)
    assert r["status"] == 200, r
    assert r["json"]["verdicts"][BAD.ss58] == "FAULT"
    forfeit = next(f["body"] for f in w.view(0)["forfeits"] if f["body"]["hotkey"] == BAD.ss58)
    assert forfeit["cause"] == "MISMATCH" and forfeit["blacklist"] is True
    assert BAD.ss58 not in {i["id"] for i in rb["tapes"][0]["body"]["inputs"]}
    assert BAD.ss58 in {i["id"] for i in tape["body"]["inputs"]}
    record.update(status=st, forfeit=forfeit, finalize=r["json"])


def test_03a_assignment_violation_without_replay(world: Any, record: dict[str, Any]) -> None:
    w = world(manifest(policy="carry", q=1.0, E=1), {k: PROBATION for k in [H3[0], BAD]})
    honest, cheat = w.miner(H3[0]), w.miner(BAD, "shifted")
    assert w.run_all([honest, cheat], 0) == ["UPLOADED"] * 2
    w.aggregate(0)
    record["audits"] = w.audit(0, [honest, cheat])
    st = w.status(0)
    assert st == {H3[0].ss58: "MATCH", BAD.ss58: "ASSIGNMENT_VIOLATION"}, st
    v = verdict(w, 0, BAD.ss58)
    assert v["first_bad_leaf"] == 1 and v["recomputed_leaves_root"] == "0" * 64  # no replay
    record.update(status=st, verdict=v)


def test_03b_wrong_data_inside_assignment(world: Any, record: dict[str, Any]) -> None:
    w = world(manifest(policy="carry", q=1.0, E=1), {k: PROBATION for k in [H3[0], BAD]})
    honest, cheat = w.miner(H3[0]), w.miner(BAD, poison=True)
    assert w.run_all([honest, cheat], 0) == ["UPLOADED"] * 2
    w.aggregate(0)
    record["audits"] = w.audit(0, [honest, cheat])
    st = w.status(0)
    assert st == {H3[0].ss58: "MATCH", BAD.ss58: "MISMATCH"}, st
    v = verdict(w, 0, BAD.ss58)
    assert v["first_bad_leaf"] == 1 and v["recomputed_leaves_root"] != "0" * 64
    record.update(status=st, verdict=v, no_replay=NO_REPLAY)


def test_04_final_step_fault(world: Any, record: dict[str, Any]) -> None:
    from hypertrain.auditor.replay import select_segments
    from hypertrain.trainer.loop import replay
    from hypertrain.trainer.optim import init_state

    w = world(manifest(policy="carry", q=1.0, E=1), {k: PROBATION for k in [H3[0], BAD]})
    honest, cheat = w.miner(H3[0]), w.miner(BAD, "laststep")
    assert w.run_all([honest, cheat], 0) == ["UPLOADED"] * 2
    w.aggregate(0)
    record["audits"] = w.audit(0, [honest, cheat])
    st = w.status(0)
    assert st == {H3[0].ss58: "MATCH", BAD.ss58: "MISMATCH"}, st
    assert verdict(w, 0, BAD.ss58)["first_bad_leaf"] == w.cfg.inner.n_leaves - 1
    m30 = manifest(policy="carry", H=30, J=1, k_segments=1, q_top=0)
    cfg = TrainConfig.from_manifest(m30.body())
    theta = init_params(cfg.model)
    carry = init_state(replace(cfg.inner, state_policy="reset"), theta)
    n = 30 * cfg.inner.micro_batch * cfg.inner.grad_accum
    full = 0
    for i in range(30):
        a = Assignment(m30.run_id(), i, tuple(range(i * n, i * n + n)), 0)
        bad = train_round(cfg, theta, a, w.get, carry=carry, after_step=bump_at(30))
        rep = replay(cfg, theta, a, w.get, bad.leaf_digests, carry=carry)
        full += rep.result == "MISMATCH" and rep.first_bad_leaf == 30
    norms = [1.0] * 30
    sigs = [hashlib.sha256(b"beacon%d" % i).hexdigest() for i in range(400)]
    final = sum(
        (29, 30) in select_segments(m30.run_id(), i, s, BAD.ss58, norms, 1, 0)
        for i, s in enumerate(sigs[:30])
    )
    ablation = sum(
        (29, 30) in select_segments(m30.run_id(), i, s, BAD.ss58, norms, 1, 0, final_always=False)
        for i, s in enumerate(sigs)
    )
    lo, hi = binom_interval(400, 1 / 30)
    record.update(status=st, full_mode=full, final_always=final, ablation=ablation, band=[lo, hi])
    assert full == 30 and final == 30
    assert lo <= ablation <= hi  # without final-always a last-step cheat is caught at ~k/u


def test_05_withheld_and_no_upload(world: Any, record: dict[str, Any]) -> None:
    lazy = key(0x51)
    w = world(manifest(policy="carry", q=1.0, E=1), {k: PROBATION for k in [H3[0], BAD, lazy]})
    honest, silent, noup = w.miner(H3[0]), w.miner(BAD), w.miner(lazy, "noupload")
    assert w.run_all([honest, silent], 0) == ["UPLOADED"] * 2
    noup.call("run_round", w=0)
    w.aggregate(0)
    record["pre_audit"] = w.status(0)
    assert record["pre_audit"][lazy.ss58] == "COMMITTED"
    record["audits"] = w.audit(0, [honest, noup], past_serve_deadline=True)  # silent never serves
    st = w.status(0)
    record["status"] = st
    assert st[H3[0].ss58] == "MATCH" and st[BAD.ss58] == "WITHHELD", st
    r = w.finalize(0)
    assert r["status"] == 200, r
    v = r["json"]["verdicts"]
    assert v == {H3[0].ss58: "MATCH", BAD.ss58: "FAULT", lazy.ss58: "NO_UPLOAD"}, v
    causes = {f["body"]["hotkey"]: f["body"]["cause"] for f in w.view(0)["forfeits"]}
    assert causes == {BAD.ss58: "WITHHELD"}, causes
    record.update(verdicts=v, forfeits=causes)


def test_06_late_commit_and_adaptive_attacker(world: Any, record: dict[str, Any]) -> None:
    from hypertrain.challenge.store import audit_selected
    from hypertrain.protocol.messages import f32hex

    w = world(manifest(policy="carry"), {k: PROBATION for k in [H3[0], BAD]})
    honest = w.miner(H3[0])
    assert honest.call("run_round", w=0) == "UPLOADED"
    late = w.miner(BAD, "late")
    assert late.call("run_round", w=0) == "COMMIT_REJECTED"
    st = w.status(0)
    assert st == {H3[0].ss58: "UPLOADED", BAD.ss58: "EXCLUDED"}, st
    q, run_id = f32hex(0.1), w.run_id
    sig = [hashlib.sha256(b"drand%d" % r).digest() for r in range(401)]

    def sel(r: int, s: bytes) -> bool:
        return audit_selected(run_id, r, s, BAD.ss58, q)

    # Adaptive attacker cheats only when the beacon it can see at commit time says
    # "unselected". With the protocol (selection keyed by the post-commit beacon) it is still
    # caught at rate ~q; if selection used the seed it saw, it would be caught 0 times.
    cheats = [r for r in range(400) if not sel(r, sig[r])]
    caught = sum(sel(r, sig[r + 1]) for r in cheats)
    predictable = sum(sel(r, sig[r]) for r in cheats)
    lo, hi = binom_interval(len(cheats), 0.1)
    record.update(
        status=st, cheats=len(cheats), caught=caught, predictable_caught=predictable, band=[lo, hi]
    )
    assert lo <= caught <= hi and caught > 0
    assert predictable == 0  # contrast by construction; the live check is the ordering below
    b = w.body(0)
    assert b["d_audit"] > b["d_commit"]  # selection beacon is unknown while commits are open


def test_07_rollback_bitwise_and_journal_anchor(world: Any, record: dict[str, Any]) -> None:
    w = world(manifest(policy="carry", q=1.0, E=1), {k: PROBATION for k in [*H3[:2], BAD]})
    miners = [w.miner(k) for k in H3[:2]]
    cheat = w.miner(BAD, "laststep")
    assert w.run_all([*miners, cheat], 0) == ["UPLOADED"] * 3
    tape0 = w.aggregate(0)
    w.audit(0, [*miners, cheat])
    assert w.status(0)[BAD.ss58] == "MISMATCH"
    commits = w.agg.call("commit_list", w=0)
    record["killed"] = {p.name: p.kill() for p in [*miners, cheat]}  # rollback needs no miner
    assert all(code != 0 for code in record["killed"].values())
    w.aggregate(1)
    rb = w.agg.call("rollback", w=0, bad=[BAD.ss58])
    new_w1 = rb["tapes"][1]["body"]
    ref = w.spawn_aggregator("aggregator-reference", w.tmp / "ref")
    clean = ref.call(
        "replay_rounds", start_key=w.state_key[0], commits=commits, exclude=[BAD.ss58], rollback=[]
    )
    keys = ("theta_hash", "outer_state_hash", "center_hash", "out_state")
    assert {k: new_w1[k] for k in keys} == {k: clean["t1"]["body"][k] for k in keys}
    again = w.spawn_aggregator("aggregator-repeat", w.tmp / "again")
    rep = again.call(
        "replay_rounds", start_key=w.state_key[0], commits=commits, exclude=[], rollback=[BAD.ss58]
    )
    assert rep["t0"]["body"]["theta_hash"] == tape0["body"]["theta_hash"]
    assert rep["rollback_state_key"] == rb["state_key"] == clean["t1"]["body"]["out_state"]
    assert rep["t1"]["body"]["theta_hash"] != new_w1["theta_hash"]  # BAD's delta mattered
    with pytest.raises(ActorError, match="SequenceError"):
        w.agg.call("rollback", w=0, bad=[BAD.ss58], post=False)
    r = w.finalize(0)
    assert r["status"] == 200 and r["json"]["verdicts"][BAD.ss58] == "FAULT", r
    w.agg.call("finalize_round", w=0)
    with pytest.raises(ActorError, match="FinalityError"):
        w.agg.call("rollback", w=0, bad=[H3[0].ss58], post=False)
    j = w.agg.call("journal")
    record["aggregator_exit"] = w.agg.close()
    path = Path(j["path"])
    path.write_bytes(b"".join(path.read_bytes().splitlines(keepends=True)[:-1]))
    bare = w.spawn_aggregator("aggregator-restart-bare", w.tmp / "agg")
    assert bare.ready  # a bare hash chain accepts the truncated journal ...
    anchored = w.spawn_aggregator(
        "aggregator-restart-anchored", w.tmp / "agg", anchor=j["anchor"], expect_fail=True
    )
    assert anchored.exit_code != 0 and "anchored entry" in anchored.tail()  # ... the anchor not
    record.update(rollback=rb["envelope"]["body"], reference=clean["t1"]["body"]["theta_hash"])


def test_08_ledger_rebuild_and_byte_flip(world: Any, record: dict[str, Any]) -> None:
    from hypertrain.ledger import ChainBreak
    from hypertrain.ledger.journal import canonical

    w = world(manifest(policy="carry", q=0.5, E=3), {k: {"probation": False} for k in H3[:2]})
    miners = [w.miner(k) for k in H3[:2]]
    assert w.run_all(miners, 0) == ["UPLOADED"] * 2
    w.aggregate(0)
    w.audit(0, miners)
    assert w.finalize(0)["status"] == 200
    fe = final_epoch(w, w.body(0)["d_final"])
    answers = {e: w.weights_raw(e, epoch_at(e)) for e in range(fe + 1, fe + 5)}
    assert any(json.loads(a)["weights"] for a in answers.values())
    led = w.ledger()
    led.verify()
    engine = led._rebuild()
    assert all(canonical(engine.answers[e]["answer"]) == answers[e] for e in answers)
    path = led.journal.path
    data = bytearray(path.read_bytes())
    i = data.index(b'"kind":"finalize"') - 5
    data[i] = ord("0") if data[i] != ord("0") else ord("1")
    path.write_bytes(bytes(data))
    with pytest.raises(ChainBreak):
        type(led)(path.parent, led.params)
    record.update(answers={e: a.decode() for e, a in answers.items()})


def test_09_dispute_bisection_localizes_op(world: Any, record: dict[str, Any]) -> None:
    from hypertrain.auditor.bisect import ceil_log

    w = world(manifest(policy="carry", q=1.0, E=1), {k: PROBATION for k in [H3[0], BAD]})
    honest = w.miner(H3[0])
    step, n_layers, H, N = w.cfg.inner.J + 1, w.cfg.model.n_layers, w.cfg.inner.H, 2
    cheat = w.miner(BAD, bump_step=step)  # trains AND serves the perturbed trajectory
    assert w.run_all([honest, cheat], 0) == ["UPLOADED"] * 2
    w.audit(0, [honest, cheat])
    assert w.status(0) == {H3[0].ss58: "MATCH", BAD.ss58: "MISMATCH"}
    assert cheat.call("dispute", w=0) == "CONTEST"
    dispute_id = cheat.call("last", kind="dispute_id", w=0)["dispute_id"]
    assert w.view(0)["state"] == "DISPUTE"
    posted = cheat.call("bisect", w=0, dispute_id=dispute_id, interval=[0, H], n=N)
    aud = w.auditor.call("bisect", dispute_id=dispute_id, target=BAD.ss58, interval=[0, H], n=N)
    assert (posted["seq"], aud["seq"]) == (0, 1)
    results = {}
    for fault in ([step, n_layers, "update"], [2, 0, "mlp"]):
        res = w.auditor.call(
            "referee",
            dispute_id=dispute_id,
            target=BAD.ss58,
            n=N,
            fault=fault,
            post=fault[2] == "update",
        )
        assert [res["step"], res["layer"], res["op"]] == fault
        assert 1 <= res["rounds"]["step"] <= ceil_log(H, N)
        assert res["resolution"]["loser"] == BAD.ss58
        results[f"{fault[1]}:{fault[2]}"] = res
    assert results[f"{n_layers}:update"]["posted"]["loser"] == BAD.ss58
    with pytest.raises(ActorError, match="RefereeError"):  # two honest trajectories agree
        w.auditor.call(
            "referee", dispute_id=dispute_id, target=BAD.ss58, n=N, fault=None, post=False
        )
    r = w.finalize(0)
    assert r["status"] == 200 and r["json"]["verdicts"][BAD.ss58] == "FAULT", r
    record.update(bisect=[posted, aud], disputes=results)


def test_10_transient_forgiven_then_forfeit(world: Any, record: dict[str, Any]) -> None:
    w = world(manifest(policy="reset", q=1.0, E=1), {H3[0]: PROBATION})
    mn = w.miner(H3[0])
    hk, seen = H3[0].ss58, []
    for wr, mode in enumerate(("honest", "laststep", "laststep")):
        assert mn.call("run_round", w=wr, mode=mode) == "UPLOADED"
        w.audit(wr, [mn])
        seen.append((w.status(wr)[hk], mn.call("dispute", w=wr)))
        w.aggregate(wr)
        r = w.finalize(wr)
        assert r["status"] == 200, r
        seen.append(r["json"]["verdicts"][hk])
    assert seen == [
        ("MATCH", None),
        "MATCH",
        ("MISMATCH", "TRANSIENT"),
        "TRANSIENT",
        ("MISMATCH", "FAULT"),
        "FAULT",
    ], seen
    ents = [e for e in w.ledger().state().entitlements if e.hotkey == hk]
    assert [e.w for e in ents] == [0]  # w1 (forgiven) earns nothing, w2 never granted
    forfeits = {f["body"]["w"]: f["body"] for f in [*w.view(1)["forfeits"], *w.view(2)["forfeits"]]}
    assert set(forfeits) == {2}
    assert forfeits[2]["escrow_burned_units"] == ents[0].amount > 0
    assert ents[0].burned == ents[0].amount
    record.update(sequence=seen, forfeit=forfeits[2])


def test_11_cluster_cascade_and_honeypots(world: Any, record: dict[str, Any]) -> None:
    from hypertrain.auditor.honeypot import Honeypot, commit
    from hypertrain.protocol.messages import f32hex

    mem, hp_h, hp_l, hp_f = key(0x52), key(0x53), key(0x54), key(0x55)
    pots = [
        Honeypot(hp_h.ss58, "honest"),
        Honeypot(hp_l.ss58, "last_step"),
        Honeypot(hp_f.ss58, "fabricate"),
    ]
    salt = "5a" * 32
    c = commit(pots, bytes.fromhex(salt))
    roster = {
        BAD: {"probation": True, "cluster": "c1"},
        mem: {"bond": True, "cluster": "c1"},
        hp_h: PROBATION,
        hp_l: PROBATION,
        hp_f: PROBATION,
    }
    w = world(manifest(policy="carry", q=0.5, E=2), roster, honeypot_commit=c)
    assert w.body(0)["honeypot_commit"] == c
    q0 = {e["hotkey"]: e["q_i"] for e in w.body(0)["roster"]}
    miners = [
        w.miner(BAD, "fabricate"),
        w.miner(mem),
        w.miner(hp_h),
        w.miner(hp_l, "laststep"),
        w.miner(hp_f, "fabricate"),
    ]
    assert w.run_all(miners, 0) == ["UPLOADED"] * 5
    w.audit(0, miners)
    st0 = w.status(0)
    assert st0 == {
        BAD.ss58: "MISMATCH",
        mem.ss58: "MATCH",
        hp_h.ss58: "MATCH",
        hp_l.ss58: "MISMATCH",
        hp_f.ss58: "MISMATCH",
    }, st0
    w.aggregate(0)
    q1 = {e["hotkey"]: e["q_i"] for e in w.body(1)["roster"]}
    one = f32hex(1.0)
    assert q0[mem.ss58] != one and q1[mem.ss58] == one  # cascade: cluster member to q = 1
    assert BAD.ss58 not in q1
    assert miners[1].call("run_round", w=1) == "UPLOADED"
    w.audit(1, [miners[1]])
    job = next(j for j in w.view(1)["jobs"] if j["target"] == mem.ss58)
    assert "cluster" in job["challenge"]["body"]["reasons"] and w.status(1)[mem.ss58] == "MATCH"
    reveal = f"/v1/admin/runs/{w.run_id}/honeypot/reveal"
    members = [{"hotkey": p.hotkey, "mode": p.mode} for p in pots]
    bad = w.c.post(
        reveal, json={"commitment": c, "members": members, "salt": "00" * 32}, headers=ADMIN_H
    )
    assert bad.status_code == 422  # reveal must match the commitment
    w.ok(
        w.c.post(reveal, json={"commitment": c, "members": members, "salt": salt}, headers=ADMIN_H)
    )
    stats = w.ok(w.c.get(f"/v1/runs/{w.run_id}/auditor-stats"))
    tot = {
        k: sum(e[k] for e in stats["epochs"])
        for k in ("hp_bad", "hp_bad_caught", "hp_honest", "hp_honest_flagged")
    }
    assert tot == {"hp_bad": 2, "hp_bad_caught": 2, "hp_honest": 1, "hp_honest_flagged": 0}
    assert {e["honeypot_catch_rate"] for e in stats["epochs"] if e["hp_bad"]} == {1.0}
    assert {e["honeypot_false_positive_rate"] for e in stats["epochs"] if e["hp_honest"]} == {0.0}
    record.update(q0=q0, q1=q1, stats=stats, cluster_job=job["challenge"]["body"])


def test_12_signed_message_rejections(world: Any, record: dict[str, Any]) -> None:
    stranger = key(0x56)
    w = world(manifest(policy="carry"), {H3[0]: PROBATION, H3[1]: PROBATION})
    w.push(w.body(0)["d_assign"])
    client = w.miner(H3[0])
    codes = client.call(
        "signed_probes", w=0, other_keyfile=w.keyfile(H3[1]), stranger_keyfile=w.keyfile(stranger)
    )
    record["codes"] = codes
    assert codes == {
        "expired": 400,
        "other_run": 400,
        "type_swap": 400,
        "type_relabel": 401,
        "bad_sig": 401,
        "accepted": 200,
        "replay": 409,
        "other_w": 404,
        "unregistered": 403,
        "signer_mismatch": 403,
    }


def test_13_hierarchical_two_region_round(world: Any, record: dict[str, Any]) -> None:
    from hypertrain.aggregator.core import AggregatorError, replay_tape, sign_tape, th

    regions = {"eu": [H3[0], H3[1]], "us": [H3[2], PEERS[3]]}
    roster = {k: {"probation": True, "region": r} for r, ks in regions.items() for k in ks}
    w = world(manifest(policy="carry", q=0.5, E=2), roster)
    miners = [w.miner(k) for k in roster]
    assert w.run_all(miners, 0) == ["UPLOADED"] * 4
    w.push(w.body(0)["d_upload"])
    out = w.agg.call(
        "regional", w=0, regions={r: [k.ss58 for k in ks] for r, ks in regions.items()}
    )
    tape = out["tape"]
    objects = LocalFSStore(w.srv.state / "objects")
    replayed = replay_tape(objects, tape, COORD.ss58)  # independent replay in this process
    assert th(replayed.theta) == tape["body"]["theta_hash"] == w.body(1)["theta_hash"]
    for ts in out["chains"].values():
        replay_tape(objects, ts[0], COORD.ss58)
    assert set(out["contributors"]) == {k.ss58 for k in roster}
    forged = {**tape["body"], "inputs": tape["body"]["inputs"][:1]}
    with pytest.raises(AggregatorError):  # a re-signed tape that drops a region
        replay_tape(objects, sign_tape(COORD, forged), COORD.ss58)
    with pytest.raises(AggregatorError):
        replay_tape(objects, {**tape, "body": forged}, COORD.ss58)
    record.update(
        global_tape=tape["body"]["theta_hash"],
        regional={r: ts[0]["body"]["theta_hash"] for r, ts in out["chains"].items()},
    )
