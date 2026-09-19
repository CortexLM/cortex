"""The agent lives in the guest service; the host only brokers attested tools and inference."""

import asyncio
import json
import struct

import pytest

from cortex.rlm import AgentRequest, AgentTask, Rule
from cortex.rlm.provider import Completion
from cortex.vm.guest import GuestIdentity
from cortex.vm.guest_server import GuestServer
from cortex.vm.models import GuestMeasurement, GuestOutput, VmError, VmSpec
from cortex.vm.research import ResearchHost, ResearchRequest
from cortex.vm.runtime import Orchestrator

from .test_runtime import FakeHypervisor, request


class MeasuredHypervisor(FakeHypervisor):
    async def execute(self, vm_id, spec, job):
        preflight = job.action.phase == "preflight"
        return GuestOutput(
            context=job.context,
            execution_id=job.execution_id,
            exit_code=0,
            stdout_tail="done",
            report_digest=("12" if preflight else "34") * 32,
            measurement=GuestMeasurement(
                flops_used=0 if preflight else 123,
                rule_checks=[{"rule_id": "originality", "passed": True}] if preflight else [],
                metrics=[] if preflight else [{"name": "quality", "value": 0.9}],
            ),
        )


class ScriptedProvider:
    model = "operator/model"

    def __init__(self, artifact_digest):
        self.actions = [
            ("vm_execute", {"operation": "run", "phase": "experiment", "argv": ["run"]}),
            (
                "finish",
                {
                    "topic_id": "topic-a",
                    "artifact_digest": artifact_digest,
                    "rule_revision": 1,
                    "outcome": "accepted",
                    "explanation": "Verified from the guest report",
                    "metric": "quality",
                    "value": 0.9,
                    "report_digest": "34" * 32,
                    "rules_checked": ["originality"],
                },
            ),
        ]

    async def complete(self, **kwargs):
        name, arguments = self.actions.pop(0)
        return Completion(
            tool_calls=[
                {
                    "id": "one",
                    "type": "function",
                    "function": {"name": name, "arguments": json.dumps(arguments)},
                }
            ],
            prompt_tokens=100,
            completion_tokens=100,
        )


class MemoryCallback:
    """Replace only the socket transport; exercise the real framed callback handler."""

    async def start(self, callback, *, path):
        self.callback = callback
        return self

    def close(self):
        pass

    async def wait_closed(self):
        pass

    async def exchange(self, message):
        reader = asyncio.StreamReader()
        raw = json.dumps(message).encode()
        reader.feed_data(struct.pack(">I", len(raw)) + raw)
        reader.feed_eof()
        output = bytearray()

        class Writer:
            def write(self, data):
                output.extend(data)

            async def drain(self):
                pass

            def close(self):
                pass

            async def wait_closed(self):
                pass

        await self.callback(reader, Writer())
        length = struct.unpack(">I", output[:4])[0]
        return json.loads(output[4 : 4 + length])


class GuestTransport:
    def __init__(self, server):
        self.server = server

    async def exchange(self, socket_path, message, budget_seconds):
        return await self.server.handle(message)


@pytest.mark.parametrize("byok", [False, True])
async def test_guest_agent_brokers_inference_and_isolated_experiments(tmp_path, monkeypatch, byok):
    base = request()
    jobs = []

    class CapturedHypervisor(MeasuredHypervisor):
        async def execute(self, vm_id, spec, job):
            jobs.append(job)
            return await super().execute(vm_id, spec, job)

    hypervisor = CapturedHypervisor()
    orchestrator = Orchestrator(tmp_path / "jobs.sqlite3", hypervisor)
    topic = await orchestrator.create(VmSpec(topic_id="topic-a", image_digest="ab" * 32))
    socket_path = tmp_path / "v.sock"
    callback = MemoryCallback()
    monkeypatch.setattr("cortex.vm.research.asyncio.start_unix_server", callback.start)
    monkeypatch.setattr("cortex.vm.research.os.chmod", lambda path, mode, **kwargs: None)
    server = GuestServer(
        GuestIdentity(topic.vm_id, "topic-a", "ab" * 32, "topic"),
        workspace=tmp_path / "guest",
        callback=callback,
    )
    research = ResearchHost(
        orchestrator,
        ScriptedProvider(base.context.artifact_digest),
        lambda vm_id: socket_path,
        transport=GuestTransport(server),
    )
    task = AgentTask(
        context=base.context,
        objective="Evaluate the artefact against the published rules.",
        metric="quality",
        rule_ids=["originality"],
        rules=[Rule(id="originality", description="No overlap", check="inspect")],
    )
    try:
        env = {"MINER_API_KEY": "private-miner-key"} if byok else {}
        outcome = await research.run(
            topic.vm_id,
            ResearchRequest(
                request=AgentRequest(task=task),
                artifact_b64=base.artifact_b64,
                params={"miner_byok": "MINER_API_KEY"} if byok else {},
                env=env,
            ),
        )
        assert outcome.run.result.outcome == "accepted"
        assert outcome.run.result.value == 0.9
        assert len(outcome.reports) == 2
        assert all(item.dedicated and item.teardown_confirmed for item in outcome.executions)
        assert {item.vm_id for item in outcome.executions}.isdisjoint({topic.vm_id})
        assert all(spec.kind == "experiment" for _, spec in hypervisor.booted[1:])
        assert [job.action.phase for job in jobs] == ["preflight", "experiment"]
        assert [job.env for job in jobs] == [{}, env]
        assert "private-miner-key" not in outcome.model_dump_json()
        assert not socket_path.with_name("v.sock_5001").exists()
    finally:
        await research.close()
        await orchestrator.close()


@pytest.mark.parametrize(
    ("env", "reason"),
    [({}, "required env missing"), ({"UNDECLARED_KEY": "private-value"}, "undeclared env")],
)
async def test_invalid_miner_environment_is_rejected_before_research_job(tmp_path, env, reason):
    base = request()
    hypervisor = MeasuredHypervisor()
    orchestrator = Orchestrator(tmp_path / "jobs.sqlite3", hypervisor)
    topic = await orchestrator.create(VmSpec(topic_id="topic-a", image_digest="ab" * 32))

    class UndispatchedTransport:
        async def exchange(self, *args):
            raise AssertionError("invalid credentials must fail before guest dispatch")

    research = ResearchHost(
        orchestrator,
        ScriptedProvider(base.context.artifact_digest),
        lambda _: tmp_path / "unused.sock",
        transport=UndispatchedTransport(),
    )
    envelope = ResearchRequest(
        request=AgentRequest(
            task=AgentTask(
                context=base.context,
                objective="Evaluate",
                metric="quality",
                rule_ids=["originality"],
                rules=[Rule(id="originality", description="No overlap", check="inspect")],
            )
        ),
        artifact_b64=base.artifact_b64,
        params={"miner_byok": "MINER_API_KEY"},
        env=env,
    )
    try:
        with pytest.raises(VmError, match=reason) as error:
            await research.run(topic.vm_id, envelope)
        assert error.value.status == 400
        assert orchestrator.db.execute("SELECT count(*) FROM research_jobs").fetchone()[0] == 0
        assert len(hypervisor.booted) == 1
    finally:
        await research.close()
        await orchestrator.close()


def test_model_claim_with_no_host_attested_experiment_is_refused():
    from cortex.rlm import AgentRun, EvaluationVerdict

    base = request()
    task = AgentTask(
        context=base.context,
        objective="Evaluate.",
        metric="quality",
        rule_ids=["originality"],
        rules=[Rule(id="originality", description="No overlap", check="inspect")],
    )
    run = AgentRun(
        result=EvaluationVerdict(
            topic_id="topic-a",
            artifact_digest=base.context.artifact_digest,
            rule_revision=1,
            outcome="accepted",
            explanation="invented",
            metric="quality",
            value=1.0,
            report_digest="12" * 32,
            rules_checked=["originality"],
        ),
        transcript=[],
        calls=1,
        tool_calls=0,
        tokens=1,
    )

    with pytest.raises(VmError, match="not produced"):
        ResearchHost._verify_result(
            ResearchRequest(request=AgentRequest(task=task)), run, {}, {"originality": True}
        )
