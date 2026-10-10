"""Real relay HTTP/storage integration; tiny objects except exact chunk-boundary test."""

from __future__ import annotations

import hashlib
from pathlib import Path

import anyio
import httpx
import pytest

from hypertrain.data.store import CorruptObjectError, LocalFSStore
from hypertrain.data.stream_store import StreamStore, blocks, spool
from hypertrain.protocol import envelope_v2, relay_envelope
from hypertrain.protocol.envelope import SignatureError
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages_v2 import RoundOpenV2
from hypertrain.protocol.relay_envelope import RelayEnvelope
from hypertrain.protocol.relay_messages import (
    AcceptedUploadAck,
    ChunkCustodyAck,
    RelayAssignment,
    RelayKey,
    RelayReceipt,
    RelayRegistryV1,
    RelaySpec,
    RetrievalRequest,
    RetrievalResponse,
    UploadChunk,
    UploadChunkManifest,
    UploadGrant,
    has_full_custody,
)
from hypertrain.relay.app import create_app
from hypertrain.relay.client import RelayClient, receipt_verified
from hypertrain.relay.core import Relay, RelayError

RUN = "ab" * 32
MASTER = Keypair(bytes([1]) * 32)
MINER = Keypair(bytes([2]) * 32)
RELAY = Keypair(bytes([3]) * 32)
NEXT = Keypair(bytes([4]) * 32)
H = "12" * 32


def test_shipped_gateway_target_port_policy(tmp_path):
    import subprocess

    import yaml

    root = Path(__file__).resolve().parents[2]
    fixtures = list(yaml.safe_load_all((root / "deploy/k8s/local/fixtures.yaml").read_text()))
    gateway = next(
        d for d in fixtures if d["kind"] == "Service" and d["metadata"]["name"] == "gateway"
    )
    target = gateway["spec"]["ports"][0]["targetPort"]
    assert gateway["spec"]["ports"][0]["port"] == 443 and target == 8443
    for overlay in ("local", "eu", "us", "apac"):
        rendered = subprocess.run(
            ["kubectl", "kustomize", str(root / "deploy/k8s/overlays" / overlay)],
            capture_output=True,
            text=True,
            check=True,
            timeout=15,
        ).stdout
        (tmp_path / (overlay + ".yaml")).write_text(rendered)
        documents = list(yaml.safe_load_all(rendered))
        policy = next(
            d
            for d in documents
            if d["kind"] == "NetworkPolicy" and d["metadata"]["name"] == "relay-authorized-flows"
        )
        assert policy["metadata"]["namespace"] == "hypertrain-relay-" + overlay
        assert policy["spec"]["podSelector"] == {"matchLabels": {"app": "hypertrain-relay"}}
        rules = [
            r
            for r in policy["spec"]["egress"]
            if not r.get("ports")
            or any(
                p.get("protocol", "TCP") == "TCP" and p.get("port", target) == target
                for p in r["ports"]
            )
        ]
        assert rules == [
            {
                "to": [
                    {
                        "namespaceSelector": {
                            "matchLabels": {"hypertrain.network/role": "gateway"}
                        },
                        "podSelector": {"matchLabels": {"hypertrain.network/role": "gateway"}},
                    }
                ],
                "ports": [{"protocol": "TCP", "port": target}],
            }
        ]
        for namespace_role, pod_role in (
            ("gateway", "gateway"),
            ("gateway", "store"),
            ("store", "gateway"),
            ("store", "store"),
        ):
            allowed = any(
                peer["namespaceSelector"]["matchLabels"]["hypertrain.network/role"]
                == namespace_role
                and peer["podSelector"]["matchLabels"]["hypertrain.network/role"] == pod_role
                for rule in rules
                for peer in rule["to"]
            )
            assert allowed == (namespace_role == pod_role == "gateway")
        assert any({"protocol": "TCP", "port": 443} in r["ports"] for r in policy["spec"]["egress"])
        deny = next(
            d
            for d in documents
            if d["kind"] == "NetworkPolicy" and d["metadata"]["name"] == "relay-deny-default"
        )
        assert deny["spec"] == {"podSelector": {}, "policyTypes": ["Ingress", "Egress"]}


class Rig:
    """Mutable injected beacon; every other boundary uses actual protocol/storage."""

    def __init__(self, path: Path, client: httpx.AsyncClient) -> None:
        self.path, self.clock, self.client = path, 20, client
        self.registry = RelayRegistryV1(
            registry_version=1,
            epoch=0,
            previous_registry_hash=None,
            specs=[
                RelaySpec(
                    id="eu-1",
                    region="eu",
                    https_url="https://relay.test",
                    pubkeys=[
                        RelayKey(
                            key_id="k1",
                            pubkey=RELAY.ss58,
                            valid_from_round=1,
                            valid_until_round=100,
                        ),
                        RelayKey(
                            key_id="k2",
                            pubkey=NEXT.ss58,
                            valid_from_round=80,
                            valid_until_round=500,
                        ),
                    ],
                    max_object_bytes=16 << 20,
                    max_inflight_bytes=32 << 20,
                    codecs=["ht-sparse-v1"],
                    mode="transport",
                )
            ],
        )
        self.store = StreamStore(LocalFSStore(path), client)
        self.relay = self.new_relay()

    async def now(self) -> int:
        return self.clock

    def new_relay(self) -> Relay:
        return Relay(
            run_id=RUN,
            master=MASTER.ss58,
            registry=self.registry,
            network_manifest_hash=H,
            observers={MASTER.ss58, MINER.ss58},
            relay_id="eu-1",
            region="eu",
            keys={"k1": RELAY, "k2": NEXT},
            active_key="k1",
            backing=self.store,
            now=self.now,
            epoch_rounds=10,
        )

    def seal(self, model, key: Keypair = MASTER, exp: int = 500) -> RelayEnvelope:
        return relay_envelope.parse_envelope(
            relay_envelope.seal(key, type(model).__name__, RUN, model, exp)
        )

    def upload(self, data: bytes, *, nonce: str = H, w: int = 1):
        chunks = [
            UploadChunk(
                index=i,
                off=off,
                len=len(data[off : off + (4 << 20)]),
                chunk_sha256=hashlib.sha256(data[off : off + (4 << 20)]).hexdigest(),
            )
            for i, off in enumerate(range(0, len(data), 4 << 20))
        ]
        manifest = UploadChunkManifest(size=len(data), chunks=chunks)
        grant = UploadGrant(
            w=w,
            hotkey=MINER.ss58,
            relay_id="eu-1",
            assignment_epoch=0,
            delta_hash=hashlib.sha256(data).hexdigest(),
            size=len(data),
            chunk_manifest_hash=manifest.chunk_manifest_hash(),
            retain_until=200,
            nonce=nonce,
            exp_drand=40,
        )
        assignment = RelayAssignment(
            w=w,
            hotkey=MINER.ss58,
            primary_id="eu-1",
            fallback_ids=[],
            assignment_epoch=0,
            manifest_hash=H,
            exp_drand=40,
        )
        opened = RoundOpenV2.model_validate(
            {
                "w": w,
                **{
                    k: H
                    for k in (
                        "prev_final_hash",
                        "theta_hash",
                        "outer_state_hash",
                        "center_hash",
                        "roster_hash",
                        "honeypot_commit",
                        "start_state_index_hash",
                    )
                },
                "d_open": 1,
                "d_assign": 2,
                "d_commit": 10,
                "d_audit": 15,
                "d_upload": 35,
                "d_final": 50,
                "contract_version": 2,
                "policy_hashes": {
                    k: H
                    for k in (
                        "admission_policy_hash",
                        "economics_policy_hash",
                        "aggregation_policy_hash",
                        "dispute_policy_hash",
                        "audit_policy_hash",
                    )
                },
                "registry_epoch": 0,
                "audit_mode": "anchored-full",
                "roster": [],
            }
        )
        raw_round = envelope_v2.parse_envelope(
            envelope_v2.seal(MASTER, "RoundOpenV2", RUN, opened, 50)
        )
        return grant, manifest, self.seal(grant), self.seal(assignment), raw_round

    async def accept(self, data: bytes):
        grant, manifest, raw, assignment, opened = self.upload(data)
        opportunity = await self.relay.accept(raw, manifest, assignment, opened)
        return grant, manifest, opportunity

    async def put(self, grant: UploadGrant, data: bytes, index: int = 0) -> RelayEnvelope:
        async def source():
            yield data

        return await self.relay.chunk(grant.digest(), index, source())

    def retrieval(
        self,
        grant: UploadGrant,
        custody: RelayEnvelope,
        *,
        requested: int | None = None,
        nonce: str = "45" * 32,
        horizon: int = 200,
        retention_hash: str | None = None,
    ) -> RelayEnvelope:
        stamp = self.clock if requested is None else requested
        if custody.type == "RelayReceipt":
            body = RelayReceipt.model_validate(custody.body)
            hashes, receipt_hash, sha, size = (
                [],
                body.digest(),
                body.delta_hash,
                body.size,
            )
        else:
            body = ChunkCustodyAck.model_validate(custody.body)
            hashes, receipt_hash, sha, size = (
                [body.digest()],
                None,
                body.chunk_sha256,
                body.len,
            )
        assignment = RelayAssignment.model_validate(self.upload(b"a")[3].body)
        request = RetrievalRequest(
            request_id=nonce,
            nonce=nonce,
            relay_id="eu-1",
            key_id="k1",
            assignment_hash=assignment.digest(),
            grant_hash=grant.digest(),
            custody_ack_hashes=hashes,
            receipt_hash=receipt_hash,
            retention_hash=retention_hash or body.digest(),
            object_or_chunk_hash=sha,
            size=size,
            requested_beacon=stamp,
            deadline_beacon=min(stamp + 10, horizon),
        )
        return self.seal(request)


def test_original_payload_when_http_upload_and_master_retrieval(tmp_path: Path) -> None:
    async def scenario() -> None:
        async with httpx.AsyncClient() as backing:
            rig = Rig(tmp_path / "objects", backing)
            app = create_app(rig.relay, "d" * 32)
            async with httpx.AsyncClient(
                transport=httpx.ASGITransport(app=app), base_url="https://relay.test"
            ) as wire:
                client = RelayClient(rig.registry, wire)
                payload = b"original sparse bytes\x00\xff" * 300
                path = tmp_path / "delta"
                path.write_bytes(payload)
                grant, _, raw, assignment, opened = rig.upload(payload)
                receipt = await client.upload(
                    path,
                    grant_raw=raw,
                    assignment=assignment,
                    round_open=opened.model_dump(mode="json"),
                )
                rig.clock = 41  # retrieval survives upload cutoff
                with spool() as file:
                    response = await client.retrieval(
                        rig.retrieval(grant, receipt), file, relay_public=RELAY.ss58
                    )
                    assert file.read() == payload
                    assert RetrievalResponse.model_validate(response.body).status == "SERVED"
                assert receipt == await rig.relay.complete(grant.digest())

    anyio.run(scenario)


@pytest.mark.parametrize("fault", ["signature", "region", "epoch", "expired", "run", "manifest"])
def test_grant_rejected_when_not_authorized(tmp_path: Path, fault: str) -> None:
    async def scenario() -> None:
        async with httpx.AsyncClient() as wire:
            rig = Rig(tmp_path, wire)
            grant, manifest, raw, assignment, opened = rig.upload(b"abc")
            if fault == "signature":
                raw = rig.seal(grant, RELAY)
            elif fault == "region":
                raw = rig.seal(UploadGrant.model_validate({**grant.body(), "relay_id": "us-1"}))
            elif fault == "epoch":
                raw = rig.seal(UploadGrant.model_validate({**grant.body(), "assignment_epoch": 1}))
            elif fault == "expired":
                rig.clock = 41
            elif fault == "run":
                raw = relay_envelope.parse_envelope(
                    relay_envelope.seal(MASTER, "UploadGrant", "cd" * 32, grant, 500)
                )
            else:
                manifest = UploadChunkManifest(
                    size=3, chunks=[UploadChunk(index=0, off=0, len=3, chunk_sha256=H)]
                )
            with pytest.raises((RelayError, SignatureError, ValueError)):
                await rig.relay.accept(raw, manifest, assignment, opened)
            assert not (await rig.relay.state()).uploads

    anyio.run(scenario)


@pytest.mark.parametrize("payload", [b"ab", b"abcd", b"xyz"])
def test_chunk_rejected_when_size_or_hash_differs(tmp_path: Path, payload: bytes) -> None:
    async def scenario() -> None:
        async with httpx.AsyncClient() as wire:
            rig = Rig(tmp_path, wire)
            grant, _, _ = await rig.accept(b"abc")
            with pytest.raises(CorruptObjectError):
                await rig.put(grant, payload)
            assert not (await rig.relay.state()).uploads[grant.digest()].chunks

    anyio.run(scenario)


def test_full_coverage_when_miners_stop_after_opportunity_or_prefix(
    tmp_path: Path,
) -> None:
    async def scenario() -> None:
        async with httpx.AsyncClient() as wire:
            rig = Rig(tmp_path, wire)
            payload = b"a" * (4 << 20) + b"suffix"
            grant, manifest, raw = await rig.accept(payload)
            opportunity = AcceptedUploadAck.model_validate(raw.body)
            assert not has_full_custody(grant, manifest, opportunity, [])
            prefix_raw = await rig.put(grant, payload[: 4 << 20])
            prefix = ChunkCustodyAck.model_validate(prefix_raw.body)
            assert not has_full_custody(grant, manifest, opportunity, [prefix])
            with pytest.raises(RelayError):
                await rig.relay.complete(grant.digest())
            rig.clock = 36
            assert not has_full_custody(grant, manifest, opportunity, [prefix, prefix])
            assert not (await rig.relay.state()).disabled_keys

    anyio.run(scenario)


def test_automatic_completion_when_last_chunk_has_custody(tmp_path: Path) -> None:
    async def scenario() -> None:
        async with httpx.AsyncClient() as wire:
            rig = Rig(tmp_path, wire)
            payload = b"a" * (4 << 20) + b"suffix"
            grant, manifest, _ = await rig.accept(payload)
            await rig.put(grant, payload[: 4 << 20])
            await rig.put(grant, payload[4 << 20 :], 1)
            record = (await rig.relay.state()).uploads[grant.digest()]
            assert record.receipt is not None
            assert has_full_custody(
                grant,
                manifest,
                AcceptedUploadAck.model_validate(record.opportunity.body),
                [ChunkCustodyAck.model_validate(e.body) for e in record.chunks.values()],
            )
            with spool() as file:
                await rig.store.get(grant.delta_hash, grant.size, file)
                assert file.read() == payload

    anyio.run(scenario)


def test_replicas_when_racing_same_upload_keep_original_receipts(
    tmp_path: Path,
) -> None:
    async def scenario() -> None:
        async with httpx.AsyncClient() as wire:
            rig = Rig(tmp_path, wire)
            peer = rig.new_relay()
            grant, manifest, raw, assignment, opened = rig.upload(b"abc")
            started = anyio.Event()
            outputs: list[RelayEnvelope] = []

            async def accept(relay: Relay) -> None:
                await started.wait()
                outputs.append(await relay.accept(raw, manifest, assignment, opened))

            async with anyio.create_task_group() as tasks:
                tasks.start_soon(accept, rig.relay)
                tasks.start_soon(accept, peer)
                started.set()
            assert outputs[0] == outputs[1]
            chunk = await rig.put(grant, b"abc")
            with spool() as file:
                file.write(b"abc")
                file.seek(0)
                assert chunk == await peer.chunk(grant.digest(), 0, blocks(file))
            assert len((await peer.state()).uploads) == 1
            assert (await peer.state()).uploads[grant.digest()].receipt == await rig.relay.complete(
                grant.digest()
            )

    anyio.run(scenario)


def test_forged_receipt_when_master_verifies_durable_claim(tmp_path: Path) -> None:
    async def scenario() -> None:
        async with httpx.AsyncClient() as wire:
            rig = Rig(tmp_path, wire)
            grant, _, _ = await rig.accept(b"abc")
            await rig.put(grant, b"abc")
            original = await rig.relay.complete(grant.digest())
            fake = rig.seal(RelayReceipt.model_validate(original.body), MINER)
            with pytest.raises(SignatureError):
                receipt_verified(
                    fake,
                    registry=rig.registry,
                    relay_id="eu-1",
                    run_id=RUN,
                    grant=grant,
                    received_before=35,
                )
            assert (await rig.relay.state()).uploads[grant.digest()].master_acceptance is None

    anyio.run(scenario)


def test_s3_streaming_when_conditional_metadata_and_durable_chunks(
    tmp_path: Path,
) -> None:
    async def scenario() -> None:
        import importlib.util
        import sys

        spec = importlib.util.spec_from_file_location(
            "relay_smoke_fixture",
            Path(__file__).parents[2] / "scripts/relay_kind_smoke.py",
        )
        assert spec is not None and spec.loader is not None
        module = importlib.util.module_from_spec(spec)
        sys.modules[spec.name] = module
        spec.loader.exec_module(module)
        secret_dir = tmp_path / "secret"
        secret_dir.mkdir()
        secret = secret_dir / "object.json"
        secret.write_text('{"access_key_id":"test","secret_access_key":"secret"}')
        secret.chmod(0o600)
        module.FIXTURE = secret_dir
        # The fixture stores under an isolated injected path; never touches cluster.
        module.OBJECT_ROOT = tmp_path / "durable"
        from hypertrain.data.store import S3Credentials, S3Store

        app = module.fixture_store()
        async with httpx.AsyncClient(
            transport=httpx.ASGITransport(app=app), base_url="http://objects.test"
        ) as wire:
            store = StreamStore(
                S3Store(
                    "http://objects.test",
                    "proof",
                    S3Credentials.from_secret_file(secret),
                    region="local",
                    prefix="relay/eu/",
                ),
                wire,
            )
            rig = Rig(tmp_path / "unused", wire)
            rig.store = store
            rig.relay.backing = store
            grant, _, opportunity = await rig.accept(b"abc")
            await rig.put(grant, b"abc")
            receipt = await rig.relay.complete(grant.digest())
            peer = rig.new_relay()
            peer.backing = store
            state = await peer.state()
            assert state.uploads[grant.digest()].opportunity == opportunity
            assert state.uploads[grant.digest()].receipt == receipt
            with spool() as file:
                await store.get(grant.delta_hash, grant.size, file)
                assert file.read() == b"abc"

    anyio.run(scenario)


def test_smoke_master_when_restart_preserves_grant_and_original_acceptance(
    tmp_path: Path,
) -> None:
    # Given: real SigV4 backing and a SQLite transport master in isolated fixture roots.
    async def scenario() -> None:
        import importlib.util
        import sys

        spec = importlib.util.spec_from_file_location(
            "relay_master_fixture",
            Path(__file__).parents[2] / "scripts/relay_kind_smoke.py",
        )
        assert spec is not None and spec.loader is not None
        module = importlib.util.module_from_spec(spec)
        sys.modules[spec.name] = module
        spec.loader.exec_module(module)
        secret = tmp_path / "secrets"
        secret.mkdir()
        (secret / "master.seed").write_text((bytes([1]) * 32).hex())
        module.FIXTURE = secret
        module.MASTER_ROOT = tmp_path / "master"
        module.RUN = RUN
        async with httpx.AsyncClient() as backing:
            rig = Rig(tmp_path / "objects", backing)
            module.registry = lambda: rig.registry
            module.backing = lambda client, region: rig.store
            grant, _, raw, _, _ = rig.upload(b"abc")
            await rig.accept(b"abc")
            await rig.put(grant, b"abc")
            receipt = await rig.relay.complete(grant.digest())
            async with httpx.AsyncClient(
                transport=httpx.ASGITransport(app=module.fixture_master()),
                base_url="https://master.test",
            ) as first:
                assert (
                    await first.post("/register", content=raw.model_dump_json())
                ).status_code == 200
                accepted = await first.post("/v2/relay-receipts", content=receipt.model_dump_json())
                assert accepted.status_code == 200
            # When: a fresh app accepts the same receipt after process restart.
            async with httpx.AsyncClient(
                transport=httpx.ASGITransport(app=module.fixture_master()),
                base_url="https://master.test",
            ) as restarted:
                duplicate = await restarted.post(
                    "/v2/relay-receipts", content=receipt.model_dump_json()
                )
            # Then: durable grant authorization and exact original signed acceptance survive.
            assert duplicate.status_code == 200
            assert duplicate.content == accepted.content

    anyio.run(scenario)


def test_master_acceptance_when_exact_original_object_read_before_cutoff(
    tmp_path: Path,
) -> None:
    async def scenario() -> None:
        async with httpx.AsyncClient() as backing:
            rig = Rig(tmp_path, backing)
            grant, _, _ = await rig.accept(b"abc")
            await rig.put(grant, b"abc")
            receipt = await rig.relay.complete(grant.digest())
            async with httpx.AsyncClient(
                transport=httpx.ASGITransport(app=create_app(rig.relay, "t" * 32)),
                base_url="https://relay.test",
            ) as wire:
                client = RelayClient(rig.registry, wire)
                accepted = await client.accept_receipt(
                    receipt,
                    grant=grant,
                    request_raw=rig.retrieval(grant, receipt),
                    master=MASTER,
                    now=20,
                    deadline=35,
                )
                assert accepted.signer == MASTER.ss58
                assert accepted.body["delta_hash"] == hashlib.sha256(b"abc").hexdigest()
                assert relay_envelope.verify_envelope(accepted.model_dump())

    anyio.run(scenario)


def test_unsigned_chunk_http_when_known_grant_hash_is_not_capability(
    tmp_path: Path,
) -> None:
    async def scenario() -> None:
        async with httpx.AsyncClient() as backing:
            rig = Rig(tmp_path, backing)
            grant, _, _ = await rig.accept(b"abc")
            async with httpx.AsyncClient(
                transport=httpx.ASGITransport(app=create_app(rig.relay, "t" * 32)),
                base_url="https://relay.test",
            ) as wire:
                response = await wire.put(f"/v1/uploads/{grant.digest()}/chunks/0", content=b"abc")
                assert response.status_code == 400
                assert not (await rig.relay.state()).uploads[grant.digest()].chunks

    anyio.run(scenario)


def test_smoke_harness_when_local_source_and_positive_resource_admission(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import importlib.util
    import json
    import sys

    spec = importlib.util.spec_from_file_location(
        "relay_harness_boundaries",
        Path(__file__).parents[2] / "scripts/relay_kind_smoke.py",
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    image = "sha256:" + "ab" * 32
    manifest = tmp_path / "source.json"
    manifest.write_text(
        json.dumps(
            {
                "files": {
                    name: hashlib.sha256((module.ROOT / name).read_bytes()).hexdigest()
                    for name in (
                        "src/hypertrain/relay/core.py",
                        "src/hypertrain/relay/app.py",
                        "src/hypertrain/data/stream_store.py",
                    )
                }
            }
        )
    )
    digest = hashlib.sha256(manifest.read_bytes()).hexdigest()
    calls = []

    def docker(*args, **kwargs):
        calls.append(args)
        if args[:3] == ("docker", "image", "inspect"):
            return json.dumps([{"Id": image}])
        if args[:2] == ("docker", "run"):
            assert "--cpus=0.5" in args and "--memory-swap=256m" in args
            assert "--tmpfs" in args
            return "LOCAL_SOURCE_BYTES_VERIFIED\n"
        return ""

    monkeypatch.setattr(module, "command", docker)
    # The admitted installed image, not an actively edited checkout, owns these bytes.
    monkeypatch.setattr(module, "ROOT", tmp_path / "absent-live-checkout")
    assert module.verify_local_image(image, manifest, digest).startswith("hypertrain-relay-proof:")
    with pytest.raises(ValueError, match="digest mismatch"):
        module.verify_local_image(image, manifest, "00" * 32)
    node = {
        "CpusetCpus": "0-3",
        "NanoCpus": 3_500_000_000,
        "Memory": 4 << 30,
        "MemorySwap": 4 << 30,
    }
    module.validate_node_limits(node)
    for key, bad in (
        ("NanoCpus", 0),
        ("NanoCpus", 4_000_000_000),
        ("Memory", 0),
        ("MemorySwap", -1),
        ("CpusetCpus", "0-7"),
    ):
        with pytest.raises(ValueError):
            module.validate_node_limits({**node, key: bad})
    monkeypatch.undo()
    assert "OpenSSL" in module.command("openssl", "version")
    with pytest.raises(ExceptionGroup) as captured:
        module.command("openssl", "rand", "-base64", "3200000", timeout=10)
    assert "diagnostic output exceeds" in str(captured.value.exceptions)


def test_integration_preparation_uses_owner_validator(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import importlib.util
    import sys
    from types import SimpleNamespace

    calls = []

    def validate(source, digest):
        calls.append((source, digest))
        raise ValueError("owner authority rejection")

    monkeypatch.setitem(
        sys.modules, "network_authority_snapshot", SimpleNamespace(validate=validate)
    )
    spec = importlib.util.spec_from_file_location(
        "relay_authority_delegate",
        Path(__file__).parents[2] / "scripts/relay_kind_smoke.py",
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    with pytest.raises(ValueError, match="owner authority rejection"):
        module.inspect_integration_snapshot(tmp_path, "11" * 32)
    assert calls == [(tmp_path, "11" * 32)]


def test_integration_preparation_render_uses_real_roles_original_dns(
    tmp_path: Path,
) -> None:
    import importlib.util
    import sys

    spec = importlib.util.spec_from_file_location(
        "relay_integration_render",
        Path(__file__).parents[2] / "scripts/relay_kind_smoke.py",
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    with pytest.raises(ValueError, match="image digest"):
        module.integration_manifests("floating:latest")
    result = module.integration_manifests("example.test/hypertrain@sha256:" + "11" * 32)
    pods = [i for i in result["items"] if i["kind"] == "Pod"]
    job = next(i for i in result["items"] if i["kind"] == "Job")
    assert {p["metadata"]["name"] for p in pods} == {"service", "relay", "gateway"}
    assert job["spec"]["backoffLimit"] == 0 and job["spec"]["activeDeadlineSeconds"] == 9600
    specs = [p["spec"] for p in pods] + [job["spec"]["template"]["spec"]]
    assert all(not p["automountServiceAccountToken"] for p in specs)
    assert sum(int(p["containers"][0]["resources"]["limits"]["cpu"][:-1]) for p in specs) <= 3500
    for spec in specs:
        command = spec["containers"][0]["command"]
        assert "--fixture" not in command and "--integration-role" in command
    assert job["spec"]["template"]["spec"]["hostAliases"][0]["hostnames"] == [
        "master.test",
        "relay.test",
    ]


def test_integration_preparation_dispatch_forbids_launch(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    import importlib.util
    import sys

    spec = importlib.util.spec_from_file_location(
        "relay_integration_dispatch",
        Path(__file__).parents[2] / "scripts/relay_kind_smoke.py",
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    monkeypatch.setattr(
        sys,
        "argv",
        ["harness", "--mode", "integration", "--prepare-integration", "--create"],
    )
    with pytest.raises(SystemExit) as error:
        module.main()
    assert error.value.code == 2


def test_integration_preparation_deadline_recovery_preserves_baseline(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import importlib.util
    import json
    import sys

    spec = importlib.util.spec_from_file_location(
        "relay_deadline_recovery",
        Path(__file__).parents[2] / "scripts/relay_kind_smoke.py",
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    journal = tmp_path / "cleanup.json"
    journal.write_text(
        json.dumps(
            {
                "state": "CREATE_INTENT",
                "cluster": "hypertrain-v2-proof",
                "experiment": "hypertrain-v2-proof",
                "proof_dir": str(tmp_path),
                "deadline_monotonic": 0,
                "cleanup_cutoff_monotonic": float("inf"),
                "boot_id": Path("/proc/sys/kernel/random/boot_id").read_text().strip(),
                "containers": ["bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"],
                "volumes": ["user-volume"],
                "networks": ["cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"],
                "provider_baselines": {
                    "docker": {
                        "containers": [
                            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                        ],
                        "volumes": ["user-volume"],
                        "networks": [
                            "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
                        ],
                    },
                    "podman": {
                        "containers": [],
                        "volumes": [],
                        "networks": [],
                        "network_names": [],
                    },
                },
            }
        )
    )
    calls = []

    def command(*args, **kwargs):
        calls.append(args)
        if args[0] == "podman":
            return "[]" if args[1:3] == ("network", "ls") else ""
        if args[:3] == ("docker", "ps", "-aq"):
            return (
                ""
                if "--filter" in args
                else "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\n"
            )
        if args[:3] == ("docker", "volume", "ls"):
            return "user-volume\n"
        if args[:3] == ("docker", "network", "ls"):
            return "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc\n"
        return ""

    def gone(pid):
        raise ProcessLookupError

    monkeypatch.setattr(module, "command", command)
    monkeypatch.setattr(module.os, "pidfd_open", gone)
    module.deadline_supervisor(journal, 99999)
    assert json.loads(journal.read_bytes())["state"] == "CLEANUP_VERIFIED"
    assert not any(args[:3] == ("kind", "delete", "cluster") for args in calls)
    assert not any("rm" in args for args in calls)


def test_launch_safety_ready_pipe_and_absolute_deadline(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import importlib.util
    import json
    import os
    import sys

    spec = importlib.util.spec_from_file_location(
        "relay_launch_ready", Path(__file__).parents[2] / "scripts/relay_kind_smoke.py"
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    journal = tmp_path / "cleanup.json"
    journal.write_text(
        json.dumps(
            {
                "boot_id": Path("/proc/sys/kernel/random/boot_id").read_text().strip(),
                "deadline_monotonic": 42,
            }
        )
    )
    monkeypatch.setattr(module.time, "monotonic", lambda: 40)
    waits = []

    class Poll:
        def register(self, fd, event):
            assert fd == 999 and event == module.select.POLLIN

        def poll(self, timeout):
            waits.append(timeout)
            return [True]

    monkeypatch.setattr(module.os, "pidfd_open", lambda pid: 999)
    close = os.close
    monkeypatch.setattr(module.os, "close", lambda fd: None if fd == 999 else close(fd))
    monkeypatch.setattr(module.select, "poll", Poll)
    cleaned = []
    monkeypatch.setattr(module, "cleanup_owned", lambda path: cleaned.append(path))
    module.deadline_supervisor(journal, 123)
    assert waits == [2000] and cleaned == [journal]
    store = module.CleanupJournal(journal)
    with pytest.raises(ValueError, match="immutable journal field"):
        store.write_text(json.dumps({"deadline_monotonic": 43}))


def test_inventory_actual_podman_network_fixture(tmp_path: Path) -> None:
    import importlib.util
    import json
    import sys

    spec = importlib.util.spec_from_file_location(
        "relay_real_network_schema", Path(__file__).parents[2] / "scripts/relay_kind_smoke.py"
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    fixture = (Path(__file__).parent / "fixtures/podman-network-list.json").read_text()
    rows = json.loads(fixture)
    calls = []

    def execute(*args):
        calls.append(args)
        return fixture if args[1:3] == ("network", "ls") else ""

    actual = module.provider_inventory("podman", execute)
    assert actual["networks"] == sorted(row["id"] for row in rows)
    assert actual["network_names"] == sorted(row["name"] for row in rows)
    assert len(actual["networks"]) == 11 and all(
        len(identity) == 64 for identity in actual["networks"]
    )
    assert ("podman", "network", "ls", "--format", "json") in calls
    for bad in ("kind\n", "", "HTTP429", "{}", '[{"name":"kind","id":"short"}]'):
        with pytest.raises(ValueError):
            module.provider_inventory("podman", lambda *a, value=bad: value)
    docker_calls = []
    module.provider_inventory("docker", lambda *a: docker_calls.append(a) or "")
    assert ("docker", "network", "ls", "-q", "--no-trunc") in docker_calls


@pytest.mark.parametrize("failure", ["docker", "podman", "command", "cutoff", None])
def test_cleanup_absence_attempt_never_inherits_historical_success(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, failure: str | None
) -> None:
    import importlib.util
    import json
    import sys

    spec = importlib.util.spec_from_file_location(
        "relay_absence_attempt", Path(__file__).parents[2] / "scripts/relay_kind_smoke.py"
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    podman_ids = [f"{i:064x}" for i in range(1, 97)]
    baseline = {
        "docker": {"containers": [], "volumes": [], "networks": []},
        "podman": {"containers": podman_ids, "volumes": [], "networks": [], "network_names": []},
    }
    journal = tmp_path / "cleanup.json"
    journal.write_text(
        json.dumps(
            {
                "state": "CLEANUP_VERIFIED",
                "absence_verified": True,
                "provider_after": baseline,
                "provider_baselines": baseline,
                "cluster": "hypertrain-v2-stale",
                "experiment": "hypertrain-v2-stale",
                "proof_dir": str(tmp_path),
                "containers": [],
                "volumes": [],
                "networks": [],
                "cleanup_cutoff_monotonic": 1200,
                "cleanup_budget_cutoff_monotonic": 1200,
            }
        )
    )
    monkeypatch.setattr(module.time, "monotonic", lambda: 1200 if failure == "cutoff" else 0)
    counts = {"docker": 0, "podman": 0}

    def command(*args, **kwargs):
        state = json.loads(journal.read_bytes())
        assert state["absence_verified"] is False and state["provider_after"] is None
        if args[0] == "podman" and args[1:3] == ("network", "ls"):
            return "[]"
        if "--filter" in args:
            return ""
        if args[1:3] == ("ps", "-aq"):
            counts[args[0]] += 1
            if counts[args[0]] == 2:
                if failure == args[0]:
                    return "HTTP429 malformed inventory"
                if failure == "command":
                    raise RuntimeError("inventory command failed")
            return "\n".join(baseline[args[0]]["containers"])
        return ""

    monkeypatch.setattr(module, "command", command)
    if failure is None:
        module.cleanup_owned(journal)
    else:
        with pytest.raises((ValueError, RuntimeError, TimeoutError)):
            module.cleanup_owned(journal)
    outcome = json.loads(journal.read_bytes())
    assert outcome["absence_history"][0]["absence_verified"] is True
    assert outcome["cleanup_budget_cutoff_monotonic"] == 1200
    assert outcome["absence_verified"] is (failure is None)
    if failure is None:
        assert (
            outcome["provider_after"] == baseline
            and len(outcome["provider_after"]["podman"]["containers"]) == 96
        )
    else:
        assert outcome["provider_after"] is None and outcome["proof_status"] == "CENSORED"


def test_provider_fix_pinned_kind_version_and_fail_closed(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import importlib.util
    import json
    import os
    import shutil
    import subprocess
    import sys

    harness = Path(__file__).parents[2] / "scripts/relay_kind_smoke.py"
    spec = importlib.util.spec_from_file_location("relay_provider_fix", harness)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    backend = tmp_path / "backend"
    shim = tmp_path / "shim"
    backend.mkdir()
    shim.mkdir()
    trace = tmp_path / "calls.jsonl"
    journal = tmp_path / "journal.json"
    journal.write_text(json.dumps({"cluster": "hypertrain-v2-provider"}))
    docker = backend / "docker"
    docker.write_text(
        "#!/usr/bin/python3\nimport json,pathlib,sys\n"
        f"with pathlib.Path({str(trace)!r}).open('a') as f:"
        "f.write(json.dumps(sys.argv[1:])+'\\n')\n"
        "if sys.argv[1:] in (['-v'],['--version']):print('Docker version 29.1.3, build mock')\n"
    )
    docker.chmod(0o700)
    wrapper = shim / "docker"
    wrapper.write_text(
        "#!" + sys.executable + "\nimport runpy,sys\n"
        f"m=runpy.run_path({str(harness)!r});m['docker_create_hook'](m['Path']({str(journal)!r}),"
        f"{str(docker)!r},sys.argv[1:])\n"
    )
    wrapper.chmod(0o700)
    for name in ("podman", "nerdctl"):
        fake = shim / name
        fake.write_text("#!/usr/bin/python3\nraise AssertionError('provider fallback forbidden')\n")
        fake.chmod(0o700)
    kind = shutil.which("kind")
    assert kind is not None, "pinned kind executable required in PATH"
    pinned = json.loads((harness.parents[1] / "deploy/k8s/versions.json").read_bytes())
    version = subprocess.run(
        [kind, "version"], capture_output=True, text=True, check=True, timeout=10
    ).stdout.split()
    assert version[:2] == ["kind", pinned["kind_version"]]
    environment = {**os.environ, "PATH": str(shim), "PYTHONPATH": str(harness.parents[1] / "src")}
    environment.pop("KIND_EXPERIMENTAL_PROVIDER", None)
    result = subprocess.run(
        [str(kind), "get", "clusters"], env=environment, capture_output=True, text=True, timeout=30
    )
    assert result.returncode == 0, result.stderr
    calls = [json.loads(line) for line in trace.read_text().splitlines()]
    assert ["-v"] in calls and any(c and c[0] == "ps" for c in calls)
    version = subprocess.run(
        [str(wrapper), "--version"], env=environment, capture_output=True, text=True, timeout=10
    )
    assert version.stdout == "Docker version 29.1.3, build mock\n"
    docker.write_text("#!/usr/bin/python3\nimport sys\nsys.exit(1)\n")
    environment["KIND_EXPERIMENTAL_PROVIDER"] = "docker"
    failed = subprocess.run(
        [str(kind), "get", "clusters"], env=environment, capture_output=True, text=True, timeout=20
    )
    assert failed.returncode != 0 and "provider fallback forbidden" not in failed.stderr
    for bad in ("HTTP429", "not-json", "", "[]"):
        if bad == "[]":
            assert module.provider_inventory(
                "podman", lambda *a: "[]" if a[1:3] == ("network", "ls") else ""
            ) == {
                "containers": [],
                "volumes": [],
                "networks": [],
                "network_names": [],
            }
        else:
            with pytest.raises(ValueError, match="malformed"):
                module.provider_inventory("podman", lambda *a, value=bad: value)
    monkeypatch.setenv("KIND_EXPERIMENTAL_PROVIDER", "podman")
    monkeypatch.setattr(module.shutil, "which", lambda name: str(kind))
    with pytest.raises(ValueError, match="fallback forbidden"):
        module.command("kind", "get", "clusters")
    monkeypatch.setattr(
        module,
        "command",
        lambda *a, **k: json.dumps(
            [
                {
                    "Id": "a" * 64,
                    "HostConfig": {},
                    "Config": {"Labels": {}},
                }
            ]
        ),
    )
    with pytest.raises(ValueError, match="pre-start caps"):
        module.require_capped_node(journal, "missing-marker")


def test_shim_raw_bytes_exit_and_pinned_kind_empty_preflight(tmp_path: Path) -> None:
    import json
    import os
    import shutil
    import subprocess
    import sys

    harness = Path(__file__).parents[2] / "scripts/relay_kind_smoke.py"
    journal = tmp_path / "journal.json"
    journal.write_text('{"cluster":"hypertrain-v2-byteproof"}')
    backend = tmp_path / "backend"
    backend.mkdir()
    docker = backend / "docker"
    shim = tmp_path / "shim"
    shim.mkdir()
    wrapper = shim / "docker"
    wrapper.write_text(
        "#!" + sys.executable + "\nimport runpy,sys\n"
        f"m=runpy.run_path({str(harness)!r});m['docker_create_hook'](m['Path']({str(journal)!r}),"
        f"{str(docker)!r},sys.argv[1:])\n"
    )
    wrapper.chmod(0o700)
    environment = {
        **os.environ,
        "PATH": str(shim),
        "PYTHONPATH": str(harness.parents[1] / "src"),
        "KIND_EXPERIMENTAL_PROVIDER": "docker",
    }
    for args, output, errors, status in (
        (["ps"], b"", b"", 0),
        (["ps"], b"one-node\n", b"", 0),
        (["ps"], b"one-node", b"", 0),
        (["--version"], b"Docker version raw\n", b"", 0),
        (["-v"], b"Docker version raw", b"", 0),
        (["ps"], b"partial\xff", b"daemon error\xfe", 73),
    ):
        docker.write_text(
            "#!/usr/bin/python3\nimport os,sys\n"
            f"os.write(1,{output!r});os.write(2,{errors!r});sys.exit({status})\n"
        )
        docker.chmod(0o700)
        result = subprocess.run(
            [str(wrapper), *args], env=environment, capture_output=True, timeout=10
        )
        assert (result.stdout, result.stderr, result.returncode) == (output, errors, status)
    for number in (9, 15):
        docker.write_text(
            "#!/usr/bin/python3\nimport os\n"
            f"os.write(1,b'no-final-newline');os.write(2,b'child-signal');os.kill(os.getpid(),{number})\n"
        )
        result = subprocess.run(
            [str(wrapper), "ps"], env=environment, capture_output=True, timeout=10
        )
        assert (result.stdout, result.stderr, result.returncode) == (
            b"no-final-newline",
            b"child-signal",
            -number,
        )
    trace = tmp_path / "kind-calls.jsonl"
    docker.write_text(
        "#!/usr/bin/python3\nimport json,pathlib,sys\n"
        f"with pathlib.Path({str(trace)!r}).open('a') as f:"
        "f.write(json.dumps(sys.argv[1:])+'\\n')\n"
        "if sys.argv[1]=='ps':sys.exit(0)\n"
        "if sys.argv[1]=='info':"
        "print(json.dumps({'CgroupVersion':'2','CgroupDriver':'systemd'}));sys.exit(0)\n"
        "sys.stderr.write('MOCK_NEXT_STAGE_NO_NODE_CREATION');sys.exit(73)\n"
    )
    for provider in ("podman", "nerdctl"):
        fake = shim / provider
        fake.write_text("#!/usr/bin/python3\nraise AssertionError('FALLBACK_FORBIDDEN')\n")
        fake.chmod(0o700)
    kind = shutil.which("kind")
    if kind is None:
        raise FileNotFoundError("pinned kind executable required in PATH")
    result = subprocess.run(
        [
            str(kind),
            "create",
            "cluster",
            "--name",
            "hypertrain-v2-byteproof",
            "--image",
            "kindest/node:v1.35.0@sha256:452d707d4862f52530247495d180205e029056831160e22870e37e3f6c1ac31f",
        ],
        env=environment,
        capture_output=True,
        timeout=20,
    )
    calls = [json.loads(line) for line in trace.read_text().splitlines()]
    assert any(c[0] == "ps" and "--format" in c for c in calls)
    assert (
        b"node(s) already exist" not in result.stderr and b"FALLBACK_FORBIDDEN" not in result.stderr
    )
    assert b"MOCK_NEXT_STAGE_NO_NODE_CREATION" in result.stderr
    assert result.returncode != 0
    assert not any(c[0] in {"run", "create", "start"} for c in calls)


@pytest.mark.parametrize("kernel_mode", ["valid", "wrong", "missing", "foreign", "forged"])
def test_pinned_kind_contract_through_capped_start(tmp_path: Path, kernel_mode: str) -> None:
    import json
    import os
    import shutil
    import subprocess
    import sys
    import time

    harness = Path(__file__).parents[2] / "scripts/relay_kind_smoke.py"
    trace = tmp_path / "trace.jsonl"
    state = tmp_path / "state.json"
    journal = tmp_path / "journal.json"
    journal.write_text(
        json.dumps(
            {
                "cluster": "hypertrain-v2-contract",
                "containers": [],
                "deadline_monotonic": time.monotonic() + 120,
                "cleanup_cutoff_monotonic": time.monotonic() + 180,
            }
        )
    )
    backend = tmp_path / "backend"
    shim = tmp_path / "shim"
    backend.mkdir()
    shim.mkdir()
    docker = backend / "docker"
    docker.write_text(
        "#!"
        + sys.executable
        + "\n"
        + (Path(__file__).parent / "fixtures/kind_contract_docker.py").read_text()
    )
    docker.chmod(0o700)
    wrapper = shim / "docker"
    owned_scope = "/system.slice/docker-" + "a" * 64 + ".scope"
    observed_scope = owned_scope if kernel_mode != "foreign" else "/system.slice/unrelated.scope"
    if kernel_mode == "forged":
        observed_scope = "/system.slice/docker-" + "b" * 64 + ".scope"
    wrapper.write_text(
        "#!" + sys.executable + "\nimport runpy,sys\n"
        f"m=runpy.run_path({str(harness)!r})\n"
        "P=m['Path'];original=P.read_text\n"
        "def read(self,*a,**kw):\n"
        " if str(self).startswith(('/proc/12345/','/sys/fs/cgroup/system.slice/')):\n"
        f"  with P({str(trace)!r}).open('a') as f:"
        "f.write(m['json'].dumps(['kernel_read',str(self)])+'\\n')\n"
        f"  if str(self).startswith('/proc/'):return {'0::' + observed_scope + '/init.scope'!r}\n"
        f"  if {kernel_mode!r}=='missing':raise FileNotFoundError('mock missing kernel control')\n"
        "  controls={'cpu.max':'350000 100000','memory.max':str(4<<30),"
        f"'memory.swap.max':{'1' if kernel_mode == 'wrong' else '0'!r},"
        "'cpuset.cpus.effective':'0-3'}\n"
        "  if self.parent.name=='init.scope':"
        "controls.update({'cpu.max':'max 100000','memory.max':'max','memory.swap.max':'max'})\n"
        "  return controls[self.name]\n"
        " return original(self,*a,**kw)\n"
        "P.read_text=read\n"
        f"m['docker_create_hook'](P({str(journal)!r}),"
        f"{str(docker)!r},sys.argv[1:])\n"
    )
    wrapper.chmod(0o700)
    for provider in ("podman", "nerdctl"):
        fake = shim / provider
        fake.write_text("#!/usr/bin/python3\nraise AssertionError('FALLBACK_FORBIDDEN')\n")
        fake.chmod(0o700)
    environment = {
        **os.environ,
        "PATH": str(shim),
        "PYTHONPATH": str(harness.parents[1] / "src"),
        "KIND_EXPERIMENTAL_PROVIDER": "docker",
        "KIND_MOCK_TRACE": str(trace),
        "KIND_MOCK_STATE": str(state),
        "KIND_MOCK_JOURNAL": str(journal),
    }
    kind = shutil.which("kind")
    if kind is None:
        raise FileNotFoundError("pinned kind executable required in PATH")
    image = (
        "kindest/node:v1.35.0@sha256:"
        "452d707d4862f52530247495d180205e029056831160e22870e37e3f6c1ac31f"
    )
    unknown = subprocess.run(
        [str(wrapper), "pull", "untrusted/image:latest"],
        env=environment,
        capture_output=True,
        timeout=10,
    )
    assert unknown.returncode != 0 and b"exact trusted pinned" in unknown.stderr
    result = subprocess.run(
        [
            str(kind),
            "create",
            "cluster",
            "--name",
            "hypertrain-v2-contract",
            "--image",
            image,
            "--kubeconfig",
            str(tmp_path / "kubeconfig"),
            "--retain",
        ],
        env=environment,
        capture_output=True,
        timeout=90,
    )
    calls = [json.loads(line) for line in trace.read_text().splitlines()]
    assert result.returncode != 0
    assert b"unsupported local kind Docker operation" not in result.stderr
    assert b"FALLBACK_FORBIDDEN" not in result.stderr
    operations = [c[0] for c in calls]
    assert "pull" in operations and operations.index("create") < operations.index("start")
    assert operations.index("start") < operations.index("kernel_read")
    receipt = json.loads(journal.read_text())
    assert (
        receipt["node_capped_before_start"] and receipt["create_host_config"]["Memory"] == 4 << 30
    )
    if kernel_mode != "valid":
        assert operations.index("kernel_read") < operations.index("stop")
        assert not any(operation in {"exec", "logs"} for operation in operations)
        assert not receipt["node_kernel_receipt"]["verified"]
        assert receipt["node_kernel_receipt"]["stopped"]
        assert json.loads(state.read_text())["State"]["Running"] is False
        return
    assert b"MOCK_CONTROLPLANE_STOP_AFTER_CAPPED_START" in result.stderr
    assert operations.index("kernel_read") < operations.index("exec")
    assert receipt["node_kernel_receipt"]["verified"]
    assert receipt["node_kernel_receipt"]["authority_cgroup"] == "/sys/fs/cgroup" + owned_scope
    assert receipt["node_kernel_receipt"]["descendants"]
    listed = subprocess.run(
        [str(kind), "get", "clusters"], env=environment, capture_output=True, timeout=20
    )
    assert listed.returncode == 0
    deleted = subprocess.run(
        [
            str(kind),
            "delete",
            "cluster",
            "--name",
            "hypertrain-v2-contract",
            "--kubeconfig",
            str(tmp_path / "kubeconfig"),
        ],
        env=environment,
        capture_output=True,
        timeout=20,
    )
    assert deleted.returncode == 0 and not state.exists()


def test_launch_safety_create_capped_before_start(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import importlib.util
    import json
    import sys
    import time

    spec = importlib.util.spec_from_file_location(
        "relay_create_policy", Path(__file__).parents[2] / "scripts/relay_kind_smoke.py"
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    journal = tmp_path / "cleanup.json"
    journal.write_text(
        json.dumps(
            {
                "cluster": "hypertrain-v2-hook",
                "containers": [],
                "deadline_monotonic": time.monotonic() + 60,
                "cleanup_cutoff_monotonic": time.monotonic() + 120,
            }
        )
    )
    calls = []
    host = {"CpusetCpus": "0-3", "NanoCpus": 3500000000, "Memory": 4 << 30, "MemorySwap": 4 << 30}

    def command(*args, **kwargs):
        calls.append(args)
        if args[1] == "create":
            assert "--cpus=3.5" in args and "--memory-swap=4g" in args
            return "owned"
        if args[1] == "inspect":
            return json.dumps(
                [
                    {
                        "Id": "owned",
                        "HostConfig": host,
                        "State": {"Running": False},
                        "Config": {
                            "Labels": {
                                "io.hypertrain.proof": "hypertrain-v2-hook",
                                "io.x-k8s.kind.cluster": "hypertrain-v2-hook",
                            }
                        },
                    }
                ]
            )
        assert args[1] == "start" and json.loads(journal.read_bytes())["node_capped_before_start"]
        return "owned"

    monkeypatch.setattr(module, "command", command)
    monkeypatch.setattr(
        module.os, "posix_spawn", lambda executable, argv, env: command(*argv) or 99
    )
    monkeypatch.setattr(module.os, "waitpid", lambda pid, flags: (99, 0))
    monkeypatch.setattr(module.sys, "exit", lambda status: None)
    module.docker_create_hook(
        journal,
        "/usr/bin/docker",
        ["create", "--label=io.x-k8s.kind.cluster=hypertrain-v2-hook", "node"],
    )
    assert [call[1] for call in calls] == ["create", "inspect"]
    host["NanoCpus"] = 0
    with pytest.raises(ValueError):
        module.docker_create_hook(journal, "/usr/bin/docker", ["start", "owned"])


@pytest.mark.parametrize(
    "field", ["container_id", "running", "pid", "host_config", "proof_label", "stop_error"]
)
def test_poststart_rejection_diagnostics(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, field: str
) -> None:
    import copy
    import json
    import runpy

    module = runpy.run_path(str(Path(__file__).parents[2] / "scripts/relay_kind_smoke.py"))
    identity = "a" * 64
    host = {"NanoCpus": 3500000000, "Memory": 4 << 30, "MemorySwap": 4 << 30, "CpusetCpus": "0-3"}
    info = {
        "Id": identity,
        "HostConfig": host,
        "State": {"Running": False, "Pid": 0},
        "Config": {
            "Labels": {
                "io.hypertrain.proof": "hypertrain-v2-diag",
                "io.x-k8s.kind.cluster": "hypertrain-v2-diag",
            }
        },
    }
    journal = tmp_path / "journal.json"
    journal.write_text(
        json.dumps(
            {
                "cluster": "hypertrain-v2-diag",
                "owned_containers": [identity],
                "create_host_config": host,
                "deadline_monotonic": 100,
                "cleanup_cutoff_monotonic": 120,
            }
        )
    )
    chronology = []

    def start(function):
        chronology.append("start")
        return 0

    def command(*args, **kwargs):
        if args[1] == "stop":
            receipt = json.loads(journal.read_text())["node_kernel_receipt"]
            assert receipt["error"] == "started node identity/PID mismatch"
            assert receipt["rejection_reasons"] and not receipt["cgroup_read_reached"]
            chronology.append("stop_after_persist")
            if field == "stop_error":
                raise RuntimeError("mock stop error")
            return ""
        chronology.append("inspect")
        observed = copy.deepcopy(info)
        if chronology.count("inspect") == 2:
            observed["State"] = {"Running": True, "Pid": 12345}
            if field == "container_id":
                observed["Id"] = "b" * 64
            elif field == "running":
                observed["State"]["Running"] = False
            elif field == "pid":
                observed["State"]["Pid"] = True
            elif field == "host_config":
                observed["HostConfig"]["Memory"] = 123
            else:
                observed["Config"]["Labels"]["io.hypertrain.proof"] = "wrong-cluster"
        return json.dumps([observed])

    monkeypatch.setattr(module["time"], "monotonic", lambda: 10)
    monkeypatch.setattr(module["anyio"], "run", start)
    module["docker_create_hook"].__globals__["command"] = command
    with pytest.raises(ValueError, match="started node identity/PID mismatch"):
        module["docker_create_hook"](journal, "/mock/docker", ["start", identity])
    receipt = json.loads(journal.read_text())["node_kernel_receipt"]
    reason = "proof_label" if field == "stop_error" else field
    assert receipt["rejection_reasons"] == [reason]
    assert receipt["observed"] and receipt["expected"]
    assert receipt["expected"]["container_id"] == identity
    assert not receipt["cgroup_read_reached"] and not receipt["verified"]
    assert chronology == ["inspect", "start", "inspect", "stop_after_persist"]
    if field == "host_config":
        assert receipt["host_config_changed_fields"] == ["Memory"]
        assert receipt["observed"]["resources"]["Memory"] == 123
        assert receipt["expected"]["resources"]["Memory"] == 4 << 30
    if field == "stop_error":
        assert receipt["stop_error"] == "mock stop error"
        assert "stopped" not in receipt
    else:
        assert receipt["stopped"]


@pytest.mark.parametrize("depletion", ["prestart", "pidread", "cleanup"])
def test_kernel_gate_absolute_budget(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, depletion: str
) -> None:
    import json
    import runpy

    module = runpy.run_path(str(Path(__file__).parents[2] / "scripts/relay_kind_smoke.py"))
    identity = "a" * 64
    now = [10.0]
    journal = tmp_path / "journal.json"
    host = {"NanoCpus": 3500000000, "Memory": 4 << 30, "MemorySwap": 4 << 30, "CpusetCpus": "0-3"}
    journal.write_text(
        json.dumps(
            {
                "cluster": "hypertrain-v2-budget",
                "owned_containers": [identity],
                "create_host_config": host,
                "deadline_monotonic": 12.0,
                "cleanup_cutoff_monotonic": 100.0,
                "cleanup_budget_cutoff_monotonic": 14.0,
            }
        )
    )
    calls = []
    reads = []

    def command(*args, **kwargs):
        calls.append((args[1], kwargs["timeout"]))
        if args[1] == "stop":
            assert args[-1] == identity and kwargs["timeout"] == 1.0
            return ""
        if depletion == "prestart":
            now[0] = 13.0
        return json.dumps(
            [
                {
                    "Id": identity,
                    "HostConfig": host,
                    "Config": {"Labels": {"io.hypertrain.proof": "hypertrain-v2-budget"}},
                    "State": {"Pid": 12345, "Running": True},
                }
            ]
        )

    def start(function):
        calls.append(("start", None))
        if depletion == "cleanup":
            now[0] = 14.0
        return 0

    original = Path.read_text

    def read(path, *args, **kwargs):
        if str(path) == "/proc/12345/cgroup":
            reads.append(str(path))
            now[0] = 13.0
            return "0::/system.slice/docker-" + identity + ".scope/init.scope"
        return original(path, *args, **kwargs)

    monkeypatch.setattr(module["time"], "monotonic", lambda: now[0])
    monkeypatch.setattr(module["anyio"], "run", start)
    monkeypatch.setattr(Path, "read_text", read)
    module["docker_create_hook"].__globals__["command"] = command
    with pytest.raises(TimeoutError, match="absolute budget"):
        module["docker_create_hook"](journal, "/mock/docker", ["start", identity])
    assert calls[0] == ("inspect", 2.0)
    if depletion == "cleanup":
        assert [c[0] for c in calls] == ["inspect", "start"]
        assert not reads
    else:
        assert calls[-1] == ("stop", 1.0)
        receipt = json.loads(journal.read_text())["node_kernel_receipt"]
        assert receipt["stopped"] and not receipt["verified"]
        assert len(reads) == (1 if depletion == "pidread" else 0)


@pytest.mark.parametrize(
    "changed",
    [
        None,
        "Memory",
        "MemorySwap",
        "NanoCpus",
        "CpuQuota",
        "CpuPeriod",
        "CpusetCpus",
        "Privileged",
        "SecurityOpt",
        "CgroupnsMode",
        "Init",
        "Binds",
        "Tmpfs",
        "OomKillDisable",
    ],
)
def test_real_hostconfig_normalization_gate(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, changed: str | None
) -> None:
    import copy
    import json
    import runpy

    module = runpy.run_path(str(Path(__file__).parents[2] / "scripts/relay_kind_smoke.py"))
    fixture = json.loads(
        (Path(__file__).parent / "fixtures/real-kind-hostconfig-normalization.json").read_bytes()
    )
    before, after = fixture["before"], fixture["after"]
    assert [name for name in before if before[name] != after[name]] == ["OomKillDisable"]
    assert before["OomKillDisable"] is False and after["OomKillDisable"] is None
    matcher = module["admitted_host_config_matches"]
    assert matcher(after, before)
    protected = {**before, "OomKillDisable": True}
    assert matcher(protected, protected)
    assert not matcher(after, protected)
    assert not matcher({**before, "OomKillDisable": 0}, before)
    observed = copy.deepcopy(after)
    mutations = {
        "Memory": 0,
        "MemorySwap": -1,
        "NanoCpus": 0,
        "CpuQuota": 350000,
        "CpuPeriod": 100000,
        "CpusetCpus": "0-4",
        "Privileged": False,
        "SecurityOpt": [],
        "CgroupnsMode": "host",
        "Init": True,
        "Binds": [],
        "Tmpfs": {},
        "OomKillDisable": True,
    }
    if changed:
        observed[changed] = mutations[changed]
        assert not matcher(observed, before)
    identity = "a" * 64
    journal = tmp_path / "journal.json"
    journal.write_text(
        json.dumps(
            {
                "cluster": "hypertrain-v2-hostconfig",
                "experiment": "hypertrain-v2-hostconfig",
                "node_capped_before_start": True,
                "owned_containers": [identity],
                "create_host_config": before,
                "deadline_monotonic": 100,
                "cleanup_cutoff_monotonic": 120,
            }
        )
    )
    chronology = []

    def command(*args, **kwargs):
        if args[1] == "stop":
            receipt = json.loads(journal.read_bytes())["node_kernel_receipt"]
            assert receipt["rejection_reasons"] == ["host_config"]
            assert not receipt["cgroup_read_reached"]
            chronology.append("stop")
            return ""
        chronology.append("inspect")
        running = "start" in chronology
        return json.dumps(
            [
                {
                    "Id": identity,
                    "HostConfig": observed if running else before,
                    "State": {"Running": running, "Pid": 12345 if running else 0},
                    "Config": {"Labels": {"io.hypertrain.proof": "hypertrain-v2-hostconfig"}},
                }
            ]
        )

    def start(function):
        chronology.append("start")
        return 0

    original = Path.read_text

    def read(path, *args, **kwargs):
        if str(path) == "/proc/12345/cgroup":
            chronology.append("cgroup_read")
            return "0::/system.slice/docker-" + identity + ".scope"
        if str(path).startswith("/sys/fs/cgroup/system.slice/docker-"):
            return {
                "cpu.max": "350000 100000",
                "memory.max": str(4 << 30),
                "memory.swap.max": "0",
                "cpuset.cpus.effective": "0-3",
            }[path.name]
        return original(path, *args, **kwargs)

    monkeypatch.setattr(module["time"], "monotonic", lambda: 10)
    monkeypatch.setattr(module["anyio"], "run", start)
    monkeypatch.setattr(Path, "read_text", read)
    module["docker_create_hook"].__globals__["command"] = command
    if changed:
        with pytest.raises(ValueError, match="started node identity/PID mismatch"):
            module["docker_create_hook"](journal, "/mock/docker", ["start", identity])
        assert "cgroup_read" not in chronology and chronology[-1] == "stop"
    else:
        module["docker_create_hook"](journal, "/mock/docker", ["start", identity])
        receipt = json.loads(journal.read_bytes())["node_kernel_receipt"]
        assert receipt["verified"] and receipt["cgroup_read_reached"]
        assert receipt["host_config_changed_fields"] == ["OomKillDisable"]
        assert "stop" not in chronology
        module["require_capped_node"](journal, identity)


def test_image_archive_stdin_through_capture_and_native_shim(tmp_path: Path) -> None:
    import hashlib
    import io
    import json
    import runpy
    import sys
    import tarfile

    harness = Path(__file__).parents[2] / "scripts/relay_kind_smoke.py"
    module = runpy.run_path(str(harness))
    journal = tmp_path / "journal.json"
    journal.write_text('{"cluster":"hypertrain-v2-stream"}')
    archive = tmp_path / "image.tar"
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w") as tar:
        for name, content in (
            (
                "manifest.json",
                b'[{"Config":"config.json","RepoTags":["tiny:test"],"Layers":["layer.tar"]}]',
            ),
            ("config.json", b'{"architecture":"amd64","os":"linux"}'),
            ("layer.tar", b"\x00\xffarchive-bytes\x00"),
        ):
            header = tarfile.TarInfo(name)
            header.size = len(content)
            tar.addfile(header, io.BytesIO(content))
    payload = buffer.getvalue()
    archive.write_bytes(payload)
    docker = tmp_path / "docker"
    docker.write_text(
        "#!" + sys.executable + "\nimport hashlib,os,sys\n"
        "data=sys.stdin.buffer.read()\n"
        f"assert len(data)=={len(payload)}\n"
        f"assert hashlib.sha256(data).hexdigest()=={hashlib.sha256(payload).hexdigest()!r}\n"
        "os.write(1,b'ARCHIVE_BYTES_EXACT_NO_LF');os.write(2,b'ctr mock stderr\\xff')\n"
        "sys.exit(73 if sys.argv[-1]=='fail' else 0)\n"
    )
    docker.chmod(0o700)
    shim = tmp_path / "shim"
    shim.write_text(
        "#!" + sys.executable + "\nimport runpy,sys\n"
        f"m=runpy.run_path({str(harness)!r});m['docker_create_hook'](m['Path']({str(journal)!r}),"
        f"{str(docker)!r},sys.argv[1:])\n"
    )
    shim.chmod(0o700)
    kind = tmp_path / "kind"
    kind.write_text(
        "#!" + sys.executable + "\nimport subprocess,sys\n"
        "assert sys.stdin.buffer.read()==b''\n"
        f"with open({str(archive)!r},'rb') as source:\n"
        f" p=subprocess.run([{str(shim)!r},'exec','--privileged','-i','owned-node','ctr',"
        "'--namespace=k8s.io','images','import','--all-platforms','--digests',"
        "'--snapshotter=overlayfs','-',sys.argv[-1]],stdin=source)\n"
        "sys.exit(p.returncode)\n"
    )
    kind.chmod(0o700)
    assert module["command"](str(kind), "load", "ok") == "ARCHIVE_BYTES_EXACT_NO_LF"
    with pytest.raises(RuntimeError, match="exit73: ctr mock stderr"):
        module["command"](str(kind), "load", "fail")
    assert json.loads(journal.read_bytes()) == {"cluster": "hypertrain-v2-stream"}


def test_launch_safety_independent_systemd_units(tmp_path: Path) -> None:
    import json
    import os
    import runpy
    import secrets
    import select
    import socket
    import subprocess
    import sys
    import time

    token = "hypertrain-v2-tiny-" + secrets.token_hex(6)
    runner, supervisor = token + "-runner", token + "-supervisor"
    proof_path = tmp_path / ("long-proof-directory-" + "a" * 80)
    proof_path.mkdir()
    harness = Path(__file__).parents[2] / "scripts/relay_kind_smoke.py"
    helpers = runpy.run_path(str(harness))
    provider_baselines = {
        provider: helpers["provider_inventory"](provider) for provider in ("docker", "podman")
    }
    for provider in provider_baselines:
        assert not helpers["command"](
            provider, "ps", "-aq", "--filter", "label=io.x-k8s.kind.cluster=" + token
        ), "pre-existing experiment label conflicts with ownership"
    listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    path = tmp_path / "probe.sock"
    listener.bind(str(path))
    listener.listen(1)
    listener.settimeout(30)
    journal = proof_path / "cleanup.json"
    journal.write_text(
        json.dumps(
            {
                "state": "ADMITTED_NOT_CREATED",
                "cluster": token,
                "experiment": token,
                "provider_baselines": provider_baselines,
                "containers": provider_baselines["docker"]["containers"],
                "volumes": provider_baselines["docker"]["volumes"],
                "networks": provider_baselines["docker"]["networks"],
                "owned_containers": [],
                "owned_volumes": [],
                "owned_networks": [],
                "runner_unit": runner + ".service",
                "supervisor_unit": supervisor + ".service",
                "proof_dir": str(proof_path.resolve()),
                "deadline_monotonic": time.monotonic() + 40,
                "cleanup_cutoff_monotonic": time.monotonic() + 60,
                "boot_id": Path("/proc/sys/kernel/random/boot_id").read_text().strip(),
            }
        )
    )
    runner_code = (
        "import runpy,socket,signal; m=runpy.run_path(" + repr(str(harness)) + ");"
        "m['start_supervisor'](m['Path'](" + repr(str(journal)) + "));"
        "s=socket.socket(socket.AF_UNIX);s.connect(" + repr(str(path)) + ");"
        "s.sendall(b'READY');s.close();signal.pause()"
    )
    try:
        subprocess.run(
            [
                "systemd-run",
                "--unit=" + runner,
                "--property=CPUQuota=45%",
                "--property=MemoryMax=384M",
                "--property=MemorySwapMax=0",
                "--property=AllowedCPUs=0-3",
                "--property=Nice=19",
                "--property=RuntimeMaxSec=60",
                "--setenv=PYTHONPATH=" + str(harness.parents[1] / "src"),
                sys.executable,
                "-c",
                runner_code,
            ],
            check=True,
            capture_output=True,
        )
        with listener.accept()[0] as connection:
            connection.settimeout(30)
            assert connection.recv(64) == b"READY"
        observed = json.loads(journal.read_bytes())["supervisor_ready"]
        assert supervisor in observed["cgroup"] and runner not in observed["cgroup"]
        assert observed["controls"]["cpu.max"] == "5000 100000"
        assert int(observed["controls"]["memory.max"]) == 128 << 20
        assert observed["controls"]["memory.swap.max"] == "0"
        pid = int(
            subprocess.check_output(
                [
                    "systemctl",
                    "show",
                    supervisor + ".service",
                    "--property=MainPID",
                    "--value",
                ],
                text=True,
            )
        )
        fd = os.pidfd_open(pid)
        try:
            poll = select.poll()
            poll.register(fd, select.POLLIN)
            subprocess.run(
                ["systemctl", "kill", "--signal=SIGKILL", runner + ".service"],
                check=True,
                capture_output=True,
            )
            assert poll.poll(30000), "actual supervisor must finish after exact parent death"
        finally:
            os.close(fd)
        log = (proof_path / "supervisor.log").read_text()
        assert "SUPERVISOR_CLEANUP_DONE" in log and "Traceback" not in log
        settled = json.loads(journal.read_bytes())
        assert settled["state"] == "CLEANUP_VERIFIED" and settled["absence_verified"] is True
        assert settled["provider_baselines"] == provider_baselines
        assert settled["provider_after"] == provider_baselines
        assert all(
            settled[key] == [] for key in ("owned_containers", "owned_volumes", "owned_networks")
        )
    finally:
        listener.close()
        for unit in (runner, supervisor):
            subprocess.run(["systemctl", "stop", unit + ".service"], capture_output=True)
            subprocess.run(["systemctl", "reset-failed", unit + ".service"], capture_output=True)


@pytest.mark.parametrize("rescue_failure", [False, True])
def test_launch_safety_rescue_before_delete_exact_owned(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, rescue_failure: bool
) -> None:
    import importlib.util
    import json
    import sys

    spec = importlib.util.spec_from_file_location(
        "relay_launch_rescue", Path(__file__).parents[2] / "scripts/relay_kind_smoke.py"
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    cluster = "hypertrain-v2-abc123"
    journal = tmp_path / "cleanup.json"
    journal.write_text(
        json.dumps(
            {
                "state": "CREATE_INTENT",
                "cluster": cluster,
                "experiment": cluster,
                "proof_dir": str(tmp_path),
                "kubeconfig": str(tmp_path / "kubeconfig"),
                "cleanup_cutoff_monotonic": float("inf"),
                "containers": ["bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"],
                "volumes": ["user-volume"],
                "networks": ["cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"],
                "provider_baselines": {
                    "docker": {
                        "containers": [
                            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                        ],
                        "volumes": ["user-volume"],
                        "networks": [
                            "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
                        ],
                    },
                    "podman": {
                        "containers": [],
                        "volumes": [],
                        "networks": [],
                        "network_names": [],
                    },
                },
            }
        )
    )
    calls = []
    deleted = False
    clock = [0]
    timeouts = []
    monkeypatch.setattr(module.time, "monotonic", lambda: clock[0])
    state = json.loads(journal.read_bytes())
    journal.write_text(json.dumps({**state, "cleanup_cutoff_monotonic": 1200}))

    def command(*args, **kwargs):
        nonlocal deleted
        calls.append(args)
        if args[0] == "podman":
            return "[]" if args[1:3] == ("network", "ls") else ""
        timeouts.append(kwargs["timeout"])
        assert kwargs["timeout"] <= 1200 - clock[0]
        clock[0] += 50
        if args[:3] == ("docker", "ps", "-aq"):
            if "--filter" in args:
                return (
                    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n"
                    if not deleted
                    else ""
                )
            return (
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\n"
                if deleted
                else "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\n"
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n"
            )
        if args[:2] == ("docker", "inspect"):
            return json.dumps(
                [
                    {
                        "Id": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                        "Config": {"Labels": {"io.hypertrain.proof": cluster}},
                        "Mounts": [],
                        "NetworkSettings": {"Networks": {}},
                    }
                ]
            )
        if args[:3] == ("docker", "volume", "ls"):
            return "user-volume\n"
        if args[:3] == ("docker", "network", "ls"):
            return "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc\n"
        if args[:3] == ("kind", "delete", "cluster"):
            assert any(c[:2] == ("docker", "cp") for c in calls)
            assert any(c[:3] == ("kind", "export", "logs") for c in calls)
            deleted = True
        if args[:2] == ("docker", "cp") and rescue_failure:
            raise ValueError("node copy failed")
        return "{}"

    monkeypatch.setattr(module, "command", command)
    module.cleanup_owned(journal)
    result = json.loads(journal.read_bytes())
    assert result["absence_verified"] and result["state"] == "CLEANUP_VERIFIED"
    assert (result["proof_status"] == "CENSORED") == rescue_failure
    assert not any(args[:2] == ("docker", "rm") for args in calls)
    assert len(timeouts) > 8 and clock[0] > 400
    clock[0] = 1200
    journal.write_text(json.dumps({**result, "state": "CREATE_INTENT"}))
    with pytest.raises(TimeoutError, match="aggregate cleanup cutoff"):
        module.cleanup_owned(journal)
    assert json.loads(journal.read_bytes())["proof_status"] == "CENSORED"
    clock[0] = 0
    journal.write_text(json.dumps({**result, "state": "CREATE_INTENT"}))
    deleted = False
    monkeypatch.setattr(
        module,
        "command",
        lambda *a, **k: (
            json.dumps(
                [
                    {
                        "Id": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                        "Config": {"Labels": {}},
                    }
                ]
            )
            if a[:2] == ("docker", "inspect")
            else (
                ("[]" if a[1:3] == ("network", "ls") else "")
                if a[0] == "podman"
                else "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\n"
            )
        ),
    )
    with pytest.raises(ValueError, match="ownership conflicts"):
        module.cleanup_owned(journal)


def test_smoke_storage_when_prefix_delete_preserves_other_region(
    tmp_path: Path,
) -> None:
    async def scenario() -> None:
        import importlib.util
        import sys

        from hypertrain.data.store import ObjectNotFound, S3Credentials, S3Store

        spec = importlib.util.spec_from_file_location(
            "relay_harness_gc",
            Path(__file__).parents[2] / "scripts/relay_kind_smoke.py",
        )
        assert spec is not None and spec.loader is not None
        module = importlib.util.module_from_spec(spec)
        sys.modules[spec.name] = module
        spec.loader.exec_module(module)
        secret = tmp_path / "secret"
        secret.mkdir()
        (secret / "object.json").write_text(
            '{"access_key_id":"local","secret_access_key":"secret"}'
        )
        module.FIXTURE, module.OBJECT_ROOT = secret, tmp_path / "objects"
        async with httpx.AsyncClient(
            transport=httpx.ASGITransport(app=module.fixture_store()),
            base_url="http://objects.test",
        ) as wire:
            credentials = S3Credentials("local", "secret")
            stores = [
                StreamStore(
                    S3Store(
                        "http://objects.test",
                        "proof",
                        credentials,
                        region="local",
                        prefix=f"relay/{region}/",
                    ),
                    wire,
                )
                for region in ("eu", "us")
            ]
            sha = hashlib.sha256(b"abc").hexdigest()
            for store in stores:
                with spool() as file:
                    file.write(b"abc")
                    await store.put(file, sha, 3)
            await stores[0].delete(sha)
            with spool() as file:
                with pytest.raises(ObjectNotFound):
                    await stores[0].get(sha, 3, file)
            with spool() as file:
                await stores[1].get(sha, 3, file)
                assert file.read() == b"abc"
            name = "56" * 32
            await stores[0].compare_swap(name, b"first", None)
            data, etag = await stores[0].journal(name)
            assert data == b"first"
            await stores[0].compare_swap(name, b"second", etag)
            newer, new_etag = await stores[0].journal(name)
            assert newer == b"second" and new_etag != etag

    anyio.run(scenario)


def test_smoke_gc_when_signed_extension_blocks_delete_and_region_prefix_isolated(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    async def scenario() -> None:
        import importlib.util
        import sys

        spec = importlib.util.spec_from_file_location(
            "relay_harness_retention",
            Path(__file__).parents[2] / "scripts/relay_kind_smoke.py",
        )
        assert spec is not None and spec.loader is not None
        module = importlib.util.module_from_spec(spec)
        sys.modules[spec.name] = module
        spec.loader.exec_module(module)
        async with httpx.AsyncClient() as client:
            rig = Rig(tmp_path / "eu", client)
            other = StreamStore(LocalFSStore(tmp_path / "us"), client)
            module.RUN = RUN
            module.registry = lambda: rig.registry
            module.key = lambda name: MASTER if name == "master" else RELAY
            module.backing = lambda client, region: rig.store if region == "eu" else other
            data = b"a" * (4 << 20) + b"tail"
            grant, manifest, _ = await rig.accept(data)
            await rig.put(grant, data[: 4 << 20])
            await rig.put(grant, b"tail", 1)
            with spool() as file:
                file.write(data[: 4 << 20])
                await other.put(file, manifest.chunks[0].chunk_sha256, 4 << 20)
            original_client = httpx.AsyncClient
            from fastapi import FastAPI

            gateway = FastAPI()
            gateway.mount("/eu", create_app(rig.relay, "t" * 32))

            def http_client(*args, **kwargs):
                if "verify" in kwargs:
                    return original_client(
                        transport=httpx.ASGITransport(app=gateway),
                        base_url="https://gateway.test",
                    )
                return original_client()

            monkeypatch.setattr(module.httpx, "AsyncClient", http_client)
            await module.fixture_gc()
            assert not rig.store.store._path(grant.delta_hash).exists()
            assert other.store.get(manifest.chunks[0].chunk_sha256) == data[: 4 << 20]

    anyio.run(scenario)


@pytest.fixture
def archive_intake(tmp_path):
    import io
    import json
    import runpy
    import tarfile
    from types import SimpleNamespace

    module = runpy.run_path(str(Path(__file__).parents[2] / "scripts/relay_kind_smoke.py"))
    reference = "d3-isolated-recovery-20261009-r5:recovered"
    files = {f"src/hypertrain/file{i}.py": "a" * 64 for i in range(109)}
    source = tmp_path / "source.json"
    source.write_text(json.dumps({"files": files}))
    config = json.dumps(
        {"os": "linux", "architecture": "amd64", "config": {"User": "65532:65532"}}
    ).encode()
    image_id = "sha256:" + hashlib.sha256(config).hexdigest()
    layer = b"tiny layer, no actual image import"
    layer_id = "sha256:" + hashlib.sha256(layer).hexdigest()
    oci = json.dumps(
        {
            "config": {"digest": image_id, "size": len(config)},
            "layers": [{"digest": layer_id, "size": len(layer)}],
        }
    ).encode()
    manifest_id = "sha256:" + hashlib.sha256(oci).hexdigest()
    documents = {
        "manifest.json": json.dumps(
            [
                {
                    "Config": "blobs/sha256/" + image_id[7:],
                    "RepoTags": [reference],
                    "Layers": ["blobs/sha256/" + layer_id[7:]],
                }
            ]
        ).encode(),
        "index.json": json.dumps(
            {
                "manifests": [
                    {
                        "digest": manifest_id,
                        "size": len(oci),
                        "annotations": {
                            "io.containerd.image.name": "docker.io/library/" + reference
                        },
                    }
                ]
            }
        ).encode(),
        "blobs/sha256/" + image_id[7:]: config,
        "blobs/sha256/" + layer_id[7:]: layer,
        "blobs/sha256/" + manifest_id[7:]: oci,
    }
    archive = tmp_path / "image.tar"
    with tarfile.open(archive, "w") as tar:
        for name, data in documents.items():
            member = tarfile.TarInfo(name)
            member.size = len(data)
            tar.addfile(member, io.BytesIO(data))
    archive.chmod(0o444)
    args = SimpleNamespace(
        mode="transport",
        local_image_id=None,
        image=None,
        image_archive=archive,
        archive_sha256=hashlib.sha256(archive.read_bytes()).hexdigest(),
        archive_image_id=image_id,
        archive_reference=reference,
        source_manifest=source,
        source_manifest_sha256=hashlib.sha256(source.read_bytes()).hexdigest(),
    )
    return module, args, files


@pytest.mark.parametrize(
    "fault",
    [
        None,
        "integration",
        "image-id",
        "remote",
        "hash",
        "reference",
        "config",
        "source",
        "writable",
        "symlink",
        "parent-link",
        "hardlink",
    ],
)
def test_transport_archive_intake_custody(archive_intake, tmp_path, fault):
    import os

    module, args, files = archive_intake
    if fault == "integration":
        args.mode = "integration"
    elif fault == "image-id":
        args.local_image_id = args.archive_image_id
    elif fault == "remote":
        args.image = "image@sha256:" + "b" * 64
    elif fault in ("hash", "config", "source"):
        setattr(
            args,
            {
                "hash": "archive_sha256",
                "config": "archive_image_id",
                "source": "source_manifest_sha256",
            }[fault],
            ("sha256:" if fault == "config" else "") + "b" * 64,
        )
    elif fault == "reference":
        args.archive_reference = "other:tag"
    elif fault == "writable":
        args.image_archive.chmod(0o644)
    elif fault == "hardlink":
        os.link(args.image_archive, tmp_path / "alias.tar")
    elif fault == "symlink":
        link = tmp_path / "link.tar"
        link.symlink_to(args.image_archive)
        args.image_archive = link
    elif fault == "parent-link":
        link = tmp_path / "parent"
        link.symlink_to(tmp_path, target_is_directory=True)
        args.image_archive = link / "image.tar"
    if fault:
        with pytest.raises((ValueError, OSError)):
            with module["transport_archive"](args):
                pytest.fail("invalid archive custody admitted")
    else:
        with module["transport_archive"](args) as (path, reference, actual):
            assert path.startswith(f"/proc/{os.getpid()}/fd/")
            assert reference == args.archive_reference and actual == files
            # Rename/substitute original path cannot replace the held descriptor.
            args.image_archive.rename(tmp_path / "original.tar")
            args.image_archive.write_bytes(b"substituted path")
            assert hashlib.sha256(Path(path).read_bytes()).hexdigest() == args.archive_sha256


@pytest.mark.parametrize("fault", [None, "identity", "reference", "source-job"])
def test_transport_archive_owned_node_verification(archive_intake, monkeypatch, fault):
    import json

    module, args, files = archive_intake
    calls, jobs = [], []

    def command(*argv, **kwargs):
        calls.append(argv)
        if argv[:3] == ("docker", "exec", "owned-control-plane"):
            return json.dumps(
                {
                    "status": {
                        "id": args.archive_image_id
                        if fault != "identity"
                        else "sha256:" + "b" * 64,
                        "repoTags": ["docker.io/library/" + args.archive_reference]
                        if fault != "reference"
                        else [],
                    }
                }
            )
        if "logs" in argv:
            return (
                "WRONG"
                if fault == "source-job"
                else "ARCHIVE_NODE_SOURCE109_RELAY_IMPORT_VERIFIED\n"
            )
        return ""

    monkeypatch.setitem(module["verify_archive_node"].__globals__, "command", command)
    monkeypatch.setitem(
        module["verify_archive_node"].__globals__, "apply", lambda argv, job: jobs.append(job)
    )
    if fault:
        with pytest.raises(ValueError):
            module["verify_archive_node"](
                "owned-control-plane",
                args.archive_reference,
                args.archive_image_id,
                files,
                ["kubectl"],
            )
    else:
        module["verify_archive_node"](
            "owned-control-plane", args.archive_reference, args.archive_image_id, files, ["kubectl"]
        )
        container = jobs[0]["spec"]["template"]["spec"]["containers"][0]
        assert (
            container["image"] == args.archive_reference and container["imagePullPolicy"] == "Never"
        )
        assert jobs[0]["spec"]["activeDeadlineSeconds"] == 60
        assert (
            "actual==expected" in container["command"][-1]
            and "import hypertrain.relay.app" in container["command"][-1]
        )
        assert "hypertrain.__file__" in container["command"][-1]
        assert "root.rglob('*')" in container["command"][-1]
        pod = jobs[0]["spec"]["template"]["spec"]
        assert pod["securityContext"] == {
            "runAsUser": 65532,
            "runAsGroup": 65532,
            "runAsNonRoot": True,
            "fsGroup": 65532,
        }
        assert pod["volumes"] == [
            {"name": "data", "emptyDir": {"medium": "Memory", "sizeLimit": "16Mi"}}
        ]
        assert container["volumeMounts"] == [{"name": "data", "mountPath": "/data"}]
    assert all(
        argv[:2] not in (("docker", "run"), ("docker", "inspect"), ("docker", "tag"))
        for argv in calls
    )


def test_transport_archive_lease_rejects_write(archive_intake):
    import os

    module, args, _ = archive_intake
    with pytest.raises((ValueError, BlockingIOError)):
        with module["transport_archive"](args):
            # Nonblocking write triggers the original read-lease break signal, no sleep.
            fd = os.open(args.image_archive, os.O_WRONLY | os.O_NONBLOCK)
            os.close(fd)
            pytest.fail("immutable archive accepted concurrent write")


def test_transport_archive_native_route_preserves_image_id(archive_intake, monkeypatch):
    module, args, files = archive_intake
    calls = []
    monkeypatch.setitem(
        module["load_transport_image"].__globals__,
        "command",
        lambda *argv, **kwargs: calls.append((argv, kwargs)),
    )
    archive = ("/proc/123/fd/9", args.archive_reference, files)
    module["load_transport_image"]("owned-cluster", args.archive_reference, archive)
    module["load_transport_image"]("owned-cluster", "hypertrain-relay-proof:" + "b" * 64)
    assert calls == [
        (
            ("kind", "load", "image-archive", archive[0], "--name", "owned-cluster"),
            {"timeout": 240},
        ),
        (
            (
                "kind",
                "load",
                "docker-image",
                "hypertrain-relay-proof:" + "b" * 64,
                "--name",
                "owned-cluster",
            ),
            {"timeout": 240},
        ),
    ]


@pytest.mark.parametrize("failure", ["fixture", "denied", "cleanup", None])
def test_transport_archive_smoke_failure_and_every_workload(
    archive_intake, monkeypatch, tmp_path, failure
):
    import json
    import re
    from contextlib import nullcontext
    from types import SimpleNamespace

    module, args, files = archive_intake
    globals_ = module["_smoke"].__globals__
    root = Path(__file__).parents[2]
    versions = json.loads((root / "deploy/k8s/versions.json").read_text())
    cni = b"bounded mock CNI"
    versions["calico_manifest_sha256"] = hashlib.sha256(cni).hexdigest()
    monkeypatch.setattr(
        globals_["Versions"], "model_validate_json", lambda _: SimpleNamespace(**versions)
    )
    args.__dict__.update(
        render=False,
        create=True,
        cluster="hypertrain-v2-tiny",
        deadline_seconds=60,
        cpus="0-3",
        cpu_quota=4,
        cleanup=True,
        proof_dir=tmp_path / "proof",
        integration_root=None,
        work_end=module["time"].monotonic() + 60,
        cleanup_end=module["time"].monotonic() + 1260,
        clock_boot_id=Path("/proc/sys/kernel/random/boot_id").read_text().strip(),
    )
    cleanup = []
    workloads = []
    created = False
    miner = None

    def check_resource(resource):
        if isinstance(resource, dict):
            if "image" in resource:
                assert resource["image"] == args.archive_reference
                assert resource["imagePullPolicy"] == "Never"
                workloads.append(resource["name"])
            for value in resource.values():
                check_resource(value)
        elif isinstance(resource, list):
            for value in resource:
                check_resource(value)

    def command(*argv, **kwargs):
        nonlocal created, miner
        if argv[:2] == ("kind", "version"):
            return versions["kind_version"]
        if argv[:2] == ("kind", "create"):
            created = True
        if argv[:4] == ("docker", "ps", "-aq", "--no-trunc"):
            return "owned-node" if created else ""
        if argv[:2] == ("docker", "inspect"):
            return json.dumps(
                [
                    {
                        "Id": "owned-node",
                        "HostConfig": {
                            "CpusetCpus": "0-3",
                            "NanoCpus": 3500000000,
                            "Memory": 1 << 30,
                            "MemorySwap": 1 << 30,
                        },
                        "Mounts": [],
                        "NetworkSettings": {"Networks": {}},
                    }
                ]
            )
        if argv[0] == "openssl":
            Path(argv[argv.index("-out") + 1]).write_bytes(b"tiny mock certificate")
            Path(argv[argv.index("-keyout") + 1]).write_bytes(b"tiny mock private key")
        if argv[:2] == ("kubectl", "kustomize"):
            return (
                (root / "deploy/k8s/base/deployment.yaml")
                .read_text()
                .replace(
                    "image: ghcr.io/cortexlm/hypertrain\n",
                    "image: " + versions["server_image"] + "\n",
                )
            )
        if "apply" in argv:
            text = kwargs["input_text"]
            if text.startswith("{"):
                check_resource(json.loads(text))
            elif "image: " + args.archive_reference in text:
                lines = text.splitlines()
                for i, line in enumerate(lines):
                    if re.match(r"\s*image:", line):
                        assert line.strip() == "image: " + args.archive_reference
                        assert lines[i + 1].strip() == "imagePullPolicy: Never"
                workloads.extend(
                    re.findall(r"(?m)^\s*- name: (objects|master|gateway|miner|relay)$", text)
                )
                if "kind: Job" in text:
                    miner = {
                        "spec": {
                            "suspend": True,
                            "template": {
                                "metadata": {},
                                "spec": {
                                    "containers": [
                                        {
                                            "name": "miner",
                                            "image": args.archive_reference,
                                            "imagePullPolicy": "Never",
                                        }
                                    ],
                                    "volumes": [
                                        {
                                            "name": "secrets",
                                            "secret": {"secretName": "proof-secrets"},
                                        }
                                    ],
                                },
                            },
                        }
                    }
                    if failure == "fixture":
                        raise RuntimeError("late fixture failure")
        if "get" in argv and "job/mock-miner" in argv:
            return json.dumps(miner)
        if "get" in argv and "pods" in argv:
            return json.dumps(
                {"items": [{"metadata": {"name": "relay-1"}}, {"metadata": {"name": "relay-2"}}]}
            )
        if "run" in argv and "unauthorized" in argv:
            override = next(a for a in argv if a.startswith("--overrides="))
            check_resource(json.loads(override.removeprefix("--overrides=")))
            return json.dumps({"metadata": {"name": "unauthorized"}})
        if "logs" in argv:
            if "job/prefix-upload" in argv:
                return '{"event": "PREFIX_DURABLE"}'
            if "job/mock-miner" in argv:
                return '{"event": "FOUR_ORIGINAL_OBJECTS_VERIFIED", "receipts": [1]}'
            if "job/replica-resume" in argv or "job/regional-fallback" in argv:
                return '{"receipts": [1]}'
            if "job/transport-gc" in argv:
                return '{"event": "TRANSPORT_RETENTION_GC_PREFIX_ISOLATION"}'
            if "unauthorized" in argv:
                return "unexpected connect success" if failure == "denied" else "TimeoutError"
        return ""

    def cleanup_owned(path):
        state = json.loads(path.read_text())
        assert state["deadline_monotonic"] == args.work_end
        assert state["cleanup_cutoff_monotonic"] == args.cleanup_end
        assert state.get("proof_status") != "PASS"
        recorded = json.loads((path.parent / "proof-result.json").read_text())
        assert recorded["transport_cluster_smoke"] == "PENDING"
        assert recorded["archive_node_verification"] == "node-source-verified"
        cleanup.append(path)
        if failure == "cleanup":
            raise RuntimeError("cleanup failed")
        module["CleanupJournal"](path).write_text(
            json.dumps({**state, "state": "CLEANUP_VERIFIED"})
        )

    for name in ("runner_admission", "start_supervisor", "require_capped_node"):
        monkeypatch.setitem(globals_, name, lambda *_, **__: None)
    monkeypatch.setitem(
        globals_, "provider_inventory", lambda _: {"containers": [], "volumes": [], "networks": []}
    )
    monkeypatch.setitem(globals_, "command", command)
    monkeypatch.setitem(globals_, "cleanup_owned", cleanup_owned)
    monkeypatch.setitem(globals_, "verify_archive_node", lambda *_: "node-source-verified")
    monkeypatch.setattr(globals_["shutil"], "which", lambda _: "/no-real-docker")
    monkeypatch.setattr(
        globals_["httpx"],
        "Client",
        lambda **_: nullcontext(
            SimpleNamespace(
                get=lambda _: SimpleNamespace(
                    content=cni, text=cni.decode(), raise_for_status=lambda: None
                )
            )
        ),
    )
    if failure:
        with pytest.raises((RuntimeError, AssertionError)):
            module["_smoke"](args, ("/proc/mock/fd/9", args.archive_reference, files))
    else:
        module["_smoke"](args, ("/proc/mock/fd/9", args.archive_reference, files))
    assert len(cleanup) == 1
    result = json.loads((args.proof_dir / "proof-result.json").read_text())
    state = json.loads((args.proof_dir / "cleanup.json").read_text())
    assert result["archive_node_verification"] == "node-source-verified"
    assert result["transport_cluster_smoke"] == ("PENDING" if failure else "PASS")
    assert state["proof_status"] == ("INTERRUPTED" if failure else "PASS")
    if failure != "fixture":
        assert {"objects", "master", "gateway", "miner", "relay", "unauthorized"} <= set(workloads)
        assert workloads.count("relay") == 2


@pytest.mark.parametrize(
    "fault", [None, "wrong-unit", "cap", "future", "missing", "boot", "expired", "duration"]
)
def test_transport_archive_absolute_cutoff_authority(archive_intake, monkeypatch, fault):
    module, args, _ = archive_intake
    globals_ = module["archive_cutoffs"].__globals__
    unit = "d3-tiny.service"
    original_read = Path.read_text

    def read_text(path, *args, **kwargs):
        if str(path) == "/proc/self/cgroup":
            return "0::/system.slice/" + ("foreign.service" if fault == "wrong-unit" else unit)
        return original_read(path, *args, **kwargs)

    monkeypatch.setattr(Path, "read_text", read_text)
    monkeypatch.setattr(globals_["time"], "monotonic", lambda: 5000.0)
    args.__dict__.update(
        launch_unit=unit,
        work_end=14800.0,
        cleanup_end=16000.0,
        caller_cap=16120.0,
        deadline_seconds=10800,
        clock_boot_id=Path("/proc/sys/kernel/random/boot_id").read_text().strip(),
    )
    if fault == "cap":
        args.caller_cap = 15999.0
    elif fault == "future":
        args.work_end = 15801.0
    elif fault == "missing":
        args.work_end = None
    elif fault == "boot":
        args.clock_boot_id = "stale"
    elif fault == "expired":
        args.work_end = 5000.0
    elif fault == "duration":
        args.cleanup_end += 1
    monkeypatch.setitem(
        globals_, "command", lambda *_: pytest.fail("manager clock lookup forbidden")
    )
    if fault:
        with pytest.raises(ValueError):
            module["archive_cutoffs"](args)
    else:
        module["archive_cutoffs"](args)
        assert args.work_end == 14800.0 and args.cleanup_end == 16000.0


def test_transport_archive_expired_admission_rejects_before_commands(archive_intake, monkeypatch):
    module, args, files = archive_intake
    globals_ = module["_smoke"].__globals__
    args.work_end = 11800.0
    args.deadline_seconds = 10800
    monkeypatch.setattr(globals_["time"], "monotonic", lambda: 11800.0)
    monkeypatch.setitem(
        globals_, "command", lambda *args, **kwargs: pytest.fail("expired admission command")
    )
    with pytest.raises(TimeoutError, match="original work deadline"):
        module["_smoke"](args, ("/proc/mock/fd/9", args.archive_reference, files))


@pytest.mark.parametrize(
    "scenario", ["runner-first", "cancel", "cutoff", "mismatch", "observation", "blind", "foreign"]
)
def test_transport_caller_lock_survives_exit_until_cleanup(
    archive_intake, monkeypatch, tmp_path, scenario
):
    import fcntl
    import json
    import signal
    import socket

    module, args, _ = archive_intake
    globals_ = module["caller_launch"].__globals__
    lock_path = tmp_path / "cpu.lock"
    lock_path.touch(mode=0o600)
    barrier_path = tmp_path / ".transport-admission.json"
    proof = tmp_path / "caller-proof"
    args.__dict__.update(
        caller_unit="d3-caller-tiny",
        cluster="hypertrain-v2-caller-tiny",
        deadline_seconds=10800,
        proof_dir=proof,
        caller_lock=lock_path,
    )
    monkeypatch.setenv("TMPDIR", str(tmp_path))
    monkeypatch.setitem(globals_, "ROOT", tmp_path)
    monkeypatch.setitem(globals_, "caller_resources", lambda: 13120.0)
    monkeypatch.setattr(globals_["time"], "monotonic", lambda: 1000.0)
    baseline = {p: {"containers": [], "volumes": [], "networks": []} for p in ("docker", "podman")}
    handlers = {}
    monkeypatch.setattr(
        globals_["signal"], "signal", lambda sig, handler: handlers.setdefault(sig, handler)
    )
    actions = []
    state = {
        "state": "CREATED",
        "cluster": args.cluster,
        "experiment": args.cluster,
        "proof_dir": str(proof),
        "boot_id": (
            "foreign-boot"
            if scenario == "foreign"
            else Path("/proc/sys/kernel/random/boot_id").read_text().strip()
        ),
        "owned_containers": ["a" * 64],
        "supervisor_ready": {
            "state": "READY",
            "unit": args.cluster + "-supervisor",
            "cgroup": "0::/system.slice/" + args.cluster + "-supervisor.service",
        },
        "cleanup_cutoff_monotonic": 13000.0,
        "deadline_monotonic": 11800.0,
        "provider_baselines": baseline,
    }
    parent, child = socket.socketpair()

    def assert_owned():
        with lock_path.open("a") as candidate, pytest.raises(BlockingIOError):
            fcntl.flock(candidate, fcntl.LOCK_EX | fcntl.LOCK_NB)

    class Events:
        def __init__(self, path):
            assert path == proof
            actions.append("subscribed")

        def wait(self, cutoff, pid):
            assert cutoff == 13000.0
            assert_owned()
            if "runner-exited" in actions:
                assert scenario in ("mismatch", "foreign")
                actions.append("original-cutoff-locked")
                raise TimeoutError("original cutoff")
            assert parent.recv(1) == b"E"  # Explicit queued runner-exit notification.
            actions.append("runner-exited")
            if scenario == "cancel":
                handlers[signal.SIGTERM](signal.SIGTERM, None)
                assert_owned()
                actions.append("cancelled-locked")
            if scenario in ("cutoff", "blind"):
                raise TimeoutError("original cutoff")
            assert_owned()
            state.update(state="CLEANUP_VERIFIED", absence_verified=True, provider_after=baseline)
            module["CleanupJournal"](proof / "cleanup.json").write_text(json.dumps(state))
            (proof / "supervisor.log").write_text("SUPERVISOR_CLEANUP_DONE\n")
            actions.append("supervisor-terminal")

        def close(self):
            assert_owned()
            expected = (
                "INCOMPLETE"
                if scenario in ("cutoff", "mismatch", "foreign", "blind")
                else "VERIFIED"
            )
            assert json.loads(barrier_path.read_text())["state"] == expected
            actions.append("durable-before-release")

    def command(*argv, **kwargs):
        assert_owned()
        if argv[0] == "systemd-run":
            assert actions == ["subscribed"]
            assert "--property=RuntimeMaxSec=12000" in argv
            assert "--property=CPUQuota=40%" in argv and "--property=MemoryMax=256M" in argv
            assert argv[argv.index("--launch-unit") + 1] == "d3-caller-tiny.service"
            assert argv[argv.index("--work-end") + 1] == "11800.0"
            assert argv[argv.index("--cleanup-end") + 1] == "13000.0"
            assert (
                argv[argv.index("--clock-boot-id") + 1] == state["boot_id"] or scenario == "foreign"
            )
            proof.mkdir()
            module["CleanupJournal"](proof / "cleanup.json").write_text(json.dumps(state))
            child.sendall(b"E")
            actions.append("dispatched")
            return ""
        assert "--value" not in argv  # No postdispatch epoch observation exists.
        if scenario == "blind":
            raise RuntimeError("manager permanently blind")
        if scenario == "observation" and "runner-exited" not in actions:
            raise RuntimeError("primary supervisor observation failed")
        return "MainPID=0\nActiveState=inactive\n"

    monkeypatch.setitem(globals_, "CleanupEvents", Events)
    monkeypatch.setitem(globals_, "command", command)
    original_bytes = Path.read_bytes

    def read_bytes(path):
        if scenario == "blind" and path == proof / "cleanup.json" and "dispatched" in actions:
            raise OSError("journal permanently blind")
        return original_bytes(path)

    monkeypatch.setattr(Path, "read_bytes", read_bytes)
    monkeypatch.setitem(
        globals_,
        "provider_inventory",
        lambda provider, execute=None: (
            {"containers": ["foreign"], "volumes": [], "networks": []}
            if scenario == "mismatch" and "supervisor-terminal" in actions
            else baseline[provider]
        ),
    )
    try:
        if scenario == "cancel":
            with pytest.raises(KeyboardInterrupt, match="verified cleanup"):
                module["caller_launch"](args, ["--create"])
        elif scenario in ("cutoff", "mismatch", "foreign", "blind"):
            with pytest.raises((TimeoutError, ValueError, OSError)):
                module["caller_launch"](args, ["--create"])
            if scenario != "blind":
                assert (
                    json.loads((proof / "cleanup.json").read_text())["caller_status"]
                    == "INCOMPLETE"
                )
            assert json.loads(barrier_path.read_text())["state"] == "INCOMPLETE"
            args.proof_dir = tmp_path / "another-proof"
            with pytest.raises(ValueError, match="INCOMPLETE"):
                module["caller_launch"](args, ["--create"])
            assert actions.count("dispatched") == 1
            if scenario not in ("cutoff", "blind"):
                assert "original-cutoff-locked" in actions
            args.create = True
            args.work_end = 11800.0
            with pytest.raises(ValueError, match="CREATE refused"):
                module["_smoke"](args, ("/proc/mock/fd/9", args.archive_reference, {}))
        elif scenario == "observation":
            with pytest.raises(RuntimeError, match="primary"):
                module["caller_launch"](args, ["--create"])
            assert json.loads(barrier_path.read_text())["state"] == "VERIFIED"
        else:
            module["caller_launch"](args, ["--create"])
        with lock_path.open("a") as released:
            fcntl.flock(released, fcntl.LOCK_EX | fcntl.LOCK_NB)
        assert actions[-1] == "durable-before-release"
    finally:
        parent.close()
        child.close()


def test_transport_caller_native_file_event_subscription(archive_intake, tmp_path):
    module, _, _ = archive_intake
    proof = tmp_path / "native-proof"
    events = module["CleanupEvents"](proof)
    try:
        proof.mkdir()
        events.wait(module["time"].monotonic() + 1, 0)  # Arms child watch and rechecks.
        (proof / "supervisor.log").write_text("SUPERVISOR_CLEANUP_DONE\n")
        events.wait(module["time"].monotonic() + 1, 0)  # Already queued inotify event.
    finally:
        events.close()


def test_transport_caller_contention_rejects_before_dispatch(archive_intake, monkeypatch, tmp_path):
    import fcntl

    module, args, _ = archive_intake
    globals_ = module["caller_launch"].__globals__
    lock_path = tmp_path / "busy.lock"
    lock_path.touch(mode=0o600)
    args.__dict__.update(
        caller_unit="d3-tiny",
        caller_lock=lock_path,
        deadline_seconds=10800,
        proof_dir=tmp_path / "busy-proof",
    )
    monkeypatch.setitem(globals_, "caller_resources", lambda: 13120.0)
    monkeypatch.setitem(
        globals_, "command", lambda *_, **__: pytest.fail("dispatch during contention")
    )
    with lock_path.open("a") as owner:
        fcntl.flock(owner, fcntl.LOCK_EX | fcntl.LOCK_NB)
        with pytest.raises(BlockingIOError):
            module["caller_launch"](args, [])
    assert not args.proof_dir.exists()


def test_transport_caller_inclusive_resource_partition(archive_intake, monkeypatch):
    from fractions import Fraction

    module, _, _ = archive_intake
    globals_ = module["caller_resources"].__globals__
    original_read = Path.read_text
    controls = {
        "cpu.max": "5000 100000",
        "memory.max": str(128 << 20),
        "memory.swap.max": "0",
        "cpuset.cpus.effective": "0-3",
    }

    def read_text(path, *args, **kwargs):
        if str(path) == "/proc/self/cgroup":
            return "0::/system.slice/d3-caller-owner.service"
        if str(path).startswith("/sys/fs/cgroup/"):
            return controls[path.name]
        return original_read(path, *args, **kwargs)

    monkeypatch.setattr(Path, "read_text", read_text)
    monkeypatch.setitem(
        globals_,
        "command",
        lambda *_: "RuntimeMaxUSec=3h 22min\nActiveEnterTimestampMonotonic=1000000000",
    )
    assert module["caller_resources"]() == 13120.0
    assert sum(map(Fraction, ["3.5", ".40", ".05", ".05"])) == 4
    assert 4096 + 256 + 128 + 128 == 4608
    controls["memory.max"] = str(129 << 20)
    with pytest.raises(ValueError, match="caller requires"):
        module["caller_resources"]()
