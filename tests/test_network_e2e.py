"""Real Python services, crypto, storage and RLM; only external boundaries are fixtures."""

import hashlib
import io
import json
import os
import tarfile
from types import SimpleNamespace

import httpx
from test_master import FakeEpochChain, master_config
from vm.test_research import MemoryCallback
from vm.test_setup_agent import ExecutingHypervisor, SetupProvider

from cortex.master import EpochState, build_master
from cortex.miner import MinerClient
from cortex.proof.backend import VmBackend
from cortex.protocol import ChallengeEntry, TrustRoot
from cortex.protocol.crypto import public_key
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
