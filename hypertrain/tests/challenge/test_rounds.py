from __future__ import annotations

import hashlib
import struct
from fractions import Fraction
from typing import Any

from hypertrain.auditor.honeypot import Honeypot, commit
from hypertrain.challenge.store import audit_selected
from hypertrain.protocol.envelope import seal
from hypertrain.protocol.messages import QUICKNET_GENESIS, f32hex, f32val

from .conftest import ADMIN, OWNER, WORKER, Net, beacon_payload, bearer, miner


def run_round0(net: Net) -> dict[str, Any]:
    net.start()
    body = net.body(0)
    net.push(body["d_assign"])
    for m in net.miners:
        assert net.accept(m, 0).status_code == 200
    net.push(body["d_assign"] + 4)
    for m in net.miners:
        r = net.commit(m, 0)
        assert r.status_code == 200 and r.json()["status"] == "COMMITTED"
        assert net.delta(m, 0).status_code == 200
    net.push(body["d_audit"])
    return body


def test_lifecycle_requires_signed_manifest_and_order(net: Net) -> None:
    assert net.push(1000).status_code == 200
    forged = seal(miner(9), "RunManifest", net.run_id, net.manifest, 2**40)
    assert net.c.post("/v1/admin/runs", json=forged, headers=bearer(ADMIN)).status_code == 403
    tampered = seal(OWNER, "RunManifest", net.run_id, net.manifest, 2**40)
    tampered["body"]["budget"]["epochs_per_round"] = 2
    assert net.c.post("/v1/admin/runs", json=tampered, headers=bearer(ADMIN)).status_code == 401
    assert net.create().status_code == 201
    assert net.create().status_code == 409
    assert net.admin_put("/paused", {"paused": False}).status_code == 409
    assert net.admin_put("/config", {"train_rounds": 0, "final_after_upload": 1}).status_code == 422
    assert (
        net.admin_put("/config", {"train_rounds": 10, "final_after_upload": 5}).status_code == 200
    )
    assert net.admin_put("/paused", {"paused": False}).status_code == 409  # empty roster
    for m in net.miners:
        assert net.admin_put(f"/roster/{m.ss58}", {"bond": True}).status_code == 200
    assert net.admin_put("/paused", {"paused": "no"}).status_code == 422
    assert net.admin_put("/paused", {"paused": False}).json()["status"] == "running"
    assert net.admin_put("/config", {"train_rounds": 9, "final_after_upload": 5}).status_code == 409
    assert net.admin_put("/paused", {"paused": True}).json()["status"] == "paused"
    status = net.c.get(f"/v1/runs/{net.run_id}").json()
    assert status["status"] == "paused" and [r["w"] for r in status["rounds"]] == [0]


def test_beacon_push_is_verified(net: Net) -> None:
    bad = beacon_payload(5) | {"signature": "00" * 32}
    assert net.c.post("/v1/admin/beacon", json=bad, headers=bearer(ADMIN)).status_code == 400
    assert net.push(5).json() == {"round": 5, "latest": 5}
    assert net.push(3).json() == {"round": 3, "latest": 5}


def test_state_machine_advances_only_on_drand_rounds(net: Net) -> None:
    net.start()
    b = net.body(0)
    seen = [net.round(0)["state"]]
    for rnd in (b["d_assign"], b["d_assign"] + 1, b["d_commit"], b["d_upload"]):
        net.push(rnd)
        seen.append(net.round(0)["state"])
    assert seen == ["OPEN", "ASSIGNED", "TRAINING", "COMMIT_CLOSED", "UPLOAD_CLOSED"]
    assert net.aggregate(0).status_code == 200
    assert net.round(0)["state"] == "APPLIED"
    net.push(net.body(1)["d_assign"])
    assert net.round(0)["state"] == "AUDIT"


def test_happy_three_miners_to_applied_and_audit(net: Net) -> None:
    b0 = run_round0(net)
    net.push(b0["d_upload"])
    assert net.round(0)["state"] == "UPLOAD_CLOSED"
    assert net.aggregate(0).status_code == 200
    assert net.round(0)["state"] == "APPLIED"
    b1 = net.body(1)
    assert b1["w"] == 1 and len(b1["roster"]) == 3
    net.push(b1["d_assign"])
    view = net.round(0)
    assert view["state"] == "AUDIT"
    assert {m["hotkey"]: m["status"] for m in view["miners"]} == {
        m.ss58: "UPLOADED" for m in net.miners
    }
    probation = net.miners[2].ss58
    assert probation in view["selected"]
    job = next(j for j in view["jobs"] if j["target"] == probation)
    assert job["challenge"]["body"]["reasons"] == ["probation", "final"]  # carry: final always
    assert net.round(1)["base"] == 3 * 8 * 30


def test_late_commit_is_excluded_without_entitlement(net: Net) -> None:
    net.start()
    b0 = net.body(0)
    net.push(b0["d_assign"])
    for m in net.miners:
        assert net.accept(m, 0).status_code == 200
    net.push(b0["d_assign"] + 1)
    for m in net.miners[:2]:
        assert net.commit(m, 0).json()["status"] == "COMMITTED"
    net.push(b0["d_audit"])
    late = net.commit(net.miners[2], 0)
    assert late.status_code == 200
    out = late.json()
    assert out["status"] == "EXCLUDED"
    assert out["receipt"]["body"]["received_round"] == b0["d_audit"]
    assert out["receipt"]["signer"] == net.coord.ss58
    for m in net.miners[:2]:
        assert net.delta(m, 0).status_code == 200
    assert net.delta(net.miners[2], 0).status_code == 409
    net.push(b0["d_upload"])
    assert net.aggregate(0).status_code == 200
    net.push(net.body(1)["d_assign"])
    jobs = net.round(0)["jobs"]
    while (lease := net.c.post("/v1/worker/lease", headers=bearer(WORKER))).status_code == 200:
        assert net.verdict(lease.json(), "MATCH").status_code == 200
    assert {j["target"] for j in jobs} <= {m.ss58 for m in net.miners[:2]}
    net.push(b0["d_final"] + 100)
    ok = sorted(m.ss58 for m in net.miners[:2])
    r = net.finalize(0, ok)
    assert r.status_code == 200, r.text
    assert set(r.json()["verdicts"]) == set(ok)
    state = net.c.app.state.store.ledger.state()  # type: ignore[attr-defined]
    assert {e.hotkey for e in state.entitlements} == set(ok)


def test_unregistered_hotkey_is_403(tmp_path, secrets_dir, master, clock) -> None:
    from .conftest import make_client

    outsider = miner(43)
    master.registered.add(outsider.ss58)
    with make_client(tmp_path / "s", secrets_dir, master, clock) as c:
        net = Net(c, master)
        net.start()
        net.push(net.body(0)["d_assign"])
        rostered = net.miners[0]
        master.registered.discard(rostered.ss58)
        r = net.accept(rostered, 0)
        assert r.status_code == 403 and "not registered" in r.json()["detail"]
        stranger = miner(42)
        env = net.signed(stranger, "Commit", net.commit_body(stranger, 0))
        r = net.post("commit", env)
        assert r.status_code == 403 and "not registered" in r.json()["detail"]
        env = net.signed(outsider, "Commit", net.commit_body(outsider, 0))
        r = net.post("commit", env)
        assert r.status_code == 403 and "roster" in r.json()["detail"]


def test_signed_intake_rejections(net: Net) -> None:
    net.start()
    b0 = net.body(0)
    net.push(b0["d_assign"])
    m = net.miners[0]
    assert net.accept(m, 0).status_code == 200
    assert net.accept(m, 0).status_code == 409  # replay
    m2 = net.miners[1]
    env = net.signed(m2, "Commit", net.commit_body(m2, 0))
    assert net.post("commit", env).status_code == 409  # not accepted yet
    other = net.signed(m, "Commit", net.commit_body(m2, 0))
    assert net.post("commit", other).status_code == 403  # body hotkey != signer
    bad = dict(env, sig="00" * 64)
    assert net.post("commit", bad).status_code == 401
    expired = net.signed(m, "Commit", net.commit_body(m, 0), exp=b0["d_assign"] - 1)
    assert net.post("commit", expired).status_code == 400
    wrong_run = seal(m, "Commit", "ab" * 32, net.commit_body(m, 0), 2**40)
    assert net.post("commit", wrong_run).status_code == 400
    good = net.signed(m, "Commit", net.commit_body(m, 0))
    assert net.post("commit", good).json()["status"] == "COMMITTED"
    assert net.post("commit", good).status_code == 409


def test_replay_set_survives_restart(tmp_path, secrets_dir, master, clock) -> None:
    from .conftest import make_client

    with make_client(tmp_path / "s", secrets_dir, master, clock) as c:
        net = Net(c, master)
        net.start()
        net.push(net.body(0)["d_assign"])
        m = net.miners[0]
        view = net.round(0)
        mine = next(a for a in view["assignment"] if a["hotkey"] == m.ss58)
        body = {
            "w": 0,
            "hotkey": m.ss58,
            "assignment_hash": mine["assignment_hash"],
            "image_digest": net.manifest.reference_spec.image_digest,
            "driver_version": "cpu",
            "n_gpus": 1,
        }
        env = net.signed(m, "Accept", body)
        assert net.post("accept", env).status_code == 200
    with make_client(tmp_path / "s", secrets_dir, master, clock) as c:
        r = c.post(f"/v1/runs/{net.run_id}/accept", json=env)
        assert r.status_code == 409 and "duplicate" in r.json()["detail"]


def test_wrong_assignment_hash_is_rejected(net: Net) -> None:
    net.start()
    net.push(net.body(0)["d_assign"])
    m = net.miners[0]
    body = {
        "w": 0,
        "hotkey": m.ss58,
        "assignment_hash": "00" * 32,
        "image_digest": net.manifest.reference_spec.image_digest,
        "driver_version": "cpu",
        "n_gpus": 1,
    }
    assert net.post("accept", net.signed(m, "Accept", body)).status_code == 422


def test_selection_reproducible_from_public_data(net: Net) -> None:
    b0 = run_round0(net)
    view = net.round(0)
    sig = bytes.fromhex(view["audit_beacon_signature"])
    assert view["audit_beacon_round"] == b0["d_audit"]
    q = {e["hotkey"]: e["q_i"] for e in b0["roster"]}
    recomputed = []
    for hk in sorted(q):
        digest = hashlib.sha256(
            b"ht-audit" + bytes.fromhex(net.run_id) + struct.pack(">Q", 0) + sig + hk.encode()
        ).digest()
        frac = Fraction(f32val(q[hk]))
        if int.from_bytes(digest, "big") * frac.denominator < frac.numerator * 2**256:
            recomputed.append(hk)
    assert recomputed == view["selected"]
    assert sorted(j["target"] for j in view["jobs"]) == recomputed
    for j in view["jobs"]:
        assert j["challenge"]["body"]["beacon_round"] == b0["d_audit"]
        assert j["challenge"]["body"]["beacon_sig_sha256"] == hashlib.sha256(sig).hexdigest()


def test_selection_rate_matches_q() -> None:
    run_id = "11" * 32
    q = f32hex(0.1)
    hits = sum(
        audit_selected(run_id, w, hashlib.sha256(b"%d" % w).digest(), "hk", q) for w in range(4000)
    )
    assert 330 <= hits <= 470  # binomial(4000, 0.1) 99.9% band
    assert all(audit_selected(run_id, w, b"s", "hk", f32hex(1.0)) for w in range(50))


def test_fault_verdict_forfeits_and_blacklists(net: Net) -> None:
    run_round0(net)
    lease = net.c.post("/v1/worker/lease", headers=bearer(WORKER))
    assert lease.status_code == 200
    job = lease.json()
    assert job["commit_envelope"]["type"] == "Commit" and job["round_open"]["type"] == "RoundOpen"
    assert job["commit"]["hotkey"] == job["target"] and len(job["leaves"]) == 7
    assert job["theta_start_sha256"] and job["assignment"]["global_step0"] == 0
    assert len(job["assignment"]["sample_ids"]) == 8 * 30
    hb = net.c.post(
        f"/v1/worker/jobs/{job['id']}/heartbeat",
        json={"lease": job["lease"]},
        headers=bearer(WORKER),
    )
    assert hb.status_code == 200
    stale = net.c.post(
        f"/v1/worker/jobs/{job['id']}/heartbeat", json={"lease": "x"}, headers=bearer(WORKER)
    )
    assert stale.status_code == 409
    r = net.verdict(job, "MISMATCH")
    assert r.status_code == 200
    forfeit = r.json()["forfeit"]
    assert forfeit["body"]["cause"] == "MISMATCH" and forfeit["body"]["blacklist"] is True
    assert forfeit["signer"] == net.coord.ss58
    roster = net.c.get(f"/v1/runs/{net.run_id}").json()["roster"]
    assert next(r for r in roster if r["hotkey"] == job["target"])["blacklisted"] is True
    stats = net.c.get(f"/run/{net.run_id}/auditor-stats").json()
    assert stats["epochs"][0]["audits"] == 1 and stats["epochs"][0]["catches"] == 1


def test_non_auditor_verdict_is_403(net: Net) -> None:
    run_round0(net)
    job = net.c.post("/v1/worker/lease", headers=bearer(WORKER)).json()
    net.auditor = miner(1)
    assert net.verdict(job, "MATCH").status_code == 403


def test_lease_expires_after_600_rounds(net: Net) -> None:
    b0 = run_round0(net)
    job = net.c.post("/v1/worker/lease", headers=bearer(WORKER)).json()
    assert job["lease_expires_round"] == b0["d_audit"] + 600
    net.push(b0["d_audit"] + 601)
    again = net.c.post("/v1/worker/lease", headers=bearer(WORKER)).json()
    assert again["id"] == job["id"] and again["lease"] != job["lease"]
    assert net.verdict(job, "MATCH").status_code == 409


def test_honeypot_commit_reveal_and_rates(net: Net) -> None:
    members = [{"hotkey": net.miners[2].ss58, "mode": "fabricate"}]
    salt = "5a" * 32
    commitment = commit([Honeypot(net.miners[2].ss58, "fabricate")], bytes.fromhex(salt))
    net.push(1000)
    assert net.create().status_code == 201
    assert net.admin_put("/honeypot", {"commitment": commitment}).status_code == 200
    net.admin_put("/config", {"train_rounds": 10, "final_after_upload": 5})
    for m, bond in zip(net.miners, (True, True, False), strict=True):
        net.admin_put(f"/roster/{m.ss58}", {"bond": bond})
    net.admin_put("/paused", {"paused": False})
    assert net.publish_state(0).status_code == 200
    b0 = net.body(0)
    assert b0["honeypot_commit"] == commitment
    net.push(b0["d_assign"])
    for m in net.miners:
        net.accept(m, 0)
    net.push(b0["d_assign"] + 1)
    for m in net.miners:
        net.commit(m, 0)
        net.delta(m, 0)
    net.push(b0["d_audit"])
    while (lease := net.c.post("/v1/worker/lease", headers=bearer(WORKER))).status_code == 200:
        job = lease.json()
        bad = job["target"] == net.miners[2].ss58
        assert net.verdict(job, "MISMATCH" if bad else "MATCH").status_code == 200
    reveal = f"/v1/admin/runs/{net.run_id}/honeypot/reveal"
    wrong = {"commitment": commitment, "members": members, "salt": "00" * 32}
    assert net.c.post(reveal, json=wrong, headers=bearer(ADMIN)).status_code == 422
    good = {"commitment": commitment, "members": members, "salt": salt}
    assert net.c.post(reveal, json=good, headers=bearer(ADMIN)).status_code == 200
    stats = net.c.get(f"/run/{net.run_id}/auditor-stats").json()["epochs"][0]
    assert stats["hp_bad"] == 1 and stats["honeypot_catch_rate"] == 1.0
    assert stats["honeypot_false_positive_rate"] is None


def test_finalize_vests_then_pays(net: Net) -> None:
    b0 = run_round0(net)
    while (lease := net.c.post("/v1/worker/lease", headers=bearer(WORKER))).status_code == 200:
        assert net.verdict(lease.json(), "MATCH").status_code == 200
    net.push(b0["d_upload"])
    assert net.aggregate(0).status_code == 200
    included = sorted(m.ss58 for m in net.miners)
    assert net.finalize(0, included).status_code == 409
    net.push(b0["d_final"])
    assert net.finalize(0, included[:2]).status_code == 422
    r = net.finalize(0, included)
    assert r.status_code == 200, r.text
    assert net.round(0)["state"] == "FINAL"
    assert net.finalize(0, included).status_code == 409
    store = net.c.app.state.store  # type: ignore[attr-defined]
    final_at = store.ledger.state().entitlements[0].final_round
    epoch_at = QUICKNET_GENESIS + (final_at + 10) * 300 + 1
    early = net.weights(final_at + 9, epoch_at - 300).json()
    assert early["weights"] == {}
    paid = net.weights(final_at + 10, epoch_at).json()
    assert set(paid["weights"]) == set(included)
    assert sum(paid["weights"].values()) == 10**6 - paid["metadata"]["units_burned_this_epoch"]
