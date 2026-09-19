from __future__ import annotations

import asyncio

import httpx
import pytest
from fastapi import FastAPI
from fastapi.responses import JSONResponse

from cortex.errors import ServiceError
from cortex.http import OperatorAuth
from cortex.proof.models import Metric, digest
from cortex.proof.service import Readiness
from cortex.proof.setup import SetupPolicy, TopicSetup, create_setup_router
from cortex.protocol.crypto import public_key
from cortex.rlm import AgentRun, SetupProposal, VmResult
from cortex.vm.research import ExecutionEvidence, ResearchOutcome
from cortex.vm.setup import SetupEvidence

OWNER = bytes([17]) * 32
SETUP = "1" * 64
BASELINE = "2" * 64
ENVIRONMENT = "3" * 64
SCRIPT = "4" * 64
PRIVATE = "5" * 64


def policy(**updates):
    return SetupPolicy(
        **{
            "topic_id": "new-research",
            "objective": "Investigate the operator-defined research objective",
            "metric": Metric(
                family="custom",
                custom_id="fixture-runner",
                primary="quality",
                direction="max",
                epsilon=0.1,
            ),
            "flops_budget": 100,
            "wall_budget_s": 30,
            **updates,
        }
    )


class SetupVmBoundary:
    def __init__(self, mutation=None):
        self.calls = []
        self.mutation = mutation or (lambda outcome: outcome)
        self.gate = asyncio.Event()
        self.entered = asyncio.Event()
        self.gate.set()

    async def readiness(self):
        return Readiness("sha256:" + "ab" * 32, "bc" * 32, frozenset({"fixture-runner"}))

    async def run_agent(self, task, *, artifact=None, env=None, params=None):
        self.calls.append((task, artifact, env, params))
        self.entered.set()
        await self.gate.wait()
        holdout = digest({"content_hashes": [PRIVATE], "dataset_ids": ["private-dataset-v1"]})
        proposal = SetupProposal.model_validate(
            {
                "topic_id": task.context.topic_id,
                "title": "Generated research challenge",
                "instructions": "Provide code and the signed artifact for reproduction.",
                "rules": [
                    {
                        "id": "artifact-integrity",
                        "description": "Artifact bytes must match the submitted digest",
                        "check": "Hash the received artifact and compare its commitment",
                        "failure": "reject",
                    }
                ],
                "endpoints": [
                    {
                        "method": "POST",
                        "suffix": "/compete",
                        "purpose": "submission",
                        "description": "Submit a signed experiment",
                        "request_schema": {"type": "object"},
                        "response_schema": {"type": "object"},
                    },
                    {
                        "method": "GET",
                        "suffix": "/instructions",
                        "purpose": "documentation",
                        "description": "Read the topic instructions",
                        "request_schema": {},
                        "response_schema": {"type": "string"},
                    },
                ],
                "metric": "quality",
                "direction": "higher",
                "floor": 0.15,
                "setup_report_digest": SETUP,
                "baseline_report_digest": BASELINE,
                "private_holdout_digest": holdout,
                "environment_digest": ENVIRONMENT,
            }
        )
        reports = [
            VmResult.model_validate(
                {
                    "topic_id": task.context.topic_id,
                    "job_id": task.context.job_id,
                    "image_digest": task.context.image_digest,
                    "sandboxed": True,
                    "network_enabled": False,
                    "execution_id": execution_id,
                    "report_digest": report_digest,
                    "exit_code": 0,
                    "metrics": [{"name": "quality", "value": 0.5}],
                    "flops_used": 10,
                    "produced_artifacts": [
                        {"kind": "environment", "digest": ENVIRONMENT},
                        {"kind": "private_holdout", "digest": holdout},
                    ],
                }
            )
            for execution_id, report_digest in [("setup-tool", SETUP), ("baseline-tool", BASELINE)]
        ]
        outcome = ResearchOutcome(
            run=AgentRun(result=proposal, transcript=[], calls=3, tool_calls=3, tokens=300),
            reports=reports,
            wall_seconds=1.0,
            executions=[
                ExecutionEvidence(
                    execution_id="baseline-tool",
                    vm_id="sister-vm",
                    report_digest=BASELINE,
                    phase="experiment",
                    dedicated=True,
                    teardown_confirmed=True,
                    environment_digest=ENVIRONMENT,
                )
            ],
            setup_evidence=SetupEvidence(
                content_hashes=[PRIVATE],
                dataset_ids=["private-dataset-v1"],
                flops_budget=100,
                wall_budget_s=30,
                script_sha256=SCRIPT,
                environment_digest=ENVIRONMENT,
                private_holdout_digest=holdout,
                setup_report_digest=SETUP,
                baseline_report_digest=BASELINE,
                teardown_confirmed=True,
            ),
        )
        return "topic-vm", self.mutation(outcome)


def service_for(setup, mutation=None, seed=lambda: OWNER):
    service = setup[0]
    backend = SetupVmBoundary(mutation)
    service.backend = backend
    return TopicSetup(service, backend, owner_seed=seed), backend


async def test_owner_objective_creates_signed_open_topic_from_measured_baseline(setup):
    creator, backend = service_for(setup)

    topic = await creator.create(policy())

    assert topic.status == "open" and topic.signature
    assert topic.metric.epsilon == 0.15
    assert topic.baseline.script_sha256 == SCRIPT
    assert topic.baseline.metrics == {"quality": 0.5}
    assert topic.params == {
        "baseline_runner": "generated-python",
        "experiment_pack_digest": "sha256:" + ENVIRONMENT,
    }
    assert topic.endpoints[0].path == "/compete"
    assert topic.endpoints[0].request_schema == {"type": "object"}
    assert topic.checklist[0].check == "Hash the received artifact and compare its commitment"
    assert setup[0].store.topic(topic.id) == topic
    assert setup[0].store.holdouts(topic.holdout_commitment) == ({PRIVATE}, {"private-dataset-v1"})
    assert PRIVATE not in topic.model_dump_json()
    assert "private-dataset-v1" not in topic.model_dump_json()
    assert backend.calls[0][0].context.purpose == "setup"
    assert backend.calls[0][2] == {}


async def test_owner_can_delegate_metric_choice_to_agent(setup):
    creator, backend = service_for(setup)
    topic = await creator.create(policy(metric=None))
    assert topic.metric.primary == "quality"
    assert topic.metric.custom_id == "fixture-runner"
    assert topic.metric.direction == "max"
    assert topic.baseline.metrics["quality"] == 0.5
    assert backend.calls[0][0].metric is None


async def test_same_setup_is_idempotent_without_rerunning_agent(setup):
    creator, backend = service_for(setup)

    first, second = await asyncio.gather(creator.create(policy()), creator.create(policy()))

    assert first == second
    assert len(backend.calls) == 1


async def test_existing_topic_cannot_be_overwritten_by_a_different_objective(setup):
    creator, backend = service_for(setup)
    await creator.create(policy())

    with pytest.raises(ServiceError, match="different setup policy"):
        await creator.create(policy(objective="A different experiment"))

    assert len(backend.calls) == 1


def alter_proposal(**changes):
    def alter(outcome):
        proposal = outcome.run.result.model_copy(update=changes)
        return outcome.model_copy(
            update={"run": outcome.run.model_copy(update={"result": proposal})}
        )

    return alter


@pytest.mark.parametrize(
    "mutation, reason",
    [
        (lambda value: value.model_copy(update={"setup_evidence": None}), "private setup evidence"),
        (alter_proposal(floor=0.01), "operator policy"),
        (alter_proposal(metric="invented-score"), "operator policy"),
        (alter_proposal(direction="lower"), "operator policy"),
        (alter_proposal(baseline_report_digest="a" * 64), "unattested reports"),
        (alter_proposal(instructions="The hidden record is " + PRIVATE), "private holdout"),
        (lambda value: value.model_copy(update={"executions": []}), "dedicated VM teardown"),
        (
            lambda value: value.model_copy(
                update={
                    "executions": [
                        value.executions[0].model_copy(update={"environment_digest": "f" * 64})
                    ]
                }
            ),
            "dedicated VM teardown",
        ),
        (
            lambda value: value.model_copy(
                update={
                    "setup_evidence": value.setup_evidence.model_copy(
                        update={"teardown_confirmed": False}
                    )
                }
            ),
            "baseline attestation",
        ),
        (
            lambda value: value.model_copy(
                update={
                    "setup_evidence": value.setup_evidence.model_copy(update={"flops_budget": 101})
                }
            ),
            "baseline attestation",
        ),
        (
            lambda value: value.model_copy(
                update={
                    "setup_evidence": value.setup_evidence.model_copy(
                        update={"dataset_ids": ["substituted"]}
                    )
                }
            ),
            "holdout commitment",
        ),
        (
            lambda value: value.model_copy(
                update={
                    "reports": [
                        value.reports[0],
                        value.reports[1].model_copy(update={"job_id": "foreign-job"}),
                    ]
                }
            ),
            "binding",
        ),
        (
            lambda value: value.model_copy(
                update={
                    "reports": [
                        value.reports[0],
                        value.reports[1].model_copy(update={"flops_used": 101}),
                    ]
                }
            ),
            "baseline attestation",
        ),
    ],
)
async def test_unverified_or_policy_violating_setup_cannot_publish(setup, mutation, reason):
    creator, _ = service_for(setup, mutation)

    with pytest.raises(ServiceError, match=reason):
        await creator.create(policy())

    assert setup[0].store.topic("new-research") is None


async def test_missing_byok_rejected_before_agent_work(setup):
    creator, backend = service_for(setup)

    with pytest.raises(ServiceError, match="required env missing"):
        await creator.create(policy(params={"miner_byok": "RESEARCH_KEY"}))

    assert not backend.calls


async def test_wrong_owner_key_rejected_before_agent_work(setup):
    creator, backend = service_for(setup, seed=lambda: bytes([99]) * 32)
    assert public_key(OWNER) != public_key(bytes([99]) * 32)

    with pytest.raises(ServiceError, match="signing key"):
        await creator.create(policy())

    assert not backend.calls


async def test_completed_setup_deletes_byok_and_reuses_topic_without_old_secret(setup):
    creator, backend = service_for(setup)
    request_policy = policy(params={"miner_byok": "RESEARCH_KEY"})
    first = await creator.create(request_policy, env={"RESEARCH_KEY": "private-test-credential"})

    second = await creator.create(request_policy)

    assert first == second and len(backend.calls) == 1
    assert list(setup[0].vault.root.iterdir()) == []
    with setup[0].store.transaction() as connection:
        rows = connection.execute("SELECT policy,env_names FROM proof_setup_jobs").fetchall()
    assert "private-test-credential" not in repr([tuple(row) for row in rows])
    assert "private-test-credential" not in first.model_dump_json()


async def test_failed_setup_deletes_byok_and_accepts_fresh_credentials_on_retry(setup):
    failed, _ = service_for(
        setup, lambda outcome: outcome.model_copy(update={"setup_evidence": None})
    )
    request_policy = policy(params={"miner_byok": "RESEARCH_KEY"})

    with pytest.raises(ServiceError, match="private setup evidence"):
        await failed.create(request_policy, env={"RESEARCH_KEY": "expired-test-credential"})

    assert list(setup[0].vault.root.iterdir()) == []
    with setup[0].store.transaction() as connection:
        row = connection.execute("SELECT state FROM proof_setup_jobs").fetchone()
    assert row["state"] == "failed"

    restored, backend = service_for(setup)
    topic = await restored.create(request_policy, env={"RESEARCH_KEY": "fresh-test-credential"})

    assert topic.status == "open"
    assert backend.calls[0][2] == {"RESEARCH_KEY": "fresh-test-credential"}
    assert list(setup[0].vault.root.iterdir()) == []


async def test_setup_vault_failure_after_pending_row_removes_row_and_credentials(
    setup, monkeypatch
):
    creator, _ = service_for(setup)
    request_policy = policy(params={"miner_byok": "RESEARCH_KEY"})
    original_put = setup[0].vault.put

    def fail_after_write(job_id, environment):
        with setup[0].store.transaction() as connection:
            row = connection.execute(
                "SELECT state FROM proof_setup_jobs WHERE id=?", (job_id,)
            ).fetchone()
        assert row is not None and row["state"] == "pending"
        original_put(job_id, environment)
        raise ServiceError(503, "injected setup vault failure")

    monkeypatch.setattr(setup[0].vault, "put", fail_after_write)

    with pytest.raises(ServiceError, match="injected setup vault failure"):
        await creator.create(request_policy, env={"RESEARCH_KEY": "private-test-credential"})

    with setup[0].store.transaction() as connection:
        assert connection.execute("SELECT count(*) FROM proof_setup_jobs").fetchone()[0] == 0
    assert list(setup[0].vault.root.iterdir()) == []


async def test_setup_cleanup_failure_keeps_job_pending_until_safe_retry(setup):
    creator, backend = service_for(setup)
    backend.gate.clear()
    request_policy = policy(params={"miner_byok": "RESEARCH_KEY"})
    waiter = asyncio.create_task(
        creator.create(request_policy, env={"RESEARCH_KEY": "private-test-credential"})
    )
    await backend.entered.wait()
    job_directory = next(setup[0].vault.root.iterdir())
    credential = job_directory / "RESEARCH_KEY"
    credential.chmod(0o640)
    backend.gate.set()

    with pytest.raises(ServiceError, match="not private"):
        await waiter

    with setup[0].store.transaction() as connection:
        row = connection.execute(
            "SELECT state FROM proof_setup_jobs WHERE id=?", (job_directory.name,)
        ).fetchone()
    assert row["state"] == "pending"
    assert credential.read_text() == "private-test-credential"
    assert setup[0].store.topic("new-research").status == "open"

    credential.chmod(0o600)
    topic = await creator.create(request_policy, env={"RESEARCH_KEY": "private-test-credential"})

    assert topic.status == "open"
    assert len(backend.calls) == 1
    assert list(setup[0].vault.root.iterdir()) == []


async def test_finished_setup_cannot_drop_a_replacement_task(setup):
    creator, _ = service_for(setup)
    old = asyncio.create_task(asyncio.sleep(0))
    await old
    release = asyncio.Event()
    replacement = asyncio.create_task(release.wait())
    creator._tasks["same-job"] = replacement

    creator._finished("same-job", old)

    assert creator._tasks["same-job"] is replacement
    replacement.cancel()
    await asyncio.gather(replacement, return_exceptions=True)
    creator._tasks.pop("same-job")


async def test_owner_disconnect_does_not_cancel_accepted_topic_setup(setup):
    creator, backend = service_for(setup)
    backend.gate.clear()
    waiter = asyncio.create_task(creator.create(policy()))
    await backend.entered.wait()

    waiter.cancel()
    with pytest.raises(asyncio.CancelledError):
        await waiter
    backend.gate.set()
    await creator.drain()

    assert setup[0].store.topic("new-research").status == "open"


async def test_interrupted_pending_setup_resumes_with_same_job_and_private_credentials(setup):
    creator, first_backend = service_for(setup)
    first_backend.gate.clear()
    request_policy = policy(params={"miner_byok": "RESEARCH_KEY"})
    waiter = asyncio.create_task(
        creator.create(request_policy, env={"RESEARCH_KEY": "private-test-credential"})
    )
    await first_backend.entered.wait()
    for task in list(creator._tasks.values()):
        task.cancel()
    with pytest.raises(asyncio.CancelledError):
        await waiter
    restored, second_backend = service_for(setup)

    await restored.resume()
    await restored.drain()

    assert setup[0].store.topic("new-research").status == "open"
    assert first_backend.calls[0][0].context.job_id == second_backend.calls[0][0].context.job_id
    assert second_backend.calls[0][2] == {"RESEARCH_KEY": "private-test-credential"}
    assert list(setup[0].vault.root.iterdir()) == []


async def test_setup_resume_fails_closed_when_pending_credential_is_missing(setup):
    creator, first_backend = service_for(setup)
    first_backend.gate.clear()
    request_policy = policy(params={"miner_byok": "RESEARCH_KEY"})
    waiter = asyncio.create_task(
        creator.create(request_policy, env={"RESEARCH_KEY": "private-test-credential"})
    )
    await first_backend.entered.wait()
    for task in list(creator._tasks.values()):
        task.cancel()
    with pytest.raises(asyncio.CancelledError):
        await waiter
    for entry in setup[0].vault.root.iterdir():
        setup[0].vault.delete(entry.name)
    restored, second_backend = service_for(setup)

    with pytest.raises(ServiceError, match="credential.*missing"):
        await restored.resume()

    assert second_backend.calls == []


async def test_setup_route_requires_operator_auth_and_returns_only_public_topic(setup, tmp_path):
    creator, backend = service_for(setup)
    app = FastAPI()

    @app.exception_handler(ServiceError)
    async def public_error(request, error):
        return JSONResponse({"error": error.reason}, status_code=error.status)

    app.include_router(create_setup_router(creator, OperatorAuth(tmp_path / "operator-token")))
    async with httpx.AsyncClient(
        transport=httpx.ASGITransport(app=app), base_url="https://proof"
    ) as client:
        denied = await client.post(
            "/v1/admin/proof/setup", json={"policy": policy().model_dump(mode="json")}
        )
        assert denied.status_code == 401 and not backend.calls

        accepted = await client.post(
            "/v1/admin/proof/setup",
            json={"policy": policy().model_dump(mode="json")},
            headers={"Authorization": "Bearer fixture-operator-token"},
        )

    assert accepted.status_code == 201 and accepted.json()["status"] == "open"
    assert PRIVATE not in accepted.text and "private-dataset-v1" not in accepted.text
