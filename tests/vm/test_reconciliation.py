"""Stored VM evidence is recovered without repeating a paid side effect."""

import hashlib
import json
from dataclasses import replace

import pytest

from cortex.rlm.models import VmAction, VmContext
from cortex.vm.firecracker import FirecrackerHypervisor
from cortex.vm.models import ExecuteRequest, VmError, VmSpec
from cortex.vm.runtime import Orchestrator

from .test_firecracker import config as firecracker_config
from .test_runtime import FakeHypervisor, artifact, request


class MeasuredHypervisor(FakeHypervisor):
    def __init__(self):
        super().__init__()
        self.executed = []

    async def execute(self, vm_id, spec, job):
        self.executed.append(job.execution_id)
        return await super().execute(vm_id, spec, job)


def side_effects(hypervisor):
    return (list(hypervisor.booted), list(hypervisor.executed), list(hypervisor.torn_down))


@pytest.fixture
async def completed(tmp_path):
    hypervisor = MeasuredHypervisor()
    host = Orchestrator(tmp_path / "jobs.sqlite3", hypervisor)
    topic = await host.create(VmSpec(topic_id="topic-a", image_digest="ab" * 32))
    job = request()
    result = await host.execute(topic.vm_id, job)
    yield host, hypervisor, topic, job, result
    await host.close()


async def test_reconcile_returns_completed_result_without_lifecycle_changes(completed):
    host, hypervisor, topic, job, expected = completed
    before = side_effects(hypervisor)
    ledger = list(host.db.iterdump())

    result = await host.reconcile(topic.vm_id, job)

    assert result == expected
    assert side_effects(hypervisor) == before
    assert list(host.db.iterdump()) == ledger


@pytest.mark.parametrize("state", ["unknown", "accepted", "running", "failed"])
async def test_reconcile_never_starts_or_replays_incomplete_jobs(completed, state):
    host, hypervisor, topic, job, _ = completed
    if state == "unknown":
        job = job.model_copy(update={"execution_id": "not-a-stored-job"})
    else:
        host.db.execute("UPDATE jobs SET state=?", (state,))
    before = side_effects(hypervisor)

    with pytest.raises(VmError):
        await host.reconcile(topic.vm_id, job)

    assert side_effects(hypervisor) == before


async def test_reconcile_rejects_changed_original_request(completed):
    host, hypervisor, topic, job, _ = completed
    changed = job.model_copy(update={"action": job.action.model_copy(update={"argv": ["other"]})})
    before = side_effects(hypervisor)

    with pytest.raises(VmError, match="different request"):
        await host.reconcile(topic.vm_id, changed)

    assert side_effects(hypervisor) == before


@pytest.mark.parametrize(
    "field,value",
    [
        ("topic_id", "other-topic"),
        ("job_id", "other-job"),
        ("execution_id", "other-execution"),
        ("image_digest", "ef" * 32),
        ("artifact_digest", "ef" * 32),
        ("exit_code", 1),
        ("network_enabled", True),
        ("sandboxed", False),
    ],
)
async def test_reconcile_rejects_corrupt_stored_evidence(completed, field, value):
    host, hypervisor, topic, job, result = completed
    body = result.model_dump(mode="json") | {field: value}
    host.db.execute("UPDATE jobs SET result=?", (json.dumps(body),))
    before = side_effects(hypervisor)

    with pytest.raises(VmError):
        await host.reconcile(topic.vm_id, job)

    assert side_effects(hypervisor) == before


@pytest.mark.parametrize("state", ["running", "uncertain", "retained"])
async def test_reconcile_requires_confirmed_experiment_destruction(completed, state):
    host, hypervisor, topic, job, _ = completed
    host.db.execute("UPDATE vms SET state=? WHERE kind='experiment'", (state,))
    before = side_effects(hypervisor)

    with pytest.raises(VmError, match="TeardownUnconfirmed"):
        await host.reconcile(topic.vm_id, job)

    assert side_effects(hypervisor) == before


async def test_reconcile_rejects_rebinding_to_the_topic_vm(completed):
    host, hypervisor, topic, job, _ = completed
    host.db.execute("UPDATE jobs SET vm_id=?", (topic.vm_id,))
    before = side_effects(hypervisor)

    with pytest.raises(VmError):
        await host.reconcile(topic.vm_id, job)

    assert side_effects(hypervisor) == before


async def test_reconcile_can_recover_setup_on_its_persistent_topic_vm(tmp_path):
    hypervisor = MeasuredHypervisor()
    host = Orchestrator(tmp_path / "jobs.sqlite3", hypervisor)
    try:
        topic = await host.create(VmSpec(topic_id="topic-a", image_digest="ab" * 32))
        job = ExecuteRequest(
            execution_id="setup-execution",
            context=VmContext(
                topic_id="topic-a", job_id="setup-job", purpose="setup", image_digest="ab" * 32
            ),
            action=VmAction(operation="run", phase="setup", argv=["prepare"]),
        )
        expected = await host.execute(topic.vm_id, job)
        before = side_effects(hypervisor)

        assert await host.reconcile(topic.vm_id, job) == expected
        assert host.get(topic.vm_id).state == "running"
        assert side_effects(hypervisor) == before
    finally:
        await host.close()


async def test_reconcile_reloads_the_same_pinned_pack_and_refuses_changed_bytes(tmp_path):
    directory = tmp_path / "packs"
    directory.mkdir(mode=0o700)
    pack = artifact()
    pack_digest = hashlib.sha256(pack).hexdigest()
    pack_path = directory / f"{pack_digest}.tar"
    pack_path.write_bytes(pack)
    preparation = FirecrackerHypervisor(replace(firecracker_config(tmp_path), pack_dir=directory))

    class PreparedHypervisor(MeasuredHypervisor):
        def prepare(self, job):
            return preparation.prepare(job)

    hypervisor = PreparedHypervisor()
    host = Orchestrator(tmp_path / "jobs.sqlite3", hypervisor)
    try:
        topic = await host.create(VmSpec(topic_id="topic-a", image_digest="ab" * 32))
        job = request(params={"experiment_pack_digest": pack_digest})
        expected = await host.execute(topic.vm_id, job)
        before = side_effects(hypervisor)

        assert await host.reconcile(topic.vm_id, job) == expected
        pack_path.write_bytes(b"replaced pack")
        with pytest.raises(VmError, match="digest mismatch"):
            await host.reconcile(topic.vm_id, job)
        assert side_effects(hypervisor) == before
    finally:
        await host.close()
