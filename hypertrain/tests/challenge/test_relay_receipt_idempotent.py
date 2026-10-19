"""A miner rerunning a finished round resubmits its relay receipt: clean answer, never a 500."""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

import anyio
import httpx
import pytest

from hypertrain.data.store import LocalFSStore
from hypertrain.data.stream_store import StreamStore
from hypertrain.protocol import envelope_v2, relay_envelope
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.relay_messages import (
    RelayAssignment,
    RelayNetworkManifest,
    RelayReceipt,
    RetrievalRequest,
    UploadChunk,
    UploadChunkManifest,
    UploadGrant,
)
from hypertrain.relay.app import create_app as relay_app
from hypertrain.relay.core import Relay

spec = importlib.util.spec_from_file_location(
    "relay_receipt_idempotent_fixture", Path(__file__).parent / "test_service_network_v2.py"
)
assert spec is not None and spec.loader is not None
fixtures = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = fixtures
spec.loader.exec_module(fixtures)
network = fixtures.network
COORD, RELAY, HOT = fixtures.COORD, fixtures.RELAY, fixtures.HOT[0].ss58
PAYLOAD = b"rerun delta"


@pytest.fixture
def uploaded(network, monkeypatch, tmp_path):
    """Real master app and real relay app; the delta is uploaded and its receipt signed."""
    run_id = network.manifest.run_id()
    registry = network.store._registry_v2(network.manifest)
    net = RelayNetworkManifest(
        run_id=run_id,
        base_manifest_hash=network.manifest.training.training_hash(),
        registry_hash=registry.digest(),
        specs_hash=sha256_hex(canonicalize([s.body() for s in registry.specs])),
        assignment_policy="master-observed-median3-v1",
    )
    chunks = UploadChunkManifest(
        size=len(PAYLOAD),
        chunks=[UploadChunk(index=0, off=0, len=len(PAYLOAD), chunk_sha256=sha256_hex(PAYLOAD))],
    )
    assignment = RelayAssignment(
        w=0,
        hotkey=HOT,
        primary_id="local",
        fallback_ids=[],
        assignment_epoch=registry.epoch,
        manifest_hash=net.digest(),
        exp_drand=network.now + 20,
    )
    grant = UploadGrant(
        w=0,
        hotkey=HOT,
        relay_id="local",
        assignment_epoch=registry.epoch,
        delta_hash=sha256_hex(PAYLOAD),
        size=len(PAYLOAD),
        chunk_manifest_hash=chunks.chunk_manifest_hash(),
        retain_until=network.now + 200,
        nonce="ad" * 32,
        exp_drand=assignment.exp_drand,
    )
    signed_grant = relay_envelope.seal(COORD, "UploadGrant", run_id, grant, grant.exp_drand)
    signed_assignment = relay_envelope.seal(
        COORD, "RelayAssignment", run_id, assignment, assignment.exp_drand
    )
    opened = {
        "w": 0,
        **{
            k: "12" * 32
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
        "d_commit": network.now + 10,
        "d_audit": network.now + 11,
        "d_upload": grant.exp_drand,
        "d_final": network.now + 30,
        "contract_version": 2,
        "policy_hashes": {
            k: getattr(network.manifest.network, k)
            for k in (
                "admission_policy_hash",
                "economics_policy_hash",
                "aggregation_policy_hash",
                "dispute_policy_hash",
                "audit_policy_hash",
            )
        },
        "registry_epoch": registry.epoch,
        "audit_mode": "anchored-full",
        "roster": [],
    }
    # Accepted master journal boundary with original signatures; no training round.
    with network.store._tx():
        network.store._put_record_v2(run_id, "grant", grant.digest(), signed_grant)
        network.store._put_record_v2(run_id, "chunk-manifest", grant.digest(), chunks.body())
        network.store._put_record_v2(run_id, "relay-assignment", "0:" + HOT, signed_assignment)
        network.store._put_record_v2(run_id, "round", "0", {"body": opened})
    real_client = httpx.AsyncClient
    backing = real_client()

    async def now():
        return network.now

    relay = Relay(
        run_id=run_id,
        master=COORD.ss58,
        registry=registry,
        network_manifest_hash=net.digest(),
        observers={a.ss58 for a in fixtures.AUDITORS},
        relay_id="local",
        region="local",
        keys={"k1": RELAY},
        active_key="k1",
        backing=StreamStore(LocalFSStore(tmp_path / "relay-backing"), backing),
        now=now,
    )

    async def upload():
        await relay.accept(
            relay_envelope.parse_envelope(signed_grant),
            chunks,
            relay_envelope.parse_envelope(signed_assignment),
            envelope_v2.parse_envelope(network.signed(COORD, "RoundOpenV2", opened)),
        )

        async def source():
            yield PAYLOAD

        await relay.chunk(grant.digest(), 0, source())
        return await relay.complete(grant.digest())

    receipt = anyio.run(upload)
    app = relay_app(relay, "x" * 32)
    monkeypatch.setattr(
        httpx,
        "AsyncClient",
        lambda *a, **kw: real_client(*a, **{**kw, "transport": httpx.ASGITransport(app=app)}),
    )
    yield relay, grant, receipt
    anyio.run(backing.aclose)


def submit(network, receipt):
    return network.client.post(
        "/v2/relay-receipts", content=canonicalize(receipt.model_dump(mode="json"))
    )


def count(network, kind):
    return network.store._db.execute(
        "SELECT COUNT(*) FROM records_v2 WHERE kind=?", (kind,)
    ).fetchone()[0]


def test_second_relay_receipt_for_same_key_is_idempotent(network, uploaded):
    _, grant, receipt = uploaded
    first = submit(network, receipt)
    assert first.status_code == 200, first.text
    assert first.json()["body"]["delta_hash"] == grant.delta_hash
    second = submit(network, receipt)
    assert second.status_code == 200, second.text
    assert second.json() == first.json()
    # The first acceptance stays authoritative: no second retrieval or acceptance.
    assert count(network, "master-acceptance") == 1
    assert count(network, "retrieval") == 1


def test_relay_receipt_different_payload_same_key_is_409(network, uploaded):
    _, grant, receipt = uploaded
    first = submit(network, receipt)
    assert first.status_code == 200, first.text
    # A genuinely relay-signed receipt for the same grant, but a different body.
    other = RelayReceipt.model_validate(receipt.body).model_copy(
        update={"received_drand": RelayReceipt.model_validate(receipt.body).received_drand + 1}
    )
    resigned = relay_envelope.parse_envelope(
        relay_envelope.seal(RELAY, "RelayReceipt", receipt.run_id, other, grant.retain_until)
    )
    assert other.digest() != RelayReceipt.model_validate(receipt.body).digest()
    refused = submit(network, resigned)
    assert refused.status_code == 409, refused.text
    assert refused.json()["detail"] == "RELAY_RECEIPT_CONFLICT"
    assert count(network, "master-acceptance") == 1
    assert count(network, "retrieval") == 1


def test_relay_400_on_retrieval_is_a_clean_error_not_500(network, uploaded):
    relay, grant, receipt = uploaded
    run_id = network.manifest.run_id()
    # Another live challenge on the same object makes the real relay answer 400.
    request = RetrievalRequest(
        request_id="ab" * 32,
        nonce="ac" * 32,
        relay_id="local",
        key_id="k1",
        assignment_hash=sha256_hex(
            canonicalize(network.store._record_v2(run_id, "relay-assignment", "0:" + HOT)["body"])
        ),
        grant_hash=grant.digest(),
        custody_ack_hashes=[],
        receipt_hash=RelayReceipt.model_validate(receipt.body).digest(),
        retention_hash=RelayReceipt.model_validate(receipt.body).digest(),
        object_or_chunk_hash=grant.delta_hash,
        size=grant.size,
        requested_beacon=network.now,
        deadline_beacon=network.now + 10,
    )
    anyio.run(
        relay.retrieve,
        relay_envelope.parse_envelope(
            relay_envelope.seal(COORD, "RetrievalRequest", run_id, request, network.now + 10)
        ),
    )
    refused = submit(network, receipt)
    assert refused.status_code == 502, refused.text
    assert "400" in refused.json()["detail"]
    assert count(network, "master-acceptance") == 0
