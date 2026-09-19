"""Owner objective to exported setup and baseline through the actual guest RLM loop."""

import base64
import hashlib
import json

from cortex.rlm import AgentRequest, AgentTask
from cortex.rlm.models import VmContext
from cortex.rlm.provider import Completion
from cortex.vm.guest import GuestExecutor, GuestIdentity
from cortex.vm.guest_server import GuestServer
from cortex.vm.models import VmSpec
from cortex.vm.research import ResearchHost, ResearchRequest
from cortex.vm.runtime import Orchestrator

from .test_research import GuestTransport, MemoryCallback
from .test_runtime import FakeHypervisor


class ExecutingHypervisor(FakeHypervisor):
    def __init__(self, root):
        super().__init__()
        self.root, self.packs = root, {}

    async def execute(self, vm_id, spec, job):
        guest = GuestExecutor(
            GuestIdentity(vm_id, spec.topic_id, spec.image_digest, spec.kind),
            workspace=self.root / vm_id,
        )
        return await guest.execute(job)

    def install_setup(self, raw, digest):
        assert hashlib.sha256(raw).hexdigest() == digest
        self.packs[digest] = raw

    def prepare(self, job):
        if job.action.phase in {"preflight", "experiment"}:
            return job.model_copy(
                update={
                    "pack_b64": base64.b64encode(
                        self.packs[job.params["experiment_pack_digest"].removeprefix("sha256:")]
                    ).decode()
                }
            )
        return job


class SetupProvider:
    model = "operator/model"

    def __init__(self):
        self.calls = 0

    async def complete(self, *, messages, **kwargs):
        self.calls += 1
        if self.calls == 1:
            script = (
                "import json,os\nfrom pathlib import Path\n"
                "p=Path(os.environ['PROOF_SETUP_DIR'])\n"
                "(p/'holdout').write_text('private')\n"
                "(p/'inspect.py').write_text('print(1)')\n"
                "(p/'run.py').write_text(\"import json,os\\n"
                "json.dump({'metrics':[{'name':'quality','value':0.5}],'flops_used':17},"
                "open(os.environ['PROOF_OUTPUT_DIR']+'/report.json','w'))\\n\")\n"
                "json.dump({'content_hashes':['"
                + hashlib.sha256(b"private").hexdigest()
                + "'],'flops_budget':100,'wall_budget_s':30},"
                "open(os.environ['PROOF_OUTPUT_DIR']+'/setup.json','w'))"
            )
            name, arguments = (
                "vm_execute",
                {
                    "operation": "run",
                    "phase": "setup",
                    "argv": ["/usr/bin/python3", "-c", script],
                },
            )
        elif self.calls == 2:
            name, arguments = (
                "vm_execute",
                {
                    "operation": "run",
                    "phase": "experiment",
                    "argv": ["baseline"],
                },
            )
        else:
            results = [json.loads(item["content"]) for item in messages if item["role"] == "tool"]
            setup, baseline = results[-2:]
            artifacts = {item["kind"]: item["digest"] for item in setup["produced_artifacts"]}
            name, arguments = (
                "finish",
                {
                    "topic_id": "topic-a",
                    "title": "Operator objective",
                    "instructions": "Submit an artifact that improves the measured result.",
                    "rules": [
                        {"id": "originality", "description": "No overlap", "check": "inspect"}
                    ],
                    "endpoints": [
                        {
                            "method": "POST",
                            "suffix": "/submit",
                            "description": "Submit",
                            "request_schema": {},
                            "response_schema": {},
                        }
                    ],
                    "metric": "quality",
                    "direction": "higher",
                    "floor": 0.5,
                    "setup_report_digest": setup["report_digest"],
                    "baseline_report_digest": baseline["report_digest"],
                    "environment_digest": artifacts["environment"],
                    "private_holdout_digest": artifacts["private_holdout"],
                },
            )
        return Completion(
            tool_calls=[
                {
                    "id": str(self.calls),
                    "type": "function",
                    "function": {"name": name, "arguments": json.dumps(arguments)},
                }
            ],
            prompt_tokens=100,
            completion_tokens=100,
        )


async def test_agent_installs_and_exports_then_runs_measured_baseline(tmp_path, monkeypatch):
    hv = ExecutingHypervisor(tmp_path / "guests")
    orchestrator = Orchestrator(tmp_path / "jobs.sqlite3", hv)
    vm = await orchestrator.create(VmSpec(topic_id="topic-a", image_digest="ab" * 32))
    callback = MemoryCallback()
    monkeypatch.setattr("cortex.vm.research.asyncio.start_unix_server", callback.start)
    monkeypatch.setattr("cortex.vm.research.os.chmod", lambda path, mode, **kwargs: None)
    guest = GuestServer(
        GuestIdentity(vm.vm_id, "topic-a", "ab" * 32, "topic"),
        workspace=tmp_path / "rlm",
        callback=callback,
    )
    research = ResearchHost(
        orchestrator,
        SetupProvider(),
        lambda vm_id: tmp_path / "v.sock",
        transport=GuestTransport(guest),
    )
    task = AgentTask(
        context=VmContext(
            topic_id="topic-a", job_id="setup-a", purpose="setup", image_digest="ab" * 32
        ),
        objective="Prepare the operator topic and measure its baseline.",
    )
    try:
        outcome = await research.run(vm.vm_id, ResearchRequest(request=AgentRequest(task=task)))
        assert outcome.setup_evidence.teardown_confirmed
        assert (
            outcome.setup_evidence.baseline_report_digest
            == outcome.run.result.baseline_report_digest
        )
        assert outcome.setup_evidence.content_hashes == [hashlib.sha256(b"private").hexdigest()]
        assert outcome.reports[-1].metrics[0].value == 0.5
        assert outcome.reports[-1].flops_used == 17
        assert outcome.executions[-1].vm_id != vm.vm_id
        assert outcome.executions[-1].teardown_confirmed
    finally:
        await research.close()
        await orchestrator.close()
