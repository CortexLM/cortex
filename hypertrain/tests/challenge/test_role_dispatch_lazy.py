"""The supervisor canary boots the image with an empty /data: role dispatch loads on first use."""

from __future__ import annotations

import json
import sys
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from types import SimpleNamespace

import pytest
from fastapi.testclient import TestClient

from hypertrain.challenge import app as app_module
from hypertrain.challenge.store import ChallengeError
from hypertrain.gpu_ops import role_dispatch

from .conftest import OWNER, make_config

TREE = Path(__file__).parents[2]


def _app(tmp_path: Path, dispatch: Path) -> app_module.FastAPI:
    # Canary: empty state dir, no secret files at all.
    config = make_config(tmp_path / "state", tmp_path / "no-secrets")
    config.role_dispatch_file = dispatch
    return app_module.create_app(config)


def _write_valid(path: Path, tmp_path: Path) -> None:
    path.write_text(
        json.dumps(
            {
                "tree": str(TREE),
                "run_dir": str(tmp_path / "lifecycle"),
                "owner_key_file": str(tmp_path / "owner"),
                "roles": {OWNER.ss58: "h1"},
                "run_id": "a" * 64,
            }
        )
    )


def _foreign(launch: role_dispatch.RoleLaunch, tmp_path: Path) -> None:
    # Foreign run: the real factory refuses before any I/O, proving the file was loaded.
    with pytest.raises(RuntimeError, match="not the authorized run"):
        launch("audit", SimpleNamespace(run_id="b" * 64), tmp_path, {})  # type: ignore[arg-type]


def test_boots_and_answers_version_without_dispatch_file(tmp_path: Path) -> None:
    with TestClient(_app(tmp_path, tmp_path / "missing" / "dispatch.json")) as client:
        response = client.get("/version")
    assert response.status_code == 200
    assert response.json()["slug"] == "hypertrain"


def test_missing_then_fixed_file_without_restart(tmp_path: Path) -> None:
    dispatch = tmp_path / "dispatch.json"
    launch = _app(tmp_path, dispatch).state.store._role_launch
    assert launch is not None
    for broken in (None, "{not json", json.dumps({"tree": str(TREE), "extra": 1})):
        if broken is not None:
            dispatch.write_text(broken)
        with pytest.raises(ChallengeError) as error:
            launch("audit", SimpleNamespace(run_id="a" * 64), tmp_path, {})
        assert (error.value.status, error.value.detail) == (503, "ROLE_DISPATCH_UNAVAILABLE")
    _write_valid(dispatch, tmp_path)
    _foreign(launch, tmp_path)


def test_valid_file_loaded_once_same_behaviour(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    dispatch = tmp_path / "dispatch.json"
    _write_valid(dispatch, tmp_path)
    calls: list[Path] = []
    real = role_dispatch.load

    def spy(path: Path) -> role_dispatch.RoleLaunch:
        calls.append(path)
        return real(path)

    monkeypatch.setattr(role_dispatch, "load", spy)
    monkeypatch.setattr(sys, "path", [p for p in sys.path if p != str(TREE / "scripts")])
    launch = _app(tmp_path, dispatch).state.store._role_launch
    assert launch is not None
    _foreign(launch, tmp_path)
    assert sys.path[0] == str(TREE / "scripts")
    _foreign(launch, tmp_path)
    _foreign(real(dispatch), tmp_path)  # same refusal as the eager loader
    assert calls == [dispatch]


def test_concurrent_first_calls_load_once(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    dispatch = tmp_path / "dispatch.json"
    _write_valid(dispatch, tmp_path)
    calls: list[Path] = []
    real = role_dispatch.load

    def slow(path: Path) -> role_dispatch.RoleLaunch:
        calls.append(path)
        time.sleep(0.05)  # widen the window: without the lock every thread loads
        return real(path)

    monkeypatch.setattr(role_dispatch, "load", slow)
    launch = _app(tmp_path, dispatch).state.store._role_launch
    assert launch is not None
    with ThreadPoolExecutor(16) as pool:
        for future in [pool.submit(_foreign, launch, tmp_path) for _ in range(16)]:
            future.result()
    assert len(calls) == 1


def test_test_run_dispatch_file_authorizes_only_its_test_run(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # Given: the TEST run file from docs/role-dispatch.md, loaded the way the challenge loads it.
    monkeypatch.setattr(sys, "path", list(sys.path))
    launch = _app(tmp_path, TREE / "docs/examples/dispatch.test.json").state.store._role_launch
    assert launch is not None
    # When / Then: the production-challenge run and the stuck TEST run are refused before any I/O.
    for foreign in (
        "7753ff754eea7867bcaf1dcc92f5064e99f4adaeef8f5ec1d5a86acf7b43ca0f",
        "b14a975efb7ad8e1ee3e1565b48f741b3d2a93fd1c9c20b069dde6c9d2272375",
    ):
        with pytest.raises(RuntimeError, match="not the authorized run"):
            launch("audit", SimpleNamespace(run_id=foreign), tmp_path, {})
    # The TEST run passes the binding, then stops at its unresolved host paths: not installable
    # until the placeholders name a real tree, lifecycle run dir and owner seed.
    with pytest.raises(OSError):
        launch(
            "audit",
            SimpleNamespace(
                run_id="16bc75dd475eaf7a6bcd85d8dbe89b9f57b31c8d080f252c6478ac52fcac3602"
            ),
            tmp_path,
            {},
        )


def test_unexpected_load_error_propagates(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    def broken(path: Path) -> role_dispatch.RoleLaunch:
        raise RuntimeError("not a missing-file problem")

    monkeypatch.setattr(role_dispatch, "load", broken)
    launch = _app(tmp_path, tmp_path / "dispatch.json").state.store._role_launch
    assert launch is not None
    with pytest.raises(RuntimeError, match="not a missing-file problem"):
        launch("audit", SimpleNamespace(run_id="a" * 64), tmp_path, {})
