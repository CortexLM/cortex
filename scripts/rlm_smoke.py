"""Opt-in real-model test; every experiment boundary here is deliberately synthetic.

This verifies model/tool interoperability, recursion and compaction. It does not
start a VM, run miner code, perform scientific reproduction or submit chain weights.
"""

from __future__ import annotations

import argparse
import asyncio
import hashlib
import json
from pathlib import Path

from cortex.rlm import (
    AgentLimits,
    AgentRun,
    AgentTask,
    EvaluationVerdict,
    Metric,
    RlmEngine,
    Rule,
    RuleCheck,
    RunJournal,
    VmContext,
    VmResult,
)
from cortex.rlm.provider import OpenRouterClient


class SyntheticExecutor:
    def __init__(self):
        self.calls = 0

    async def execute(self, context, action):
        self.calls += 1
        report = hashlib.sha256(f"synthetic-execution-{self.calls}".encode()).hexdigest()
        return VmResult(
            topic_id=context.topic_id,
            job_id=context.job_id,
            image_digest=context.image_digest,
            artifact_digest=context.artifact_digest,
            execution_id=f"synthetic-{self.calls}",
            report_digest=report,
            sandboxed=True,
            network_enabled=False,
            exit_code=0,
            stdout_tail=(
                "Synthetic observation; no scientific or VM evidence. " * 240
                + "inspection_complete=true; fixture_integrity=valid; evidence_kind=synthetic."
            )
            if action.phase == "inspect"
            else "Synthetic fixture completed.",
            rule_checks=[RuleCheck(rule_id="fixture-integrity", passed=True)]
            if action.phase == "preflight"
            else [],
            metrics=[Metric(name="fixture_quality", value=0.8)]
            if action.phase == "experiment"
            else [],
        )


def compaction_archives(journal: RunJournal, run_id: str, run: AgentRun) -> int:
    archives = set()
    for event in run.transcript:
        if event.kind != "model":
            continue
        request = json.loads(journal.read_archive(run_id, event.input_digest))
        for message in request["messages"]:
            if message.get("role") != "user":
                continue
            try:
                content = json.loads(message.get("content", ""))
            except (ValueError, TypeError):
                continue
            if (
                not isinstance(content, dict)
                or content.get("kind") != "untrusted_compacted_history"
            ):
                continue
            digest = content["archive_digest"]
            removed = json.loads(journal.read_archive(run_id, digest))
            if not isinstance(removed, list) or not removed:
                raise RuntimeError("invalid smoke compaction archive")
            archives.add(digest)
    return len(archives)


async def run(args):
    task = AgentTask(
        context=VmContext(
            topic_id="synthetic-smoke",
            job_id="live-model-smoke",
            purpose="evaluate",
            image_digest=hashlib.sha256(b"synthetic-image-not-a-real-pin").hexdigest(),
            artifact_digest=hashlib.sha256(b"synthetic-artifact").hexdigest(),
        ),
        objective=(
            "Run the synthetic interoperability smoke. This executor produces synthetic fixtures, "
            "not real VM or scientific evidence. First delegate exactly one subtask with this "
            "objective: execute exactly one vm_execute phase inspect argv [inspect], then "
            "immediately finish with a "
            "ResearchSummary citing only its report_digest as evidence. An archive_digest "
            "identifies memory text and must not be cited as execution evidence. "
            "The long fixture output contains repeated synthetic padding to force compaction. "
            "The compacted completed_phase and report_digest provide all required evidence. "
            "Do not repeat inspect or read padding with memory_read; "
            "this is not iterative research. "
            "After the child returns, "
            "run vm_execute phase experiment argv [run] and finish with its measured "
            "fixture metric. "
            "Do not repeat preflight, which already ran automatically. Describe synthetic evidence "
            "honestly in explanation."
        ),
        metric="fixture_quality",
        rule_ids=["fixture-integrity"],
        rules=[
            Rule(
                id="fixture-integrity",
                description="Synthetic fixture integrity",
                check="The synthetic preflight reports fixture-integrity",
            )
        ],
    )
    args.state_dir.mkdir(parents=True, mode=0o700, exist_ok=True)
    journal = RunJournal(args.state_dir)
    try:
        engine = RlmEngine(
            OpenRouterClient(model=args.model, api_key_file=args.key_file),
            SyntheticExecutor(),
            limits=AgentLimits(
                max_calls=10,
                max_tokens=100000,
                wall_seconds=180.0,
                context_bytes=16384,
                compact_keep_exchanges=0,
                completion_tokens=2048,
            ),
            journal=journal,
        )
        result = await engine.run(task, resume=args.resume)
        recursive = any(event.depth > 0 for event in result.transcript)
        archive_count = compaction_archives(
            journal, f"{task.context.topic_id}/{task.context.job_id}", result
        )
        verdict = result.result
        if (
            not isinstance(verdict, EvaluationVerdict)
            or verdict.outcome != "accepted"
            or verdict.value != 0.8
            or not recursive
            or archive_count == 0
        ):
            raise RuntimeError("model did not complete the recursive fixture evaluation")
        print(
            json.dumps(
                {
                    "model": args.model,
                    "calls": result.calls,
                    "tokens": result.tokens,
                    "tool_calls": result.tool_calls,
                    "recursive": recursive,
                    "compaction_archives": archive_count,
                    "outcome": verdict.outcome,
                    "synthetic_execution": True,
                }
            )
        )
    finally:
        journal.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--key-file", type=Path, required=True)
    parser.add_argument("--state-dir", type=Path, required=True)
    parser.add_argument("--model", default="deepseek/deepseek-v4.1-flash")
    parser.add_argument("--resume", action="store_true")
    args = parser.parse_args()
    try:
        asyncio.run(run(args))
    except Exception as error:
        from cortex.rlm.errors import RlmError

        detail = str(error) if isinstance(error, RlmError) else type(error).__name__
        raise SystemExit(f"RLM smoke failed: {detail}") from None


if __name__ == "__main__":
    main()
