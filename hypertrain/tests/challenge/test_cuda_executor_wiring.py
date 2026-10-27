"""The challenge selects the generic CUDA executor with HYPERTRAIN_CUDA_EXECUTOR (ADR 0008): lazy
like the Vast dispatch file, mutually exclusive with it, inherited by the child stores."""

from __future__ import annotations

import json
import shutil
from collections.abc import Callable
from pathlib import Path
from types import SimpleNamespace
from typing import Any

import pytest

from hypertrain.challenge import app as app_module
from hypertrain.challenge.store import ChallengeError
from hypertrain.gpu_ops import cuda_executor, role_dispatch

from .conftest import make_config

RUN = "a" * 64


@pytest.fixture(autouse=True)
def _no_ssh_client_needed(monkeypatch: pytest.MonkeyPatch) -> None:
    """Executor() looks for an ssh binary but runs none here: do not depend on the CI image."""
    which = shutil.which

    def fake_which(cmd: str, *args: Any, **kwargs: Any) -> str | None:
        return "/usr/bin/ssh" if cmd == "ssh" else which(cmd, *args, **kwargs)

    monkeypatch.setattr(cuda_executor.shutil, "which", fake_which)


def executor_config(tmp_path: Path, **over: Any) -> dict[str, Any]:
    key, known = tmp_path / "id_ed25519", tmp_path / "known_hosts"
    key.write_text("operator-provided private key lives here")
    key.chmod(0o600)
    known.write_text("gpu-1.example.com ssh-ed25519 AAAA\n")
    return {
        "run_id": RUN,
        "image_repository": "registry.test/hypertrain-gpu",
        "state_dir": str(tmp_path / "executor-state"),
        "hosts": [
            {
                "label": "h1",
                "address": "gpu-1.example.com",
                "user": "ubuntu",
                "identity_file": str(key),
                "known_hosts_file": str(known),
            }
        ],
        **over,
    }


def write_valid(path: Path, tmp_path: Path) -> None:
    path.write_text(json.dumps(executor_config(tmp_path)))


def make_app(tmp_path: Path, **files: Path | None) -> Any:
    # Canary: empty state dir, no secret files at all.
    config = make_config(tmp_path / "state", tmp_path / "no-secrets")
    config.cuda_executor_file = files.get("executor")
    config.role_dispatch_file = files.get("dispatch")
    return app_module.create_app(config)


def served(launch: Callable[..., Any], tmp_path: Path) -> None:
    # The real factory of the executor answers: it was loaded from the file. An operation outside
    # reference/audit/referee is refused before any host is contacted.
    with pytest.raises(RuntimeError, match="does not serve 'probe'"):
        launch("probe", SimpleNamespace(run_id=RUN), tmp_path, {})


def test_env_selects_the_executor_and_children_inherit_it(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    file = tmp_path / "executor.json"
    monkeypatch.setenv("HYPERTRAIN_CUDA_EXECUTOR", str(file))
    monkeypatch.setenv("CHALLENGE_STATE_DIR", str(tmp_path / "state"))
    parsed = app_module.Config.from_env()
    assert (parsed.cuda_executor_file, parsed.role_dispatch_file) == (file, None)
    store = app_module.create_app(parsed).state.store
    assert store._role_launch is not None
    assert store._open_child(tmp_path / "child")._role_launch is store._role_launch
    monkeypatch.delenv("HYPERTRAIN_CUDA_EXECUTOR")
    assert app_module.create_app(app_module.Config.from_env()).state.store._role_launch is None


@pytest.mark.parametrize("selected", ["vast", "executor"])
def test_each_dispatch_file_is_loaded_by_its_own_loader_and_never_by_the_other(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, selected: str
) -> None:
    file = tmp_path / "dispatch.json"
    served, calls = object(), []

    def loader(name: str) -> Callable[[Path], Callable[..., Any]]:
        def load(path: Path) -> Callable[..., Any]:
            calls.append((name, path))
            return lambda *args: served

        return load

    monkeypatch.setattr(role_dispatch, "load", loader("vast"))
    monkeypatch.setattr(cuda_executor, "load", loader("executor"))
    launch = make_app(
        tmp_path, **{"dispatch" if selected == "vast" else "executor": file}
    ).state.store._role_launch
    assert launch is not None and calls == []  # nothing is loaded at start-up
    assert launch("audit", SimpleNamespace(run_id=RUN), tmp_path, {}) is served
    assert launch("audit", SimpleNamespace(run_id=RUN), tmp_path, {}) is served
    assert calls == [(selected, file)]  # read on the first use only: its host locks are shared


def test_both_dispatch_targets_set_is_refused_at_start_naming_both(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("HYPERTRAIN_ROLE_DISPATCH", str(tmp_path / "dispatch.json"))
    monkeypatch.setenv("HYPERTRAIN_CUDA_EXECUTOR", str(tmp_path / "executor.json"))
    monkeypatch.setenv("CHALLENGE_STATE_DIR", str(tmp_path / "state"))
    with pytest.raises(ValueError, match="HYPERTRAIN_ROLE_DISPATCH.*HYPERTRAIN_CUDA_EXECUTOR"):
        app_module.create_app(app_module.Config.from_env())


def _drop_the_file(path: Path, tmp_path: Path) -> None:
    path.unlink(missing_ok=True)


def _not_json(path: Path, tmp_path: Path) -> None:
    path.write_text("{not json")


def _extra_field(path: Path, tmp_path: Path) -> None:
    path.write_text(json.dumps(executor_config(tmp_path, **{"pass" + "word": "x"})))


def _key_file_gone(path: Path, tmp_path: Path) -> None:
    path.write_text(json.dumps(executor_config(tmp_path)))
    (tmp_path / "id_ed25519").unlink()


def _key_file_readable_by_others(path: Path, tmp_path: Path) -> None:
    path.write_text(json.dumps(executor_config(tmp_path)))
    (tmp_path / "id_ed25519").chmod(0o644)


@pytest.mark.parametrize(
    "break_it",
    [_drop_the_file, _not_json, _extra_field, _key_file_gone, _key_file_readable_by_others],
)
def test_an_unusable_file_answers_503_and_a_fixed_one_is_picked_up_without_restart(
    tmp_path: Path, break_it: Callable[[Path, Path], None]
) -> None:
    file = tmp_path / "executor.json"
    launch = make_app(tmp_path, executor=file).state.store._role_launch
    assert launch is not None
    break_it(file, tmp_path)
    for _ in range(2):  # a failed load is not cached
        with pytest.raises(ChallengeError) as error:
            launch("audit", SimpleNamespace(run_id=RUN), tmp_path, {})
        assert (error.value.status, error.value.detail) == (503, "ROLE_DISPATCH_UNAVAILABLE")
    write_valid(file, tmp_path)
    served(launch, tmp_path)


def test_the_loaded_executor_serves_only_its_authorized_run(tmp_path: Path) -> None:
    file = tmp_path / "executor.json"
    write_valid(file, tmp_path)
    launch = make_app(tmp_path, executor=file).state.store._role_launch
    assert launch is not None
    foreign = SimpleNamespace(run_id="b" * 64)
    dispatched = launch("audit", foreign, tmp_path, {})
    with pytest.raises(RuntimeError, match="not the authorized run"):
        dispatched(foreign, tmp_path, backend="cuda")
