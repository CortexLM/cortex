import pytest
from pydantic import ValidationError

from cortex.rlm import AgentLimits, AgentRequest, AgentTask, VmAction, VmContext
from cortex.vm.models import VmError, VmSpec
from cortex.vm.research import ResearchHost, ResearchRequest
from cortex.vm.runtime import Orchestrator
from cortex.vm.setup import SetupEvidence

from .test_runtime import FakeHypervisor


def request(wall=3600):
    return ResearchRequest(
        request=AgentRequest(
            task=AgentTask(
                context=VmContext(
                    topic_id="topic-a", job_id="setup-a", purpose="setup", image_digest="a" * 64
                ),
                objective="Set up the owner topic",
                wall_budget_s=wall,
            )
        )
    )


class NoInference:
    model = "test/model"

    async def complete(self, **kwargs):
        pytest.fail("Rejected topic must not perform paid inference")


def test_tool_contract_accepts_full_two_hour_budget_without_clamping():
    action = VmAction(operation="run", argv=["evaluate"], timeout_seconds=7200)
    assert action.timeout_seconds == 7200
    with pytest.raises(ValidationError):
        VmAction(operation="run", argv=["evaluate"], timeout_seconds=7201)


async def test_host_refuses_topic_above_ceiling_before_creating_job(tmp_path):
    orchestrator = Orchestrator(tmp_path / "host.sqlite3", FakeHypervisor())
    vm = await orchestrator.create(VmSpec(topic_id="topic-a", image_digest="a" * 64))
    host = ResearchHost(orchestrator, NoInference(), lambda _: tmp_path / "sock")
    try:
        with pytest.raises(VmError, match="wall budget exceeds"):
            await host.run(vm.vm_id, request())
        assert orchestrator.db.execute("SELECT count(*) FROM research_jobs").fetchone()[0] == 0
    finally:
        await orchestrator.close()


async def test_setup_manifest_timeout_must_match_policy_and_host(tmp_path):
    orchestrator = Orchestrator(tmp_path / "host.sqlite3", FakeHypervisor())
    host = ResearchHost(
        orchestrator,
        NoInference(),
        lambda _: tmp_path / "sock",
        limits=AgentLimits(wall_seconds=7200.0, tool_timeout_seconds=3600.0),
    )
    evidence = SetupEvidence(
        content_hashes=["a" * 64],
        flops_budget=100,
        wall_budget_s=3600,
        script_sha256="b" * 64,
        environment_digest="c" * 64,
        private_holdout_digest="d" * 64,
        setup_report_digest="e" * 64,
    )
    try:
        host._validate_setup_limits(request(), evidence)
        with pytest.raises(VmError, match="does not match owner policy"):
            host._validate_setup_limits(request(1800), evidence)
        with pytest.raises(VmError, match="exceeds"):
            host._validate_setup_limits(
                request(), evidence.model_copy(update={"wall_budget_s": 3601})
            )
    finally:
        await orchestrator.close()
