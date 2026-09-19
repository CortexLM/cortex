"""Lost VM responses recover by exact ledger lookup, without a second execution."""

import json

import pytest

from cortex.rlm import AgentLimits
from cortex.rlm.models import VmAction
from cortex.vm.models import VmError

from .test_research_recovery import recovery as recovery
from .test_research_recovery import resume


def uninterrupted_model(provider, monkeypatch):
    monkeypatch.setattr(provider, "fail_on", 0)


@pytest.mark.parametrize("lost_at", ["host", "guest"])
async def test_completed_vm_result_reconciles_without_reexecution(recovery, monkeypatch, lost_at):
    host, orchestrator, topic, hypervisor, provider, envelope = recovery
    uninterrupted_model(provider, monkeypatch)
    initial = host()
    if lost_at == "host":
        execute = orchestrator.execute

        async def lose_result(vm_id, job):
            result = await execute(vm_id, job)
            if job.action.phase == "experiment":
                raise ConnectionError("host did not receive the durable experiment result")
            return result

        monkeypatch.setattr(orchestrator, "execute", lose_result)
    else:
        callback = initial.transport.server.callback
        exchange = callback.exchange

        async def lose_response(message):
            response = await exchange(message)
            if (
                message["type"] == "execute"
                and message["request"]["action"]["phase"] == "experiment"
            ):
                raise ConnectionError("guest did not receive the completed callback")
            return response

        monkeypatch.setattr(callback, "exchange", lose_response)

    with pytest.raises(VmError):
        await initial.run(topic.vm_id, envelope)
    await initial.close()
    booted = list(hypervisor.booted)
    assert len(booted) == 3
    assert len(provider.requests) == 1

    outcome = await host().run(topic.vm_id, resume(envelope))

    assert outcome.run.result.outcome == "accepted"
    assert outcome.run.result.value == 0.9
    assert outcome.run.calls == 2
    assert outcome.run.tool_calls == 3
    assert len(outcome.reports) == len(outcome.executions) == 2
    assert all(item.dedicated and item.teardown_confirmed for item in outcome.executions)
    assert hypervisor.booted == booted
    assert len(provider.requests) == 2
    progress = json.loads(
        orchestrator.db.execute("SELECT progress FROM research_progress").fetchone()[0]
    )
    assert progress["remaining_tools"] == AgentLimits().max_tool_calls - 2
    assert progress["pending_execution"] is None


async def test_unknown_vm_outcome_never_triggers_a_replacement_experiment(recovery, monkeypatch):
    host, orchestrator, topic, hypervisor, provider, envelope = recovery
    uninterrupted_model(provider, monkeypatch)
    execute = orchestrator.execute

    async def interrupted_dispatch(vm_id, job):
        if job.action.phase == "experiment":
            raise ConnectionError("dispatch state unknown")
        return await execute(vm_id, job)

    monkeypatch.setattr(orchestrator, "execute", interrupted_dispatch)
    initial = host()
    with pytest.raises(VmError):
        await initial.run(topic.vm_id, envelope)
    await initial.close()
    booted = list(hypervisor.booted)

    with pytest.raises(VmError, match="reconciliation"):
        await host().run(topic.vm_id, resume(envelope))

    assert hypervisor.booted == booted
    assert len(provider.requests) == 1
    assert orchestrator.db.execute("SELECT count(*) FROM jobs").fetchone()[0] == 1


@pytest.mark.parametrize("attempt", ["inference", "execute", "changed_action", "unknown_id"])
async def test_pending_intent_blocks_spend_and_unbound_lookup(recovery, monkeypatch, attempt):
    host, orchestrator, topic, hypervisor, provider, envelope = recovery
    uninterrupted_model(provider, monkeypatch)
    execute = orchestrator.execute

    async def lose_result(vm_id, job):
        result = await execute(vm_id, job)
        if job.action.phase == "experiment":
            raise ConnectionError("result lost")
        return result

    monkeypatch.setattr(orchestrator, "execute", lose_result)
    initial = host()
    with pytest.raises(VmError):
        await initial.run(topic.vm_id, envelope)
    await initial.close()
    progress = json.loads(
        orchestrator.db.execute("SELECT progress FROM research_progress").fetchone()[0]
    )
    operation_id = progress["pending_execution"]
    action = progress["operations"][operation_id]["action"]
    operation = {
        "execution_id": operation_id,
        "context": envelope.request.task.context.model_dump(mode="json"),
        "action": action,
    }
    if attempt == "changed_action":
        operation["action"] = {**action, "argv": ["different-command"]}
    if attempt == "unknown_id":
        operation["execution_id"] = "unknown-operation"
    message = (
        {"type": "inference", "request": provider.requests[0]}
        if attempt == "inference"
        else {"type": "execute" if attempt == "execute" else "reconcile", "request": operation}
    )
    recovering = host()
    callback = recovering.transport.server.callback
    responses = []

    class ProbeTransport:
        async def exchange(self, socket_path, request, budget_seconds):
            responses.append(await callback.exchange(message))
            raise VmError("probe finished")

    recovering.transport = ProbeTransport()
    with pytest.raises(VmError):
        await recovering.run(topic.vm_id, resume(envelope))

    assert responses == [{"error": "host callback refused"}]
    assert len(hypervisor.booted) == 3
    assert len(provider.requests) == 1


async def test_guest_cannot_finish_with_old_evidence_while_another_operation_is_pending(
    recovery, monkeypatch
):
    host, orchestrator, topic, hypervisor, provider, envelope = recovery
    uninterrupted_model(provider, monkeypatch)
    initial = host()
    transport = initial.transport
    callback = transport.server.callback
    execute = orchestrator.execute

    async def uncertain_inspect(vm_id, job):
        if job.action.phase == "inspect":
            raise ConnectionError("additional operation is uncertain")
        return await execute(vm_id, job)

    monkeypatch.setattr(orchestrator, "execute", uncertain_inspect)

    class GuestWithUnresolvedWork:
        async def exchange(self, socket_path, message, budget_seconds):
            completed = await transport.exchange(socket_path, message, budget_seconds)
            refused = await callback.exchange(
                {
                    "type": "execute",
                    "request": {
                        "execution_id": "unresolved-inspection",
                        "context": envelope.request.task.context.model_dump(mode="json"),
                        "action": VmAction(operation="run", argv=["inspect"]).model_dump(
                            mode="json"
                        ),
                    },
                }
            )
            assert refused == {"error": "host callback refused"}
            return completed

    initial.transport = GuestWithUnresolvedWork()
    with pytest.raises(VmError, match="reconciliation"):
        await initial.run(topic.vm_id, envelope)

    assert orchestrator.db.execute("SELECT state FROM research_jobs").fetchone()[0] != "succeeded"
    assert len(hypervisor.booted) == 3
