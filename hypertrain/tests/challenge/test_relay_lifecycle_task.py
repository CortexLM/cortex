"""The master starts its unattended relay lifecycle task in production and nowhere else."""

from __future__ import annotations

import dataclasses
from pathlib import Path

import httpx
import pytest
from fastapi.testclient import TestClient

from hypertrain.challenge.app import Config, create_app

from .conftest import Master, make_config, verify_fixture


def served(tmp_path: Path, secrets_dir: Path, master: Master, interval: float | None):
    config = make_config(tmp_path / "state", secrets_dir)
    if interval is not None:
        config = dataclasses.replace(config, relay_lifecycle_interval=interval)
    return create_app(
        config,
        clock=lambda: 1_800_000_000.0,
        transport=httpx.MockTransport(master.handler),
        verify_beacon=verify_fixture,
    )


def test_startup_runs_the_lifecycle_task_and_shutdown_stops_it(
    tmp_path: Path, secrets_dir: Path, master: Master
) -> None:
    app = served(tmp_path, secrets_dir, master, 3600.0)
    with TestClient(app):
        task = app.state.relay_lifecycle
        assert not task.done()
    assert task.cancelled()


def test_startup_runs_no_lifecycle_task_for_a_hand_built_config(
    tmp_path: Path, secrets_dir: Path, master: Master
) -> None:
    app = served(tmp_path, secrets_dir, master, None)
    with TestClient(app):
        assert not hasattr(app.state, "relay_lifecycle")


def test_production_config_runs_the_lifecycle_every_two_minutes_unless_told_otherwise(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.delenv("HYPERTRAIN_RELAY_LIFECYCLE_INTERVAL", raising=False)
    assert Config.from_env().relay_lifecycle_interval == 120.0
    monkeypatch.setenv("HYPERTRAIN_RELAY_LIFECYCLE_INTERVAL", "0")
    assert Config.from_env().relay_lifecycle_interval == 0.0
