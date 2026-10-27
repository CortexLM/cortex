"""Fixtures of the audit-cheaters suite: one graduated snapshot per module, one clone per test.

Graduating five identities costs about three minutes, so it happens once per shape: the
snapshot lives under pytest's tmp_path_factory, or under $AUDIT_CHEATERS_SNAPSHOT_DIR when set
(a later runner can then pay the graduation once for many processes; delete the directory after
any change of the fixture network, a stale snapshot is refused by run_id).
$AUDIT_CHEATERS_DEVICE other than "cpu" skips the whole suite.

A module that needs another shape overrides `ac_shape`:

    @pytest.fixture(scope="module")
    def ac_shape():
        return Shape(identities=8, graduated=5)
"""

from __future__ import annotations

import os
from collections.abc import Iterator
from pathlib import Path

import pytest


@pytest.fixture(scope="session")
def ac_device() -> str:
    from ac_world import require_cpu

    return require_cpu(os.environ.get("AUDIT_CHEATERS_DEVICE", "cpu"))


@pytest.fixture(scope="session")
def ac_snapshot_root(tmp_path_factory: pytest.TempPathFactory) -> Path:
    override = os.environ.get("AUDIT_CHEATERS_SNAPSHOT_DIR")
    if override:
        return Path(override).expanduser().resolve()
    return tmp_path_factory.mktemp("ac-snapshots")


@pytest.fixture(scope="module")
def ac_shape():
    from ac_world import Shape

    return Shape()


@pytest.fixture(scope="module")
def ac_snapshot(
    ac_snapshot_root: Path, ac_shape, ac_device: str, tmp_path_factory: pytest.TempPathFactory
) -> Iterator:
    from ac_world import ensure_snapshot

    snapshot = ensure_snapshot(
        ac_snapshot_root, ac_shape, tmp_path_factory.mktemp("ac-origin"), ac_device
    )
    try:
        yield snapshot
    finally:
        snapshot.close()


@pytest.fixture
def ac_clone(ac_snapshot, tmp_path: Path) -> Iterator:
    """A World restored from the graduated snapshot and installed (clock, in-process relay)."""
    world = ac_snapshot.clone(tmp_path)
    try:
        with world:
            yield world
    finally:
        world.close()
