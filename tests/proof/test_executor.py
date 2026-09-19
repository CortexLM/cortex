from __future__ import annotations

import asyncio
import hashlib
import io
import tarfile

import httpx
import pytest

from cortex.errors import ServiceError
from cortex.proof.executor import (
    EvalExecutorOffer,
    ExecutorOfferRegistry,
    ExecutorPin,
    FamilyMux,
    HarvestExecution,
    HarvestOverrides,
    LiumBackend,
    LiumLease,
    sign_executor_offer,
)
from cortex.proof.models import (
    Baseline,
    Metric,
    Rule,
    Topic,
    TopicEvalExecutor,
    digest,
)
from cortex.proof.scoring import judge
from cortex.proof.service import sign_submission, sign_topic
from cortex.protocol.crypto import public_key

from .conftest import MINER, OFFER, OWNER
from .harvest_fixtures import FixtureMaterialSource, harvest_export

EXECUTOR_SEED = bytes([31]) * 32
IMAGE_HEX = "ab" * 32
IMAGE = "sha256:" + IMAGE_HEX
INFERENCE_COMMITMENT = OFFER.commitment()


def artifact_bytes() -> bytes:
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode="w") as archive:
        body = b"print('measured fixture')\n"
        info = tarfile.TarInfo("train.py")
        info.size = len(body)
        archive.addfile(info, io.BytesIO(body))
    return output.getvalue()


def executor_pin(**updates) -> ExecutorPin:
    return ExecutorPin(
        **{
            "eval_image_digest": IMAGE,
            "issuer_public_key": public_key(EXECUTOR_SEED).hex(),
            "allowed_template_prefixes": ("proof-eval-",),
            **updates,
        }
    )


def executor_offer(**updates) -> EvalExecutorOffer:
    values = {
        "offer_id": "lium-1x-v1",
        "lium_template_id": "proof-eval-" + IMAGE_HEX[:12],
        "machine_shape": "1x",
        "max_proof_deadline_s": 120,
        "eval_image_digest": IMAGE,
        "config_commitment": "0" * 64,
        "issuer_public_key": public_key(EXECUTOR_SEED).hex(),
        "status": "open",
        "valid_from_unix": 100,
        "valid_until_unix": 4102444800,
        "signature": "0" * 128,
        **updates,
    }
    unsigned = EvalExecutorOffer(**values)
    unsigned = unsigned.model_copy(
        update={"config_commitment": unsigned.expected_config_commitment()}
    )
    return sign_executor_offer(unsigned, EXECUTOR_SEED)


def harvest_topic(**updates) -> Topic:
    baseline_metrics = {"holdout_nll": 1.0, "split_nll/private": 1.0}
    _, material = harvest_export().verified()
    topic = Topic(
        **{
            "id": "harvest-topic",
            "statement": "Improve held-out likelihood",
            "status": "open",
            "metric": Metric(
                family="nll",
                primary="holdout_nll",
                direction="min",
                epsilon=0.02,
            ),
            "flops_budget": 1_000,
            "wall_budget_s": 30,
            "checklist": [Rule(id="integrity", text="Verify the artifact")],
            "params": {
                "experiment_pack_digest": "sha256:" + material.environment_digest,
                "miner_env_allowlist": "LIUM_API_KEY",
            },
            "baseline": Baseline(
                script_sha256=material.script_sha256,
                metrics=baseline_metrics,
                metrics_commitment=digest(baseline_metrics),
                evidence_digest="cd" * 32,
                flops_budget=1_000,
                wall_budget_s=30,
            ),
            "holdout_commitment": material.private_holdout_digest,
            "eval_image_digest": IMAGE,
            "inference_offer_commitment": INFERENCE_COMMITMENT,
            "eval_executor": TopicEvalExecutor(max_proof_deadline_s=30),
            **updates,
        }
    )
    return sign_topic(topic, OWNER)


def throughput_topic() -> Topic:
    baseline_metrics = {
        "tokens_per_sec": 100.0,
        "holdout_nll": 1.0,
        "split_nll/private": 1.0,
    }
    return harvest_topic(
        metric=Metric(
            family="throughput",
            primary="tokens_per_sec",
            direction="max",
            epsilon=0.05,
            relative=True,
        ),
        baseline=harvest_topic().baseline.model_copy(
            update={
                "metrics": baseline_metrics,
                "metrics_commitment": digest(baseline_metrics),
            }
        ),
    )


class FakeLiumAdapter:
    def __init__(self) -> None:
        self.available = True
        self.rents = []
        self.executions = []
        self.terminated = []
        self.termination_confirmed = True
        self.error: Exception | None = None
        self.stdout_tail = "bounded fixture output"

    async def probe(self) -> bool:
        return self.available

    async def rent(self, request):
        self.rents.append(request)
        return LiumLease(
            instance_id="lium-fixture-pod",
            template_id=request.plan.template_id,
            gpu_count=request.plan.gpu_count,
            image_digest=request.eval_image_digest,
        )

    async def execute(self, lease, request):
        self.executions.append((lease, request))
        if self.error is not None:
            raise self.error
        metrics = {"holdout_nll": 0.9, "split_nll/private": 0.99}
        if request.metric == "tokens_per_sec":
            metrics["tokens_per_sec"] = 110.0
        return HarvestExecution(
            request_commitment=request.commitment(),
            topic_digest=request.topic_digest,
            environment_digest=request.material.environment_digest,
            private_holdout_digest=request.material.private_holdout_digest,
            inference_offer_commitment=request.inference_offer_commitment,
            experiment_vm_id="experiment-fixture-vm",
            teardown_confirmed=True,
            topic_id=request.topic_id,
            job_id=request.job_id,
            artifact_digest=request.artifact_digest,
            eval_image_digest=request.eval_image_digest,
            executor_config_commitment=request.plan.config_commitment,
            gpu_count=1,
            exit_code=0,
            verdict="clean",
            reproduced=True,
            claim_holds=True,
            metrics=metrics,
            rule_results={"integrity": True},
            flops_used=100,
            wall_seconds=2.0,
            evidence_digest="12" * 32,
            stdout_tail=self.stdout_tail,
            sandboxed=True,
        )

    async def terminate(self, lease):
        self.terminated.append(lease.instance_id)

    async def verify_terminated(self, lease):
        return self.termination_confirmed


def lium_backend(adapter: FakeLiumAdapter, offer: EvalExecutorOffer | None = None) -> LiumBackend:
    registry = ExecutorOfferRegistry(executor_pin(), offer or executor_offer(), clock=lambda: 101)
    return LiumBackend(
        registry=registry,
        adapter=adapter,
        inference_offer_commitment=INFERENCE_COMMITMENT,
        topic_public_key=public_key(OWNER),
        material_source=FixtureMaterialSource(),
        teardown_grace_seconds=1,
    )


def signed_submission(topic_id: str = "harvest-topic"):
    artifact = artifact_bytes()
    submission = sign_submission(
        {
            "topic_id": topic_id,
            "artifact_digest": hashlib.sha256(artifact).hexdigest(),
            "claim": "The held-out loss improves",
            "manifest": {"train_content_hashes": ["fixture-training-data"]},
            "submit_nonce": "56" * 32,
        },
        MINER,
    )
    return submission, artifact


def test_signed_offer_binds_every_rent_configuration_field():
    pin = executor_pin()
    offer = executor_offer()

    offer.verify(pin, now=101)

    for field, value in [
        ("lium_template_id", "proof-eval-" + "cd" * 6),
        ("machine_shape", "2x"),
        ("max_proof_deadline_s", 121),
        ("eval_image_digest", "sha256:" + "cd" * 32),
    ]:
        changed = offer.model_copy(update={field: value})
        with pytest.raises(ValueError):
            changed.verify(pin, now=101)


@pytest.mark.parametrize(
    "offer_updates,pin_updates,reason",
    [
        ({"machine_shape": "2x"}, {}, "exactly 1x"),
        ({"max_proof_deadline_s": 121}, {"max_proof_deadline_s": 120}, "deadline"),
        ({"lium_template_id": "4a36877c-1234-1234-1234-123456789abc"}, {}, "raw"),
        ({"status": "closed"}, {}, "closed"),
    ],
)
def test_invalid_executor_is_rejected_before_any_rent(offer_updates, pin_updates, reason):
    pin = executor_pin(**pin_updates)

    with pytest.raises(ValueError, match=reason):
        registry = ExecutorOfferRegistry(pin, executor_offer(**offer_updates), clock=lambda: 101)
        registry.require_open()


def test_topic_deadline_can_tighten_but_never_silently_loosen_offer():
    registry = ExecutorOfferRegistry(executor_pin(), executor_offer(), clock=lambda: 101)
    assert registry.plan(harvest_topic()).deadline_s == 30

    with pytest.raises(ValueError, match="cannot exceed"):
        registry.plan(
            harvest_topic(
                wall_budget_s=180,
                baseline=harvest_topic().baseline.model_copy(update={"wall_budget_s": 180}),
                eval_executor=TopicEvalExecutor(max_proof_deadline_s=180),
            )
        )


async def test_successful_harvest_scores_only_after_confirmed_teardown():
    adapter = FakeLiumAdapter()
    backend = lium_backend(adapter)
    topic = harvest_topic()
    submission, artifact = signed_submission()

    report = await backend.evaluate(
        job_id="34" * 32,
        topic=topic,
        submission=submission,
        artifact=artifact,
        env={"LIUM_API_KEY": "fixture-secret"},
    )

    assert report.metrics["holdout_nll"] == 0.9
    assert report.executor_offer_commitment == executor_offer().config_commitment
    assert report.teardown_confirmed is True
    assert adapter.rents[0].plan.gpu_count == 1
    assert adapter.rents[0].env == {"LIUM_API_KEY": "fixture-secret"}
    assert adapter.terminated == ["lium-fixture-pod"]


async def test_throughput_family_uses_the_same_pinned_harvest_contract():
    adapter = FakeLiumAdapter()
    backend = lium_backend(adapter)
    topic = throughput_topic()
    submission, artifact = signed_submission()

    report = await backend.evaluate(
        job_id="34" * 32,
        topic=topic,
        submission=submission,
        artifact=artifact,
        env={},
    )

    assert judge(topic, report) == []
    assert report.metrics["tokens_per_sec"] == 110.0


async def test_non_one_gpu_override_is_refused_before_provider_rent():
    adapter = FakeLiumAdapter()
    registry = ExecutorOfferRegistry(executor_pin(), executor_offer(), clock=lambda: 101)
    backend = LiumBackend(
        registry=registry,
        adapter=adapter,
        inference_offer_commitment=INFERENCE_COMMITMENT,
        topic_public_key=public_key(OWNER),
        material_source=FixtureMaterialSource(),
        overrides=HarvestOverrides(gpu_count=2),
    )
    submission, artifact = signed_submission()

    with pytest.raises(ServiceError, match="exactly 1x"):
        await backend.evaluate(
            job_id="34" * 32,
            topic=harvest_topic(),
            submission=submission,
            artifact=artifact,
            env={},
        )

    assert adapter.rents == []


async def test_success_is_withheld_when_lium_teardown_is_unconfirmed():
    adapter = FakeLiumAdapter()
    adapter.termination_confirmed = False
    backend = lium_backend(adapter)
    submission, artifact = signed_submission()

    with pytest.raises(ServiceError, match="teardown unconfirmed"):
        await backend.evaluate(
            job_id="34" * 32,
            topic=harvest_topic(),
            submission=submission,
            artifact=artifact,
            env={},
        )

    assert adapter.terminated == ["lium-fixture-pod"]


async def test_provider_error_is_bounded_and_still_tears_down():
    adapter = FakeLiumAdapter()
    secret = "private-provider-token"
    adapter.error = ServiceError(503, secret + "x" * 100_000)
    backend = lium_backend(adapter)
    submission, artifact = signed_submission()

    with pytest.raises(ServiceError) as caught:
        await backend.evaluate(
            job_id="34" * 32,
            topic=harvest_topic(),
            submission=submission,
            artifact=artifact,
            env={"LIUM_API_KEY": secret},
        )

    assert len(caught.value.reason.encode()) <= 16_384
    assert secret not in caught.value.reason
    assert adapter.terminated == ["lium-fixture-pod"]


async def test_cancellation_after_rent_tears_down_before_propagating():
    class BlockingAdapter(FakeLiumAdapter):
        def __init__(self):
            super().__init__()
            self.entered = asyncio.Event()

        async def execute(self, lease, request):
            self.executions.append((lease, request))
            self.entered.set()
            await asyncio.Future()

    adapter = BlockingAdapter()
    backend = lium_backend(adapter)
    submission, artifact = signed_submission()
    evaluation = asyncio.create_task(
        backend.evaluate(
            job_id="34" * 32,
            topic=harvest_topic(),
            submission=submission,
            artifact=artifact,
            env={},
        )
    )
    await adapter.entered.wait()
    evaluation.cancel()

    with pytest.raises(asyncio.CancelledError):
        await evaluation

    assert adapter.terminated == ["lium-fixture-pod"]


async def test_repeated_cancellation_cannot_interrupt_lium_teardown_or_verification():
    class BlockingTeardownAdapter(FakeLiumAdapter):
        def __init__(self):
            super().__init__()
            self.execute_entered = asyncio.Event()
            self.terminate_entered = asyncio.Event()
            self.terminate_release = asyncio.Event()
            self.verify_entered = asyncio.Event()
            self.verify_release = asyncio.Event()
            self.verified = []

        async def execute(self, lease, request):
            self.executions.append((lease, request))
            self.execute_entered.set()
            await asyncio.Future()

        async def terminate(self, lease):
            self.terminate_entered.set()
            await self.terminate_release.wait()
            self.terminated.append(lease.instance_id)

        async def verify_terminated(self, lease):
            self.verify_entered.set()
            await self.verify_release.wait()
            self.verified.append(lease.instance_id)
            return True

    adapter = BlockingTeardownAdapter()
    backend = lium_backend(adapter)
    submission, artifact = signed_submission()
    evaluation = asyncio.create_task(
        backend.evaluate(
            job_id="34" * 32,
            topic=harvest_topic(),
            submission=submission,
            artifact=artifact,
            env={},
        )
    )
    await adapter.execute_entered.wait()
    evaluation.cancel()
    await adapter.terminate_entered.wait()
    evaluation.cancel()
    adapter.terminate_release.set()
    await adapter.verify_entered.wait()
    evaluation.cancel()
    adapter.verify_release.set()

    with pytest.raises(asyncio.CancelledError):
        await evaluation

    assert adapter.terminated == ["lium-fixture-pod"]
    assert adapter.verified == ["lium-fixture-pod"]


async def test_success_output_is_omitted_from_the_durable_report():
    adapter = FakeLiumAdapter()
    secret = "private-provider-token"
    adapter.stdout_tail = "completed with " + secret
    backend = lium_backend(adapter)
    submission, artifact = signed_submission()

    report = await backend.evaluate(
        job_id="34" * 32,
        topic=harvest_topic(),
        submission=submission,
        artifact=artifact,
        env={"LIUM_API_KEY": secret},
    )

    assert secret not in report.rationale
    assert report.rationale == "Lium evaluation completed with verified evidence"


async def test_unready_harvest_fails_before_nonce_or_submission_row(setup):
    service, custom, _, _ = setup
    adapter = FakeLiumAdapter()
    adapter.available = False
    service.backend = FamilyMux(custom=custom, harvest=lium_backend(adapter))
    topic = harvest_topic()
    evidence = {
        "metrics": topic.baseline.metrics,
        "script_sha256": topic.baseline.script_sha256,
        "eval_image_digest": topic.eval_image_digest,
        "flops_budget": topic.flops_budget,
        "wall_budget_s": topic.wall_budget_s,
        "sandboxed": True,
        "teardown_confirmed": True,
    }
    evidence_digest = service.store.register_evidence(topic.id, evidence)
    holdout = service.store.register_holdouts(["private-harvest-record"], [])
    topic = sign_topic(
        topic.model_copy(
            update={
                "baseline": topic.baseline.model_copy(update={"evidence_digest": evidence_digest}),
                "holdout_commitment": holdout,
            }
        ),
        OWNER,
    )
    service.store.publish(topic, epoch=4)
    submission, artifact = signed_submission()

    with pytest.raises(ServiceError, match="harvest executor unavailable"):
        await service.submit(submission, artifact)

    with service.store.transaction() as connection:
        assert connection.execute("SELECT count(*) FROM proof_nonces").fetchone()[0] == 0
        assert connection.execute("SELECT count(*) FROM proof_jobs").fetchone()[0] == 0
        assert connection.execute("SELECT count(*) FROM proof_submissions").fetchone()[0] == 0
    assert adapter.rents == []


async def test_status_excludes_harvest_topic_with_incompatible_executor_offer(setup):
    service, custom, fixture_topic, app = setup
    service.store.publish(
        sign_topic(fixture_topic.model_copy(update={"revision": 2, "status": "closed"}), OWNER),
        epoch=4,
    )
    adapter = FakeLiumAdapter()
    service.backend = FamilyMux(custom=custom, harvest=lium_backend(adapter))
    topic = harvest_topic(eval_executor=TopicEvalExecutor(require_offer_commitment="99" * 32))
    evidence = {
        "metrics": topic.baseline.metrics,
        "script_sha256": topic.baseline.script_sha256,
        "eval_image_digest": topic.eval_image_digest,
        "flops_budget": topic.flops_budget,
        "wall_budget_s": topic.wall_budget_s,
        "sandboxed": True,
        "teardown_confirmed": True,
    }
    evidence_digest = service.store.register_evidence(topic.id, evidence)
    holdout = service.store.register_holdouts(["private-harvest-record"], [])
    topic = sign_topic(
        topic.model_copy(
            update={
                "baseline": topic.baseline.model_copy(update={"evidence_digest": evidence_digest}),
                "holdout_commitment": holdout,
            }
        ),
        OWNER,
    )
    service.store.publish(topic, epoch=4)

    async with httpx.AsyncClient(
        transport=httpx.ASGITransport(app), base_url="http://test"
    ) as client:
        status = await client.get("/v1/status")

    assert status.status_code == 200
    assert status.json()["can_score"] is False
    assert status.json()["open_topics"] == 1
    assert status.json()["scorable_topics"] == []
    assert status.json()["harvest_scorable_topics"] == []
    assert "offer" in status.json()["reason"]


async def test_executor_api_rotates_or_closes_only_with_operator_auth(setup):
    service, custom, _, _ = setup
    adapter = FakeLiumAdapter()
    harvest = lium_backend(adapter)
    service.backend = FamilyMux(custom=custom, harvest=harvest)
    app = setup[3]
    closed = executor_offer(status="closed")
    async with httpx.AsyncClient(
        transport=httpx.ASGITransport(app), base_url="http://test"
    ) as client:
        public = await client.get("/v1/proof/executor")
        denied = await client.post("/v1/admin/proof/executor", json=closed.model_dump())
        rotated = await client.post(
            "/v1/admin/proof/executor",
            headers={"Authorization": "Bearer fixture-operator-token"},
            json=closed.model_dump(),
        )
        forged = await client.post(
            "/v1/admin/proof/executor",
            headers={"Authorization": "Bearer fixture-operator-token"},
            json=closed.model_copy(update={"machine_shape": "2x"}).model_dump(),
        )
        after = await client.get("/v1/proof/executor")

    assert public.status_code == 200
    assert public.json()["ready"] is True
    assert public.json()["eval_executor"]["machine_shape"] == "1x"
    assert denied.status_code == 401
    assert rotated.status_code == 200
    assert rotated.json()["eval_executor"]["status"] == "closed"
    assert forged.status_code == 400
    assert after.json()["ready"] is False
