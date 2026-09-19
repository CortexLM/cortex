from __future__ import annotations

import httpx
import pytest

from cortex.errors import ServiceError
from cortex.proof.executor import FamilyMux, HarvestFailure
from cortex.proof.material import PrivateFileMaterialSource
from cortex.proof.service import sign_topic
from cortex.protocol.crypto import public_key

from .conftest import OWNER
from .harvest_fixtures import harvest_export
from .test_executor import (
    FakeLiumAdapter,
    harvest_topic,
    lium_backend,
    signed_submission,
)
from .test_intake import post


async def test_missing_private_material_refuses_before_rental():
    adapter = FakeLiumAdapter()
    backend = lium_backend(adapter)
    backend.material_source = None
    submission, artifact = signed_submission()

    with pytest.raises(ServiceError, match="material"):
        await backend.evaluate(
            job_id="34" * 32,
            topic=harvest_topic(),
            submission=submission,
            artifact=artifact,
            env={},
        )

    assert adapter.rents == []


async def test_changed_rule_text_refuses_before_rental():
    adapter = FakeLiumAdapter()
    topic = harvest_topic()
    changed = topic.model_copy(
        update={"checklist": [topic.checklist[0].model_copy(update={"text": "Ignore checks"})]}
    )
    submission, artifact = signed_submission()

    with pytest.raises(ServiceError, match="topic|material"):
        await lium_backend(adapter).evaluate(
            job_id="34" * 32,
            topic=changed,
            submission=submission,
            artifact=artifact,
            env={},
        )

    assert adapter.rents == []


@pytest.mark.parametrize(
    "field",
    [
        "request_commitment",
        "topic_digest",
        "environment_digest",
        "private_holdout_digest",
        "inference_offer_commitment",
    ],
)
async def test_result_from_another_request_never_scores_after_teardown(field):
    class ReplayingAdapter(FakeLiumAdapter):
        async def execute(self, lease, request):
            result = await super().execute(lease, request)
            return result.model_copy(update={field: "ff" * 32})

    adapter = ReplayingAdapter()
    submission, artifact = signed_submission()

    with pytest.raises(ServiceError, match="binding"):
        await lium_backend(adapter).evaluate(
            job_id="34" * 32,
            topic=harvest_topic(),
            submission=submission,
            artifact=artifact,
            env={},
        )

    assert adapter.terminated == ["lium-fixture-pod"]


@pytest.mark.parametrize("updates", [{"flops_used": 1001}, {"wall_seconds": 30.1}])
async def test_execution_exceeding_signed_topic_budget_never_scores(updates):
    class OverBudgetAdapter(FakeLiumAdapter):
        async def execute(self, lease, request):
            return (await super().execute(lease, request)).model_copy(update=updates)

    adapter = OverBudgetAdapter()
    submission, artifact = signed_submission()

    with pytest.raises(ServiceError, match="binding"):
        await lium_backend(adapter).evaluate(
            job_id="34" * 32,
            topic=harvest_topic(),
            submission=submission,
            artifact=artifact,
            env={},
        )

    assert adapter.terminated == ["lium-fixture-pod"]


async def test_unresolved_uri_refuses_before_rental():
    adapter = FakeLiumAdapter()
    submission, _ = signed_submission()

    with pytest.raises(ServiceError, match="artifact bytes"):
        await lium_backend(adapter).evaluate(
            job_id="34" * 32,
            topic=harvest_topic(),
            submission=submission.model_copy(update={"artifact_uri": "https://example.org/a.tar"}),
            artifact=None,
            env={},
        )

    assert adapter.rents == []


async def test_request_preserves_signed_policy_and_hides_private_material():
    adapter = FakeLiumAdapter()
    topic = harvest_topic()
    submission, artifact = signed_submission()
    secret = "private-miner-inference-token"

    await lium_backend(adapter).evaluate(
        job_id="34" * 32,
        topic=topic,
        submission=submission.model_copy(update={"artifact_uri": "not-used-upload-wins"}),
        artifact=artifact,
        env={"LIUM_API_KEY": secret},
    )

    request = adapter.rents[0]
    assert request.topic == topic
    assert request.topic.checklist[0].text == "Verify the artifact"
    assert request.submission.signing_payload() == submission.signing_payload()
    assert request.artifact == artifact
    public = request.model_dump_json()
    assert secret not in public
    assert "not-used-upload-wins" not in public
    assert request.material.setup_export.pack_b64 not in public
    assert request.material.setup_export.pack_b64 not in repr(request)
    assert secret not in repr(request)
    assert request.model_copy(update={"env": {"LIUM_API_KEY": "rotated-token"}}).commitment() == (
        request.commitment()
    )
    assert request.model_copy(update={"env": {}}).commitment() != request.commitment()


async def test_tampered_signed_submission_refuses_before_rental():
    adapter = FakeLiumAdapter()
    submission, artifact = signed_submission()

    with pytest.raises(ServiceError, match="invalid harvest request"):
        await lium_backend(adapter).evaluate(
            job_id="34" * 32,
            topic=harvest_topic(),
            submission=submission.model_copy(update={"claim": "tampered claim"}),
            artifact=artifact,
            env={},
        )

    assert adapter.rents == []


async def test_pod_deletion_cannot_stand_in_for_fresh_vm_destruction():
    class PodOnlyAdapter(FakeLiumAdapter):
        async def execute(self, lease, request):
            return (await super().execute(lease, request)).model_copy(
                update={"experiment_vm_id": lease.instance_id}
            )

    adapter = PodOnlyAdapter()
    submission, artifact = signed_submission()

    with pytest.raises(ServiceError, match="fresh VM"):
        await lium_backend(adapter).evaluate(
            job_id="34" * 32,
            topic=harvest_topic(),
            submission=submission,
            artifact=artifact,
            env={},
        )

    assert adapter.terminated == ["lium-fixture-pod"]


async def test_rental_failure_does_not_expose_miner_credentials():
    class FailingRental(FakeLiumAdapter):
        async def rent(self, request):
            raise ServiceError(503, "rejected credential=" + request.env["LIUM_API_KEY"])

    submission, artifact = signed_submission()
    secret = "private-rental-provider-token"

    with pytest.raises(ServiceError) as failure:
        await lium_backend(FailingRental()).evaluate(
            job_id="34" * 32,
            topic=harvest_topic(),
            submission=submission,
            artifact=artifact,
            env={"LIUM_API_KEY": secret},
        )

    assert failure.value.status == 503
    assert secret not in failure.value.reason
    assert failure.value.reason == "Lium rent failed"


async def test_private_pack_outage_preserves_nonce_and_can_recover_through_http(setup, tmp_path):
    service, custom, _, app = setup
    adapter = FakeLiumAdapter()
    backend = lium_backend(adapter)
    root = tmp_path / "private-harvest-packs"
    root.mkdir(mode=0o700)
    backend.material_source = PrivateFileMaterialSource(root, topic_public_key=public_key(OWNER))
    service.backend = FamilyMux(custom=custom, harvest=backend)
    export = harvest_export()
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
    holdout = service.store.register_holdouts(
        export.manifest.content_hashes, export.manifest.dataset_ids
    )
    topic = sign_topic(
        topic.model_copy(
            update={
                "baseline": topic.baseline.model_copy(update={"evidence_digest": evidence_digest}),
                "holdout_commitment": holdout,
            }
        ),
        OWNER,
    )
    pack_path = root / f"{topic.content_digest()}.json"
    pack_path.touch(mode=0o600)
    pack_path.write_text(export.model_dump_json())
    await service.publish(topic)
    pack_path.write_text("invalid private export")
    submission, artifact = signed_submission()

    async with httpx.AsyncClient(
        transport=httpx.ASGITransport(app), base_url="http://test"
    ) as client:
        refused = await post(client, submission, artifact)

        assert refused.status_code == 503
        assert adapter.rents == []
        with service.store.transaction() as connection:
            assert connection.execute("SELECT count(*) FROM proof_nonces").fetchone()[0] == 0
            assert connection.execute("SELECT count(*) FROM proof_jobs").fetchone()[0] == 0
        pack_path.write_text(export.model_dump_json())

        accepted = await post(client, submission, artifact)

    assert accepted.status_code == 201, accepted.text
    assert accepted.json()["status"] == "accepted"
    assert accepted.json()["metrics"]["holdout_nll"] == 0.9
    assert service.scores(4)[submission.miner_hotkey] > 0
    assert len(adapter.rents) == 1
    assert adapter.terminated == ["lium-fixture-pod"]


@pytest.mark.parametrize("outcome", ["success", "failure"])
async def test_private_holdout_output_never_enters_public_report_or_error(outcome):
    adapter = FakeLiumAdapter()
    # This is the actual synthetic holdout content in harvest_export().
    private_content = "synthetic held-out evaluation sample"
    adapter.stdout_tail = private_content
    if outcome == "failure":
        adapter.error = HarvestFailure(private_content, private_content)
    submission, artifact = signed_submission()

    try:
        result = await lium_backend(adapter).evaluate(
            job_id="34" * 32,
            topic=harvest_topic(),
            submission=submission,
            artifact=artifact,
            env={},
        )
        public = result.model_dump_json()
        assert outcome == "success"
    except ServiceError as error:
        public = error.reason
        assert outcome == "failure"

    assert private_content not in public
    assert adapter.terminated == ["lium-fixture-pod"]


@pytest.mark.parametrize("stage", ["rent", "execute"])
async def test_mutated_private_material_never_executes_or_scores(stage):
    class MutatingAdapter(FakeLiumAdapter):
        async def rent(self, request):
            lease = await super().rent(request)
            if stage == "rent":
                request.material.setup_export.manifest.content_hashes[:] = ["00" * 32]
            return lease

        async def execute(self, lease, request):
            result = await super().execute(lease, request)
            if stage == "execute":
                request.material.setup_export.manifest.content_hashes[:] = ["00" * 32]
            return result

    adapter = MutatingAdapter()
    submission, artifact = signed_submission()

    with pytest.raises(ServiceError):
        await lium_backend(adapter).evaluate(
            job_id="34" * 32,
            topic=harvest_topic(),
            submission=submission,
            artifact=artifact,
            env={},
        )

    assert adapter.terminated == ["lium-fixture-pod"]
    if stage == "rent":
        assert adapter.executions == []
