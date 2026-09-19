"""Research recovery keeps host attestations and budgets across explicit retries."""

import json
import time

import pytest

from cortex.rlm import AgentLimits, AgentRequest, AgentTask, Rule
from cortex.rlm.journal import RunJournal
from cortex.vm.guest import GuestIdentity
from cortex.vm.guest_server import GuestServer
from cortex.vm.models import VmError, VmSpec
from cortex.vm.research import ResearchHost, ResearchRequest
from cortex.vm.runtime import Orchestrator

from .test_research import GuestTransport, MeasuredHypervisor, MemoryCallback, ScriptedProvider
from .test_runtime import request as execution_request
from .test_setup_agent import ExecutingHypervisor, SetupProvider


class InterruptedProvider:
    def __init__(self, provider, fail_on=2):
        self.provider, self.model, self.fail_on = provider, provider.model, fail_on
        self.requests = []

    async def complete(self, **kwargs):
        self.requests.append(kwargs)
        if len(self.requests) == self.fail_on:
            raise ConnectionError("inference response lost")
        return await self.provider.complete(**kwargs)


@pytest.fixture
async def recovery(tmp_path, monkeypatch, request):
    base = execution_request()
    setup = getattr(request, "param", "evaluate") == "setup"
    hypervisor = ExecutingHypervisor(tmp_path / "guests") if setup else MeasuredHypervisor()
    orchestrator = Orchestrator(tmp_path / "jobs.sqlite3", hypervisor)
    topic = await orchestrator.create(VmSpec(topic_id="topic-a", image_digest="ab" * 32))
    callback = MemoryCallback()
    monkeypatch.setattr("cortex.vm.research.asyncio.start_unix_server", callback.start)
    monkeypatch.setattr("cortex.vm.research.os.chmod", lambda path, mode, **kwargs: None)
    guest = GuestServer(
        GuestIdentity(topic.vm_id, "topic-a", "ab" * 32, "topic"),
        workspace=tmp_path / "guest",
        callback=callback,
    )
    provider = InterruptedProvider(
        SetupProvider() if setup else ScriptedProvider(base.context.artifact_digest),
        fail_on=3 if setup else 2,
    )
    task = AgentTask(
        context=base.context,
        objective="Evaluate the submitted artifact.",
        metric="quality",
        rule_ids=["originality"],
        rules=[Rule(id="originality", description="No overlap", check="inspect")],
    )
    if setup:
        task = AgentTask(
            context=base.context.model_copy(update={"purpose": "setup", "artifact_digest": None}),
            objective="Prepare the operator topic and measure its baseline.",
        )
    envelope = ResearchRequest(
        request=AgentRequest(task=task), artifact_b64="" if setup else base.artifact_b64
    )
    hosts = []

    def host(*, limits=None):
        instance = ResearchHost(
            orchestrator,
            provider,
            lambda _: tmp_path / "v.sock",
            transport=GuestTransport(guest),
            limits=limits,
        )
        hosts.append(instance)
        return instance

    yield host, orchestrator, topic, hypervisor, provider, envelope
    for instance in hosts:
        await instance.close()
    await orchestrator.close()


def resume(envelope):
    return envelope.model_copy(
        update={"request": envelope.request.model_copy(update={"resume": True})}
    )


async def interrupt_after_experiment(host, topic, envelope):
    with pytest.raises(VmError, match="topic RLM execution failed"):
        await host.run(topic.vm_id, envelope)
    await host.close()


async def test_explicit_resume_restores_evidence_without_repeating_paid_experiment(recovery):
    host, orchestrator, topic, hypervisor, provider, envelope = recovery
    await interrupt_after_experiment(host(), topic, envelope)
    booted = list(hypervisor.booted)

    outcome = await host().run(topic.vm_id, resume(envelope))

    assert outcome.run.result.outcome == "accepted"
    assert outcome.run.result.value == 0.9
    assert outcome.run.calls == 3
    assert len(outcome.reports) == 2
    assert all(item.teardown_confirmed for item in outcome.executions)
    assert hypervisor.booted == booted
    assert len(provider.requests) == 3
    assert await host().run(topic.vm_id, resume(envelope)) == outcome


@pytest.mark.parametrize("recovery", ["setup"], indirect=True)
async def test_resume_preserves_setup_export_and_confirmed_baseline(recovery):
    host, orchestrator, topic, hypervisor, provider, envelope = recovery
    await interrupt_after_experiment(host(), topic, envelope)
    booted = list(hypervisor.booted)

    outcome = await host().run(topic.vm_id, resume(envelope))

    assert outcome.run.calls == 4
    assert outcome.setup_evidence.teardown_confirmed
    assert (
        outcome.setup_evidence.baseline_report_digest == outcome.run.result.baseline_report_digest
    )
    assert outcome.setup_evidence.environment_digest == outcome.run.result.environment_digest
    assert outcome.reports[-1].metrics[0].value == 0.5
    assert outcome.executions[-1].teardown_confirmed
    assert hypervisor.booted == booted


@pytest.mark.parametrize("changed", ["task", "model", "limits"])
async def test_resume_rejects_changed_immutable_policy_before_inference(recovery, changed):
    host, orchestrator, topic, hypervisor, provider, envelope = recovery
    await interrupt_after_experiment(host(), topic, envelope)
    limits = None
    if changed == "task":
        task = envelope.request.task.model_copy(update={"objective": "Change the scoring rules"})
        envelope = envelope.model_copy(update={"request": AgentRequest(task=task)})
    elif changed == "model":
        provider.model = "other/model"
    else:
        limits = AgentLimits(max_calls=17)

    with pytest.raises(VmError, match="reused|cannot change"):
        await host(limits=limits).run(topic.vm_id, resume(envelope))

    assert len(provider.requests) == 2
    assert len(hypervisor.booted) == 3


async def test_recovery_preserves_reserved_inference_budget(recovery):
    host, orchestrator, topic, hypervisor, provider, envelope = recovery
    limits = AgentLimits(max_calls=2)
    await interrupt_after_experiment(host(limits=limits), topic, envelope)

    with pytest.raises(VmError, match="topic RLM execution failed"):
        await host(limits=limits).run(topic.vm_id, resume(envelope))

    assert len(provider.requests) == 2
    assert len(hypervisor.booted) == 3


@pytest.mark.parametrize("budget", ["inference", "tools"])
async def test_host_independently_enforces_spent_budgets_after_resume(recovery, budget):
    host, orchestrator, topic, hypervisor, provider, envelope = recovery
    limits = AgentLimits(max_calls=2) if budget == "inference" else AgentLimits(max_tool_calls=2)
    await interrupt_after_experiment(host(limits=limits), topic, envelope)
    recovering = host(limits=limits)
    callback = recovering.transport.server.callback
    responses = []
    probe = (
        {"type": "inference", "request": provider.requests[0]}
        if budget == "inference"
        else {
            "type": "execute",
            "request": {
                "execution_id": "extra-tool",
                "context": envelope.request.task.context.model_dump(mode="json"),
                "action": {"operation": "run", "phase": "experiment", "argv": ["run"]},
            },
        }
    )

    class ProbeTransport:
        async def exchange(self, socket_path, message, budget_seconds):
            responses.append(await callback.exchange(probe))
            raise VmError("probe completed")

    recovering.transport = ProbeTransport()

    with pytest.raises(VmError, match="probe completed"):
        await recovering.run(topic.vm_id, resume(envelope))

    assert responses == [{"error": "host callback refused"}]
    assert len(provider.requests) == 2
    assert len(hypervisor.booted) == 3


async def test_spend_and_vm_intents_are_durable_before_dispatch(recovery, monkeypatch):
    host, orchestrator, topic, hypervisor, provider, envelope = recovery
    measured, complete = hypervisor.execute, provider.complete
    provider_reservations, vm_intents = [], []

    def progress():
        return json.loads(
            orchestrator.db.execute("SELECT progress FROM research_progress").fetchone()[0]
        )

    async def inspect_inference(**kwargs):
        provider_reservations.append(progress())
        return await complete(**kwargs)

    async def inspect_vm(vm_id, spec, job):
        vm_intents.append((job.execution_id, progress()))
        return await measured(vm_id, spec, job)

    monkeypatch.setattr(provider, "complete", inspect_inference)
    monkeypatch.setattr(hypervisor, "execute", inspect_vm)

    await interrupt_after_experiment(host(), topic, envelope)

    assert [item["calls"] for item in provider_reservations] == [1, 2]
    assert all(item["tokens"] >= AgentLimits().completion_tokens for item in provider_reservations)
    assert all(item["pending_execution"] == execution for execution, item in vm_intents)
    assert [item["remaining_tools"] for _, item in vm_intents] == [47, 46]


async def test_recovery_preserves_original_deadline(recovery, monkeypatch):
    host, orchestrator, topic, hypervisor, provider, envelope = recovery
    await interrupt_after_experiment(host(), topic, envelope)
    expired = time.time() + AgentLimits().wall_seconds + 1
    monkeypatch.setattr("cortex.vm.research.time.time", lambda: expired)

    with pytest.raises(VmError, match="budget exhausted"):
        await host().run(topic.vm_id, resume(envelope))

    assert len(provider.requests) == 2


async def test_interrupted_job_requires_explicit_resume(recovery):
    host, orchestrator, topic, hypervisor, provider, envelope = recovery
    await interrupt_after_experiment(host(), topic, envelope)

    with pytest.raises(VmError, match="research_resume_required"):
        await host().run(topic.vm_id, envelope)

    assert len(provider.requests) == 2


async def test_interrupted_job_without_progress_cannot_receive_fresh_budget(recovery):
    host, orchestrator, topic, hypervisor, provider, envelope = recovery
    await interrupt_after_experiment(host(), topic, envelope)
    orchestrator.db.execute("DELETE FROM research_progress")

    with pytest.raises(VmError, match="progress.*recovery"):
        await host().run(topic.vm_id, resume(envelope))

    assert len(provider.requests) == 2


async def test_missing_guest_checkpoint_never_restarts_completed_vm_phases(recovery):
    host, orchestrator, topic, hypervisor, provider, envelope = recovery
    await interrupt_after_experiment(host(), topic, envelope)
    recovering = host()
    journal = RunJournal(recovering.transport.server.workspace / "rlm-journal")
    with journal.connection:
        journal.connection.execute("DELETE FROM runs")
    journal.close()
    booted = list(hypervisor.booted)

    with pytest.raises(VmError, match="topic RLM execution failed"):
        await recovering.run(topic.vm_id, resume(envelope))

    assert len(provider.requests) == 2
    assert hypervisor.booted == booted


async def test_uncertain_vm_intent_requires_reconciliation_without_replay(recovery, monkeypatch):
    host, orchestrator, topic, hypervisor, provider, envelope = recovery
    measured = hypervisor.execute

    async def uncertain(vm_id, spec, job):
        if job.action.phase == "experiment":
            raise ConnectionError("execution response lost after dispatch")
        return await measured(vm_id, spec, job)

    monkeypatch.setattr(hypervisor, "execute", uncertain)
    await interrupt_after_experiment(host(), topic, envelope)
    booted = list(hypervisor.booted)

    with pytest.raises(VmError, match="orchestrator reconciliation"):
        await host().run(topic.vm_id, resume(envelope))

    assert hypervisor.booted == booted
    assert len(provider.requests) == 1


async def test_progress_keeps_usage_but_never_persists_miner_credentials(recovery):
    host, orchestrator, topic, hypervisor, provider, envelope = recovery
    secret = "private-miner-credential"
    envelope = envelope.model_copy(
        update={
            "env": {"MINER_API_KEY": secret},
            "params": {"miner_byok": "MINER_API_KEY", "miner_env_allowlist": "MINER_API_KEY"},
        }
    )
    await interrupt_after_experiment(host(), topic, envelope)

    encoded = orchestrator.db.execute("SELECT progress FROM research_progress").fetchone()[0]
    progress = json.loads(encoded)

    assert secret not in encoded
    assert "env" not in progress
    assert progress["calls"] == 2
    assert progress["tokens"] > 200
    assert progress["remaining_tools"] == AgentLimits().max_tool_calls - 2
    assert len(progress["reports"]) == 2
