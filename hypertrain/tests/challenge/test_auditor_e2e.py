from __future__ import annotations

import sys
from pathlib import Path
from typing import Any

import hypertrain.trainer  # noqa: F401  (determinism setup precedes torch)

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "trainer"))

from trainer_fixtures import sample, small_manifest_body  # noqa: E402

from hypertrain.auditor.replay import pack_state  # noqa: E402
from hypertrain.auditor.worker import Auditor, HttpApi  # noqa: E402
from hypertrain.protocol.hashing import MerkleTree, sha256_hex  # noqa: E402
from hypertrain.protocol.messages import ReplayEnv, RunManifest  # noqa: E402
from hypertrain.trainer.config import TrainConfig  # noqa: E402
from hypertrain.trainer.loop import Assignment, train_round  # noqa: E402
from hypertrain.trainer.model import init_params  # noqa: E402

from .conftest import ADMIN, CONFIG, WORKER, Net, bearer  # noqa: E402

ENV = ReplayEnv(
    image_digest="sha256:" + "1" * 64, driver="cpu", gpu_uuid_sha256=sha256_hex(b"cpu"), sm_count=1
)


def _commit_body(net: Net, hotkey: str, res: Any, tokens: int) -> dict[str, Any]:
    metrics = [bytes.fromhex(x.preimage.loss_f32 + x.preimage.norm_f32) for x in res.leaves]
    return {
        "w": 0,
        "hotkey": hotkey,
        "leaf_scheme": "ht-leaf-v1",
        "n_leaves": len(res.leaves),
        "leaves_root": res.leaves_root,
        "metrics_root": MerkleTree(metrics).root.hex(),
        "final_theta_hash": res.final_theta_hash,
        "ef_in_hash": res.ef_in_hash,
        "ef_out_hash": res.ef_out_hash,
        "delta_hash": res.delta_hash,
        "delta_bytes": len(res.delta_payload),
        "tokens": tokens,
    }


def test_real_auditor_settles_match_and_mismatch(net: Net) -> None:
    net.manifest = RunManifest.model_validate(small_manifest_body())
    net.run_id = net.manifest.run_id()
    honest, cheat = net.miners[0], net.miners[1]
    cfg = TrainConfig.from_manifest(net.manifest.body())
    theta = init_params(cfg.model)
    store = net.c.app.state.store  # type: ignore[attr-defined]
    assert net.push(1000).status_code == 200
    assert net.create().status_code == 201
    assert net.admin_put("/config", CONFIG).status_code == 200
    for m in (honest, cheat):
        assert net.admin_put(f"/roster/{m.ss58}", {"probation": True}).status_code == 200
    assert net.admin_put("/paused", {"paused": False}).status_code == 200
    sha = store.objects.put(pack_state(theta))
    r = net.c.put(
        f"/v1/aggregator/runs/{net.run_id}/rounds/0/state",
        json={"theta_start_sha256": sha},
        headers=bearer(ADMIN),
    )
    assert r.status_code == 200
    b0 = net.body(0)
    net.push(b0["d_assign"])
    assignment = {a["hotkey"]: a for a in net.round(0)["assignment"]}
    for m in (honest, cheat):
        assert net.accept(m, 0).status_code == 200
    net.push(b0["d_assign"] + 1)
    for m in (honest, cheat):
        a = Assignment(net.run_id, 0, tuple(assignment[m.ss58]["samples"]), 0)
        start = theta if m is honest else {n: x + 1e-3 for n, x in theta.items()}
        res = train_round(cfg, start, a, sample(cfg))
        tokens = len(a.sample_ids) * cfg.model.seq_len
        env = net.signed(m, "Commit", _commit_body(net, m.ss58, res, tokens))
        assert net.post("commit", env).json()["status"] == "COMMITTED"
        pres = [x.preimage.model_dump(mode="json") for x in res.leaves]
        assert net.post_leaves_raw(m, 0, pres).status_code == 200
    net.push(b0["d_audit"])
    assert sorted(net.round(0)["selected"]) == sorted([honest.ss58, cheat.ss58])
    auditor = Auditor(HttpApi(net.c, WORKER), net.auditor, ENV, sample(cfg))
    results = {auditor.run_once(), auditor.run_once()}
    assert results == {"MATCH", "MISMATCH"}
    assert auditor.run_once() is None
    status = {m["hotkey"]: m["status"] for m in net.round(0)["miners"]}
    assert status == {honest.ss58: "MATCH", cheat.ss58: "MISMATCH"}
    forfeits = net.round(0)["forfeits"]
    assert [f["body"]["hotkey"] for f in forfeits] == [cheat.ss58]
