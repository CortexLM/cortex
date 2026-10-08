from __future__ import annotations

import json
import time
from pathlib import Path

import httpx
import pytest
from fastapi.testclient import TestClient

from hypertrain.challenge.app import Config, create_app
from hypertrain.ledger import Ledger, Params
from hypertrain.protocol.messages import QUICKNET_GENESIS

from .conftest import ADMIN, INTERNAL, PARAMS, SLUG, WORKER, Master, Net, bearer, make_client


def weights(client: TestClient, epoch: int, token: str | None = INTERNAL, slug: str | None = SLUG):
    headers = {"x-platform-challenge-slug": slug} if slug else {}
    if token:
        headers.update(bearer(token))
    return client.get(f"/internal/v1/get_weights?epoch={epoch}", headers=headers)


def test_version_and_health(client: TestClient) -> None:
    assert client.get("/version").json() == {
        "slug": SLUG,
        "version": "0.1.0",
        "contract": 1,
        "capabilities": ["get_weights", "proxy_routes"],
    }
    assert client.get("/health").status_code == 200


def test_get_weights_auth(client: TestClient) -> None:
    assert weights(client, 1, token=None).status_code == 401
    assert weights(client, 1, token="wrong").status_code == 401
    assert weights(client, 1, token=ADMIN).status_code == 401
    assert weights(client, 1, slug=None).status_code == 403
    assert weights(client, 1, slug="opentype").status_code == 403
    raw = client.get("/internal/v1/get_weights?epoch=1", headers={"authorization": INTERNAL})
    assert raw.status_code == 401
    assert weights(client, -1).status_code == 422


def test_get_weights_first_answer_is_immutable(client: TestClient, clock: list[float]) -> None:
    first = weights(client, 7)
    assert first.status_code == 200
    body = first.json()
    assert body["challenge_slug"] == SLUG and body["epoch"] == 7
    assert body["weights"] == {} and body["full_share_mass"] == 1_000_000
    clock[0] += 3600
    assert weights(client, 7).content == first.content
    assert weights(client, 8).json()["computed_at"] != body["computed_at"]


def test_canary_without_secrets(tmp_path: Path, master: Master, clock: list[float]) -> None:
    missing = tmp_path / "none"
    with make_client(tmp_path / "state", missing, master, clock) as canary:
        assert canary.get("/version").json()["contract"] == 1
        assert canary.get("/health").status_code == 503
        assert weights(canary, 1).status_code == 503
        assert canary.post("/v1/worker/lease", headers=bearer(WORKER)).status_code == 503
        assert canary.post("/v1/admin/beacon", headers=bearer(ADMIN), json={}).status_code == 503
        assert canary.get("/v1/runs").status_code == 200


def test_health_needs_coord_key(tmp_path: Path, secrets_dir: Path, master: Master, clock) -> None:
    (secrets_dir / "coord.key").unlink()
    with make_client(tmp_path / "state", secrets_dir, master, clock) as c:
        assert c.get("/health").status_code == 503


def test_token_files_are_read_per_request(client: TestClient, secrets_dir: Path) -> None:
    (secrets_dir / "internal.token").write_text("rotated\n")
    assert weights(client, 1).status_code == 401
    assert weights(client, 1, token="rotated").status_code == 200
    (secrets_dir / "internal.token").unlink()
    assert weights(client, 2, token="rotated").status_code == 503
    assert client.get("/health").status_code == 503


@pytest.mark.parametrize(
    ("method", "path"),
    [
        ("post", "/v1/worker/lease"),
        ("post", "/v1/worker/jobs/a_x/heartbeat"),
        ("post", "/v1/worker/jobs/a_x/complete"),
        ("post", "/v1/worker/jobs/a_x/fail"),
        ("post", "/v1/runs/" + "0" * 64 + "/resolution"),
        ("post", "/v1/admin/beacon"),
        ("post", "/v1/admin/runs"),
        ("put", "/v1/admin/runs/r/config"),
        ("put", "/v1/admin/runs/r/paused"),
        ("put", "/v1/admin/runs/r/honeypot"),
        ("get", "/v1/aggregator/runs/r/rounds/0/inputs"),
        ("post", "/v1/aggregator/runs/r/rounds/0/aggregate"),
        ("post", "/v1/aggregator/runs/r/rounds/0/finalize"),
    ],
)
def test_guarded_routes_need_their_own_bearer(client: TestClient, method: str, path: str) -> None:
    for token in (None, INTERNAL, "nope"):
        headers = bearer(token) if token else {}
        assert getattr(client, method)(path, headers=headers).status_code == 401
    worker_route = path.startswith("/v1/worker") or path.endswith("/resolution")
    other = ADMIN if worker_route else WORKER
    assert getattr(client, method)(path, headers=bearer(other)).status_code == 401


def test_oversized_body_is_413(net: Net) -> None:
    big = json.dumps({"pad": "x" * (1024 * 1024)})
    r = net.c.post(
        f"/v1/runs/{net.run_id}/commit", content=big, headers={"content-type": "application/json"}
    )
    assert r.status_code == 413


def test_duplicate_json_keys_and_nan_are_rejected(net: Net) -> None:
    net.start()
    env = net.signed(net.miners[0], "Commit", net.commit_body(net.miners[0], 0))
    text = json.dumps(env)
    dup = text[:-1] + ', "sig": "' + "0" * 128 + '"}'
    r = net.c.post(f"/v1/runs/{net.run_id}/commit", content=dup)
    assert r.status_code == 400 and "duplicate" in r.json()["detail"]
    r = net.c.post(f"/v1/runs/{net.run_id}/commit", content=b'{"a": NaN}')
    assert r.status_code == 400
    assert net.c.post(f"/v1/runs/{net.run_id}/commit", content=b"{not json").status_code == 400


def test_get_weights_is_200_empty_during_audit(net: Net) -> None:
    net.start()
    net.push(1021)
    for m in net.miners:
        assert net.accept(m, 0).status_code == 200
    net.push(1025)
    for m in net.miners:
        assert net.commit(m, 0).json()["status"] == "COMMITTED"
        assert net.delta(m, 0).status_code == 200
    net.push(1071)
    net.push(1331)
    assert net.aggregate(0).status_code == 200
    body1 = net.body(1)
    net.push(body1["d_assign"])
    assert net.round(0)["state"] == "AUDIT"
    r = net.weights(0, QUICKNET_GENESIS + 4000)
    assert r.status_code == 200
    assert r.json()["weights"] == {} and r.json()["metadata"]["units_burned_this_epoch"] == 10**6


def test_get_weights_p99_under_2s_with_65536_hotkeys(
    tmp_path: Path, secrets_dir: Path, master: Master, clock: list[float]
) -> None:
    n = 65_536
    state = tmp_path / "state"
    ledger = Ledger(state / "ledger", PARAMS)
    at = QUICKNET_GENESIS + 10
    keys = [f"hk{i:06d}" for i in range(n)]
    for k in keys:
        ledger.commit(0, k, at)
    for k in keys:
        ledger.verdict(0, k, "UNSAMPLED", 1000, 0, at)
    ledger.finalize(0, at)
    del ledger
    vested_at = QUICKNET_GENESIS + 11 * PARAMS.epoch_seconds
    with make_client(state, secrets_dir, master, clock) as c:
        samples: list[float] = []
        bodies: dict[int, bytes] = {}
        for i in range(40):
            epoch = 11 + i % 4
            t0 = time.perf_counter()
            r = c.get(
                f"/internal/v1/get_weights?epoch={epoch}&epoch_at={vested_at + epoch}",
                headers={**bearer(INTERNAL), "x-platform-challenge-slug": SLUG},
            )
            samples.append(time.perf_counter() - t0)
            assert r.status_code == 200
            assert bodies.setdefault(epoch, r.content) == r.content
        paid = json.loads(bodies[11])["weights"]
        assert len(paid) == n and set(paid.values()) == {float(10**6 // n)}
        samples.sort()
        p99 = samples[int(0.99 * (len(samples) - 1))]
        assert p99 < 2.0, f"p99 {p99:.3f}s"


def test_config_from_env(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    monkeypatch.setenv("CHALLENGE_STATE_DIR", str(tmp_path))
    monkeypatch.setenv("HYPERTRAIN_EPOCH_SECONDS", "300")
    cfg = Config.from_env()
    assert cfg.slug == "hypertrain" and cfg.state_dir == tmp_path
    assert cfg.params == Params("hypertrain", QUICKNET_GENESIS, 300, 1, 10)
    app = create_app(cfg, transport=httpx.MockTransport(lambda r: httpx.Response(500)))
    with TestClient(app) as c:
        assert c.get("/health").status_code == 503
