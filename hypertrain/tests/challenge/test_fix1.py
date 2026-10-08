from __future__ import annotations

import hashlib
import struct
from fractions import Fraction
from typing import Any

import pytest

import hypertrain.trainer  # noqa: F401  (determinism setup precedes torch)
from hypertrain.challenge.store import audit_selected, rerun_message
from hypertrain.protocol.envelope import body_digest, seal
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import QUICKNET_GENESIS, f32hex, f32val

from .conftest import ADMIN, OWNER, WORKER, Net, bearer, h, miner


def _to_audit(net: Net) -> dict[str, Any]:
    net.start(bonded=(True, True, False))
    b0 = net.body(0)
    net.push(b0["d_assign"])
    for m in net.miners:
        assert net.accept(m, 0).status_code == 200
    net.push(b0["d_assign"] + 2)
    for m in net.miners:
        assert net.commit(m, 0).json()["status"] == "COMMITTED"
        assert net.delta(m, 0).status_code == 200
    net.push(b0["d_audit"])
    return b0


def _lease_target(net: Net, hotkey: str) -> dict[str, Any]:
    while True:
        job = net.c.post("/v1/worker/lease", headers=bearer(WORKER)).json()
        if job["target"] == hotkey:
            return job
        assert net.verdict(job, "MATCH").status_code == 200


def _resolution(net: Net, dispute_id: str, loser: str, referee: Keypair) -> Any:
    body = {
        "dispute_id": dispute_id,
        "op_spec": "matmul",
        "inputs_hash": h("in"),
        "output_hash": h("out"),
        "loser": loser,
    }
    return net.c.post(
        f"/v1/runs/{net.run_id}/resolution",
        json=seal(referee, "Resolution", net.run_id, body, 2**40),
        headers=bearer(WORKER),
    )


def test_pay_on_vindication_after_finality(net: Net) -> None:
    referee = Keypair(b"\x31" * 32)
    net.manifest = net.manifest.model_copy(update={"auditors": [net.auditor.ss58, referee.ss58]})
    net.run_id = net.manifest.run_id()
    b0 = _to_audit(net)
    victim = net.miners[2]
    job = _lease_target(net, victim.ss58)
    verdict = net.verdict(job, "MISMATCH")
    assert verdict.status_code == 200
    mine = next(m for m in net.round(0)["miners"] if m["hotkey"] == victim.ss58)
    vhash = body_digest(mine["verdict"]["body"])
    contest = {"hotkey": victim.ss58, "verdict_hash": vhash, "action": "contest"}
    disp = net.post("dispute", net.signed(victim, "Dispute", contest))
    assert disp.status_code == 200 and disp.json()["state"] == "open"
    while (lease := net.c.post("/v1/worker/lease", headers=bearer(WORKER))).status_code == 200:
        assert net.verdict(lease.json(), "MATCH").status_code == 200
    net.push(b0["d_upload"])
    assert net.aggregate(0).status_code == 200
    net.push(b0["d_final"])
    others = sorted(m.ss58 for m in net.miners[:2])
    assert net.finalize(0, others).status_code == 409
    rb = {
        "w": 0,
        "excluded": [victim.ss58],
        "old_theta_hash_w2": h("old"),
        "new_theta_hash_w2": h("new"),
        "new_outer_state_hash": h("outer"),
        "recomputed": ["agg_w", "step_w"],
        "cause_hashes": [vhash],
    }
    r = net.c.post(
        f"/v1/aggregator/runs/{net.run_id}/rounds/0/rollback",
        json=net.signed(net.coord, "Rollback", rb),
        headers=bearer(ADMIN),
    )
    assert r.status_code == 200, r.text
    r = net.finalize(0, others)
    assert r.status_code == 200, r.text
    assert r.json()["verdicts"][victim.ss58] == "NO_UPLOAD"
    store = net.c.app.state.store  # type: ignore[attr-defined]
    assert victim.ss58 not in {e.hotkey for e in store.ledger.state().entitlements}
    did = disp.json()["dispute_id"]
    assert _resolution(net, did, net.auditor.ss58, net.auditor).status_code == 403
    res = _resolution(net, did, net.auditor.ss58, referee)
    assert res.status_code == 200 and res.json()["credited"] is True
    assert _resolution(net, did, net.auditor.ss58, referee).status_code == 409
    state = store.ledger.state()
    assert state.minted == state.burned + state.paid + state.pending
    victim_units = sum(e.amount for e in state.entitlements if e.hotkey == victim.ss58)
    assert victim_units > 0
    credit_round = next(e.final_round for e in state.entitlements if e.hotkey == victim.ss58)
    paid: dict[str, float] = {}
    for k in range(1, 25):
        epoch = credit_round + k
        ans = net.weights(epoch, QUICKNET_GENESIS + epoch * 300 + 1).json()
        for hk, units in ans["weights"].items():
            paid[hk] = paid.get(hk, 0) + units
    assert paid[victim.ss58] == victim_units


def test_transient_rerun_route(net: Net) -> None:
    _to_audit(net)
    victim = net.miners[2]
    job = _lease_target(net, victim.ss58)
    assert net.verdict(job, "MISMATCH").status_code == 200
    root = h("recomputed")
    sig = victim.sign(rerun_message(net.run_id, 0, victim.ss58, root)).hex()
    body = {"w": 0, "hotkey": victim.ss58, "leaves_root": root, "sig": sig}
    forged = dict(body, sig="00" * 64)
    assert net.post("rerun", forged).status_code == 401
    r = net.post("rerun", body)
    assert r.status_code == 200 and r.json()["classification"] == "TRANSIENT"
    assert net.post("rerun", body).status_code == 409
    view = net.round(0)
    assert next(m for m in view["miners"] if m["hotkey"] == victim.ss58)["status"] == "TRANSIENT"
    assert view["forfeits"] == []


def test_manifest_coord_must_be_container_key(net: Net) -> None:
    net.push(1000)
    body = net.manifest.model_dump(mode="json")
    body["coord_pubkey"] = miner(5).ss58
    from hypertrain.protocol.messages import RunManifest

    other = RunManifest.model_validate(body)
    env = seal(OWNER, "RunManifest", other.run_id(), other, 2**40)
    r = net.c.post("/v1/admin/runs", json=env, headers=bearer(ADMIN))
    assert r.status_code == 422 and "coord_pubkey" in r.json()["detail"]


def test_selection_preimage_binds_every_field() -> None:
    run_id, sig, q = "11" * 32, b"sig", f32hex(0.5)
    keys = [miner(i).ss58 for i in range(1, 65)]

    def ref(rid: str, w: int, s: bytes, hk: str) -> bool:
        d = hashlib.sha256(
            b"ht-audit" + bytes.fromhex(rid) + struct.pack(">Q", w) + s + hk.encode()
        ).digest()
        f = Fraction(f32val(q))
        return int.from_bytes(d, "big") * f.denominator < f.numerator * 2**256

    picks = [audit_selected(run_id, 3, sig, k, q) for k in keys]
    assert picks == [ref(run_id, 3, sig, k) for k in keys]
    assert 0 < sum(picks) < len(keys)
    assert picks != [ref(run_id, 4, sig, k) for k in keys]
    assert picks != [ref("22" * 32, 3, sig, k) for k in keys]
    assert picks != [ref(run_id, 3, b"other", k) for k in keys]


def test_serves_and_objects_need_worker_and_lease(net: Net) -> None:
    _to_audit(net)
    job = net.c.post("/v1/worker/lease", headers=bearer(WORKER)).json()
    url = f"/v1/worker/jobs/{job['id']}/serves"
    assert net.c.get(url, params={"lease": job["lease"]}).status_code == 401
    assert net.c.get(url, params={"lease": "x"}, headers=bearer(WORKER)).status_code == 409
    ok = net.c.get(url, params={"lease": job["lease"]}, headers=bearer(WORKER)).json()
    assert ok["serves"] == [] and ok["now_round"] >= 1
    sha = job["theta_start_sha256"]
    assert net.c.get(f"/v1/objects/{sha}").status_code == 401
    blob = net.c.get(f"/v1/objects/{sha}", headers=bearer(WORKER))
    assert blob.status_code == 200 and hashlib.sha256(blob.content).hexdigest() == sha
    assert net.c.get(f"/v1/objects/{'0' * 64}", headers=bearer(WORKER)).status_code == 404
    assert net.c.get("/v1/objects/zz", headers=bearer(WORKER)).status_code == 400


def test_leaves_must_hash_to_committed_root(net: Net) -> None:
    net.start()
    b0 = net.body(0)
    net.push(b0["d_assign"])
    m = net.miners[0]
    net.accept(m, 0)
    env = net.signed(m, "Commit", net.commit_body(m, 0))
    assert net.post("commit", env).json()["status"] == "COMMITTED"
    pres = net.preimages(m, 0)
    pres[1]["rng_ctr"] = 1
    bad = net.post("leaves", {"w": 0, "hotkey": m.ss58, "preimages": pres})
    assert bad.status_code == 422
    assert net.post_leaves(m, 0).status_code == 200


def _drain_match(net: Net, skip: str | None = None) -> dict[str, Any] | None:
    held = None
    while (lease := net.c.post("/v1/worker/lease", headers=bearer(WORKER))).status_code == 200:
        job = lease.json()
        if job["target"] == skip:
            held = job
            continue
        assert net.verdict(job, "MATCH").status_code == 200
    return held


def test_dispute_lost_after_finality_burns_earlier_escrow(net: Net) -> None:
    referee = Keypair(b"\x31" * 32)
    net.manifest = net.manifest.model_copy(update={"auditors": [net.auditor.ss58, referee.ss58]})
    net.run_id = net.manifest.run_id()
    store = net.c.app.state.store  # type: ignore[attr-defined]
    victim = net.miners[2]
    everyone = sorted(m.ss58 for m in net.miners)
    others = sorted(m.ss58 for m in net.miners[:2])
    b0 = _to_audit(net)
    _drain_match(net)
    net.push(b0["d_upload"])
    assert net.aggregate(0).status_code == 200
    b1 = net.body(1)
    net.push(max(b0["d_final"], b1["d_assign"]))
    assert net.finalize(0, everyone).status_code == 200
    escrow = sum(
        e.outstanding for e in store.ledger.state().entitlements if e.hotkey == victim.ss58
    )
    assert escrow > 0
    for m in net.miners:
        assert net.accept(m, 1).status_code == 200
    net.push(b1["d_assign"] + 2)
    for m in net.miners:
        assert net.commit(m, 1).json()["status"] == "COMMITTED"
        assert net.delta(m, 1).status_code == 200
    net.push(b1["d_audit"])
    job = _drain_match(net, skip=victim.ss58)
    assert job is not None and job["w"] == 1
    assert job["assignment"]["global_step0"] == 1 * net.manifest.inner.H
    assert net.verdict(job, "MISMATCH").status_code == 200
    forfeit = net.round(1)["forfeits"][0]["body"]
    assert (forfeit["round_reward_burned"], forfeit["escrow_burned_units"]) == (0, 0)
    mine = next(m for m in net.round(1)["miners"] if m["hotkey"] == victim.ss58)
    vhash = body_digest(mine["verdict"]["body"])
    contest = {"hotkey": victim.ss58, "verdict_hash": vhash, "action": "contest"}
    disp = net.post("dispute", net.signed(victim, "Dispute", contest))
    assert disp.status_code == 200
    net.push(b1["d_upload"])
    assert net.aggregate(1).status_code == 200
    net.push(b1["d_final"])
    rb = {
        "w": 1,
        "excluded": [victim.ss58],
        "old_theta_hash_w2": h("old"),
        "new_theta_hash_w2": h("new"),
        "new_outer_state_hash": h("outer"),
        "recomputed": ["agg_w", "step_w"],
        "cause_hashes": [vhash],
    }
    r = net.c.post(
        f"/v1/aggregator/runs/{net.run_id}/rounds/1/rollback",
        json=net.signed(net.coord, "Rollback", rb),
        headers=bearer(ADMIN),
    )
    assert r.status_code == 200, r.text
    assert net.finalize(1, others).json()["verdicts"][victim.ss58] == "NO_UPLOAD"
    burned_before = store.ledger.state().burned
    did = disp.json()["dispute_id"]
    res = _resolution(net, did, victim.ss58, referee)
    assert res.status_code == 200 and res.json()["faulted"] is True
    assert _resolution(net, did, victim.ss58, referee).status_code == 409
    state = store.ledger.state()
    assert victim.ss58 in state.blacklist
    assert sum(e.outstanding for e in state.entitlements if e.hotkey == victim.ss58) == 0
    slice_ = state.burned - burned_before - escrow
    assert slice_ == 0
    assert state.minted == state.burned + state.paid + state.pending
    forfeit = next(f for f in net.round(1)["forfeits"] if f["body"]["hotkey"] == victim.ss58)
    assert forfeit["signer"] == net.coord.ss58 and forfeit["body"]["cause"] == "DISPUTE_LOST"
    assert forfeit["body"]["escrow_burned_units"] == escrow
    assert (
        forfeit["body"]["round_reward_burned"]
        == store.ledger.burned_for_fault(1, victim.ss58)[0]
        > 0
    )
    assert store.ledger.fault(net.run_id, 1, victim.ss58, 2**40) is False
    for k in range(1, 25):
        epoch = state.last_finalized + k + 20
        ans = net.weights(epoch, QUICKNET_GENESIS + epoch * 300 + 1).json()
        assert victim.ss58 not in ans["weights"]


def test_state_serve_tensor_root_must_match_blob(net: Net) -> None:
    import torch

    from hypertrain.auditor.replay import pack_state, tensor_root
    from hypertrain.trainer.optim import OptState

    inner = net.manifest.inner.model_copy(update={"state_policy": "carry"})
    net.manifest = net.manifest.model_copy(update={"inner": inner})
    net.run_id = net.manifest.run_id()
    _to_audit(net)
    job = net.c.post("/v1/worker/lease", headers=bearer(WORKER)).json()
    assert job["challenge"]["mode"] == "segments"
    target = next(m for m in net.miners if m.ss58 == job["target"])
    theta = {"w": torch.arange(4, dtype=torch.float32)}
    st = OptState(m={"w": torch.ones(4)}, v={"w": torch.zeros(4)}, step=3)
    store = net.c.app.state.store  # type: ignore[attr-defined]
    blob = store.objects.put(pack_state(theta, st))
    good = tensor_root(theta, st)

    def serve(root: str) -> Any:
        body = {
            "hotkey": target.ss58,
            "challenge_hash": job["challenge_hash"],
            "t": 2,
            "uri": f"/v1/objects/{blob}",
            "tensor_root": root,
            "merkle_proof_leaf_in_leaves_root": [],
        }
        return net.post("state", net.signed(target, "StateServe", body))

    assert serve(h("wrong")).status_code == 422
    assert serve(good).status_code == 200
    served = net.c.get(
        f"/v1/worker/jobs/{job['id']}/serves",
        params={"lease": job["lease"]},
        headers=bearer(WORKER),
    ).json()["serves"]
    assert [(x["serve"]["tensor_root"], x["blob_sha256"]) for x in served] == [(good, blob)]


@pytest.mark.parametrize("contested", [False, True])
def test_finalize_reseals_forfeit_with_ledger_slice(net: Net, contested: bool) -> None:
    referee = Keypair(b"\x31" * 32)
    net.manifest = net.manifest.model_copy(update={"auditors": [net.auditor.ss58, referee.ss58]})
    net.run_id = net.manifest.run_id()
    store = net.c.app.state.store  # type: ignore[attr-defined]
    victim = net.miners[2]
    b0 = _to_audit(net)
    job = _drain_match(net, skip=victim.ss58)
    assert job is not None
    assert net.verdict(job, "MISMATCH").status_code == 200
    if contested:
        mine = next(m for m in net.round(0)["miners"] if m["hotkey"] == victim.ss58)
        vhash = body_digest(mine["verdict"]["body"])
        contest = {"hotkey": victim.ss58, "verdict_hash": vhash, "action": "contest"}
        disp = net.post("dispute", net.signed(victim, "Dispute", contest))
        assert disp.status_code == 200
        res = _resolution(net, disp.json()["dispute_id"], victim.ss58, referee)
        assert res.status_code == 200 and "faulted" not in res.json()
    net.push(b0["d_upload"])
    assert net.aggregate(0).status_code == 200
    net.push(b0["d_final"])
    r = net.finalize(0, sorted(m.ss58 for m in net.miners[:2]))
    assert r.status_code == 200 and r.json()["verdicts"][victim.ss58] == "FAULT"
    forfeit = next(f for f in net.round(0)["forfeits"] if f["body"]["hotkey"] == victim.ss58)
    burned = store.ledger.burned_for_fault(0, victim.ss58)
    assert forfeit["body"]["round_reward_burned"] == burned[0] > 0
    assert forfeit["body"]["escrow_burned_units"] == burned[1]
    state = store.ledger.state()
    assert state.minted == state.burned + state.paid + state.pending
