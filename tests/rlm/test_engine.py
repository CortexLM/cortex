from __future__ import annotations

import asyncio
import json
from pathlib import Path

import httpx
import pytest

from cortex.rlm import (
    AgentLimits,
    AgentTask,
    BudgetExceeded,
    InvalidResponse,
    Metric,
    OpenRouterClient,
    ProducedArtifact,
    ProviderError,
    RlmEngine,
    Rule,
    RuleCheck,
    RunJournal,
    ToolRejected,
    VmContext,
    VmResult,
)

IMAGE = "1" * 64
ARTIFACT = "2" * 64
REPORT = "3" * 64
ENVIRONMENT = "4" * 64
HOLDOUT = "5" * 64
EXPERIMENT = "6" * 64


def task(*, evaluate=False):
    return AgentTask(
        context=VmContext(
            topic_id="operator-topic",
            job_id="job-1",
            purpose="evaluate" if evaluate else "setup",
            image_digest=IMAGE,
            artifact_digest=ARTIFACT if evaluate else None,
        ),
        objective="Prepare the owner-defined research objective.",
        rule_ids=["originality"] if evaluate else [],
        rules=[Rule(id="originality", description="No holdout overlap", check="Check overlap")]
        if evaluate
        else [],
        metric="quality" if evaluate else None,
    )


def setup_proposal():
    return {
        "topic_id": "operator-topic",
        "title": "Operator-defined topic",
        "instructions": "Submit an artifact following the generated request schema.",
        "rules": [
            {"id": "originality", "description": "No holdout overlap", "check": "Check overlap"}
        ],
        "endpoints": [
            {
                "method": "POST",
                "suffix": "/submissions",
                "description": "Submit an artifact",
                "request_schema": {"type": "object"},
                "response_schema": {"type": "object"},
            }
        ],
        "metric": "quality",
        "direction": "higher",
        "floor": 0.5,
        "setup_report_digest": REPORT,
        "baseline_report_digest": REPORT,
        "private_holdout_digest": HOLDOUT,
        "environment_digest": ENVIRONMENT,
    }


def verdict(**changes):
    return {
        "topic_id": "operator-topic",
        "artifact_digest": ARTIFACT,
        "rule_revision": 1,
        "outcome": "accepted",
        "explanation": "All published checks passed and the experiment measured this result.",
        "metric": "quality",
        "value": 0.8,
        "report_digest": EXPERIMENT,
        "rules_checked": ["originality"],
        **changes,
    }


def tool(name, arguments):
    return {
        "choices": [
            {
                "finish_reason": "tool_calls",
                "message": {
                    "role": "assistant",
                    "content": None,
                    "tool_calls": [
                        {
                            "id": "call-1",
                            "type": "function",
                            "function": {"name": name, "arguments": json.dumps(arguments)},
                        }
                    ],
                },
            }
        ],
        "usage": {"prompt_tokens": 20, "completion_tokens": 30, "total_tokens": 50},
    }


def command(phase="setup"):
    return tool("vm_execute", {"operation": "run", "phase": phase, "argv": ["job-adaptor"]})


def report(*, evaluate=False, **changes):
    return VmResult(
        **{
            "topic_id": "operator-topic",
            "job_id": "job-1",
            "image_digest": IMAGE,
            "artifact_digest": ARTIFACT if evaluate else None,
            "sandboxed": True,
            "network_enabled": not evaluate,
            "execution_id": "run-1",
            "report_digest": REPORT,
            "exit_code": 0,
            "metrics": [Metric(name="quality", value=0.8)],
            "produced_artifacts": [
                ProducedArtifact(kind="environment", digest=ENVIRONMENT),
                ProducedArtifact(kind="private_holdout", digest=HOLDOUT),
            ],
            **changes,
        }
    )


class ScriptedHTTP:
    def __init__(self, responses):
        self.responses = iter(responses)
        self.requests = []

    def __call__(self, request):
        self.requests.append(json.loads(request.content))
        response = next(self.responses)
        return (
            response if isinstance(response, httpx.Response) else httpx.Response(200, json=response)
        )


class VM:
    def __init__(self, results):
        self.results = iter(results)
        self.requests = []

    async def execute(self, context, action):
        self.requests.append((context, action))
        return next(self.results)


def engine(tmp_path: Path, responses, results=(), *, limits=None, journal=None):
    key = tmp_path / "provider-key"
    key.write_text("test-only-not-a-real-key")
    key.chmod(0o600)
    transport = ScriptedHTTP(responses)
    provider = OpenRouterClient(
        model="test/research-model",
        api_key_file=key,
        client=httpx.AsyncClient(transport=httpx.MockTransport(transport)),
    )
    vm = VM(results)
    return RlmEngine(provider, vm, limits=limits, journal=journal), transport, vm


async def test_setup_requires_real_vm_evidence_and_records_provenance(tmp_path):
    runner, http, vm = engine(tmp_path, [command(), tool("finish", setup_proposal())], [report()])

    run = await runner.run(task())

    assert run.result.environment_digest == ENVIRONMENT
    assert run.result.baseline_report_digest == REPORT
    assert [event.kind for event in run.transcript] == ["model", "tool", "model", "finish"]
    assert run.calls == 2 and run.tokens == 100
    assert vm.requests[0][0] == task().context
    assert http.requests[0]["provider"]["allow_fallbacks"] is False
    assert "test-only-not-a-real-key" not in run.model_dump_json()


async def test_evaluation_returns_only_measured_metric_after_attested_preflight(tmp_path):
    runner, _, vm = engine(
        tmp_path,
        [command("experiment"), tool("finish", verdict())],
        [
            report(evaluate=True, rule_checks=[RuleCheck(rule_id="originality", passed=True)]),
            report(evaluate=True, report_digest=EXPERIMENT, execution_id="run-2"),
        ],
    )

    run = await runner.run(task(evaluate=True))

    assert run.result.value == 0.8
    assert len(vm.requests) == 2


async def test_red_preflight_stops_all_paid_inference_and_experiments(tmp_path):
    runner, http, vm = engine(
        tmp_path,
        [command("preflight"), command("experiment")],
        [report(evaluate=True, rule_checks=[RuleCheck(rule_id="originality", passed=False)])],
    )

    run = await runner.run(task(evaluate=True))

    assert run.result.outcome == "rejected" and run.result.value is None
    assert len(vm.requests) == 1
    assert not http.requests


async def test_missing_preflight_checks_prevent_inference_and_experiment(tmp_path):
    runner, http, vm = engine(tmp_path, [command("experiment")], [report(evaluate=True)])

    with pytest.raises(ToolRejected, match="checks must pass"):
        await runner.run(task(evaluate=True))

    assert len(vm.requests) == 1
    assert not http.requests


@pytest.mark.parametrize(
    ("name", "arguments"),
    [
        ("host_shell", {"command": "cat /etc/passwd"}),
        ("vm_execute", {"operation": "run", "argv": ["python"], "host": True}),
        ("vm_execute", {"operation": "read_file", "path": "/workspace/../etc/passwd"}),
        ("vm_execute", {"operation": "run", "argv": ["python"], "timeout_seconds": "1"}),
    ],
)
async def test_malicious_or_unknown_tools_are_rejected_before_execution(tmp_path, name, arguments):
    runner, _, vm = engine(tmp_path, [tool(name, arguments)])

    with pytest.raises(ToolRejected):
        await runner.run(task())

    assert not vm.requests


@pytest.mark.parametrize("change", [{"job_id": "another-job"}, {"image_digest": "a" * 64}])
async def test_vm_results_from_another_execution_cannot_supply_evidence(tmp_path, change):
    runner, _, _ = engine(tmp_path, [command()], [report(**change)])

    with pytest.raises(ToolRejected, match="does not bind"):
        await runner.run(task())


async def test_networked_evaluation_vm_is_rejected(tmp_path):
    runner, _, _ = engine(
        tmp_path, [command("preflight")], [report(evaluate=True, network_enabled=True)]
    )

    with pytest.raises(ToolRejected, match="no network"):
        await runner.run(task(evaluate=True))


async def test_model_cannot_forge_better_measurement(tmp_path):
    runner, _, _ = engine(
        tmp_path,
        [command("experiment"), tool("finish", verdict(value=1.0))],
        [
            report(evaluate=True, rule_checks=[RuleCheck(rule_id="originality", passed=True)]),
            report(evaluate=True, report_digest=EXPERIMENT),
        ],
    )

    with pytest.raises(InvalidResponse, match="VM-measured metric"):
        await runner.run(task(evaluate=True))


async def test_preflight_measurement_cannot_substitute_for_paid_experiment(tmp_path):
    runner, _, _ = engine(
        tmp_path,
        [tool("finish", verdict(report_digest=REPORT))],
        [report(evaluate=True, rule_checks=[RuleCheck(rule_id="originality", passed=True)])],
    )

    with pytest.raises(InvalidResponse, match="experiment report"):
        await runner.run(task(evaluate=True))


async def test_setup_cannot_invent_holdout_digest(tmp_path):
    proposal = setup_proposal() | {"private_holdout_digest": "f" * 64}
    runner, _, _ = engine(tmp_path, [command(), tool("finish", proposal)], [report()])

    with pytest.raises(InvalidResponse, match="production evidence"):
        await runner.run(task())


async def test_recursive_children_share_parent_model_budget(tmp_path):
    runner, http, _ = engine(
        tmp_path,
        [tool("delegate", {"objective": "investigate"})] * 3,
        limits=AgentLimits(max_calls=2),
    )

    with pytest.raises(BudgetExceeded, match="model-call"):
        await runner.run(task())

    assert len(http.requests) == 2


async def test_recursive_depth_limit_applies_before_child_inference(tmp_path):
    runner, http, _ = engine(
        tmp_path,
        [tool("delegate", {"objective": "investigate"})],
        limits=AgentLimits(max_depth=0),
    )

    with pytest.raises(BudgetExceeded, match="recursion"):
        await runner.run(task())

    assert len(http.requests) == 1


async def test_recursive_research_preserves_owner_policy_and_returns_real_child_evidence(tmp_path):
    runner, http, _ = engine(
        tmp_path,
        [
            tool("delegate", {"objective": "Inspect the evidence in a subtask"}),
            command(),
            tool(
                "finish",
                {
                    "findings": "Environment and baseline measured. " * 300,
                    "evidence_digests": [REPORT],
                },
            ),
            tool("finish", setup_proposal()),
        ],
        [report(stdout_tail="Synthetic observed evidence " * 500)],
        limits=AgentLimits(context_bytes=8192, compact_keep_exchanges=0),
    )

    run = await runner.run(task())

    child_input = json.loads(http.requests[1]["messages"][1]["content"])
    assert child_input["owner_objective"] == task().objective
    assert child_input["investigation"] == "Inspect the evidence in a subtask"
    assert child_input["recursion_depth"] == 1
    assert child_input["return_type"] == "ResearchSummary"
    assert run.calls == 4
    assert any(event.depth == 1 and event.kind == "finish" for event in run.transcript)
    assert run.result.baseline_report_digest == REPORT
    compacted = json.loads(http.requests[-1]["messages"][2]["content"])
    assert compacted["completed_subtasks"][0]["objective"] == "Inspect the evidence in a subtask"


@pytest.mark.parametrize("invalid", ["archive", "schema"])
async def test_child_can_correct_invalid_summary_without_repeating_vm_work(tmp_path, invalid):
    journal = RunJournal(tmp_path / "journal")
    archive = journal.archive("operator-topic/job-1", "synthetic archived observations")
    arguments = {"findings": "Observed setup", "evidence_digests": [REPORT, archive]}
    if invalid == "schema":
        arguments["findings"] = {"private-invalid-field": "not a string"}
    runner, http, vm = engine(
        tmp_path,
        [
            tool("delegate", {"objective": "Inspect the setup"}),
            command(),
            tool("finish", arguments),
            tool("finish", {"findings": "Observed setup", "evidence_digests": [REPORT]}),
            tool("finish", setup_proposal()),
        ],
        [report()],
        journal=journal,
    )
    try:
        run = await runner.run(task())

        assert run.result.baseline_report_digest == REPORT
        assert run.calls == 5 and run.tool_calls == 5
        assert len(vm.requests) == 1
        correction = json.loads(http.requests[3]["messages"][-1]["content"])
        assert correction["error"] == "invalid_research_summary"
        assert correction["recent_report_digests"] == [REPORT]
        assert archive not in correction["recent_report_digests"]
        assert "private-invalid-field" not in json.dumps(correction)
        assert len([event for event in run.transcript if event.kind == "finish"]) == 2
    finally:
        journal.close()


async def test_repeated_invalid_child_summaries_exhaust_shared_budget_without_result(tmp_path):
    journal = RunJournal(tmp_path / "journal")
    archive = journal.archive("operator-topic/job-1", "synthetic archived observations")
    invalid = tool("finish", {"findings": "Observed setup", "evidence_digests": [archive]})
    runner, http, vm = engine(
        tmp_path,
        [tool("delegate", {"objective": "Inspect the setup"}), command(), invalid, invalid],
        [report()],
        limits=AgentLimits(max_calls=4),
        journal=journal,
    )
    try:
        with pytest.raises(BudgetExceeded, match="model-call"):
            await runner.run(task())

        assert len(http.requests) == 4
        assert len(vm.requests) == 1
        saved = journal.load("operator-topic/job-1")
        assert saved["status"] != "complete"
        assert saved["state"]["result"] is None
        assert saved["state"]["completed_subtasks"] == []
    finally:
        journal.close()


async def test_child_summary_feedback_survives_restart_without_vm_replay(tmp_path):
    journal_path = tmp_path / "journal"
    journal = RunJournal(journal_path)
    archive = journal.archive("operator-topic/job-1", "synthetic archived observations")
    first, _, first_vm = engine(
        tmp_path,
        [
            tool("delegate", {"objective": "Inspect the setup"}),
            command(),
            tool("finish", {"findings": "Observed setup", "evidence_digests": [archive]}),
            httpx.Response(503),
        ],
        [report()],
        journal=journal,
    )
    try:
        with pytest.raises(ProviderError):
            await first.run(task())
    finally:
        journal.close()
    restored = RunJournal(journal_path)
    resumed, http, second_vm = engine(
        tmp_path,
        [
            tool("finish", {"findings": "Observed setup", "evidence_digests": [REPORT]}),
            tool("finish", setup_proposal()),
        ],
        journal=restored,
    )
    try:
        run = await resumed.run(task(), resume=True)

        assert run.calls == 6
        assert len(first_vm.requests) == 1 and not second_vm.requests
        feedback = json.loads(http.requests[0]["messages"][-1]["content"])
        assert feedback["error"] == "invalid_research_summary"
        assert feedback["recent_report_digests"] == [REPORT]
        assert run.result.baseline_report_digest == REPORT
    finally:
        restored.close()


async def test_token_budget_exhaustion_prevents_network_request(tmp_path):
    runner, http, _ = engine(tmp_path, [], limits=AgentLimits(max_tokens=256))

    with pytest.raises(BudgetExceeded, match="token budget"):
        await runner.run(task())

    assert not http.requests


async def test_hung_vm_tool_is_cancelled_at_deadline(tmp_path):
    runner, _, _ = engine(
        tmp_path,
        [tool("vm_execute", {"operation": "run", "argv": ["run"], "timeout_seconds": 1})],
        limits=AgentLimits(tool_timeout_seconds=1.0),
    )

    class HungVM:
        async def execute(self, context, action):
            await asyncio.Event().wait()

    runner.executor = HungVM()

    with pytest.raises(BudgetExceeded, match="VM tool deadline"):
        await runner.run(task())


async def test_tool_timeout_over_ceiling_rejected_without_starting_vm(tmp_path):
    runner, _, vm = engine(tmp_path, [command()], limits=AgentLimits(tool_timeout_seconds=1.0))

    with pytest.raises(ToolRejected, match="tool ceiling"):
        await runner.run(task())

    assert not vm.requests


async def test_signed_topic_timeout_rejected_without_starting_vm(tmp_path):
    runner, _, vm = engine(tmp_path, [command()])

    with pytest.raises(ToolRejected, match="signed topic wall budget"):
        await runner.run(task().model_copy(update={"wall_budget_s": 10}))

    assert not vm.requests


async def test_remaining_wall_budget_never_silently_shortens_tool_request(tmp_path):
    runner, _, vm = engine(tmp_path, [command()], limits=AgentLimits(wall_seconds=1.0))

    with pytest.raises(BudgetExceeded, match="cannot fit"):
        await runner.run(task())

    assert not vm.requests


async def test_compaction_keeps_policy_evidence_and_exact_archived_history(tmp_path):
    journal = RunJournal(tmp_path / "journal")
    runner, http, _ = engine(
        tmp_path,
        [command(), tool("finish", setup_proposal())],
        [report(stdout_tail="untrusted miner text " * 700)],
        limits=AgentLimits(context_bytes=8192, compact_keep_exchanges=0),
        journal=journal,
    )

    run = await runner.run(task())

    first, second = http.requests
    assert first["messages"][:2] == second["messages"][:2]
    compacted = json.loads(second["messages"][2]["content"])
    assert compacted["evidence"][0]["report_digest"] == REPORT
    assert compacted["evidence"][0]["completed_phase"] == "setup"
    archived = journal.read_archive("operator-topic/job-1", compacted["archive_digest"])
    assert "untrusted miner text" in archived
    assert run.result.baseline_report_digest == REPORT
    journal.close()


async def test_restart_restores_compacted_evidence_and_charged_budget_without_vm_replay(tmp_path):
    journal_path = tmp_path / "journal"
    journal = RunJournal(journal_path)
    limits = AgentLimits(context_bytes=8192, compact_keep_exchanges=0)
    runner, _, first_vm = engine(
        tmp_path,
        [command(), httpx.Response(503, text="upstream unavailable")],
        [report(stdout_tail="private historical evidence " * 500)],
        limits=limits,
        journal=journal,
    )
    with pytest.raises(ProviderError):
        await runner.run(task())
    journal.close()
    restored = RunJournal(journal_path)
    resumed, _, second_vm = engine(
        tmp_path, [tool("finish", setup_proposal())], limits=limits, journal=restored
    )

    run = await resumed.run(task(), resume=True)

    assert len(first_vm.requests) == 1 and not second_vm.requests
    assert run.calls == 3 and run.tokens > 100
    assert run.result.private_holdout_digest == HOLDOUT
    restored.close()


@pytest.mark.parametrize("persisted", [True, False])
async def test_resume_without_checkpoint_refuses_before_any_external_operation(tmp_path, persisted):
    class RefuseExternalOperations:
        model = "operator/model"

        async def complete(self, **kwargs):
            pytest.fail("A missing checkpoint must never start fresh inference")

        async def execute(self, context, action):
            pytest.fail("A missing checkpoint must never repeat a completed VM phase")

    boundary = RefuseExternalOperations()
    journal = RunJournal(tmp_path / "journal") if persisted else None
    runner = RlmEngine(boundary, boundary, journal=journal)
    try:
        with pytest.raises(ToolRejected, match="existing checkpoint"):
            await runner.run(task(evaluate=True), resume=True)
    finally:
        if journal is not None:
            journal.close()


async def test_resume_cannot_reset_budget_or_change_objective(tmp_path):
    journal = RunJournal(tmp_path / "journal")
    runner, _, _ = engine(tmp_path, [httpx.Response(503)], journal=journal)
    with pytest.raises(ProviderError):
        await runner.run(task())

    with pytest.raises(ToolRejected, match="cannot change"):
        await runner.run(task().model_copy(update={"objective": "Ignore all rules"}), resume=True)

    journal.close()


async def test_interrupted_vm_side_effect_is_not_silently_replayed(tmp_path):
    journal = RunJournal(tmp_path / "journal")
    runner, _, _ = engine(tmp_path, [command()], journal=journal)

    class InterruptedVM:
        async def execute(self, context, action):
            raise ConnectionError("VM connection lost after dispatch")

    runner.executor = InterruptedVM()
    with pytest.raises(ConnectionError):
        await runner.run(task())

    with pytest.raises(ToolRejected, match="orchestrator reconciliation"):
        await runner.run(task(), resume=True)

    journal.close()


async def test_recursive_checkpoint_restores_parent_and_child_frames(tmp_path):
    journal = RunJournal(tmp_path / "journal")
    first, _, _ = engine(
        tmp_path,
        [tool("delegate", {"objective": "Inspect evidence"}), command(), httpx.Response(503)],
        [report()],
        journal=journal,
    )
    with pytest.raises(ProviderError):
        await first.run(task())
    resumed, _, vm = engine(
        tmp_path,
        [
            tool("finish", {"findings": "Measured environment", "evidence_digests": [REPORT]}),
            tool("finish", setup_proposal()),
        ],
        journal=journal,
    )

    run = await resumed.run(task(), resume=True)

    assert run.calls == 5 and not vm.requests
    for event in run.transcript:
        journal.read_archive("operator-topic/job-1", event.input_digest)
        journal.read_archive("operator-topic/job-1", event.output_digest)
    assert await resumed.run(task(), resume=True) == run
    journal.close()
