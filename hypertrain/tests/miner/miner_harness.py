from __future__ import annotations

import hashlib
import json
import sys
from pathlib import Path
from typing import Any

import hypertrain.trainer  # noqa: F401

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "trainer"))

from trainer_fixtures import (
    sample,  # noqa: E402
    small_manifest_body,  # noqa: E402
)

from challenge.conftest import (  # noqa: E402
    ADMIN,
    COORD_SEED,
    OWNER,
    Master,  # noqa: E402
    beacon_payload,
    bearer,
)
from hypertrain.auditor.replay import pack_state  # noqa: E402
from hypertrain.miner.core import Miner, MinerConfig  # noqa: E402
from hypertrain.protocol.envelope import seal  # noqa: E402
from hypertrain.protocol.keys import Keypair  # noqa: E402
from hypertrain.protocol.messages import RunManifest  # noqa: E402
from hypertrain.trainer.compress import decompress, state_hash  # noqa: E402
from hypertrain.trainer.config import TrainConfig  # noqa: E402
from hypertrain.trainer.loop import Params  # noqa: E402
from hypertrain.trainer.model import init_params  # noqa: E402

CONFIG = {"train_rounds": 10, "final_after_upload": 5}


def manifest(
    policy: str = "reset", dataset: dict[str, Any] | None = None, k_segments: int | None = None
) -> RunManifest:
    body: Any = small_manifest_body()
    if k_segments is not None:
        body["verify"]["k_segments"] = k_segments
    body["inner"]["state_policy"] = policy
    body["reference_spec"]["image_digest"] = "sha256:" + "1" * 64
    if dataset:
        body["dataset"].update(dataset)
    cfg = TrainConfig.from_manifest(body)
    body["init_state_hash"] = state_hash(init_params(cfg.model))
    return RunManifest.model_validate(body)


def write_keyfile(path: Path, seed: bytes, mode: int = 0o600) -> Path:
    path.write_text(seed.hex())
    path.chmod(mode)
    return path


class Operator:
    """Plays K_owner/K_coord/beacon relay: pushes drand rounds and opens round w+1 after uploads,
    with theta^{w+1} = theta^w - mean(decompressed deltas) published by TH in ``state_dir``."""

    def __init__(self, client: Any, store: Any, m: RunManifest, state_dir: Path) -> None:
        self.c, self.store, self.m = client, store, m
        self.run_id = m.run_id()
        self.coord = Keypair(COORD_SEED)
        self.state_dir = state_dir
        self.cfg = TrainConfig.from_manifest(m.body())
        self.theta: dict[int, Params] = {0: init_params(self.cfg.model)}
        self.d = 0
        self.max_w = 10**9

    def push(self, rnd: int) -> None:
        while self.d < rnd:
            self.d += 1
            r = self.c.post("/v1/admin/beacon", json=beacon_payload(self.d), headers=bearer(ADMIN))
            assert r.status_code == 200, r.text

    def admin(self, method: str, path: str, body: Any) -> Any:
        r = self.c.request(
            method, f"/v1/admin/runs/{self.run_id}{path}", json=body, headers=bearer(ADMIN)
        )
        assert r.status_code in (200, 201), r.text
        return r.json()

    def start(self, hotkeys: list[str], at: int = 1000) -> None:
        self.push(at)
        env = seal(OWNER, "RunManifest", self.run_id, self.m, 2**40)
        assert self.c.post("/v1/admin/runs", json=env, headers=bearer(ADMIN)).status_code == 201
        self.admin("PUT", "/config", CONFIG)
        for hk in hotkeys:
            self.admin("PUT", f"/roster/{hk}", {"probation": True})
        self.admin("PUT", "/paused", {"paused": False})
        self.publish(0)

    def publish(self, w: int) -> None:
        blob = pack_state(self.theta[w])
        sha = self.store.objects.put(blob)
        (self.state_dir / state_hash(self.theta[w])).write_bytes(blob)
        r = self.c.put(
            f"/v1/aggregator/runs/{self.run_id}/rounds/{w}/state",
            json={"theta_start_sha256": sha},
            headers=bearer(ADMIN),
        )
        assert r.status_code == 200, r.text

    def round(self, w: int) -> dict[str, Any] | None:
        r = self.c.get(f"/v1/runs/{self.run_id}/rounds/{w}")
        return dict(r.json()) if r.status_code == 200 else None

    def aggregate(self, w: int) -> None:
        base = f"/v1/aggregator/runs/{self.run_id}/rounds/{w}"
        inputs = self.c.get(base + "/inputs", headers=bearer(ADMIN)).json()
        ups = [m for m in inputs["miners"] if m["status"] == "UPLOADED"]
        theta = {n: x.clone() for n, x in self.theta[w].items()}
        for m in ups:
            _, delta = decompress(self.store.objects.get(m["delta_manifest"]["body"]["delta_hash"]))
            for n in theta:
                theta[n] = theta[n] - delta[n] / len(ups)
        self.theta[w + 1] = theta
        th = state_hash(theta)
        body = {
            **inputs["next_round_template"],
            "prev_final_hash": th,
            "theta_hash": th,
            "outer_state_hash": hashlib.sha256(b"outer" + th.encode()).hexdigest(),
            "center_hash": hashlib.sha256(b"center" + th.encode()).hexdigest(),
        }
        env = seal(self.coord, "RoundOpen", self.run_id, body, 2**40)
        r = self.c.post(base + "/aggregate", json=env, headers=bearer(ADMIN))
        assert r.status_code == 200, r.text
        self.publish(w + 1)

    def tick(self, target: int) -> None:
        """Miner wait hook: advance the beacon one round (at least to ``target`` when the miner
        is waiting on a deadline) and open the next round once uploads closed."""
        self.push(max(self.d + 1, min(target, self.d + 60)))
        w = max(self.theta)
        view = self.round(w)
        if view is None or w >= self.max_w:
            return
        d_upload = view["round_open"]["body"]["d_upload"]
        if self.d >= d_upload:
            self.aggregate(w)


def dumps(x: Any) -> str:
    return json.dumps(x, sort_keys=True)


SEEDS = [bytes([0x41 + i]) * 32 for i in range(3)]


class World:
    def __init__(
        self, tmp: Path, c: Any, master: Master, policy: str, n: int, bad_init: bool, k: Any
    ):
        m = manifest(policy, k_segments=k)
        if bad_init:
            m = m.model_copy(update={"init_state_hash": "0" * 64})
        self.c, self.tmp, self.m = c, tmp, m
        self.keys = [Keypair(s) for s in SEEDS[:n]]
        for k in self.keys:
            master.registered.add(k.ss58)
        (tmp / "states").mkdir()
        self.op = Operator(c, c.app.state.store, m, tmp / "states")
        self.op.start([k.ss58 for k in self.keys])
        self.get = sample(TrainConfig.from_manifest(m.body()))

    def cfg(self, i: int) -> MinerConfig:
        key = write_keyfile(self.tmp / f"miner{i}.key", SEEDS[i])
        return MinerConfig(
            api="",
            keyfile=key,
            workdir=self.tmp / f"work{i}",
            state_source=str(self.tmp / "states"),
            image_digest=self.m.reference_spec.image_digest,
            run_id=self.op.run_id,
            allow_file_upload=True,
        )

    def miner(self, i: int, get: Any = None, sink: bool = False) -> Miner:
        return Miner(
            self.cfg(i),
            self.c,
            get or self.get,
            wait=self.op.tick,
            blob_sink=self.c.app.state.store.objects.put if sink else None,
        )

    def status(self, w: int) -> dict[str, str]:
        view = self.op.round(w)
        assert view is not None
        return {m["hotkey"]: m["status"] for m in view["miners"]}
