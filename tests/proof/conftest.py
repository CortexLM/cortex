import asyncio
import hashlib
import io
import tarfile

import pytest

from cortex.http import OperatorAuth
from cortex.proof.api import create_app
from cortex.proof.artifacts import FileVault
from cortex.proof.models import Baseline, Endpoint, EvaluationReport, Metric, Rule, Topic, digest
from cortex.proof.service import ProofService, Readiness, sign_submission, sign_topic
from cortex.proof.store import ProofStore
from cortex.protocol.crypto import public_key
from cortex.rlm import AgentLimits
from cortex.rlm.offer import InferenceOffer, sign_offer

OWNER = bytes([17]) * 32
MINER = bytes([23]) * 32
OFFER = sign_offer(
    InferenceOffer(
        model="test/fixture-model",
        limits=AgentLimits(),
        issuer_public_key=public_key(OWNER).hex(),
        status="open",
        valid_from_unix=0,
        valid_until_unix=4102444800,
        signature="0" * 128,
    ),
    OWNER,
)


def artifact_bytes():
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode="w") as archive:
        body = b"# fixture experiment; never used by a live runner\n"
        info = tarfile.TarInfo("experiment.py")
        info.size = len(body)
        archive.addfile(info, io.BytesIO(body))
    return output.getvalue()


class FakeVmBoundary:
    """External VM boundary only; real intake, signatures, storage and payout run."""

    def __init__(self):
        self.jobs = []
        self.entered = asyncio.Event()
        self.release = asyncio.Event()
        self.release.set()
        self.misbind = False

    async def readiness(self):
        return Readiness("sha256:" + "ab" * 32, OFFER.commitment(), frozenset({"fixture-runner"}))

    async def evaluate(self, *, job_id, topic, submission, artifact, env):
        self.jobs.append(job_id)
        self.entered.set()
        await self.release.wait()
        return EvaluationReport(
            topic_id=topic.id,
            topic_digest=topic.content_digest(),
            submission_id=job_id,
            artifact_digest="ef" * 32 if self.misbind else submission.artifact_digest,
            verdict="clean",
            reproduced=True,
            claim_holds=True,
            rule_results={rule.id: True for rule in topic.checklist},
            metrics={"quality": 0.8},
            flops_used=10,
            wall_seconds=1,
            evidence_digest="cd" * 32,
            vm_id="fake-hypervisor-test",
            sandboxed=True,
            teardown_confirmed=True,
        )


@pytest.fixture
def setup(tmp_path):
    store = ProofStore(tmp_path / "proof.sqlite3")
    backend = FakeVmBoundary()
    vault = FileVault(tmp_path / "vault")
    token = tmp_path / "operator-token"
    token.write_text("fixture-operator-token")
    token.chmod(0o600)
    service = ProofService(
        store=store,
        topic_public_key=public_key(OWNER),
        vault=vault,
        artifact_dir=tmp_path / "artifacts",
        backend=backend,
        epoch=lambda: 4,
    )
    evidence = {
        "metrics": {"quality": 0.5},
        "script_sha256": "de" * 32,
        "eval_image_digest": "sha256:" + "ab" * 32,
        "flops_budget": 100,
        "wall_budget_s": 30,
        "sandboxed": True,
        "teardown_confirmed": True,
    }
    evidence_digest = store.register_evidence("fixture-topic", evidence)
    holdout = store.register_holdouts(["private-fixture-content"], ["private-fixture-dataset"])
    topic = sign_topic(
        Topic(
            id="fixture-topic",
            statement="Measure the fixture experiment",
            status="open",
            payout_mode="wta",
            metric=Metric(
                family="custom",
                primary="quality",
                direction="max",
                epsilon=0.1,
                custom_id="fixture-runner",
            ),
            flops_budget=100,
            wall_budget_s=30,
            checklist=[Rule(id="integrity-check", text="Verify the committed artifact")],
            baseline=Baseline(
                script_sha256="de" * 32,
                metrics={"quality": 0.5},
                metrics_commitment=digest({"quality": 0.5}),
                evidence_digest=evidence_digest,
                flops_budget=100,
                wall_budget_s=30,
            ),
            holdout_commitment=holdout,
            eval_image_digest="sha256:" + "ab" * 32,
            inference_offer_commitment=OFFER.commitment(),
            documentation="Send a signed tar artifact.",
            endpoints=[
                Endpoint(
                    path="/submit",
                    method="POST",
                    purpose="submission",
                    description="Submit an experiment",
                )
            ],
        ),
        OWNER,
    )
    store.publish(topic)
    yield service, backend, topic, create_app(service, OperatorAuth(token))
    store.close()


def submission(topic_id="fixture-topic", nonce="12" * 32, env=None, manifest=None):
    data = artifact_bytes()
    body = {
        "topic_id": topic_id,
        "artifact_digest": hashlib.sha256(data).hexdigest(),
        "claim": "A reproducible fixture improvement",
        "submit_nonce": nonce,
        "env": env or {},
        "manifest": manifest or {},
    }
    return sign_submission(body, MINER), data
