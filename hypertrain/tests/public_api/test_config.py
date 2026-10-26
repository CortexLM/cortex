"""RunConfig.total_rounds and the /public mount, through the real challenge app."""

from __future__ import annotations

import json
import sqlite3

from fastapi.testclient import TestClient

from challenge.conftest import CONFIG, Net
from challenge.conftest import client as client
from challenge.conftest import clock as clock
from challenge.conftest import master as master
from challenge.conftest import secrets_dir as secrets_dir

from .frontend_validators import is_snapshot


def test_old_config_stored_byte_identically(client: TestClient, master) -> None:  # type: ignore[no-untyped-def]
    net = Net(client, master)
    net.start()
    db = sqlite3.connect(client.app.state.store._db.execute("PRAGMA database_list").fetchone()[2])  # type: ignore[attr-defined]
    stored = db.execute("SELECT config FROM runs").fetchone()[0]
    assert stored == json.dumps(CONFIG, sort_keys=True, separators=(",", ":"))
    assert "total_rounds" not in stored


def test_total_rounds_accepted_and_bounded(client: TestClient, master) -> None:  # type: ignore[no-untyped-def]
    net = Net(client, master)
    net.start()
    net.admin_put("/paused", {"paused": True})
    assert (
        net.admin_put("/config", {**CONFIG, "total_rounds": 7}).json()["config"]["total_rounds"]
        == 7
    )
    assert net.admin_put("/config", {**CONFIG, "total_rounds": 0}).status_code == 422
    assert net.admin_put("/config", {**CONFIG, "total_rounds": 1.5}).status_code == 422
    assert net.admin_put("/config", {**CONFIG, "total_rounds": None}).json()["config"] == CONFIG


def test_public_mount_serves_live_run(client: TestClient, master) -> None:  # type: ignore[no-untyped-def]
    net = Net(client, master)
    net.start()
    runs = client.get("/public/v1/runs")
    assert runs.status_code == 200 and [r["id"] for r in runs.json()] == [net.run_id]
    snap = client.get(f"/public/v1/runs/{net.run_id}").json()
    assert is_snapshot(snap)
    assert snap["run"]["totalRounds"] == 1 and snap["run"]["status"] == "running"
    assert client.get("/v1/runs").status_code in (401, 403, 404, 405, 200)
