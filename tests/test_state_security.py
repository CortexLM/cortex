"""Filesystem trust-boundary tests for persistent master state."""

import os
import stat

import pytest

from cortex.gateway import GatewayStore
from cortex.proof.store import ProofStore
from cortex.state import (
    StateSecurityError,
    prepare_master_state,
    secure_sqlite_path,
    secure_state_directory,
)


def mode(path) -> int:
    return stat.S_IMODE(path.stat().st_mode)


def test_prepare_master_state_creates_private_directory_and_databases(tmp_path):
    state = prepare_master_state(tmp_path / "state")

    assert mode(state) == 0o700
    assert {path.name for path in state.iterdir()} == {
        "gateway.sqlite3",
        "proof.sqlite3",
        "emission.sqlite3",
    }
    assert all(path.is_file() and mode(path) == 0o600 for path in state.iterdir())


@pytest.mark.parametrize("attack", ["symlink", "permissive"])
def test_state_directory_refuses_unsafe_existing_path(tmp_path, attack):
    state = tmp_path / "state"
    if attack == "symlink":
        target = tmp_path / "target"
        target.mkdir(mode=0o700)
        state.symlink_to(target, target_is_directory=True)
    else:
        state.mkdir(mode=0o700)
        state.chmod(0o750)

    with pytest.raises(StateSecurityError):
        secure_state_directory(state)


@pytest.mark.parametrize("attack", ["symlink", "permissive", "fifo", "hardlink"])
def test_sqlite_path_refuses_filesystem_aliases_and_special_files(tmp_path, attack):
    database = tmp_path / "state.sqlite3"
    target = tmp_path / "target.sqlite3"
    target.touch(mode=0o600)
    if attack == "symlink":
        database.symlink_to(target)
    elif attack == "permissive":
        database.touch(mode=0o600)
        database.chmod(0o644)
    elif attack == "fifo":
        os.mkfifo(database, mode=0o600)
    else:
        os.link(target, database)

    with pytest.raises(StateSecurityError):
        secure_sqlite_path(database)


def test_sqlite_path_refuses_a_group_writable_parent(tmp_path):
    state = tmp_path / "state"
    state.mkdir(mode=0o700)
    state.chmod(0o770)

    with pytest.raises(StateSecurityError):
        secure_sqlite_path(state / "database.sqlite3")


@pytest.mark.parametrize("store_type", [ProofStore, GatewayStore])
def test_stores_create_private_regular_databases(tmp_path, store_type):
    path = tmp_path / f"{store_type.__name__}.sqlite3"

    store = store_type(path)
    store.close()

    assert path.is_file()
    assert mode(path) == 0o600


@pytest.mark.parametrize("attack", ["symlink", "permissive", "hardlink"])
def test_gateway_store_refuses_unsafe_existing_database(tmp_path, attack):
    database = tmp_path / "gateway.sqlite3"
    target = tmp_path / "target.sqlite3"
    target.touch(mode=0o600)
    if attack == "symlink":
        database.symlink_to(target)
    elif attack == "permissive":
        database.touch(mode=0o600)
        database.chmod(0o644)
    else:
        os.link(target, database)

    with pytest.raises(StateSecurityError):
        GatewayStore(database)


def test_in_memory_database_remains_supported():
    assert secure_sqlite_path(":memory:") == ":memory:"
