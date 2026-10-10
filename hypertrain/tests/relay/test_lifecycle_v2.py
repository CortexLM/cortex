"""Restart, signed retention, nonce, drain/key and exact custody failure checks."""

from __future__ import annotations

import hashlib
from pathlib import Path

import anyio
import httpx
import pytest
from test_transport_v2 import MASTER, MINER, NEXT, H, Rig

from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.relay_messages import (
    AcceptedUploadAck,
    AvailabilityContext,
    ChunkCustodyAck,
    CustodyRelease,
    RelayAssignment,
    RelayFailureEvidence,
    RelayReceipt,
    RetentionExtension,
    RetrievalRequest,
    RetrievalResponse,
)
from hypertrain.relay.app import create_app
from hypertrain.relay.client import receipt_verified
from hypertrain.relay.core import RelayError, Settlement


@pytest.mark.parametrize("scope", ["chunk", "receipt", "failed-delete"])
def test_gc_when_new_same_hash_custody_races_physical_delete(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, scope: str
) -> None:
    async def scenario() -> None:
        from hypertrain.protocol import envelope_v2
        from hypertrain.protocol.relay_messages import UploadGrant

        async with httpx.AsyncClient() as client:
            rig = Rig(tmp_path, client)
            data = b"abc" if scope == "chunk" else b"a" * (4 << 20) + b"abc"
            old, _, _ = await rig.accept(data)
            for index, off in enumerate(range(0, len(data), 4 << 20)):
                await rig.put(old, data[off : off + (4 << 20)], index)
            record = (await rig.relay.state()).uploads[old.digest()]
            rig.clock = 201

            async def settlement(grant_hash: str) -> Settlement:
                return Settlement(H, 100, H, 120, 110)

            rig.relay.settlement = settlement
            for digest in record.pins:
                if (
                    scope == "receipt"
                    and digest != RelayReceipt.model_validate(record.receipt.body).digest()
                ):
                    continue
                await rig.relay.release(
                    rig.seal(
                        CustodyRelease(
                            grant_hash=old.digest(),
                            custody_hashes=[digest],
                            retention_hash=digest,
                            finality_hash=H,
                            vesting_beacon=110,
                            closed_dispute_root=H,
                            release_beacon=180,
                        )
                    )
                )

            # Subscribe before collect; only physical-delete scheduling is intercepted.
            deleting, resume = anyio.Event(), anyio.Event()
            physical_delete = rig.store.delete
            collected: list[str] = []

            async def paused_delete(sha: str) -> None:
                if sha == old.delta_hash:
                    deleting.set()
                    await resume.wait()
                    if scope == "failed-delete":
                        raise OSError("physical deletion outcome uncertain")
                await physical_delete(sha)

            monkeypatch.setattr(rig.store, "delete", paused_delete)
            peer = rig.new_relay()
            peer.active_key = "k2"
            new, manifest, _, assignment, opened = rig.upload(data, nonce="78" * 32, w=2)
            new = UploadGrant.model_validate({**new.body(), "retain_until": 400, "exp_drand": 250})
            assignment = rig.seal(
                RelayAssignment.model_validate(
                    {
                        **assignment.body,
                        "exp_drand": 250,
                    }
                )
            )
            opened = envelope_v2.parse_envelope(
                envelope_v2.seal(
                    MASTER,
                    "RoundOpenV2",
                    opened.run_id,
                    {**opened.body, "d_upload": 245, "d_final": 260},
                    300,
                )
            )
            await peer.accept(rig.seal(new), manifest, assignment, opened)

            async def collect() -> None:
                if scope == "failed-delete":
                    with pytest.raises(OSError, match="outcome uncertain"):
                        await rig.relay.collect()
                else:
                    collected.extend(await rig.relay.collect())

            async def source(part: bytes):
                yield part

            with anyio.fail_after(10):
                async with anyio.create_task_group() as tasks:
                    tasks.start_soon(collect)
                    await deleting.wait()
                    # A fresh replica reads the durable deletion generation, not local locks.
                    fresh = rig.new_relay()
                    assert old.delta_hash in (await fresh.state()).deletions
                    try:
                        with pytest.raises(RelayError, match="deletion reservation"):
                            for index, off in enumerate(range(0, len(data), 4 << 20)):
                                await peer.chunk(
                                    new.digest(), index, source(data[off : off + (4 << 20)])
                                )
                        target = (await fresh.state()).uploads[new.digest()]
                        assert target.receipt is None
                        if scope != "receipt":
                            assert not target.chunks
                    finally:
                        resume.set()

            if scope == "failed-delete":
                assert old.delta_hash in (await peer.state()).deletions
                with pytest.raises(RelayError, match="deletion reservation"):
                    await peer.chunk(new.digest(), 0, source(data))
                assert not (await peer.state()).uploads[new.digest()].chunks
                return
            assert old.delta_hash in collected
            assert not rig.store.store._path(old.delta_hash).exists()
            assert not (await peer.state()).deletions
            # Reservation ends only after deletion; retry writes before acknowledging custody.
            for index, off in enumerate(range(0, len(data), 4 << 20)):
                await peer.chunk(new.digest(), index, source(data[off : off + (4 << 20)]))
            assert (await peer.state()).uploads[new.digest()].receipt is not None
            assert rig.store.store.get(new.delta_hash) == data
            assert new.delta_hash not in await rig.relay.collect()

    anyio.run(scenario)


def test_restart_when_custody_persisted_before_forwarding(tmp_path: Path) -> None:
    async def scenario() -> None:
        async with httpx.AsyncClient() as client:
            rig = Rig(tmp_path, client)
            grant, _, opportunity = await rig.accept(b"abc")
            chunk = await rig.put(grant, b"abc")
            receipt = await rig.relay.complete(grant.digest())
            rig.relay = rig.new_relay()
            await rig.relay.recover()
            record = (await rig.relay.state()).uploads[grant.digest()]
            assert (record.opportunity, record.chunks["0"], record.receipt) == (
                opportunity,
                chunk,
                receipt,
            )
            assert record.master_acceptance is None

    anyio.run(scenario)


def test_recovery_when_complete_custody_misses_receipt_deadline(tmp_path: Path) -> None:
    # Given: crash at the full-custody boundary before receipt assembly.
    async def scenario() -> None:
        async with httpx.AsyncClient() as client:
            rig = Rig(tmp_path, client)
            grant, _, _ = await rig.accept(b"abc")
            await rig.put(grant, b"abc")

            def interrupted(state) -> None:
                state.uploads[grant.digest()].receipt = None

            await rig.relay._change(interrupted)
            chunk = (await rig.relay.state()).uploads[grant.digest()].chunks["0"]
            rig.clock = 41
            # When: a restarted relay reconstructs at a beacon after upload cutoff.
            relay = rig.new_relay()
            await relay.recover()
            response = await relay.retrieve(rig.retrieval(grant, chunk))
            # Then: no backdated receipt; exact retained chunk still serves.
            record = (await relay.state()).uploads[grant.digest()]
            assert record.receipt is None
            assert record.forward_error == "RelayError"
            assert RetrievalResponse.model_validate(response.body).status == "SERVED"

    anyio.run(scenario)


@pytest.mark.parametrize("fault", ["nonce", "key", "horizon", "scope"])
def test_retrieval_when_wrong_nonce_key_or_custody(tmp_path: Path, fault: str) -> None:
    async def scenario() -> None:
        async with httpx.AsyncClient() as client:
            rig = Rig(tmp_path, client)
            grant, _, _ = await rig.accept(b"abc")
            await rig.put(grant, b"abc")
            receipt = await rig.relay.complete(grant.digest())
            original = rig.retrieval(grant, receipt)
            request = RetrievalRequest.model_validate(original.body)
            if fault == "nonce":
                await rig.relay.retrieve(original)
                altered = {**request.body(), "request_id": "78" * 32}
            elif fault == "key":
                altered = {**request.body(), "key_id": "k2"}
            elif fault == "horizon":
                altered = {**request.body(), "retention_hash": H}
            else:
                altered = {**request.body(), "object_or_chunk_hash": H}
            with pytest.raises(RelayError):
                await rig.relay.retrieve(rig.seal(RetrievalRequest.model_validate(altered)))

    anyio.run(scenario)


@pytest.mark.parametrize("fault", ["late", "forged", "previous", "sequence"])
def test_extension_when_no_timely_authenticated_successor(tmp_path: Path, fault: str) -> None:
    async def scenario() -> None:
        async with httpx.AsyncClient() as client:
            rig = Rig(tmp_path, client)
            grant, _, _ = await rig.accept(b"abc")
            await rig.put(grant, b"abc")
            receipt = RelayReceipt.model_validate((await rig.relay.complete(grant.digest())).body)
            rig.clock = 200 if fault == "late" else 190
            extension = RetentionExtension(
                grant_hash=grant.digest(),
                custody_hashes=[receipt.digest()],
                seq=2 if fault == "sequence" else 1,
                previous_retention_hash=H if fault == "previous" else receipt.digest(),
                dispute_ids=[H],
                issued_beacon=rig.clock,
                retain_until=300,
            )
            with pytest.raises(ValueError):
                await rig.relay.retention(
                    rig.seal(extension, MINER if fault == "forged" else MASTER)
                )
            pin = (await rig.relay.state()).uploads[grant.digest()].pins[receipt.digest()]
            assert pin.horizon == 200
            assert not (await rig.relay.state()).disabled_keys

    anyio.run(scenario)


def test_retention_when_dispute_extends_before_original_expiry(tmp_path: Path) -> None:
    async def scenario() -> None:
        async with httpx.AsyncClient() as client:
            rig = Rig(tmp_path, client)
            grant, _, _ = await rig.accept(b"abc")
            await rig.put(grant, b"abc")
            receipt_raw = await rig.relay.complete(grant.digest())
            receipt = RelayReceipt.model_validate(receipt_raw.body)
            rig.clock = 190
            extension = RetentionExtension(
                grant_hash=grant.digest(),
                custody_hashes=[receipt.digest()],
                seq=1,
                previous_retention_hash=receipt.digest(),
                dispute_ids=[H],
                issued_beacon=190,
                retain_until=300,
            )
            raw = rig.seal(extension)
            ack = await rig.relay.retention(raw)
            assert ack == await rig.new_relay().retention(raw)
            rig.clock = 210
            response = await rig.relay.retrieve(
                rig.retrieval(grant, receipt_raw, horizon=300, retention_hash=extension.digest())
            )
            assert RetrievalResponse.model_validate(response.body).status == "SERVED"
            assert not await rig.relay.eligible_deletions()

    anyio.run(scenario)


def test_near_horizon_when_retrieval_uses_signed_retention_not_upload(tmp_path: Path) -> None:
    async def scenario() -> None:
        async with httpx.AsyncClient() as client:
            rig = Rig(tmp_path, client)
            grant, _, _ = await rig.accept(b"abc")
            await rig.put(grant, b"abc")
            receipt = await rig.relay.complete(grant.digest())
            rig.clock = 198
            raw = rig.retrieval(grant, receipt)
            assert RetrievalRequest.model_validate(raw.body).deadline_beacon == 200
            assert (
                RetrievalResponse.model_validate((await rig.relay.retrieve(raw)).body).status
                == "SERVED"
            )

    anyio.run(scenario)


def test_drain_when_existing_upload_can_finish_new_upload_rejects(tmp_path: Path) -> None:
    async def scenario() -> None:
        async with httpx.AsyncClient() as client:
            rig = Rig(tmp_path, client)
            grant, _, _ = await rig.accept(b"abc")
            app = create_app(rig.relay, "t" * 32)
            async with httpx.AsyncClient(
                transport=httpx.ASGITransport(app=app), base_url="https://relay.test"
            ) as wire:
                assert (await wire.post("/internal/drain")).status_code == 403
                response = await wire.post(
                    "/internal/drain", headers={"Authorization": "Bearer " + "t" * 32}
                )
                assert response.status_code == 200
                assert (await wire.get("/readyz")).status_code == 503
                assert (await wire.get("/livez")).status_code == 200
            await rig.put(grant, b"abc")
            _, manifest, raw, assignment, opened = rig.upload(b"next", nonce="56" * 32, w=2)
            with pytest.raises(RelayError):
                await rig.relay.accept(raw, manifest, assignment, opened)
            assert (await rig.relay.state()).uploads[grant.digest()].receipt is not None

    anyio.run(scenario)


def test_rotation_when_old_receipt_remains_verifiable_new_key_signs(tmp_path: Path) -> None:
    async def scenario() -> None:
        async with httpx.AsyncClient() as client:
            rig = Rig(tmp_path, client)
            grant, _, _ = await rig.accept(b"abc")
            await rig.put(grant, b"abc")
            old = await rig.relay.complete(grant.digest())
            rig.clock = 90
            rig.relay.active_key = "k2"
            _, manifest, raw, assignment, opened = rig.upload(b"next", nonce="56" * 32, w=2)
            # Fresh signed upload deadlines, no historical acceptance backdating.
            from hypertrain.protocol import envelope_v2
            from hypertrain.protocol.relay_messages import UploadGrant

            fresh = UploadGrant.model_validate({**raw.body, "exp_drand": 120})
            raw = rig.seal(fresh)
            assignment_body = RelayAssignment.model_validate({**assignment.body, "exp_drand": 120})
            opened_body = {**opened.body, "d_upload": 115, "d_final": 130}
            opened = envelope_v2.parse_envelope(
                envelope_v2.seal(MASTER, "RoundOpenV2", opened.run_id, opened_body, 150)
            )
            ack = await rig.relay.accept(raw, manifest, rig.seal(assignment_body), opened)
            assert ack.signer == NEXT.ss58
            receipt_verified(
                old,
                registry=rig.registry,
                relay_id="eu-1",
                run_id=old.run_id,
                grant=grant,
                received_before=35,
            )

    anyio.run(scenario)


def test_release_when_unresolved_dispute_blocks_deletion(tmp_path: Path) -> None:
    async def scenario() -> None:
        async with httpx.AsyncClient() as client:
            rig = Rig(tmp_path, client)
            grant, _, _ = await rig.accept(b"abc")
            await rig.put(grant, b"abc")
            receipt = RelayReceipt.model_validate((await rig.relay.complete(grant.digest())).body)
            rig.clock = 190
            raw = rig.seal(
                CustodyRelease(
                    grant_hash=grant.digest(),
                    custody_hashes=[receipt.digest()],
                    retention_hash=receipt.digest(),
                    finality_hash=H,
                    vesting_beacon=100,
                    closed_dispute_root=H,
                    release_beacon=180,
                )
            )

            async def settlement(grant_hash: str) -> Settlement:
                assert grant_hash == grant.digest()
                return Settlement(H, 100, H, None, 100)

            rig.relay.settlement = settlement
            with pytest.raises(ValueError):
                await rig.relay.release(raw)
            assert not await rig.relay.eligible_deletions()

    anyio.run(scenario)


def test_release_when_closed_graced_horizons_expire_only_then_eligible(tmp_path: Path) -> None:
    async def scenario() -> None:
        async with httpx.AsyncClient() as client:
            rig = Rig(tmp_path, client)
            grant, _, _ = await rig.accept(b"abc")
            await rig.put(grant, b"abc")
            record = (await rig.relay.state()).uploads[grant.digest()]
            rig.clock = 201

            async def settlement(grant_hash: str) -> Settlement:
                return Settlement(H, 100, H, 120, 110)

            rig.relay.settlement = settlement
            for digest in record.pins:
                await rig.relay.release(
                    rig.seal(
                        CustodyRelease(
                            grant_hash=grant.digest(),
                            custody_hashes=[digest],
                            retention_hash=digest,
                            finality_hash=H,
                            vesting_beacon=110,
                            closed_dispute_root=H,
                            release_beacon=180,
                        )
                    )
                )
            assert await rig.relay.eligible_deletions() == [grant.delta_hash]

    anyio.run(scenario)


@pytest.mark.parametrize("full", [False, True])
def test_failure_when_completion_requires_full_custody_not_miner_assertions(
    tmp_path: Path, full: bool
) -> None:
    async def scenario() -> None:
        async with httpx.AsyncClient() as client:
            rig = Rig(tmp_path, client)
            data = b"a" * (4 << 20) + b"suffix"
            grant, _, _ = await rig.accept(data)
            await rig.put(grant, data[: 4 << 20])
            if full:
                await rig.put(grant, b"suffix", 1)
            record = (await rig.relay.state()).uploads[grant.digest()]
            opportunity = AcceptedUploadAck.model_validate(record.opportunity.body)
            artifacts = list(record.chunks.values())
            evidence = RelayFailureEvidence(
                run_id=rig.relay.run_id,
                relay_id="eu-1",
                key_id="k1",
                assignment_hash=RelayAssignment.model_validate(record.assignment.body).digest(),
                grant_hash=grant.digest(),
                upload_ack_hash=opportunity.digest(),
                chunk_manifest_hash=grant.chunk_manifest_hash,
                chunk_ack_hashes=[
                    ChunkCustodyAck.model_validate(e.body).digest() for e in artifacts
                ],
                receipt_hash=None,
                retention_hash=(
                    hashlib.sha256(
                        canonicalize(
                            sorted(
                                ChunkCustodyAck.model_validate(e.body).digest() for e in artifacts
                            ),
                            allow_float=False,
                        )
                    ).hexdigest()
                    if full
                    else ChunkCustodyAck.model_validate(artifacts[0].body).digest()
                ),
                request=None,
                response=None,
                observer_ids=[MASTER.ss58, MINER.ss58],
                observed_beacons=[36, 36],
                result="CONTRACTUAL_SERVICE_FAILURE",
                availability_context=AvailabilityContext(
                    master_healthy=True,
                    beacon_healthy=True,
                    reference_healthy=True,
                    observer_healthy=True,
                ),
                evidence_hash=hashlib.sha256(
                    canonicalize([e.model_dump(mode="json") for e in artifacts], allow_float=False)
                ).hexdigest(),
            )
            rig.clock = 36
            if full:
                await rig.relay.failure(
                    rig.seal(evidence), artifacts, [rig.seal(evidence), rig.seal(evidence, MINER)]
                )
                assert (await rig.relay.state()).disabled_keys == ["k1"]
            else:
                with pytest.raises(RelayError):
                    await rig.relay.failure(
                        rig.seal(evidence),
                        artifacts,
                        [rig.seal(evidence), rig.seal(evidence, MINER)],
                    )
                assert not (await rig.relay.state()).disabled_keys

    anyio.run(scenario)


def test_missing_prefix_when_post_upload_retrieval_only_faults_acknowledged_bytes(
    tmp_path: Path,
) -> None:
    async def scenario() -> None:
        async with httpx.AsyncClient() as client:
            rig = Rig(tmp_path, client)
            payload = b"a" * (4 << 20) + b"unreceived-suffix"
            grant, _, _ = await rig.accept(payload)
            custody = await rig.put(grant, payload[: 4 << 20])
            ack = ChunkCustodyAck.model_validate(custody.body)
            rig.clock = 41
            rig.store.store._path(ack.chunk_sha256).unlink()
            request_raw = rig.retrieval(grant, custody)
            response_raw = await rig.relay.retrieve(request_raw)
            response = RetrievalResponse.model_validate(response_raw.body)
            assert response.status == "NOT_FOUND"
            record = (await rig.relay.state()).uploads[grant.digest()]
            opportunity = AcceptedUploadAck.model_validate(record.opportunity.body)
            artifacts = [custody, request_raw, response_raw]
            evidence = RelayFailureEvidence(
                run_id=rig.relay.run_id,
                relay_id="eu-1",
                key_id="k1",
                assignment_hash=RelayAssignment.model_validate(record.assignment.body).digest(),
                grant_hash=grant.digest(),
                upload_ack_hash=opportunity.digest(),
                chunk_manifest_hash=grant.chunk_manifest_hash,
                chunk_ack_hashes=[ack.digest()],
                receipt_hash=None,
                retention_hash=ack.digest(),
                request=RetrievalRequest.model_validate(request_raw.body),
                response=response,
                observer_ids=[MASTER.ss58, MINER.ss58],
                observed_beacons=[52, 52],
                result="CONTRACTUAL_SERVICE_FAILURE",
                availability_context=AvailabilityContext(
                    master_healthy=True,
                    beacon_healthy=True,
                    reference_healthy=True,
                    observer_healthy=True,
                ),
                evidence_hash=hashlib.sha256(
                    canonicalize([e.model_dump(mode="json") for e in artifacts], allow_float=False)
                ).hexdigest(),
            )
            rig.clock = 52
            with pytest.raises(ValueError):
                await rig.relay.failure(rig.seal(evidence), artifacts, [])
            assert record.receipt is None
            framed = RelayFailureEvidence.model_validate({**evidence.body(), "retention_hash": H})
            with pytest.raises(RelayError):
                await rig.relay.failure(
                    rig.seal(framed), artifacts, [rig.seal(framed), rig.seal(framed, MINER)]
                )
            await rig.relay.failure(
                rig.seal(evidence), artifacts, [rig.seal(evidence), rig.seal(evidence, MINER)]
            )
            saved = (await rig.relay.state()).failures[evidence.digest()]
            assert saved.body["receipt_hash"] is None
            assert saved.body["chunk_ack_hashes"] == [ack.digest()]

    anyio.run(scenario)


def test_forward_unavailable_when_custody_still_recovers_without_fake_acceptance(
    tmp_path: Path,
) -> None:
    async def scenario() -> None:
        async with httpx.AsyncClient() as client:
            rig = Rig(tmp_path, client)
            grant, _, _ = await rig.accept(b"abc")

            async def failed(raw):
                raise httpx.ConnectError("master unavailable")

            rig.relay.forward = failed
            with pytest.raises(httpx.ConnectError):
                await rig.put(grant, b"abc")
            await rig.relay.recover()
            record = (await rig.relay.state()).uploads[grant.digest()]
            assert record.receipt is not None
            assert record.master_acceptance is None
            assert record.forward_error == "ConnectError"
            assert (
                await rig.relay.retrieve(rig.retrieval(grant, record.receipt))
            ).type == "RetrievalResponse"

    anyio.run(scenario)


def test_ready_when_durable_probe_and_key_valid(tmp_path: Path) -> None:
    async def scenario() -> None:
        async with httpx.AsyncClient() as client:
            rig = Rig(tmp_path, client)
            assert await rig.relay.ready()
            rig.clock = 101
            with pytest.raises(RelayError):
                await rig.relay.ready()
            async with httpx.AsyncClient(
                transport=httpx.ASGITransport(app=create_app(rig.relay, "t" * 32)),
                base_url="https://relay.test",
            ) as wire:
                assert (await wire.get("/readyz")).status_code == 503

    anyio.run(scenario)


def test_ready_when_shared_upload_capacity_exhausted(tmp_path: Path) -> None:
    # Given: a durable outstanding upload exhausts the shared reservation quota.
    async def scenario() -> None:
        async with httpx.AsyncClient() as client:
            rig = Rig(tmp_path, client)
            rig.relay.max_uploads = 1
            await rig.accept(b"abc")
            # When: readiness is evaluated without any active request in this process.
            ready = await rig.relay.ready()
            # Then: new traffic is not routed into an exhausted replica.
            assert not ready

    anyio.run(scenario)


def test_retrieval_when_release_races_durable_read(tmp_path: Path) -> None:
    # Given: an authenticated chunk challenge is in flight while finality releases custody.
    async def scenario() -> None:
        async with httpx.AsyncClient() as client:
            rig = Rig(tmp_path, client)
            grant, _, _ = await rig.accept(b"abc")
            chunk = await rig.put(grant, b"abc")
            custody = ChunkCustodyAck.model_validate(chunk.body)
            request = rig.retrieval(grant, chunk)
            entered, resume = anyio.Event(), anyio.Event()
            original = rig.store.get

            async def interrupted(sha, size, file):
                await original(sha, size, file)
                entered.set()
                await resume.wait()

            rig.store.get = interrupted

            async def settlement(grant_hash: str) -> Settlement:
                return Settlement(H, 1, H, 1, 1)

            rig.relay.settlement = settlement

            async def read() -> None:
                with pytest.raises(ValueError):
                    await rig.relay.retrieve(request)

            # When: the persisted release lands after the backing read starts.
            with anyio.fail_after(5):
                async with anyio.create_task_group() as tasks:
                    tasks.start_soon(read)
                    await entered.wait()
                    await rig.relay.release(
                        rig.seal(
                            CustodyRelease(
                                grant_hash=grant.digest(),
                                custody_hashes=[custody.digest()],
                                retention_hash=custody.digest(),
                                finality_hash=H,
                                vesting_beacon=1,
                                closed_dispute_root=H,
                                release_beacon=11,
                            )
                        )
                    )
                    resume.set()
            # Then: no nonce/response reservation is minted against released custody.
            assert not (await rig.relay.state()).uploads[grant.digest()].retrievals

    anyio.run(scenario)
