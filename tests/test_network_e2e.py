"""Real Python services, crypto, storage and RLM; only external boundaries are fixtures."""

import hashlib
import io
import json
import os
import tarfile
from dataclasses import replace
from types import SimpleNamespace

import httpx
import pytest
import sr25519
from fastapi import FastAPI, Request
from fastapi.responses import JSONResponse
from test_master import FakeEpochChain, master_config
from vm.test_research import MemoryCallback
from vm.test_setup_agent import ExecutingHypervisor, SetupProvider

from cortex.master import EpochState, build_master
from cortex.miner import MinerClient, pair_payload
from cortex.proof.backend import VmBackend
from cortex.protocol import ChallengeEntry, MetagraphRow, NoScore, NoScoreReason, Score, TrustRoot
from cortex.protocol.crypto import encode_hotkey, public_key
from cortex.rlm.offer import InferenceOffer, sign_offer
from cortex.rlm.provider import Completion
from cortex.validator import SubmissionJournal, Validator
from cortex.vm.api import create_app
from cortex.vm.guest import GuestIdentity
from cortex.vm.guest_server import GuestServer
from cortex.vm.research import ResearchHost
from cortex.vm.runtime import Orchestrator


class NetworkProvider(SetupProvider):
    """Deterministic model boundary; measures fixture inputs, never scientific claims."""

    async def complete(self, *, messages, **kwargs):
        frame = json.loads(messages[1]["content"])
        context = frame["context"]
        if context["purpose"] == "setup":
            response = await super().complete(messages=messages, **kwargs)
            call = response.tool_calls[0].function
            args = json.loads(call.arguments)
            if call.name == "vm_execute" and args["phase"] == "setup":
                inspect = (
                    "import json,os\n"
                    "json.dump({'rule_checks':[{'rule_id':'originality','passed':True}],"
                    "'flops_used':0},open(os.environ['PROOF_OUTPUT_DIR']+'/report.json','w'))"
                )
                measure = (
                    "import json,os\nfrom pathlib import Path\n"
                    "p=Path(os.environ['PROOF_ARTIFACT_DIR'])/'value.txt'\n"
                    "value=float(p.read_text()) if p.exists() else 0.5\n"
                    "json.dump({'metrics':[{'name':'quality','value':value}],'flops_used':17},"
                    "open(os.environ['PROOF_OUTPUT_DIR']+'/report.json','w'))"
                )
                args["argv"][-1] += (
                    "\n(p/'inspect.py').write_text(" + repr(inspect) + ")"
                    "\n(p/'run.py').write_text(" + repr(measure) + ")"
                )
            elif call.name == "finish":
                args["floor"] = 0.1
            call.arguments = json.dumps(args)
            return response
        results = [json.loads(item["content"]) for item in messages if item["role"] == "tool"]
        measured = next((result for result in results if result.get("metrics")), None)
        if measured is None:
            name = "vm_execute"
            arguments = {
                "operation": "run",
                "phase": "experiment",
                "argv": ["run"],
                "timeout_seconds": 5,
            }
        else:
            name = "finish"
            arguments = {
                "topic_id": context["topic_id"],
                "artifact_digest": context["artifact_digest"],
                "rule_revision": 1,
                "outcome": "accepted",
                "explanation": "Fixture measured by the guest process.",
                "metric": "quality",
                "value": measured["metrics"][0]["value"],
                "report_digest": measured["report_digest"],
                "rules_checked": ["originality"],
            }
        return Completion(
            tool_calls=[
                {
                    "id": "evaluation",
                    "type": "function",
                    "function": {"name": name, "arguments": json.dumps(arguments)},
                }
            ],
            prompt_tokens=100,
            completion_tokens=100,
        )


class HostHttp(httpx.ASGITransport):
    async def handle_async_request(self, request):
        response = await super().handle_async_request(request)
        assert response.status_code < 500, (request.url, await response.aread())
        return response


class GuestWire:
    def __init__(self, host, root, callback):
        self.host, self.root, self.callback = host, root, callback

    async def exchange(self, socket_path, message, budget_seconds):
        record = self.host.get(socket_path.name)
        spec = record.spec
        guest = GuestServer(
            GuestIdentity(record.vm_id, spec.topic_id, spec.image_digest, spec.kind),
            workspace=self.root / record.vm_id,
            callback=self.callback,
        )
        return await guest.handle(message)


class FakeChallenges:
    """Contract-v1 challenge containers, routed by the container host name."""

    def __init__(self):
        self.weights: dict[str, dict[str, float]] = {}
        self.full_share_mass: dict[str, float | None] = {}
        self.failing: set[str] = set()
        self.calls: list[tuple[str, int]] = []
        self.pairs: list[dict] = []
        app = FastAPI()

        @app.get("/internal/v1/get_weights")
        async def get_weights(request: Request, epoch: int):
            slug = request.headers["host"].removeprefix("cortex-challenge-").split(":")[0]
            if request.headers.get("authorization") != f"Bearer {slug}-internal":
                return JSONResponse({"error": "unauthorized"}, status_code=401)
            if request.headers.get("x-platform-challenge-slug") != slug:
                return JSONResponse({"error": "forbidden"}, status_code=403)
            self.calls.append((slug, epoch))
            if slug in self.failing:
                return JSONResponse({"error": "feed unavailable"}, status_code=503)
            return {
                "challenge_slug": slug,
                "epoch": epoch,
                "weights": self.weights.get(slug, {}),
                "full_share_mass": self.full_share_mass.get(slug),
                "metadata": {},
                "computed_at": "2026-09-24T00:00:00Z",
            }

        @app.post("/v1/pair", status_code=201)
        async def pair(request: Request):
            body = await request.json()
            payload = pair_payload(body["account_id"], body["nonce"], body["exp"])
            public = bytes.fromhex(body["hotkey"]) if len(body["hotkey"]) == 64 else None
            assert public is None or sr25519.verify(
                bytes.fromhex(body["signature"]), payload, public
            )
            self.pairs.append(body)
            return {"session": "fixture-session"}

        self.transport = httpx.ASGITransport(app)


def challenge_secrets(config, *slugs):
    for slug in slugs:
        directory = config.challenge_secrets_dir / slug
        directory.mkdir(parents=True, exist_ok=True)
        token = directory / "internal.token"
        token.write_text(f"{slug}-internal")
        token.chmod(0o600)


def registry_file(tmp_path, *slugs):
    path = tmp_path / "challenge-registry.toml"
    path.write_text(
        "version = 1\n"
        + "".join(
            f'[[challenge]]\nid = "{slug}"\nimage = "ghcr.io/fixture/{slug}"\n'
            f'source = "https://github.com/fixture/{slug}"\n'
            for slug in slugs
        )
    )
    return path


@pytest.mark.parametrize(
    "version,counts,payouts",
    [
        (1, (3, 0), (0, 0)),
        (2, (0, 0), (0, 0)),
        (2, (1, 0), (0.03, 0)),
        (2, (2, 3), (0.06, 0.09)),
        (2, (4, 5), (0.12, 0.15)),
        (2, (4, 6), (0.12, 0.18)),
        (2, (8, 12), (0.12, 0.18)),
        (3, (0, 0), (0, 0)),
        (3, (1, 0), (0.03, 0)),
        (3, (2, 3), (0.06, 0.09)),
        (3, (4, 6), (0.12, 0.18)),
        (3, (8, 12), (0.12, 0.18)),
    ],
)
async def test_bounty_container_weights_seal_and_validator_dispatch(
    tmp_path, version, counts, payouts
):
    """Algorithm 3 with full_share_mass=10 pays exactly what algorithm 2 pays."""
    config = replace(
        master_config(tmp_path), challenge_registry_file=registry_file(tmp_path, "bounty")
    )
    challenge_secrets(config, "bounty")
    chain = FakeEpochChain()
    second_key = public_key(bytes([22]) * 32)
    chain.rows += (MetagraphRow(second_key, 2),)
    bounty_share = 2000 if version == 1 else 3000
    trust = TrustRoot(
        (
            ChallengeEntry(b"bounty", public_key(bytes([1]) * 32), bounty_share),
            ChallengeEntry(b"proof", public_key(bytes([2]) * 32), 10000 - bounty_share),
        ),
        hashlib.sha256(b"\x00").digest(),
        public_key(bytes([7]) * 32),
        challenges_version=version,
    )
    fake = FakeChallenges()
    miner_key = public_key(bytes([21]) * 32)
    fake.weights["bounty"] = {
        encode_hotkey(key): count
        for key, count in zip((miner_key, second_key), counts, strict=True)
        if count
    }
    fake.weights["bounty"][encode_hotkey(public_key(bytes([99]) * 32))] = 50  # not registered
    fake.full_share_mass["bounty"] = 10
    runtime = await build_master(
        config,
        chain=chain,
        epochs=chain,
        trust=trust,
        challenge_http=httpx.AsyncClient(transport=fake.transport),
    )
    journal = SubmissionJournal(tmp_path / "validator.sqlite3")
    try:
        async with httpx.AsyncClient(
            transport=httpx.ASGITransport(runtime.app()),
            base_url="https://master.fixture",
        ) as http:
            miner = MinerClient(
                base_url="https://master.fixture", seed=bytes([21]) * 32, client=http
            )
            paired = await miner.pair_bounty(account_id="bounty-fixture", accept_terms=True)
            assert paired == {"session": "fixture-session"}
            assert fake.pairs[0]["hotkey"] == miner.hotkey
            internal = await http.get("/challenge/bounty/internal/v1/get_weights?epoch=12")
            assert internal.status_code == 404

            await runtime.emitter.tick()
            chain.state = EpochState(13, 100, 105)
            assert await runtime.emitter.tick() == [12]
            # Algorithm 1 never asks a container: its legacy lattice is not a weight.
            assert fake.calls == ([] if version == 1 else [("bounty", 12)])
            leaves = runtime.gateway.store.leaves(12)
            assert {leaf.miner_hotkey for leaf in leaves if leaf.challenge_id == b"bounty"} == {
                row.hotkey for row in chain.rows
            }
            mine = next(
                leaf
                for leaf in leaves
                if leaf.challenge_id == b"bounty" and leaf.miner_hotkey == miner_key
            )
            if version == 1:
                assert mine.score == NoScore(NoScoreReason.CHALLENGE_INTERNAL)
            elif not counts[0]:
                assert mine.score == NoScore(NoScoreReason.NOT_ATTEMPTED)
            elif version == 3:
                assert mine.score == Score(10**12 * counts[0] // max(10, sum(counts)))
            else:
                assert mine.score == Score(counts[0])
            assert all(
                leaf.score == NoScore(NoScoreReason.CHALLENGE_INTERNAL)
                for leaf in leaves
                if leaf.challenge_id == b"proof"
            )
            latest = (await http.get("/v1/weights/latest")).json()
            assert latest["sealed"] is True
            assert latest["algorithm_version"] == version
            # The chain normalizes: a zero-miner vector burns only the declared shares
            # (the prod zero-miner burn fix), which is still a full burn once normalized.
            total = sum(latest["weights"])
            weights = {u: w / total for u, w in zip(latest["uids"], latest["weights"], strict=True)}
            assert weights[0] == pytest.approx(1 - sum(payouts))
            assert weights.get(1, 0) == pytest.approx(payouts[0])
            assert weights.get(2, 0) == pytest.approx(payouts[1])
            metagraph = (await http.get("/v1/metagraph/latest")).json()
            assert metagraph["hotkeys"][miner.hotkey] == 1

            preflight = Validator(
                gateway_url="https://master.fixture",
                netuid=541,
                trust=trust,
                chain=chain,
                journal=journal,
                http=http,
                verify_only=True,
            )
            assert (await preflight.run_once()).outcome == "verified"
            assert chain.submissions == []
            validator = Validator(
                gateway_url="https://master.fixture",
                netuid=541,
                trust=trust,
                chain=chain,
                journal=journal,
                http=http,
            )
            assert (await validator.run_once()).outcome == "submitted"
            assert chain.submissions == [tuple(tuple(pair) for pair in latest["final_vector"])]
    finally:
        journal.close()
        await runtime.close()


async def test_three_container_challenges_burn_failures_and_unpaid_mass(tmp_path):
    config = replace(
        master_config(tmp_path),
        challenge_registry_file=registry_file(tmp_path, "bounty", "opentype"),
    )
    challenge_secrets(config, "bounty", "opentype")
    for slug, seed in (("opentype", 3), ("audit", 4)):
        key = tmp_path / f"{slug}.key"
        key.write_bytes(bytes([seed]) * 32)
        key.chmod(0o600)
    chain = FakeEpochChain()
    trust = TrustRoot(
        (
            ChallengeEntry(b"audit", public_key(bytes([4]) * 32), 1000),
            ChallengeEntry(b"bounty", public_key(bytes([1]) * 32), 3000),
            ChallengeEntry(b"opentype", public_key(bytes([3]) * 32), 4000),
            ChallengeEntry(b"proof", public_key(bytes([2]) * 32), 2000),
        ),
        hashlib.sha256(b"\x00").digest(),
        public_key(bytes([7]) * 32),
        challenges_version=3,
    )
    fake = FakeChallenges()
    miner = encode_hotkey(chain.rows[1].hotkey)
    fake.weights["opentype"] = {miner: 0.25}
    fake.full_share_mass["opentype"] = 1.0
    fake.failing.add("bounty")
    runtime = await build_master(
        config,
        chain=chain,
        epochs=chain,
        trust=trust,
        challenge_http=httpx.AsyncClient(transport=fake.transport),
    )
    try:
        await runtime.emitter.tick()
        chain.state = EpochState(13, 100, 105)
        assert await runtime.emitter.tick() == [12]
        leaves = {
            (leaf.challenge_id, leaf.miner_hotkey): leaf.score
            for leaf in runtime.gateway.store.leaves(12)
        }
        # Registered in trust but not in the registry, and a failing container: both burn.
        for challenge in (b"audit", b"bounty"):
            assert {score for (cid, _), score in leaves.items() if cid == challenge} == {
                NoScore(NoScoreReason.CHALLENGE_INTERNAL)
            }
        assert leaves[(b"opentype", chain.rows[1].hotkey)] == Score(250_000_000_000)
        latest = runtime.gateway.latest()
        weights = dict(zip(latest["uids"], latest["weights"], strict=True))
        assert weights[1] == pytest.approx(0.4 * 0.25)
        assert weights[0] == pytest.approx(1 - 0.4 * 0.25)
    finally:
        await runtime.close()


async def test_owner_setup_miner_submission_seal_and_validator_dispatch(tmp_path, monkeypatch):
    config = master_config(tmp_path)
    chain = FakeEpochChain()
    trust = TrustRoot(
        (
            ChallengeEntry(b"bounty", public_key(bytes([1]) * 32), 2000),
            ChallengeEntry(b"proof", public_key(bytes([2]) * 32), 8000),
        ),
        hashlib.sha256(b"\x00").digest(),
        public_key(bytes([7]) * 32),
    )
    hypervisor = ExecutingHypervisor(tmp_path / "guests")
    hypervisor.config = SimpleNamespace(images={"ab" * 32: None})
    host = Orchestrator(tmp_path / "host.sqlite3", hypervisor)
    callback = MemoryCallback()
    monkeypatch.setattr("cortex.vm.research.asyncio.start_unix_server", callback.start)
    # The memory transport has no filesystem socket to chmod; leave credential modes intact.
    chmod = os.chmod

    def socket_chmod(path, mode, **kwargs):
        if not str(path).endswith("_5001"):
            chmod(path, mode, **kwargs)

    monkeypatch.setattr("cortex.vm.research.os.chmod", socket_chmod)
    research = ResearchHost(
        host,
        NetworkProvider(),
        lambda vm_id: tmp_path / vm_id,
        transport=GuestWire(host, tmp_path / "rlm-guests", callback),
    )
    offer_seed = bytes([2]) * 32
    offer = sign_offer(
        InferenceOffer(
            model=research.provider.model,
            limits=research.limits,
            issuer_public_key=public_key(offer_seed).hex(),
            status="open",
            valid_from_unix=0,
            valid_until_unix=4102444800,
            signature="0" * 128,
        ),
        offer_seed,
    )
    backend = VmBackend(
        url="https://host.fixture",
        token_file=config.operator_token_file,
        image_digest="sha256:" + "ab" * 32,
        inference_offer_commitment=offer.commitment(),
        custom_ids=frozenset({"fixture-runner"}),
        transport=HostHttp(
            create_app(
                host,
                config.operator_token_file,
                research,
                inference_offer_commitment=offer.commitment(),
                inference_offer=offer,
                custom_ids=("fixture-runner",),
            )
        ),
    )
    runtime = await build_master(
        config, chain=chain, epochs=chain, trust=trust, proof_backend=backend
    )
    journal = SubmissionJournal(tmp_path / "validator.sqlite3")
    try:
        async with httpx.AsyncClient(
            transport=HostHttp(runtime.app()), base_url="https://master.fixture"
        ) as http:
            setup = await http.post(
                "/v1/admin/proof/setup",
                headers={"authorization": "Bearer fixture-operator-token"},
                json={
                    "policy": {
                        "topic_id": "topic-a",
                        "objective": "Measure a fixture value from the submitted artifact.",
                        "flops_budget": 100,
                        "wall_budget_s": 30,
                        "payout_mode": "wta",
                    }
                },
            )
            assert setup.status_code == 201, setup.text
            topic = setup.json()
            assert topic["status"] == "open"
            assert topic["baseline"]["metrics"] == {"quality": 0.5}
            assert "content_hashes" not in setup.text and "dataset_ids" not in setup.text
            miner = MinerClient(
                base_url="https://master.fixture",
                seed=bytes([21]) * 32,
                proof_public_key=public_key(bytes([2]) * 32),
                client=http,
            )
            stream = io.BytesIO()
            with tarfile.open(fileobj=stream, mode="w") as archive:
                info = tarfile.TarInfo("value.txt")
                info.size = 3
                archive.addfile(info, io.BytesIO(b"0.8"))
            submitted = await miner.submit_proof(
                topic_id="topic-a", artifact=stream.getvalue(), claim="Fixture improves to 0.8"
            )
            assert submitted["status"] == "accepted"
            assert submitted["metrics"] == {"quality": 0.8}
            experiments = [vm_id for vm_id, spec in hypervisor.booted if spec.kind == "experiment"]
            assert len(experiments) == 3  # baseline, preflight, measured evaluation
            assert all(host.get(vm_id).state == "destroyed" for vm_id in experiments)
            await runtime.emitter.tick()
            chain.state = EpochState(13, 100, 105)
            assert await runtime.emitter.tick() == [12]
            latest = (await http.get("/v1/weights/latest")).json()
            assert latest["sealed"] is True
            assert latest["final_vector"] == [[0, 13107], [1, 52428]]
            validator = Validator(
                gateway_url="https://master.fixture",
                netuid=541,
                trust=trust,
                chain=chain,
                journal=journal,
                http=http,
            )
            assert (await validator.run_once()).outcome == "submitted"
            assert chain.submissions == [((0, 13107), (1, 52428))]
    finally:
        journal.close()
        await runtime.close()
        await backend.close()
        await research.close()
        await host.close()
