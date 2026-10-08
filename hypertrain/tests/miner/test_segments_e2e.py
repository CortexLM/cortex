from __future__ import annotations

import functools
from typing import Any

import pytest
from miner_harness import World

import hypertrain.miner.core as core
from challenge.conftest import AUDITOR_SEED, WORKER
from hypertrain.auditor.bisect import Executor, Fault
from hypertrain.auditor.replay import select_segments
from hypertrain.auditor.worker import Auditor, HttpApi
from hypertrain.miner.core import Miner
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import LeafPreimage, ReplayEnv, f32val

ENV = ReplayEnv(
    image_digest="sha256:" + "1" * 64, driver="cpu", gpu_uuid_sha256=sha256_hex(b"cpu"), sm_count=1
)
DELTA = 1e-3


class TamperServe(Miner):
    def served_state(self, states: Any, leaf: int) -> Any:
        theta, st = states[leaf]
        if leaf == 1:
            theta = {n: x.clone() for n, x in theta.items()}
            theta[sorted(theta)[0]].view(-1)[0] += DELTA
        return theta, st


def _audit(w: World, miner: Miner) -> tuple[list[str | None], dict[str, Any]]:
    view = w.op.round(0)
    assert view is not None
    w.op.push(view["round_open"]["body"]["d_audit"])
    job = next(j for j in w.op.round(0)["jobs"])  # type: ignore[index]
    ch = job["challenge"]["body"]
    assert ch["mode"] == "segments"
    assert miner.serve_challenges(0) == len(ch["segments"])
    auditor = Auditor(HttpApi(w.c, WORKER), Keypair(AUDITOR_SEED), ENV, w.get)
    results = []
    while (r := auditor.run_once()) is not None:
        results.append(r)
    return results, ch


def _expected_segments(w: World, ch: dict[str, Any]) -> list[list[int]]:
    store = w.c.app.state.store
    with store._tx() as db:
        row = db.execute("SELECT preimages FROM miners WHERE hotkey=?", (ch["target"],)).fetchone()
    import json

    pres = [LeafPreimage.model_validate(p) for p in json.loads(row["preimages"])]
    v = w.m.verify
    norms = [f32val(p.norm_f32) for p in pres[1:]]
    segs = select_segments(
        w.op.run_id, 0, ch["beacon_sig_sha256"], ch["target"], norms, v.k_segments, v.Q_top
    )
    return [list(s) for s in segs]


def _world(world_factory: Any, k: int) -> World:
    w: World = world_factory(policy="carry", n=1, k_segments=k)
    return w


@pytest.mark.parametrize("k", [0, 3])
def test_honest_carry_miner_matches_and_segments_are_leaf_windows(
    world_factory: Any, k: int
) -> None:
    w = _world(world_factory, k)
    m = w.miner(0, sink=True)
    assert m.run_round(0) == "UPLOADED"
    results, ch = _audit(w, m)
    u = w.m.inner.H // w.m.inner.J
    assert ch["segments"] == _expected_segments(w, ch)
    assert all(0 <= a < b <= u and b == a + 1 for a, b in ch["segments"])
    assert [u - 1, u] in ch["segments"]
    assert results == ["MATCH"]
    assert w.status(0) == {w.keys[0].ss58: "MATCH"}


def test_tampered_served_state_is_caught(world_factory: Any) -> None:
    w = _world(world_factory, 3)
    m = TamperServe(w.cfg(0), w.c, w.get, wait=w.op.tick, blob_sink=w.c.app.state.store.objects.put)
    assert m.run_round(0) == "UPLOADED"
    results, ch = _audit(w, m)
    assert [1, 2] in ch["segments"]
    with w.c.app.state.store._tx() as db:
        reason = db.execute("SELECT fail_reason FROM jobs").fetchone()[0]
    assert results == ["BAD_PROOF"], reason
    assert w.status(0) == {w.keys[0].ss58: "BAD_PROOF"}
    assert [f["body"]["hotkey"] for f in w.op.round(0)["forfeits"]] == [w.keys[0].ss58]  # type: ignore[index]


def test_middle_window_cheat_is_mismatch(
    world_factory: Any, monkeypatch: pytest.MonkeyPatch
) -> None:
    w = _world(world_factory, 3)
    step = w.m.inner.J + 1  # inside window [1, 2]
    n_layers = w.m.model.n_layers

    def bump(t: int, theta: Any) -> None:
        if t == step:
            theta[sorted(theta)[0]].view(-1)[0] += DELTA

    real_train = core.train_round
    monkeypatch.setattr(core, "train_round", functools.partial(real_train, after_step=bump))
    monkeypatch.setattr(
        core, "Executor", functools.partial(Executor, fault=Fault(step, n_layers, "update", DELTA))
    )
    m = w.miner(0, sink=True)
    assert m.run_round(0) == "UPLOADED"
    results, ch = _audit(w, m)
    assert [1, 2] in ch["segments"]
    assert results == ["MISMATCH"]
    view = w.op.round(0)
    assert view is not None
    assert w.status(0) == {w.keys[0].ss58: "MISMATCH"}
    assert view["miners"][0]["verdict"]["body"]["first_bad_leaf"] == 2
    assert [f["body"]["hotkey"] for f in view["forfeits"]] == [w.keys[0].ss58]
