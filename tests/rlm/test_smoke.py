"""The model smoke must demonstrate compaction, not merely write provenance."""

import argparse
import hashlib
import json
import runpy
from pathlib import Path

import httpx
import pytest

SMOKE = runpy.run_path(str(Path(__file__).resolve().parents[2] / "scripts" / "rlm_smoke.py"))


def completion(name, arguments):
    return {
        "choices": [
            {
                "finish_reason": "tool_calls",
                "message": {
                    "role": "assistant",
                    "content": None,
                    "tool_calls": [
                        {
                            "id": "synthetic-call",
                            "type": "function",
                            "function": {"name": name, "arguments": json.dumps(arguments)},
                        }
                    ],
                },
            }
        ],
        "usage": {"prompt_tokens": 20, "completion_tokens": 30, "total_tokens": 50},
    }


def scripted_provider(monkeypatch, *, inspect):
    def report(number):
        return hashlib.sha256(f"synthetic-execution-{number}".encode()).hexdigest()

    calls = [completion("delegate", {"objective": "Inspect the synthetic fixture once"})]
    if inspect:
        calls.append(
            completion("vm_execute", {"operation": "run", "phase": "inspect", "argv": ["inspect"]})
        )
    calls.extend(
        [
            completion(
                "finish",
                {
                    "findings": "Synthetic observation",
                    "evidence_digests": [report(2 if inspect else 1)],
                },
            ),
            completion("vm_execute", {"operation": "run", "phase": "experiment", "argv": ["run"]}),
            completion(
                "finish",
                {
                    "topic_id": "synthetic-smoke",
                    "artifact_digest": hashlib.sha256(b"synthetic-artifact").hexdigest(),
                    "rule_revision": 1,
                    "outcome": "accepted",
                    "explanation": "Synthetic fixtures only; no scientific execution",
                    "metric": "fixture_quality",
                    "value": 0.8,
                    "report_digest": report(3 if inspect else 2),
                    "rules_checked": ["fixture-integrity"],
                },
            ),
        ]
    )
    responses = iter(calls)
    requests = []

    def respond(request):
        requests.append(json.loads(request.content))
        return httpx.Response(200, json=next(responses))

    real_client = httpx.AsyncClient
    monkeypatch.setattr(
        httpx,
        "AsyncClient",
        lambda **kwargs: real_client(transport=httpx.MockTransport(respond), **kwargs),
    )
    return requests


@pytest.mark.parametrize("inspect", [True, False])
async def test_smoke_requires_model_observed_compaction_not_provenance_alone(
    tmp_path, monkeypatch, capsys, inspect
):
    requests = scripted_provider(monkeypatch, inspect=inspect)
    key = tmp_path / "synthetic-key"
    key.touch(mode=0o600)
    key.write_text("synthetic-fixture-key")
    args = argparse.Namespace(
        key_file=key,
        state_dir=tmp_path / "private-state",
        model="deepseek/deepseek-v4.1-flash",
        resume=False,
    )

    if inspect:
        await SMOKE["run"](args)
        report = json.loads(capsys.readouterr().out)
        assert report["compaction_archives"] == 1
        assert report["calls"] == 5
        assert report["recursive"] is True
    else:
        with pytest.raises(RuntimeError, match="recursive fixture evaluation"):
            await SMOKE["run"](args)
        assert capsys.readouterr().out == ""
    assert len(requests) == (5 if inspect else 4)
