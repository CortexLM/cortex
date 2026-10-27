"""Fixtures of the audit-cheaters suite: one graduated snapshot per module, one clone per test.

Graduating five identities costs about three minutes, so it happens once per shape: the
snapshot lives under pytest's tmp_path_factory, or under $AUDIT_CHEATERS_SNAPSHOT_DIR when set
(a later runner can then pay the graduation once for many processes; delete the directory after
any change of the fixture network, a stale snapshot is refused by run_id).
$AUDIT_CHEATERS_DEVICE other than "cpu" skips every test that builds a graduated network.

The service-level scenarios (every test that restores a graduated clone: full rounds with five
miners, about 25 minutes for the directory) are opt-in: they skip unless $AUDIT_CHEATERS_FULL=1,
like the GPU tests. The audit-level, relay, admission and ledger tests always run.

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

HERE = Path(__file__).resolve().parent
FULL = "AUDIT_CHEATERS_FULL"
SERVICE_LEVEL = frozenset({"ac_snapshot", "ac_clone"})


def pytest_collection_modifyitems(items: list[pytest.Item]) -> None:
    """Skip the service-level scenarios of this directory unless AUDIT_CHEATERS_FULL=1."""
    if os.environ.get(FULL) == "1":
        return
    skip = pytest.mark.skip(
        reason=f"service-level scenarios are opt-in: set {FULL}=1 (about 25 minutes, CPU)"
    )
    for item in items:
        fixtures = frozenset(getattr(item, "fixturenames", ()))
        if HERE in Path(str(item.fspath)).resolve().parents and fixtures & SERVICE_LEVEL:
            item.add_marker(skip)


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
