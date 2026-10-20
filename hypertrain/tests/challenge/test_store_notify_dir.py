"""Shared v2 notification directories: safe open/close, and the test session's leak cleanup."""

from __future__ import annotations

import shutil
import socket
import tempfile
import threading
import time
from collections.abc import Iterator
from pathlib import Path

import pytest

from conftest import remove_new_empty_notify_dirs
from hypertrain.challenge.store import ChallengeStore
from hypertrain.data.store import LocalFSStore

from .conftest import PARAMS


@pytest.fixture
def root(monkeypatch: pytest.MonkeyPatch) -> Iterator[Path]:
    # A private temp root, short enough for AF_UNIX paths (tmp_path is not), so cleanup is
    # observed without touching other sessions' directories in the real one.
    private = Path(tempfile.mkdtemp(prefix="htnd-"))
    monkeypatch.setattr(tempfile, "tempdir", str(private))
    yield private
    shutil.rmtree(private)


def _store(state: Path, objects: Path) -> ChallengeStore:
    return ChallengeStore(state, PARAMS, None, None, lambda _: None, LocalFSStore(objects))


def _started(state: Path, objects: Path) -> ChallengeStore:
    store = _store(state, objects)
    store._start_notifications_v2()
    return store


def _directory(store: ChallengeStore) -> Path:
    assert store._ipc_path_v2 is not None
    return store._ipc_path_v2.parent


def _snapshot(root: Path) -> set[str]:
    return {path.name for path in root.glob("ht-notify-*")}


def test_session_cleanup_removes_directory_left_by_closed_store(root: Path, tmp_path: Path) -> None:
    before = _snapshot(root)
    store = _started(tmp_path / "state", tmp_path / "objects")
    directory = _directory(store)
    assert directory.parent == root
    store.close_v2_notifications()
    store._db.close()
    assert directory.is_dir()  # production keeps the empty directory
    remove_new_empty_notify_dirs(root, before)
    assert not directory.exists()


def test_session_cleanup_keeps_directory_with_live_socket(root: Path, tmp_path: Path) -> None:
    before = _snapshot(root)
    first, second = (_started(tmp_path / "state", tmp_path / f"objects-{i}") for i in range(2))
    first.close_v2_notifications()
    remove_new_empty_notify_dirs(root, before)
    assert second._ipc_path_v2 is not None and second._ipc_path_v2.is_socket()
    second.close_v2_notifications()
    for store in (first, second):
        store._db.close()


def test_session_cleanup_keeps_directory_that_predates_snapshot(root: Path) -> None:
    existing = root / "ht-notify-preexisting"
    existing.mkdir(mode=0o700)
    remove_new_empty_notify_dirs(root, _snapshot(root))
    assert existing.is_dir()


def test_shared_directory_survives_concurrent_start_close(
    root: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # Regression: removing the shared directory on close raced a peer's mkdir/bind
    # (FileNotFoundError at bind, FileExistsError at mkdir).
    class SlowBind(socket.socket):
        def bind(self, address: object) -> None:
            time.sleep(0.001)  # widen the mkdir-to-bind window a peer's close could hit
            super().bind(address)  # type: ignore[arg-type]

    monkeypatch.setattr(socket, "socket", SlowBind)
    threads, cycles = 8, 100
    stores = [_store(tmp_path / "state", tmp_path / f"objects-{i}") for i in range(threads)]
    gate = threading.Barrier(threads)
    errors: list[OSError] = []

    def cycle(store: ChallengeStore) -> None:
        gate.wait()
        for _ in range(cycles):
            try:
                store._start_notifications_v2()
                store.close_v2_notifications()
            except OSError as error:  # keep cycling: one lost race must not end the probe
                errors.append(error)

    workers = [threading.Thread(target=cycle, args=(store,)) for store in stores]
    for worker in workers:
        worker.start()
    for worker in workers:
        worker.join()
    for store in stores:
        store.close_v2_notifications()
        store._db.close()
    assert errors == []
