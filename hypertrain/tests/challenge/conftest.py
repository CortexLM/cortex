from __future__ import annotations

import hashlib
from collections.abc import Iterator
from pathlib import Path
from typing import Any

import httpx
import pytest
from fastapi.testclient import TestClient

from hypertrain.beacon import BeaconRound, BeaconVerificationError, FixtureBeacon
from hypertrain.challenge.app import Config, create_app
from hypertrain.ledger import Params
from hypertrain.protocol.envelope import seal
from hypertrain.protocol.example import example_manifest
from hypertrain.protocol.hashing import MerkleTree
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import QUICKNET_GENESIS, LeafPreimage, RunManifest, f32hex

SLUG = "hypertrain"
INTERNAL, ADMIN, WORKER = "internal-secret", "admin-secret", "worker-secret"
COORD_SEED = bytes(range(32))  # example_manifest coord key
AUDITOR_SEED = bytes(31) + b"\x01"  # example_manifest auditor
OWNER = Keypair(b"\x77" * 32)
EPOCH_SECONDS = 300
PARAMS = Params(SLUG, QUICKNET_GENESIS, EPOCH_SECONDS, 1, 10)
CONFIG = {"train_rounds": 10, "final_after_upload": 5}
_ORACLE = FixtureBeacon(current=2**40)


def bearer(token: str) -> dict[str, str]:
    return {"authorization": f"Bearer {token}"}


def beacon_payload(rnd: int) -> dict[str, Any]:
    br = _ORACLE.get(rnd)
    return {"round": br.round, "signature": br.signature, "randomness": br.randomness}


def verify_fixture(payload: Any) -> BeaconRound:
    rnd = payload.get("round")
    if type(rnd) is not int or rnd < 1:
        raise BeaconVerificationError("bad round")
    if dict(payload) != beacon_payload(rnd):
        raise BeaconVerificationError("signature does not verify")
    return _ORACLE.get(rnd)


def miner(i: int) -> Keypair:
    return Keypair(bytes([i]) * 32)


def h(label: str) -> str:
    return hashlib.sha256(label.encode()).hexdigest()


class Master:
    def __init__(self) -> None:
        self.registered: set[str] = set()

    def handler(self, request: httpx.Request) -> httpx.Response:
        assert request.url.path == "/v1/metagraph/latest"
        return httpx.Response(200, json={"hotkeys": {k: i for i, k in enumerate(self.registered)}})


@pytest.fixture
def secrets_dir(tmp_path: Path) -> Path:
    d = tmp_path / "secrets"
    d.mkdir()
    (d / "internal.token").write_text(INTERNAL + "\n")
    (d / "admin.token").write_text(ADMIN + "\n")
    (d / "worker.token").write_text(WORKER + "\n")
    (d / "coord.key").write_text(COORD_SEED.hex())
    return d


@pytest.fixture
def master() -> Master:
    return Master()


def make_config(state: Path, secrets: Path) -> Config:
    return Config(
        SLUG,
        state,
        "http://master.test",
        secrets / "internal.token",
        secrets / "admin.token",
        secrets / "worker.token",
        secrets / "coord.key",
        OWNER.ss58,
        PARAMS,
    )


def make_client(state: Path, secrets: Path, master: Master, now: list[float]) -> TestClient:
    app = create_app(
        make_config(state, secrets),
        clock=lambda: now[0],
        transport=httpx.MockTransport(master.handler),
        verify_beacon=verify_fixture,
    )
    return TestClient(app)


@pytest.fixture
def clock() -> list[float]:
    return [1_800_000_000.0]


@pytest.fixture
def client(
    tmp_path: Path, secrets_dir: Path, master: Master, clock: list[float]
) -> Iterator[TestClient]:
    with make_client(tmp_path / "state", secrets_dir, master, clock) as c:
        yield c


class Net:
    """Drives one run through the HTTP surface; the clock is pushed drand rounds."""

    def __init__(self, client: TestClient, master: Master, n_miners: int = 3) -> None:
        self.c = client
        self.master = master
        self.coord = Keypair(COORD_SEED)
        self.auditor = Keypair(AUDITOR_SEED)
        self.manifest: RunManifest = example_manifest()
        self.run_id = self.manifest.run_id()
        self.miners = [miner(i + 1) for i in range(n_miners)]
        for m in self.miners:
            master.registered.add(m.ss58)

    def push(self, rnd: int) -> httpx.Response:
        return self.c.post("/v1/admin/beacon", json=beacon_payload(rnd), headers=bearer(ADMIN))

    def create(self) -> httpx.Response:
        env = seal(OWNER, "RunManifest", self.run_id, self.manifest, 2**40)
        return self.c.post("/v1/admin/runs", json=env, headers=bearer(ADMIN))

    def admin_put(self, path: str, body: dict[str, Any]) -> httpx.Response:
        return self.c.put(f"/v1/admin/runs/{self.run_id}{path}", json=body, headers=bearer(ADMIN))

    def start(self, bonded: tuple[bool, ...] = (True, True, False), at: int = 1000) -> None:
        assert self.push(at).status_code == 200
        assert self.create().status_code == 201
        assert self.admin_put("/config", CONFIG).status_code == 200
        for m, bond in zip(self.miners, bonded, strict=True):
            r = self.admin_put(f"/roster/{m.ss58}", {"bond": bond, "region": "eu"})
            assert r.status_code == 200
        assert self.admin_put("/paused", {"paused": False}).status_code == 200
        assert self.publish_state(0).status_code == 200

    def publish_state(self, w: int) -> httpx.Response:
        sha = self.c.app.state.store.objects.put(f"theta-start-{w}".encode())  # type: ignore[attr-defined]
        return self.c.put(
            f"/v1/aggregator/runs/{self.run_id}/rounds/{w}/state",
            json={"theta_start_sha256": sha},
            headers=bearer(ADMIN),
        )

    def preimages(self, m: Keypair, w: int) -> list[dict[str, Any]]:
        n = self.manifest.inner.H // self.manifest.inner.J + 1
        return [
            LeafPreimage(
                run_id=self.run_id,
                w=w,
                t=i * self.manifest.inner.J,
                stages=[{"theta": h(f"th{w}{m.ss58}{i}"), "m": h("m"), "v": h("v")}],
                batch_ids_sha256=h(f"b{w}{i}"),
                rng_ctr=0,
                loss_f32=f32hex(1.0),
                norm_f32=f32hex(0.5),
            ).model_dump(mode="json")
            for i in range(n)
        ]

    def leaves_root(self, m: Keypair, w: int) -> str:
        pres = [LeafPreimage.model_validate(x) for x in self.preimages(m, w)]
        return MerkleTree([bytes.fromhex(x.digest()) for x in pres]).root.hex()

    def post_leaves(self, m: Keypair, w: int) -> httpx.Response:
        return self.post_leaves_raw(m, w, self.preimages(m, w))

    def post_leaves_raw(self, m: Keypair, w: int, pres: list[dict[str, Any]]) -> httpx.Response:
        return self.post("leaves", {"w": w, "hotkey": m.ss58, "preimages": pres})

    def round(self, w: int) -> dict[str, Any]:
        r = self.c.get(f"/v1/runs/{self.run_id}/rounds/{w}")
        assert r.status_code == 200, r.text
        out: dict[str, Any] = r.json()
        return out

    def body(self, w: int) -> dict[str, Any]:
        out: dict[str, Any] = self.round(w)["round_open"]["body"]
        return out

    def signed(self, kp: Keypair, t: str, body: dict[str, Any], exp: int = 2**40) -> Any:
        return seal(kp, t, self.run_id, body, exp)

    def post(self, path: str, env: Any, **kw: Any) -> httpx.Response:
        return self.c.post(f"/v1/runs/{self.run_id}/{path}", json=env, **kw)

    def accept(self, m: Keypair, w: int) -> httpx.Response:
        view = self.round(w)
        mine = next(a for a in view["assignment"] if a["hotkey"] == m.ss58)
        body = {
            "w": w,
            "hotkey": m.ss58,
            "assignment_hash": mine["assignment_hash"],
            "image_digest": self.manifest.reference_spec.image_digest,
            "driver_version": "cpu",
            "n_gpus": 1,
        }
        return self.post("accept", self.signed(m, "Accept", body))

    def commit_body(self, m: Keypair, w: int) -> dict[str, Any]:
        return {
            "w": w,
            "hotkey": m.ss58,
            "leaf_scheme": "ht-leaf-v1",
            "n_leaves": self.manifest.inner.H // self.manifest.inner.J + 1,
            "leaves_root": self.leaves_root(m, w),
            "metrics_root": h(f"metrics{w}{m.ss58}"),
            "final_theta_hash": h(f"theta{w}{m.ss58}"),
            "ef_in_hash": h("ef-in"),
            "ef_out_hash": h("ef-out"),
            "delta_hash": h(f"delta{w}{m.ss58}"),
            "delta_bytes": 1024,
            "tokens": 240 * 256,
        }

    def commit(self, m: Keypair, w: int) -> httpx.Response:
        r = self.post("commit", self.signed(m, "Commit", self.commit_body(m, w)))
        if r.status_code == 200 and r.json()["status"] == "COMMITTED":
            assert self.post_leaves(m, w).status_code == 200
        return r

    def delta(self, m: Keypair, w: int) -> httpx.Response:
        body = {
            "w": w,
            "hotkey": m.ss58,
            "delta_hash": h(f"delta{w}{m.ss58}"),
            "uri": f"objects/{h(f'delta{w}{m.ss58}')}",
            "size": 1024,
            "format": "ht-dense-int8-v1",
            "chunks": [{"off": 0, "len": 1024, "sha256": h(f"delta{w}{m.ss58}")}],
        }
        return self.post("delta", self.signed(m, "DeltaManifest", body))

    def aggregate(self, w: int) -> httpx.Response:
        base = f"/v1/aggregator/runs/{self.run_id}/rounds/{w}"
        inputs = self.c.get(base + "/inputs", headers=bearer(ADMIN)).json()
        body = {
            **inputs["next_round_template"],
            "prev_final_hash": h(f"final{w}"),
            "theta_hash": h(f"theta{w + 1}"),
            "outer_state_hash": h(f"outer{w + 1}"),
            "center_hash": h(f"center{w + 1}"),
        }
        env = self.signed(self.coord, "RoundOpen", body)
        r = self.c.post(base + "/aggregate", json=env, headers=bearer(ADMIN))
        if r.status_code == 200:
            assert self.publish_state(w + 1).status_code == 200
        return r

    def verdict(self, job: dict[str, Any], result: str) -> httpx.Response:
        body = {
            "challenge_hash": job["challenge_hash"],
            "first_bad_leaf": None if result == "MATCH" else 1,
            "result": result,
            "recomputed_leaves_root": h("recomputed"),
            "replay_env": {
                "image_digest": self.manifest.reference_spec.image_digest,
                "driver": "cpu",
                "gpu_uuid_sha256": h("gpu"),
                "sm_count": 170,
            },
        }
        env = self.signed(self.auditor, "ReplayVerdict", body)
        return self.c.post(
            f"/v1/worker/jobs/{job['id']}/complete",
            json={"lease": job["lease"], "verdict": env},
            headers=bearer(WORKER),
        )

    def finalize(self, w: int, included: list[str]) -> httpx.Response:
        body = {
            "w": w,
            "final_theta_hash_w1": h(f"theta{w + 1}"),
            "included": included,
            "entitlements_root": h(f"ent{w}"),
        }
        env = self.signed(self.coord, "Finalize", body)
        return self.c.post(
            f"/v1/aggregator/runs/{self.run_id}/rounds/{w}/finalize",
            json=env,
            headers=bearer(ADMIN),
        )

    def weights(self, epoch: int, epoch_at: int | None = None) -> httpx.Response:
        q = f"epoch={epoch}" + (f"&epoch_at={epoch_at}" if epoch_at is not None else "")
        return self.c.get(
            f"/internal/v1/get_weights?{q}",
            headers={**bearer(INTERNAL), "x-platform-challenge-slug": SLUG},
        )


@pytest.fixture
def net(client: TestClient, master: Master) -> Net:
    return Net(client, master)
