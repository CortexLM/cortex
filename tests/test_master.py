import asyncio
import io
import tarfile
from dataclasses import replace
from hashlib import sha256
from types import SimpleNamespace

import httpx
import pytest

from cortex.bounty import PublicBackend
from cortex.config import MasterConfig, read_seed
from cortex.errors import ServiceError
from cortex.master import BittensorEpochProvider, EpochClock, EpochState, build_master
from cortex.proof.models import Baseline, Endpoint, EvaluationReport, Metric, Rule, Topic, digest
from cortex.proof.service import Readiness, sign_submission, sign_topic
from cortex.protocol import ChallengeEntry, MetagraphRow, NoScore, NoScoreReason, Score, TrustRoot
from cortex.protocol.crypto import public_key
from cortex.validator import ChainSnapshot, SubmissionJournal, Validator


class FakeEpochChain:
    def __init__(self):
        self.state = EpochState(12, 90, 95)
        self.subtensor = SimpleNamespace(chain_endpoint="wss://fixture-chain.invalid")
        self.rows = (
            MetagraphRow(public_key(bytes([20]) * 32), 0),
            MetagraphRow(public_key(bytes([21]) * 32), 1),
        )
        self.submissions = []
        self.fail = False

    async def epoch_state(self, netuid):
        if self.fail:
            raise OSError("chain unavailable")
        return self.state

    async def current_block(self):
        return self.state.current_block

    async def snapshot(self, block, netuid):
        return ChainSnapshot(
            block, sha256(str(block).encode()).digest(), self.rows, self.rows[0].hotkey, frozenset()
        )

    async def submit(self, netuid, vector, version_key):
        self.submissions.append(vector)
        return True


async def test_historical_epoch_end_locates_last_block_before_next_index():
    class Rpc:
        def get_block_hash(self, block):
            return str(block)

        def get_subnet_epoch_index(self, netuid, *, block):
            return 12 if block < 100 else 13 if block < 110 else 14

    provider = BittensorEpochProvider(Rpc())
    assert await provider.end_block(541, 12, 90, EpochState(14, 110, 115)) == 99


async def test_restart_cannot_roll_persisted_epoch_back(master):
    await master.runtime.emitter.tick()
    master.chain.state = EpochState(11, 80, 85)
    with pytest.raises(ServiceError, match="backwards"):
        await build_master(
            master.config, chain=master.chain, epochs=master.chain, trust=master.trust
        )


def master_config(tmp_path):
    def secret(name, value):
        path = tmp_path / name
        path.write_bytes(value)
        path.chmod(0o600)
        return path

    return MasterConfig(
        netuid=541,
        state_dir=tmp_path / "state",
        owner_public_file=tmp_path / "owner.pubkey",
        challenges_file=tmp_path / "challenges.toml",
        measurements_file=tmp_path / "measurements.toml",
        gateway_seed_file=secret("gateway.key", bytes([7]) * 32),
        bounty_seed_file=secret("bounty.key", bytes([1]) * 32),
        proof_seed_file=secret("proof.key", bytes([2]) * 32),
        bounty_session_secret_file=secret("session.key", bytes([8]) * 32),
        operator_token_file=secret("operator.token", b"fixture-operator-token"),
    )


@pytest.fixture
async def master(tmp_path):
    config = master_config(tmp_path)
    chain = FakeEpochChain()
    trust = TrustRoot(
        (
            ChallengeEntry(b"bounty", public_key(bytes([1]) * 32), 2000),
            ChallengeEntry(b"proof", public_key(bytes([2]) * 32), 8000),
        ),
        sha256(b"\x00").digest(),
        public_key(bytes([7]) * 32),
    )
    runtime = await build_master(config, chain=chain, epochs=chain, trust=trust)
    yield SimpleNamespace(runtime=runtime, config=config, chain=chain, trust=trust)
    await runtime.close()


async def test_unwired_challenges_cover_exact_epoch_with_internal_absences(master):
    runtime = master.runtime
    assert await runtime.emitter.tick() == []
    assert runtime.gateway.store.leaves(12) == ()
    assert runtime.gateway.latest()["sealed"] is False
    master.chain.state = EpochState(13, 100, 105)
    assert await runtime.emitter.tick() == [12]
    leaves = runtime.gateway.store.leaves(12)
    assert len(leaves) == 4
    assert {leaf.challenge_id for leaf in leaves} == {b"bounty", b"proof"}
    assert all(leaf.epoch == 12 for leaf in leaves)
    assert all(leaf.score == NoScore(NoScoreReason.CHALLENGE_INTERNAL) for leaf in leaves)
    assert runtime.gateway.latest()["sealed"] is True


async def test_completed_epoch_seals_but_current_epoch_stays_open_for_submissions(master):
    runtime = master.runtime
    await runtime.emitter.tick()
    master.chain.state = EpochState(13, 100, 105)
    assert await runtime.emitter.tick() == [12]
    latest = runtime.gateway.latest()
    assert latest["epoch"] == 12 and latest["sealed"] is True
    assert latest["metagraph_block"] == 99
    assert latest["chain_endpoint"] == "wss://fixture-chain.invalid"
    assert latest["emission_shares"] == {"bounty": 0.2, "proof": 0.8}
    assert latest["final_vector"] == [[0, 65535]]
    assert runtime.gateway.store.bundle(13) is None
    app = runtime.app()
    journal = SubmissionJournal(":memory:")
    async with httpx.AsyncClient(
        transport=httpx.ASGITransport(app), base_url="https://master"
    ) as client:
        validator = Validator(
            gateway_url="https://master",
            netuid=541,
            trust=master.trust,
            chain=master.chain,
            journal=journal,
            http=client,
        )
        assert (await validator.run_once()).outcome == "submitted"
    journal.close()
    assert master.chain.submissions == [((0, 65535),)]


async def test_latest_projection_strips_rpc_credentials_and_provider_paths(master, tmp_path):
    master.chain.subtensor.chain_endpoint = (
        "wss://provider-user:provider-secret@rpc.invalid/provider-token?api_key=secret#fragment"
    )
    runtime = await build_master(
        replace(master.config, state_dir=tmp_path / "sanitized-endpoint"),
        chain=master.chain,
        epochs=master.chain,
        trust=master.trust,
    )
    try:
        assert runtime.gateway.latest()["chain_endpoint"] == "wss://rpc.invalid"
    finally:
        await runtime.close()


async def test_latest_projection_tracks_the_sdk_active_fallback_endpoint(master, tmp_path):
    master.chain.subtensor.chain_endpoint = "wss://primary.invalid"
    master.chain.subtensor.substrate = SimpleNamespace(chain_endpoint="wss://fallback-a.invalid")
    runtime = await build_master(
        replace(master.config, state_dir=tmp_path / "active-fallback-endpoint"),
        chain=master.chain,
        epochs=master.chain,
        trust=master.trust,
    )
    try:
        assert runtime.gateway.latest()["chain_endpoint"] == "wss://fallback-a.invalid"
        master.chain.subtensor.substrate.chain_endpoint = "wss://fallback-b.invalid"
        assert runtime.gateway.latest()["chain_endpoint"] == "wss://fallback-b.invalid"
    finally:
        await runtime.close()


async def test_restart_recovers_epoch_pin_and_replaces_stale_positive_with_internal_burn(master):
    from cortex.protocol import sign_leaf

    runtime = master.runtime
    await runtime.emitter.tick()
    scored = sign_leaf(bytes([2]) * 32, b"proof", master.chain.rows[1].hotkey, 12, Score(100))
    runtime.gateway.accept_leaf(scored)
    restarted = await build_master(
        master.config, chain=master.chain, epochs=master.chain, trust=master.trust
    )
    try:
        master.chain.state = EpochState(13, 100, 105)
        assert await restarted.emitter.tick() == [12]
        assert restarted.gateway.latest()["final_vector"] == [[0, 65535]]
        assert all(
            leaf.score == NoScore(NoScoreReason.CHALLENGE_INTERNAL)
            for leaf in restarted.gateway.store.leaves(12)
        )
        original = restarted.gateway.bundle_bytes(12)
        assert await restarted.emitter.tick() == []
        assert restarted.gateway.bundle_bytes(12) == original
    finally:
        await restarted.close()


class PausedVmBoundary:
    def __init__(self, stage):
        self.stage = stage
        self.pause = False
        self.entered = asyncio.Event()
        self.release = asyncio.Event()

    async def readiness(self):
        if self.pause and self.stage == "admission":
            self.pause = False
            self.entered.set()
            await self.release.wait()
        return Readiness("sha256:" + "ab" * 32, "bc" * 32, frozenset({"fixture-runner"}))

    async def evaluate(self, *, job_id, topic, submission, artifact, env):
        assert artifact is not None
        assert sha256(artifact).hexdigest() == submission.artifact_digest
        if self.stage == "evaluation":
            self.entered.set()
            await self.release.wait()
        return EvaluationReport(
            topic_id=topic.id,
            topic_digest=topic.content_digest(),
            submission_id=job_id,
            artifact_digest=submission.artifact_digest,
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


async def publish_fixture_topic(runtime):
    evidence = {
        "metrics": {"quality": 0.5},
        "script_sha256": "de" * 32,
        "eval_image_digest": "sha256:" + "ab" * 32,
        "flops_budget": 100,
        "wall_budget_s": 30,
        "sandboxed": True,
        "teardown_confirmed": True,
    }
    topic = Topic(
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
            evidence_digest=runtime.proof.store.register_evidence("fixture-topic", evidence),
            flops_budget=100,
            wall_budget_s=30,
        ),
        holdout_commitment=runtime.proof.store.register_holdouts(["private-test-content"], []),
        eval_image_digest="sha256:" + "ab" * 32,
        inference_offer_commitment="bc" * 32,
        endpoints=[
            Endpoint(
                path="/submit",
                method="POST",
                purpose="submission",
                description="Submit the fixture experiment",
            )
        ],
    )
    await runtime.proof.publish(sign_topic(topic, bytes([2]) * 32))


@pytest.mark.parametrize(
    "stage,path",
    [
        ("admission", "/v1/submissions"),
        ("evaluation", "/v1/proof/topics/fixture-topic/submit"),
    ],
)
async def test_epoch_seal_waits_for_submission_then_preserves_proof_share(master, stage, path):
    runtime = master.runtime
    backend = PausedVmBoundary(stage)
    runtime.proof.backend = backend
    await publish_fixture_topic(runtime)
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode="w") as archive:
        info = tarfile.TarInfo("experiment.txt")
        content = b"fixture input"
        info.size = len(content)
        archive.addfile(info, io.BytesIO(content))
    artifact = output.getvalue()
    submission = sign_submission(
        {
            "topic_id": "fixture-topic",
            "artifact_digest": sha256(artifact).hexdigest(),
            "claim": "Fixture improvement",
            "submit_nonce": "ef" * 32,
        },
        bytes([21]) * 32,
    )
    backend.pause = True
    async with httpx.AsyncClient(
        transport=httpx.ASGITransport(runtime.app()), base_url="https://master"
    ) as client:
        request = asyncio.create_task(
            client.post(
                path,
                files={
                    "json": (None, submission.model_dump_json()),
                    "artifact": ("experiment.tar", artifact, "application/x-tar"),
                },
            )
        )
        try:
            await asyncio.wait_for(backend.entered.wait(), timeout=2)
            await runtime.emitter.tick()
            master.chain.state = EpochState(13, 100, 105)
            assert await runtime.emitter.tick() == []
            assert runtime.gateway.store.bundle(12) is None
            master.chain.state = EpochState(14, 110, 115)
            assert await runtime.emitter.tick() == []
            assert runtime.gateway.store.bundle(13) is None
        finally:
            backend.release.set()
            response = await request
        assert response.status_code == 201
        assert response.json()["status"] == "accepted"
        assert response.json()["epoch"] == 12
        assert await runtime.emitter.tick() == [12]
        assert runtime.gateway.latest()["final_vector"] == [[0, 13107], [1, 52428]]
        assert runtime.gateway.latest()["sealed"] is True
        assert runtime.gateway.latest()["epoch"] == 12
        assert await runtime.emitter.tick() == [13]
        assert runtime.gateway.latest()["epoch"] == 13


async def test_chain_epoch_outage_expires_intake_clock_instead_of_reusing_stale_epoch():
    chain = FakeEpochChain()
    now = [0.0]
    clock = EpochClock(chain, 541, stale_seconds=60, monotonic=lambda: now[0])
    with pytest.raises(ServiceError, match="unavailable"):
        clock()
    await clock.refresh()
    assert clock() == 12
    chain.fail = True
    with pytest.raises(OSError):
        await clock.refresh()
    now[0] = 60
    with pytest.raises(ServiceError, match="stale"):
        clock()


async def test_private_routes_require_rotating_file_and_master_routes_are_composed(master):
    app = master.runtime.app()
    async with httpx.AsyncClient(
        transport=httpx.ASGITransport(app), base_url="https://master"
    ) as client:
        assert (await client.get("/livez")).json()["role"] == "master"
        readiness = await client.get("/readyz")
        assert readiness.status_code == 200
        assert readiness.json() == {"ready": True, "epoch": 12, "role": "master"}
        assert (await client.get("/v1/proof/topics")).json() == {"topics": []}
        assert (await client.get("/bounty/v1/status")).json()["challenge_id"] == "bounty"
        assert (await client.get("/v1/reports")).status_code == 401
        master.config.operator_token_file.write_text("rotated-token")
        assert (
            await client.get("/v1/reports", headers={"authorization": "Bearer rotated-token"})
        ).status_code == 200


async def test_master_ready_is_independent_of_a_readable_bounty_feed(master):
    def empty_feed(request):
        route = request.url.path.rsplit("/", 1)[-1]
        if route == "status":
            body = {
                "api_version": 1,
                "revision": "1",
                "adjudication_available": True,
                "published": 0,
                "valid": 0,
                "duplicate": 0,
                "already_fixed_not_prod": 0,
                "invalid_malicious": 0,
                "hotkeys": 0,
                "awaiting_adjudication": 0,
                "unpriced_valid": 0,
            }
        else:
            body = {
                "api_version": 1,
                "revision": "1",
                "items": [],
                "has_more": False,
                **({"count": 0, "next_cursor": None} if route == "reports" else {}),
            }
        return httpx.Response(200, json=body)

    master.runtime.bounty.backend = PublicBackend(
        "https://backend.invalid",
        transport=httpx.MockTransport(empty_feed),
    )

    async with httpx.AsyncClient(
        transport=httpx.ASGITransport(master.runtime.app()), base_url="https://master"
    ) as client:
        readiness = await client.get("/readyz")

    assert readiness.status_code == 200
    assert readiness.json() == {"ready": True, "epoch": 12, "role": "master"}


async def test_master_readiness_does_not_probe_the_bounty_feed(master):
    def unexpected_probe(request):
        raise AssertionError("master readiness must not call the external scorer")

    master.runtime.bounty.backend = PublicBackend(
        "https://backend.invalid",
        transport=httpx.MockTransport(unexpected_probe),
    )

    async with httpx.AsyncClient(
        transport=httpx.ASGITransport(master.runtime.app()), base_url="https://master"
    ) as client:
        readiness = await client.get("/readyz")

    assert readiness.status_code == 200
    assert readiness.json() == {"ready": True, "epoch": 12, "role": "master"}


def test_private_seed_rejects_symlinks_permissions_and_accepts_raw_or_hex(tmp_path):
    config = master_config(tmp_path)
    assert read_seed(config.gateway_seed_file) == bytes([7]) * 32
    config.gateway_seed_file.write_text("07" * 32)
    assert read_seed(config.gateway_seed_file) == bytes([7]) * 32
    link = tmp_path / "link"
    link.symlink_to(config.gateway_seed_file)
    with pytest.raises(ServiceError):
        read_seed(link)
    config.gateway_seed_file.chmod(0o644)
    with pytest.raises(ServiceError):
        read_seed(config.gateway_seed_file)


def test_config_base_aliases_fail_closed_and_do_not_accept_inline_secret_values():
    env = {
        "BASE_NETUID": "541",
        "BASE_GATEWAY_SK_FILE": "/private/gateway",
        "BOUNTY_SK_FILE": "/private/bounty",
        "PROOF_SK_FILE": "/private/proof",
        "BOUNTY_SESSION_SECRET_FILE": "/private/session",
        "BASE_GATEWAY_ADMIN_TOKEN_FILE": "/private/token",
    }
    alias = {key.replace("BASE_", "CORTEX_"): value for key, value in env.items()}
    assert MasterConfig.from_env(env) == MasterConfig.from_env(alias)
    with pytest.raises(ValueError, match="conflicting"):
        MasterConfig.from_env({**env, "CORTEX_NETUID": "1"})
    del env["BASE_GATEWAY_SK_FILE"]
    env["BASE_GATEWAY_SK"] = "secret-inline-material"
    with pytest.raises(ValueError, match="BASE_GATEWAY_SK_FILE"):
        MasterConfig.from_env(env)


def test_backend_config_rejects_credentials_in_url(tmp_path):
    with pytest.raises(ValueError, match="HTTPS without credentials"):
        replace(master_config(tmp_path), bounty_backend_url="https://key@example.org")


@pytest.mark.parametrize("value", [float("inf"), float("nan"), 0, -1])
def test_config_rejects_invalid_emission_intervals(tmp_path, value):
    with pytest.raises(ValueError, match="intervals"):
        replace(master_config(tmp_path), emit_poll_seconds=value)
