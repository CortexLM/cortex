"""Setup recovery reconciles exported packs and paid baselines without repeating them."""

import hashlib
import json

import pytest

from cortex.rlm import AgentLimits, SetupProposal
from cortex.rlm.models import VmResult
from cortex.vm.models import VmError
from cortex.vm.setup import GENERATED_RUNNER

from .test_research_recovery import recovery as recovery
from .test_research_recovery import resume


@pytest.mark.parametrize("recovery", ["setup"], indirect=True)
@pytest.mark.parametrize("phase", ["setup", "experiment"])
@pytest.mark.parametrize("lost_at", ["host", "guest"])
async def test_setup_response_loss_recovers_exact_export_and_baseline(
    recovery, monkeypatch, phase, lost_at
):
    host, orchestrator, topic, hypervisor, provider, envelope = recovery
    provider.fail_on = 0
    original_params = {"operator_policy": "preserve-original-setup-inputs"}
    envelope = envelope.model_copy(update={"params": original_params})
    initial = host()
    measured = hypervisor.execute
    executed_jobs = []
    lost = False

    async def record_execution(vm_id, spec, job):
        executed_jobs.append(job.model_copy(deep=True))
        return await measured(vm_id, spec, job)

    monkeypatch.setattr(hypervisor, "execute", record_execution)
    if lost_at == "host":
        execute = orchestrator.execute

        async def lose_result(vm_id, job):
            nonlocal lost
            result = await execute(vm_id, job)
            if not lost and job.action.phase == phase:
                lost = True
                raise ConnectionError("setup execution result lost after durable success")
            return result

        monkeypatch.setattr(orchestrator, "execute", lose_result)
    else:
        callback = initial.transport.server.callback
        exchange = callback.exchange

        async def lose_response(message):
            nonlocal lost
            response = await exchange(message)
            if (
                not lost
                and message["type"] == "execute"
                and message["request"]["action"]["phase"] == phase
                and "result" in response
            ):
                lost = True
                raise ConnectionError("completed setup callback response lost")
            return response

        monkeypatch.setattr(callback, "exchange", lose_response)

    with pytest.raises(VmError, match="topic RLM execution failed"):
        await initial.run(topic.vm_id, envelope)
    await initial.close()
    assert lost
    completed_phases = 1 if phase == "setup" else 2
    assert len(executed_jobs) == completed_phases
    assert len(provider.requests) == completed_phases
    completed = {
        row["execution_id"]: dict(row) for row in orchestrator.db.execute("SELECT * FROM jobs")
    }
    assert len(completed) == completed_phases
    assert all(row["state"] == "succeeded" for row in completed.values())
    setup_job = executed_jobs[0]
    exported = orchestrator.setup_evidence(setup_job.execution_id)
    assert exported is not None
    original_pack = hypervisor.packs[exported.environment_digest]
    assert hashlib.sha256(original_pack).hexdigest() == exported.environment_digest
    progress_before = json.loads(
        orchestrator.db.execute("SELECT progress FROM research_progress").fetchone()[0]
    )

    outcome = await host().run(topic.vm_id, resume(envelope))

    assert [job.action.phase for job in executed_jobs] == ["setup", "experiment"]
    assert executed_jobs[0].params == original_params
    assert executed_jobs[1].params == {
        **original_params,
        "baseline_runner": GENERATED_RUNNER,
        "experiment_pack_digest": exported.environment_digest,
    }
    assert len(hypervisor.booted) == 2
    assert hypervisor.booted[0][0] == topic.vm_id
    baseline_vm_id, baseline_spec = hypervisor.booted[1]
    assert baseline_spec.kind == "experiment"
    assert baseline_vm_id != topic.vm_id
    assert hypervisor.torn_down == [(baseline_vm_id, False)]
    assert orchestrator.get(baseline_vm_id).state == "destroyed"
    assert orchestrator.get(topic.vm_id).state == "running"
    assert hypervisor.packs == {exported.environment_digest: original_pack}

    persisted = {
        row["execution_id"]: dict(row) for row in orchestrator.db.execute("SELECT * FROM jobs")
    }
    assert len(persisted) == 2
    assert all(persisted[execution_id] == row for execution_id, row in completed.items())
    baseline = VmResult.model_validate_json(persisted[executed_jobs[1].execution_id]["result"])
    assert baseline.metrics[0].name == "quality"
    assert baseline.metrics[0].value == 0.5
    assert baseline.flops_used == 17
    assert outcome.setup_evidence == exported.model_copy(
        update={"baseline_report_digest": baseline.report_digest, "teardown_confirmed": True}
    )
    assert outcome.run.result == SetupProposal(
        topic_id="topic-a",
        title="Operator objective",
        instructions="Submit an artifact that improves the measured result.",
        rules=[{"id": "originality", "description": "No overlap", "check": "inspect"}],
        endpoints=[
            {
                "method": "POST",
                "suffix": "/submit",
                "description": "Submit",
                "request_schema": {},
                "response_schema": {},
            }
        ],
        metric="quality",
        direction="higher",
        floor=0.5,
        setup_report_digest=exported.setup_report_digest,
        baseline_report_digest=baseline.report_digest,
        environment_digest=exported.environment_digest,
        private_holdout_digest=exported.private_holdout_digest,
    )
    assert outcome.reports == [
        VmResult.model_validate_json(persisted[job.execution_id]["result"]) for job in executed_jobs
    ]
    assert len(outcome.executions) == 2
    assert outcome.executions[0].phase == "setup"
    assert outcome.executions[0].vm_id == topic.vm_id
    assert not outcome.executions[0].dedicated
    assert outcome.executions[0].environment_digest is None
    assert outcome.executions[1].phase == "experiment"
    assert outcome.executions[1].vm_id == baseline_vm_id
    assert outcome.executions[1].dedicated and outcome.executions[1].teardown_confirmed
    assert outcome.executions[1].environment_digest == exported.environment_digest
    assert outcome.run.calls == len(provider.requests) == 3
    assert outcome.run.tokens == 600
    assert outcome.run.tool_calls == 3
    progress_after = json.loads(
        orchestrator.db.execute("SELECT progress FROM research_progress").fetchone()[0]
    )
    assert progress_after["calls"] == 3
    assert progress_after["tokens"] == 600
    assert progress_after["remaining_tools"] == AgentLimits().max_tool_calls - 2
    assert progress_after["deadline"] == progress_before["deadline"]
    assert progress_after["pending_execution"] is None
    assert (
        progress_after["operations"][setup_job.execution_id]["params"]
        == progress_before["operations"][setup_job.execution_id]["params"]
        == original_params
    )
