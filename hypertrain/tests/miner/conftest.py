from __future__ import annotations

import sys
from collections.abc import Iterator
from pathlib import Path
from typing import Any

import pytest

import hypertrain.trainer  # noqa: F401

sys.path.insert(0, str(Path(__file__).resolve().parent))
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from miner_harness import World  # noqa: E402

from challenge.conftest import Master, make_client  # noqa: E402


@pytest.fixture
def world_factory(tmp_path: Path) -> Iterator[Any]:
    clients: list[Any] = []

    def make(
        policy: str = "reset", n: int = 1, bad_init: bool = False, k_segments: int | None = None
    ) -> World:
        master = Master()
        secrets = tmp_path / "secrets"
        if not secrets.exists():
            secrets.mkdir()
            from challenge.conftest import ADMIN, COORD_SEED, INTERNAL, WORKER

            (secrets / "internal.token").write_text(INTERNAL)
            (secrets / "admin.token").write_text(ADMIN)
            (secrets / "worker.token").write_text(WORKER)
            (secrets / "coord.key").write_text(COORD_SEED.hex())
        c = make_client(tmp_path / "state", secrets, master, [1_800_000_000.0]).__enter__()
        clients.append(c)
        return World(tmp_path, c, master, policy, n, bad_init, k_segments)

    yield make
    for c in clients:
        c.__exit__(None, None, None)
