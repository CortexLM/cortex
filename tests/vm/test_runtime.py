"""Lifecycle guarantees tested with a fake hypervisor, never live Firecracker."""

import asyncio
import base64
import hashlib
import io
import tarfile

import pytest

from cortex.rlm.models import VmAction, VmContext
from cortex.vm.models import ExecuteRequest, GuestMeasurement, GuestOutput, VmError, VmSpec
from cortex.vm.runtime import Orchestrator


def artifact():
    stream = io.BytesIO()
    with tarfile.open(fileobj=stream, mode="w") as archive:
        member = tarfile.TarInfo("submission.txt")
        member.size = 4
        archive.addfile(member, io.BytesIO(b"code"))
    return stream.getvalue()


def request(execution_id="execution-01", **changes):
    data = artifact()
    values = dict(
        execution_id=execution_id,
        context=VmContext(
            topic_id="topic-a",
            job_id="job-a",
            purpose="evaluate",
            image_digest="ab" * 32,
            artifact_digest=hashlib.sha256(data).hexdigest(),
        ),
        action=VmAction(operation="run", phase="experiment", argv=["/operator/run"]),
        artifact_b64=base64.b64encode(data).decode(),
    )
    return ExecuteRequest(**(values | changes))


class FakeHypervisor:
    def __init__(self):
        self.booted = []
        self.torn_down = []
        self.fail = False
        self.confirm = True
        self.entered = asyncio.Event()
        self.release = asyncio.Event()
        self.release.set()

    async def ready(self):
        return None

    async def boot(self, vm_id, spec):
        self.booted.append((vm_id, spec))
        return 123

    async def execute(self, vm_id, spec, job):
        self.entered.set()
        await self.release.wait()
        if self.fail:
            raise VmError("adaptor failed")
        return GuestOutput(
            context=job.context,
            execution_id=job.execution_id,
            exit_code=0,
            stdout_tail="finished",
            report_digest="cd" * 32,
            measurement=GuestMeasurement(
                metrics=[{"name": "accuracy", "value": 0.9}], flops_used=120
            ),
        )

    async def teardown(self, vm_id, retain):
        self.torn_down.append((vm_id, retain))
        return self.confirm


@pytest.fixture
async def runtime(tmp_path):
    hv = FakeHypervisor()
    orchestrator = Orchestrator(tmp_path / "jobs.sqlite3", hv, max_experiments=1)
    topic = await orchestrator.create(VmSpec(topic_id="topic-a", image_digest="ab" * 32))
    yield orchestrator, hv, topic
    await orchestrator.close()


async def test_paid_job_gets_own_vm_destroyed_before_result_and_is_idempotent(runtime):
    host, hv, topic = runtime
    job = request()

    result = await host.execute(topic.vm_id, job)
    repeated = await host.execute(topic.vm_id, job)

    assert result == repeated
    assert len(hv.booted) == 2
    assert hv.booted[1][1].kind == "experiment"
    assert hv.torn_down == [(hv.booted[1][0], False)]
    assert result.sandboxed and not result.network_enabled
    assert result.metrics[0].value == 0.9


async def test_failed_job_retains_vm_and_never_returns_scores(runtime):
    host, hv, topic = runtime
    hv.fail = True

    with pytest.raises(VmError, match="adaptor failed"):
        await host.execute(topic.vm_id, request())

    assert hv.torn_down == [(hv.booted[1][0], True)]


async def test_unconfirmed_teardown_cannot_release_success_or_capacity(runtime):
    host, hv, topic = runtime
    hv.confirm = False

    with pytest.raises(VmError, match="TeardownUnconfirmed"):
        await host.execute(topic.vm_id, request())
    with pytest.raises(VmError, match="capacity"):
        await host.execute(topic.vm_id, request("execution-02"))


async def test_disconnected_waiter_does_not_cancel_accepted_vm_job(runtime):
    host, hv, topic = runtime
    hv.release.clear()
    waiter = asyncio.create_task(host.execute(topic.vm_id, request()))
    await hv.entered.wait()
    waiter.cancel()
    with pytest.raises(asyncio.CancelledError):
        await waiter
    hv.release.set()

    result = await host.execute(topic.vm_id, request())

    assert result.exit_code == 0
    assert hv.torn_down == [(hv.booted[1][0], False)]


async def test_topic_mismatch_refuses_before_any_experiment_boot(runtime):
    host, hv, topic = runtime
    wrong = request().context.model_copy(update={"topic_id": "topic-b"})

    with pytest.raises(VmError, match="topic_mismatch"):
        await host.execute(topic.vm_id, request(context=wrong))

    assert len(hv.booted) == 1


async def test_successful_job_survives_restart_without_another_paid_run(tmp_path):
    hv = FakeHypervisor()
    path = tmp_path / "jobs.sqlite3"
    first = Orchestrator(path, hv)
    topic = await first.create(VmSpec(topic_id="topic-a", image_digest="ab" * 32))
    result = await first.execute(topic.vm_id, request())
    await first.close()
    second = Orchestrator(path, hv)
    try:
        replayed = await second.execute(topic.vm_id, request())
        assert replayed == result
        assert len(hv.booted) == 2
    finally:
        await second.close()


async def test_changed_payload_cannot_reuse_a_successful_execution(runtime):
    host, hv, topic = runtime
    await host.execute(topic.vm_id, request())

    with pytest.raises(VmError, match="reused"):
        await host.execute(
            topic.vm_id,
            request(action=VmAction(operation="run", phase="experiment", argv=["different"])),
        )

    assert len(hv.booted) == 2


async def test_paid_capacity_reservation_rejects_parallel_second_job(runtime):
    host, hv, topic = runtime
    hv.release.clear()
    running = asyncio.create_task(host.execute(topic.vm_id, request()))
    await hv.entered.wait()
    try:
        with pytest.raises(VmError, match="capacity"):
            await host.execute(topic.vm_id, request("execution-other"))
    finally:
        hv.release.set()
        await running


async def test_recovery_retains_previous_vms_and_never_reexecutes_jobs(tmp_path):
    hv = FakeHypervisor()
    path = tmp_path / "jobs.sqlite3"
    first = Orchestrator(path, hv)
    topic = await first.create(VmSpec(topic_id="topic-a", image_digest="ab" * 32))
    await first.close()
    second = Orchestrator(path, hv)
    try:
        await second.recover()
        assert second.get(topic.vm_id).state == "retained"
        assert hv.torn_down == [(topic.vm_id, True)]
        assert len(hv.booted) == 1
    finally:
        await second.close()
