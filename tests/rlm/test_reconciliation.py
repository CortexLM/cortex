"""Recover a measured VM result by identity without executing that job again."""

import json

import pytest

from cortex.rlm import (
    AgentTask,
    RlmEngine,
    Rule,
    RunJournal,
    ToolRejected,
    VmAction,
    VmContext,
    VmResult,
)
from cortex.rlm.provider import Completion
from cortex.vm.guest_server import ToolExecutor

PREFLIGHT = "1" * 64
EXPERIMENT = "2" * 64


def task():
    return AgentTask(
        context=VmContext(
            topic_id="topic-a",
            job_id="job-a",
            purpose="evaluate",
            image_digest="a" * 64,
            artifact_digest="b" * 64,
        ),
        objective="Evaluate the submitted artifact using the published policy.",
        metric="quality",
        rule_ids=["originality"],
        rules=[Rule(id="originality", description="No overlap", check="inspect")],
    )


def call(name, arguments, *, identifier="selected-tool"):
    return Completion(
        tool_calls=[
            {
                "id": identifier,
                "type": "function",
                "function": {"name": name, "arguments": json.dumps(arguments)},
            }
        ],
        prompt_tokens=100,
        completion_tokens=100,
    )


def experiment():
    return call("vm_execute", {"operation": "run", "phase": "experiment", "argv": ["measure"]})


def finish():
    return call(
        "finish",
        {
            "topic_id": "topic-a",
            "artifact_digest": "b" * 64,
            "rule_revision": 1,
            "outcome": "accepted",
            "explanation": "The isolated experiment measured this value.",
            "metric": "quality",
            "value": 0.75,
            "report_digest": EXPERIMENT,
            "rules_checked": ["originality"],
        },
        identifier="final-result",
    )


class ScriptedProvider:
    model = "operator/model"

    def __init__(self, responses):
        self.responses = iter(responses)
        self.requests = []

    async def complete(self, **kwargs):
        self.requests.append(json.loads(json.dumps(kwargs)))
        return next(self.responses)


class LedgerVm:
    def __init__(self, journal, *, interrupt="experiment", state="succeeded", red=False):
        self.journal = journal
        self.interrupt, self.state, self.red = interrupt, state, red
        self.executed, self.reconciled, self.intents = [], [], []
        self.results = {}

    async def execute(self, context, action):
        return await self.execute_once(context, action, f"legacy-{len(self.executed)}")

    async def execute_once(self, context, action, execution_id):
        self.executed.append((context, action, execution_id))
        self.intents.append(self.journal.load("topic-a/job-a")["state"]["pending"])
        preflight = action.phase == "preflight"
        result = VmResult(
            **context.model_dump(exclude={"purpose"}),
            sandboxed=True,
            network_enabled=False,
            execution_id=execution_id,
            report_digest=PREFLIGHT if preflight else EXPERIMENT,
            exit_code=0,
            rule_checks=[{"rule_id": "originality", "passed": not self.red}] if preflight else [],
            metrics=[] if preflight else [{"name": "quality", "value": 0.75}],
        )
        self.results[execution_id] = (context, action, result)
        if action.phase == self.interrupt:
            self.interrupt = None
            raise ConnectionError("response lost after the VM completed")
        return result

    async def reconcile(self, context, action, execution_id):
        self.reconciled.append((context, action, execution_id))
        if self.state != "succeeded":
            raise ToolRejected("VM result unavailable for reconciliation")
        previous_context, previous_action, result = self.results[execution_id]
        if previous_context != context or previous_action != action:
            raise ToolRejected("VM intent mismatch")
        return result


@pytest.fixture
def journal(tmp_path):
    store = RunJournal(tmp_path / "journal")
    yield store
    store.close()


@pytest.mark.parametrize("interrupted", ["preflight", "experiment"])
async def test_resume_reconciles_original_operation_without_new_spend(journal, interrupted):
    provider = ScriptedProvider([experiment(), finish()])
    vm = LedgerVm(journal, interrupt=interrupted)
    engine = RlmEngine(provider, vm, journal=journal)
    with pytest.raises(ConnectionError):
        await engine.run(task())
    charged = journal.load("topic-a/job-a")["state"]["tool_calls"]

    run = await RlmEngine(provider, vm, journal=journal).run(task(), resume=True)

    assert run.result.value == 0.75
    assert run.calls == 2 and run.tool_calls == 3
    assert charged == (1 if interrupted == "preflight" else 2)
    assert len(vm.executed) == 2 and len(vm.reconciled) == 1
    for (_, action, execution_id), intent in zip(vm.executed, vm.intents, strict=True):
        assert intent["execution_id"] == execution_id
        assert intent["action"] == action.model_dump(mode="json")
    interrupted_call = provider.requests[-1]["messages"][-2:]
    assert interrupted_call[0]["tool_calls"] == [experiment().tool_calls[0].model_dump(mode="json")]
    assert interrupted_call[1]["tool_call_id"] == "selected-tool"
    assert json.loads(interrupted_call[1]["content"])["report_digest"] == EXPERIMENT


async def test_reconciled_red_preflight_rejects_without_inference(journal):
    provider = ScriptedProvider([])
    vm = LedgerVm(journal, interrupt="preflight", red=True)
    with pytest.raises(ConnectionError):
        await RlmEngine(provider, vm, journal=journal).run(task())

    run = await RlmEngine(provider, vm, journal=journal).run(task(), resume=True)

    assert run.result.outcome == "rejected"
    assert not provider.requests
    assert len(vm.executed) == 1


@pytest.mark.parametrize("state", ["missing", "running", "failed"])
async def test_unresolved_vm_result_never_reexecutes_or_calls_model(journal, state):
    provider = ScriptedProvider([experiment(), finish()])
    vm = LedgerVm(journal, state=state)
    with pytest.raises(ConnectionError):
        await RlmEngine(provider, vm, journal=journal).run(task())

    with pytest.raises(ToolRejected, match="unavailable for reconciliation"):
        await RlmEngine(provider, vm, journal=journal).run(task(), resume=True)

    assert len(vm.executed) == 2
    assert len(provider.requests) == 1
    assert journal.load("topic-a/job-a")["state"]["pending"]["kind"] == "vm"


async def test_recovery_returns_original_result_to_recursive_child(journal):
    provider = ScriptedProvider(
        [
            call("delegate", {"objective": "Measure this artifact"}, identifier="delegate-child"),
            experiment(),
            call("finish", {"findings": "Measured quality", "evidence_digests": [EXPERIMENT]}),
            finish(),
        ]
    )
    vm = LedgerVm(journal)
    with pytest.raises(ConnectionError):
        await RlmEngine(provider, vm, journal=journal).run(task())

    run = await RlmEngine(provider, vm, journal=journal).run(task(), resume=True)

    assert run.calls == 4 and run.tool_calls == 5
    assert len(vm.executed) == 2
    recovered = [event for event in run.transcript if event.kind == "tool" and event.depth == 1]
    assert len(recovered) == 1
    child_request = json.loads(provider.requests[2]["messages"][1]["content"])
    assert child_request["recursion_depth"] == 1
    assert provider.requests[3]["messages"][-1]["tool_call_id"] == "delegate-child"


async def test_recovery_handles_evidence_recorded_before_checkpoint(journal, monkeypatch):
    provider = ScriptedProvider([experiment(), finish()])
    vm = LedgerVm(journal, interrupt=None)
    archive = journal.archive
    failed = False

    def interrupt_tool_event(run_id, body):
        nonlocal failed
        if not failed and json.loads(body) == json.loads(
            experiment().tool_calls[0].function.arguments
        ):
            failed = True
            raise ConnectionError("checkpoint interrupted after VM evidence arrived")
        return archive(run_id, body)

    monkeypatch.setattr(journal, "archive", interrupt_tool_event)
    with pytest.raises(ConnectionError):
        await RlmEngine(provider, vm, journal=journal).run(task())
    assert EXPERIMENT in journal.load("topic-a/job-a")["state"]["reports"]

    run = await RlmEngine(provider, vm, journal=journal).run(task(), resume=True)

    assert run.result.value == 0.75
    assert len(vm.executed) == 2
    assert len([event for event in run.transcript if event.name == "vm_execute"]) == 1


@pytest.mark.parametrize("change", ["arguments", "call", "frame"])
async def test_changed_pending_intent_fails_before_lookup_or_spend(journal, change):
    provider = ScriptedProvider([experiment(), finish()])
    vm = LedgerVm(journal)
    with pytest.raises(ConnectionError):
        await RlmEngine(provider, vm, journal=journal).run(task())
    saved = journal.load("topic-a/job-a")
    if change == "frame":
        saved["state"]["frames"][-1]["objective"] = "Different child objective"
    elif change == "call":
        saved["state"]["pending"]["call"]["id"] = "other-call"
    else:
        saved["state"]["pending"]["arguments"]["argv"] = ["different-job"]
    journal.save("topic-a/job-a", **{key: value for key, value in saved.items() if key != "status"})

    with pytest.raises(ToolRejected, match="intent|checkpoint"):
        await RlmEngine(provider, vm, journal=journal).run(task(), resume=True)

    assert not vm.reconciled
    assert len(vm.executed) == 2 and len(provider.requests) == 1


async def test_guest_executor_uses_same_id_for_execution_and_read_only_reconciliation():
    action = VmAction(operation="run", phase="experiment", argv=["measure"])
    requests = []

    class Callback:
        async def exchange(self, message):
            requests.append(message)
            context = task().context
            result = VmResult(
                **context.model_dump(exclude={"purpose"}),
                sandboxed=True,
                network_enabled=False,
                execution_id=message["request"]["execution_id"],
                report_digest=EXPERIMENT,
                exit_code=0,
            )
            return {"result": result.model_dump(mode="json")}

    executor = ToolExecutor(Callback())
    result = await executor.execute_once(task().context, action, "persisted-execution")
    recovered = await executor.reconcile(task().context, action, "persisted-execution")

    assert result == recovered
    assert [item["type"] for item in requests] == ["execute", "reconcile"]
    assert requests[0]["request"] == requests[1]["request"]


async def test_legacy_pending_vm_checkpoint_stays_closed(journal):
    provider = ScriptedProvider([experiment(), finish()])
    vm = LedgerVm(journal)
    with pytest.raises(ConnectionError):
        await RlmEngine(provider, vm, journal=journal).run(task())
    saved = journal.load("topic-a/job-a")
    saved["state"]["pending"] = {
        "kind": "vm",
        "arguments_digest": saved["state"]["pending"]["arguments_digest"],
    }
    journal.save("topic-a/job-a", **{key: value for key, value in saved.items() if key != "status"})

    with pytest.raises(ToolRejected, match="orchestrator reconciliation required"):
        await RlmEngine(provider, vm, journal=journal).run(task(), resume=True)

    assert not vm.reconciled
    assert len(vm.executed) == 2 and len(provider.requests) == 1


@pytest.mark.parametrize("field", ["execution_id", "job_id", "artifact_digest"])
async def test_reconciliation_refuses_a_result_from_another_execution(journal, field):
    provider = ScriptedProvider([experiment(), finish()])
    vm = LedgerVm(journal)
    with pytest.raises(ConnectionError):
        await RlmEngine(provider, vm, journal=journal).run(task())
    identity = vm.executed[-1][-1]
    context, action, result = vm.results[identity]
    vm.results[identity] = (context, action, result.model_copy(update={field: "c" * 64}))

    with pytest.raises(ToolRejected, match="does not bind"):
        await RlmEngine(provider, vm, journal=journal).run(task(), resume=True)

    assert len(vm.executed) == 2 and len(provider.requests) == 1
