"""The VM job ledger uses the same filesystem trust boundary as master state."""

import os
import stat

import pytest

from cortex.state import StateSecurityError
from cortex.vm.models import VmSpec
from cortex.vm.runtime import Orchestrator

from .test_runtime import FakeHypervisor


@pytest.mark.parametrize("attack", ["hardlink", "permissions"])
async def test_vm_ledger_refuses_unsafe_existing_database(tmp_path, attack):
    database = tmp_path / "jobs.sqlite3"
    target = tmp_path / "other.sqlite3"
    target.touch(mode=0o600)
    if attack == "hardlink":
        os.link(target, database)
    else:
        database.touch(mode=0o600)
        database.chmod(0o640)
    runtime = None

    try:
        with pytest.raises((StateSecurityError, ValueError)):
            runtime = Orchestrator(database, FakeHypervisor())
        assert target.read_bytes() == b""
    finally:
        if runtime is not None:
            await runtime.close()


async def test_vm_ledger_refuses_a_replaceable_parent(tmp_path):
    directory = tmp_path / "shared"
    directory.mkdir(mode=0o700)
    directory.chmod(0o770)
    runtime = None

    try:
        with pytest.raises(StateSecurityError):
            runtime = Orchestrator(directory / "jobs.sqlite3", FakeHypervisor())
        assert list(directory.iterdir()) == []
    finally:
        if runtime is not None:
            await runtime.close()


async def test_vm_ledger_creates_private_state_and_preserves_topic_after_reopen(tmp_path):
    path = tmp_path / "new-state" / "jobs.sqlite3"
    hypervisor = FakeHypervisor()
    first = Orchestrator(path, hypervisor)
    try:
        topic = await first.create(VmSpec(topic_id="topic-a", image_digest="ab" * 32))
    finally:
        await first.close()

    reopened = Orchestrator(path, hypervisor)
    try:
        assert reopened.by_topic("topic-a") == topic
        assert stat.S_IMODE(path.stat().st_mode) == 0o600
        assert stat.S_IMODE(path.parent.stat().st_mode) == 0o700
    finally:
        await reopened.close()
