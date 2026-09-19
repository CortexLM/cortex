"""Close the callback session before a completed research result can escape."""

import asyncio
import json

import pytest

from cortex.vm.models import ExecuteRequest, VmError

from .test_research_recovery import recovery as recovery


def extra_operation(envelope):
    return {
        "type": "execute",
        "request": {
            "execution_id": "late-operation",
            "context": envelope.request.task.context.model_dump(mode="json"),
            "action": {"operation": "run", "phase": "inspect", "argv": ["inspect"]},
        },
    }


async def test_callback_arriving_during_final_close_cannot_launch_another_vm(recovery, monkeypatch):
    host, orchestrator, topic, hypervisor, provider, envelope = recovery
    provider.fail_on = 0
    research = host()
    callback = research.transport.server.callback
    late_responses = []

    async def deliver_late_frame():
        late_responses.append(await callback.exchange(extra_operation(envelope)))

    monkeypatch.setattr(callback, "wait_closed", deliver_late_frame)

    result = await research.run(topic.vm_id, envelope)

    assert result.run.result.outcome == "accepted"
    assert len(hypervisor.booted) == 3
    assert len(provider.requests) == 2
    assert late_responses == [{"error": "host callback refused"}]
    progress = json.loads(
        orchestrator.db.execute("SELECT progress FROM research_progress").fetchone()[0]
    )
    assert progress["pending_execution"] is None
    assert "late-operation" not in progress["operations"]
    assert orchestrator.db.execute("SELECT count(*) FROM jobs").fetchone()[0] == 2


async def test_in_flight_vm_cannot_become_successful_research_during_final_close(
    recovery, monkeypatch
):
    host, orchestrator, topic, hypervisor, provider, envelope = recovery
    provider.fail_on = 0
    research = host()
    original_transport = research.transport
    callback = original_transport.server.callback
    execute = hypervisor.execute
    entered, release = asyncio.Event(), asyncio.Event()
    callback_task = None
    callback_finished_before_close = []

    async def pause_extra_operation(vm_id, spec, job):
        if job.execution_id == "late-operation":
            entered.set()
            await release.wait()
        return await execute(vm_id, spec, job)

    class GuestWithConcurrentOperation:
        async def exchange(self, socket_path, message, budget_seconds):
            nonlocal callback_task
            result = await original_transport.exchange(socket_path, message, budget_seconds)
            callback_task = asyncio.create_task(callback.exchange(extra_operation(envelope)))
            await entered.wait()
            return result

    async def observe_close_order():
        callback_finished_before_close.append(callback_task.done())

    monkeypatch.setattr(hypervisor, "execute", pause_extra_operation)
    monkeypatch.setattr(callback, "wait_closed", observe_close_order)
    research.transport = GuestWithConcurrentOperation()
    try:
        with pytest.raises(VmError, match="reconciliation"):
            await research.run(topic.vm_id, envelope)
        assert callback_finished_before_close == [True]
        assert callback_task.cancelled()
        assert len(hypervisor.booted) == 4
        assert len(provider.requests) == 2
        release.set()
        await orchestrator.execute(
            topic.vm_id,
            # Rejoin the already running operation; no replacement job may start.
            ExecuteRequest(
                **extra_operation(envelope)["request"], artifact_b64=envelope.artifact_b64
            ),
        )
        assert len(hypervisor.booted) == 4
        assert orchestrator.db.execute("SELECT state FROM research_jobs").fetchone()[0] == "failed"
        progress = json.loads(
            orchestrator.db.execute("SELECT progress FROM research_progress").fetchone()[0]
        )
        assert progress["pending_execution"] == "late-operation"
    finally:
        release.set()
        if callback_task is not None:
            await asyncio.gather(callback_task, return_exceptions=True)
