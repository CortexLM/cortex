"""Restart, signed retention, nonce, drain/key and exact custody failure checks."""

from __future__ import annotations

import hashlib
from pathlib import Path

import anyio
import httpx
import pytest
from test_transport_v2 import MASTER, MINER, NEXT, H, Rig

from hypertrain.data.store import CorruptObjectError, ObjectNotFound
from hypertrain.protocol import envelope_v2
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages_v2 import RosterEntryV2
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
    UploadGrant,
)
from hypertrain.relay import core as relay_core
from hypertrain.relay.app import create_app
from hypertrain.relay.client import receipt_verified
from hypertrain.relay.core import AssemblyMismatch, RelayError, Settlement


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


async def settled(grant_hash: str) -> Settlement:
    return Settlement(H, 1, H, 1, 1)


def round_nonce(w: int) -> str:
    return f"{w:064x}"


async def upload_round(rig: Rig, w: int, data: bytes | None = None) -> UploadGrant:
    data = data or b"round-%d" % w
    grant, manifest, raw, assignment, opened = rig.upload(data, nonce=round_nonce(w), w=w)
    await rig.relay.accept(raw, manifest, assignment, opened)
    await rig.put(grant, data)
    return grant


async def release_pins(rig: Rig, grant: UploadGrant, only: list[str] | None = None) -> None:
    for digest in only or (await rig.relay.state()).uploads[grant.digest()].pins:
        await rig.relay.release(
            rig.seal(
                CustodyRelease(
                    grant_hash=grant.digest(),
                    custody_hashes=[digest],
                    retention_hash=digest,
                    finality_hash=H,
                    vesting_beacon=1,
                    closed_dispute_root=H,
                    release_beacon=11,
                )
            )
        )


def late_upload(rig: Rig, data: bytes, w: int):
    # A grant for a round opened after the custody horizon (200) of the first rounds lapsed.
    grant, manifest, _, assignment, opened = rig.upload(data, nonce=round_nonce(w), w=w)
    grant = UploadGrant.model_validate({**grant.body(), "retain_until": 400, "exp_drand": 250})
    assignment = rig.seal(RelayAssignment.model_validate({**assignment.body, "exp_drand": 250}))
    opened = envelope_v2.parse_envelope(
        envelope_v2.seal(
            MASTER,
            "RoundOpenV2",
            opened.run_id,
            {**opened.body, "d_upload": 245, "d_final": 260},
            300,
        )
    )
    return grant, manifest, rig.seal(grant), assignment, opened


def with_roster(opened, miners: int):
    # The signed RoundOpenV2 a relay stores in every record carries the whole roster.
    roster = [
        RosterEntryV2(
            hotkey=Keypair(bytes([i + 1]) * 32).ss58,
            slot=i,
            q_i="0000803f",
            admission_id=H,
            coldkey_group=f"owner{i}",
            state="ACTIVE",
            eligible_weight=1 << 22,
        ).model_dump(mode="json")
        for i in range(miners)
    ]
    return envelope_v2.parse_envelope(
        envelope_v2.seal(
            MASTER, "RoundOpenV2", opened.run_id, {**opened.body, "roster": roster}, 50
        )
    )


async def completed_upload(rig: Rig, miners: int):
    """One real one-chunk upload of a round with `miners` rostered; the record and the journal
    bytes it adds (the record plus its index and nonce entries)."""
    data = b"round-1"
    grant, manifest, raw, assignment, opened = rig.upload(data, nonce=round_nonce(1))
    empty = len((await rig.relay.state()).serialize())
    await rig.relay.accept(raw, manifest, assignment, with_roster(opened, miners))
    await rig.put(grant, data)
    state = await rig.relay.state()
    return state.uploads[grant.digest()], len(state.serialize()) - empty


def hold_copies(record, count: int):
    """Journal change that holds `count` copies of a record under distinct grants and nonces."""

    def change(state) -> None:
        for i in range(count):
            key = f"{i + 1:064x}"
            state.uploads[key] = record
            state.nonces["f" * 8 + key[8:]] = key

    return change


def test_quota_when_settled_uploads_are_released(tmp_path: Path) -> None:
    # Given: a quota of three held uploads and a miner pinned to this relay every round.
    async def scenario() -> None:
        async with httpx.AsyncClient() as client:
            rig = Rig(tmp_path, client)
            rig.relay.max_uploads = 3
            rig.relay.settlement = settled
            grants = []
            for w in range(1, 11):
                grants.append(await upload_round(rig, w))
                if w >= 3:  # the round two behind has settled and is released
                    await release_pins(rig, grants[w - 3])
            await upload_round(rig, 11)  # rounds 9, 10 and 11 hold the three places
            _, manifest, raw, assignment, opened = rig.upload(
                b"twelve", nonce=round_nonce(12), w=12
            )
            # When: a twelfth upload arrives while nothing else is released.
            with pytest.raises(RelayError, match="quota"):
                await rig.relay.accept(raw, manifest, assignment, opened)
            # Then: releasing one held upload frees exactly one place.
            await release_pins(rig, grants[8])
            await rig.relay.accept(raw, manifest, assignment, opened)

    anyio.run(scenario)


def test_quota_when_custody_is_released_in_part(tmp_path: Path) -> None:
    async def scenario() -> None:
        async with httpx.AsyncClient() as client:
            rig = Rig(tmp_path, client)
            rig.relay.max_uploads = 1
            rig.relay.settlement = settled
            grant = await upload_round(rig, 1)
            record = (await rig.relay.state()).uploads[grant.digest()]
            receipt = RelayReceipt.model_validate(record.receipt.body).digest()
            _, manifest, raw, assignment, opened = rig.upload(b"two", nonce=round_nonce(2), w=2)
            # When: the receipt is released but the chunk acknowledgment is still pinned.
            await release_pins(rig, grant, [receipt])
            with pytest.raises(RelayError, match="quota"):
                await rig.relay.accept(raw, manifest, assignment, opened)
            # Then: the place frees only with the last pin.
            await release_pins(rig, grant, [d for d in record.pins if d != receipt])
            await rig.relay.accept(raw, manifest, assignment, opened)

    anyio.run(scenario)


def test_quota_when_reservation_deadline_passed(tmp_path: Path) -> None:
    async def scenario() -> None:
        async with httpx.AsyncClient() as client:
            rig = Rig(tmp_path, client)
            rig.relay.max_uploads = 1
            await rig.accept(b"abc")  # reservation with service deadline 35, no bytes ever sent
            _, manifest, raw, assignment, opened = late_upload(rig, b"def", 2)
            # When: the deadline is still current, then one beacon later.
            rig.clock = 35
            with pytest.raises(RelayError, match="quota"):
                await rig.relay.accept(raw, manifest, assignment, opened)
            rig.clock = 36
            # Then: the abandoned reservation stops holding the place.
            await rig.relay.accept(raw, manifest, assignment, opened)

    anyio.run(scenario)


def test_quota_when_custody_horizon_lapsed(tmp_path: Path) -> None:
    async def scenario() -> None:
        async with httpx.AsyncClient() as client:
            rig = Rig(tmp_path, client)
            rig.relay.max_uploads = 2
            first = await upload_round(rig, 1)
            await upload_round(rig, 2)
            _, manifest, raw, assignment, opened = late_upload(rig, b"late", 3)
            rig.relay.active_key = "k2"
            # Retrieval is served up to and including the signed horizon (200): still held.
            for clock in (100, 200):
                rig.clock = clock
                with pytest.raises(RelayError, match="quota"):
                    await rig.relay.accept(raw, manifest, assignment, opened)
            # When: the signed custody horizon of both uploads passes without a release.
            rig.clock = 201
            await rig.relay.accept(raw, manifest, assignment, opened)
            # Then: lapsed custody stops counting but is not dropped from the journal.
            assert (await rig.relay.state()).uploads[first.digest()].receipt is not None

    anyio.run(scenario)


def test_quota_when_default_relay_takes_more_than_64_uploads(tmp_path: Path) -> None:
    # Retained uploads used to be capped at 64 for the life of the run (TRN-02, T-LV-03).
    async def scenario() -> None:
        async with httpx.AsyncClient() as client:
            rig = Rig(tmp_path, client)
            for w in range(1, 71):
                await upload_round(rig, w)
            assert len((await rig.relay.state()).uploads) == 70
            assert await rig.relay.ready()

    anyio.run(scenario)


def test_ready_stays_up_when_quota_exhausted(tmp_path: Path) -> None:
    # Given: one held reservation fills the configured quota.
    async def scenario() -> None:
        async with httpx.AsyncClient() as client:
            rig = Rig(tmp_path, client)
            rig.relay.max_uploads = 1
            grant, _, _ = await rig.accept(b"abc")
            # When: readiness is evaluated without any active request in this process.
            # Then: the replica stays routable; chunk PUTs, retention and release use the Service.
            assert await rig.relay.ready()
            with pytest.raises(RelayError, match="quota"):
                await upload_round(rig, 2)
            await rig.put(grant, b"abc")
            assert (await rig.relay.state()).uploads[grant.digest()].receipt is not None

    anyio.run(scenario)


def test_accept_when_journal_budget_exhausted(tmp_path: Path) -> None:
    # Given: a twin relay that accepted both reservations shows the journal size they need.
    async def scenario() -> None:
        async with httpx.AsyncClient() as client:
            twin = Rig(tmp_path / "twin", client)
            await twin.accept(b"abc")
            _, manifest, raw, assignment, opened = twin.upload(b"def", nonce=round_nonce(2), w=2)
            await twin.relay.accept(raw, manifest, assignment, opened)
            needed = len((await twin.relay.state()).serialize())

            rig = Rig(tmp_path / "rig", client)
            held, _, _ = await rig.accept(b"abc")
            before = len((await rig.relay.state()).serialize())
            # When: the record fits the budget but not the acknowledgment its chunk will add.
            rig.relay.journal_budget = needed
            with pytest.raises(RelayError, match="journal capacity"):
                await rig.relay.accept(raw, manifest, assignment, opened)
            # Then: nothing was written, and the held upload still completes past the budget.
            assert len((await rig.relay.state()).serialize()) == before
            await rig.put(held, b"abc")
            assert (await rig.relay.state()).uploads[held.digest()].receipt is not None
            # The limit is exact: one byte short is refused, the exact sum is accepted.
            exact = len((await rig.relay.state()).serialize()) + needed - before
            rig.relay.journal_budget = exact + relay_core.ACK_BYTES - 1
            with pytest.raises(RelayError, match="journal capacity"):
                await rig.relay.accept(raw, manifest, assignment, opened)
            rig.relay.journal_budget = exact + relay_core.ACK_BYTES
            await rig.relay.accept(raw, manifest, assignment, opened)

    anyio.run(scenario)


def test_journal_budget_when_chunk_acknowledgment_is_written(tmp_path: Path) -> None:
    # The guard reserves ACK_BYTES per chunk at accept so that the acknowledgment fits later.
    async def scenario() -> None:
        async with httpx.AsyncClient() as client:
            rig = Rig(tmp_path, client)
            data = b"a" * (4 << 20) + b"z"
            grant, _, _ = await rig.accept(data)
            before = len((await rig.relay.state()).serialize())
            await rig.put(grant, data[: 4 << 20], 0)
            grown = len((await rig.relay.state()).serialize()) - before
            assert 0 < grown <= relay_core.ACK_BYTES

    anyio.run(scenario)


def test_accept_when_default_journal_budget_nears_the_metadata_quota(tmp_path: Path) -> None:
    # StreamStore refuses a journal above 16 MiB on every write, release included: the default
    # budget must stop new uploads well before that, and must not stop them early.
    async def scenario() -> None:
        async with httpx.AsyncClient() as client:
            rig = Rig(tmp_path, client)

            def filler(megabytes: int):
                def change(state) -> None:
                    state.nonces["filler"] = "x" * (megabytes << 20)

                return change

            await rig.relay._change(filler(8))
            await upload_round(rig, 1)
            await rig.relay._change(filler(15))
            with pytest.raises(RelayError, match="journal capacity"):
                await upload_round(rig, 2)

    anyio.run(scenario)


@pytest.mark.parametrize(
    ("miners", "fits", "refused"),
    [(16, 1_200, 1_300), (32, 900, 1_000)],
    ids=["16-miners", "32-miners"],
)
def test_journal_budget_when_rounds_carry_a_roster(
    tmp_path: Path, miners: int, fits: int, refused: int
) -> None:
    # The estimate in docs/network-v2.md: a completed one-chunk upload adds about 10 KB to the
    # journal at 16 miners (the roster is stored in every record) and 13.8 KB at 32, so the 12 MiB
    # budget holds about 1,200 uploads (75 rounds of 16) and about 900 (28 rounds of 32).
    # One real record is measured, copied into the journal, and the real guard decides.
    async def scenario() -> None:
        async with httpx.AsyncClient() as client:
            rig = Rig(tmp_path, client)
            record, grown = await completed_upload(rig, miners)
            assert grown == pytest.approx({16: 10_000, 32: 13_800}[miners], rel=0.03)

            _, manifest, raw, assignment, opened = rig.upload(b"next", nonce=round_nonce(2), w=2)
            opened = with_roster(opened, miners)
            await rig.relay._change(hold_copies(record, refused))
            with pytest.raises(RelayError, match="journal capacity"):
                await rig.relay.accept(raw, manifest, assignment, opened)

            await rig.relay._change(lambda state: state.uploads.clear())
            await rig.relay._change(hold_copies(record, fits))
            await rig.relay.accept(raw, manifest, assignment, opened)

    anyio.run(scenario)


def test_readyz_when_journal_fill_crosses_eighty_percent_of_the_budget(
    tmp_path: Path, caplog: pytest.LogCaptureFixture
) -> None:
    # D53: an unattended run never blocks on the journal, and the operator is told before it fills.
    async def scenario() -> None:
        async with httpx.AsyncClient() as client:
            rig = Rig(tmp_path, client)
            # A budget whose 80 % is a whole number of bytes makes the threshold exact.
            budget = rig.relay.journal_budget = 1000
            async with httpx.AsyncClient(
                transport=httpx.ASGITransport(app=create_app(rig.relay, "t" * 32)),
                base_url="https://relay.test",
            ) as wire:
                await rig.relay._change(lambda state: state.nonces.update(filler=""))
                base = len((await rig.relay.state()).serialize())
                limit = budget * 4 // 5
                for size, alarm in ((limit, False), (limit + 1, True)):
                    await rig.relay._change(
                        lambda state, n=size - base: state.nonces.update(filler="x" * n)
                    )
                    caplog.clear()
                    answer = await wire.get("/readyz")
                    # The alarm reports; it never makes the replica unready.
                    assert answer.status_code == 200, answer.text
                    assert answer.json() == {
                        "ready": True,
                        "journal_bytes": size,
                        "journal_budget": budget,
                        "journal_fill": pytest.approx(size / budget, abs=1e-4),
                        "alarm": alarm,
                    }
                    logged = [
                        r.getMessage() for r in caplog.records if "journal fill" in r.getMessage()
                    ]
                    assert bool(logged) == alarm
                    if alarm:
                        assert logged == [f"relay journal fill 80% ({size} of {budget} bytes)"]

    anyio.run(scenario)


@pytest.mark.parametrize(
    ("miners", "fill", "alarm"),
    [(16, 0.46, False), (32, 1.26, True)],
    ids=["16-miners", "32-miners"],
)
def test_readyz_when_custody_horizon_of_rounds_has_no_early_release(
    tmp_path: Path, miners: int, fill: float, alarm: bool
) -> None:
    # Sizing in docs/network-v2.md: custody lives to its horizon, about 10 hours = 36 rounds of
    # 16.75 minutes. 16 miners x 36 rounds x 10 KB = 5.8 MB is under half of the budget; 32 miners
    # x 36 x 13.8 KB = 15.9 MB is over it, so that run needs the early release. Over budget, the
    # replica still serves what it holds and only refuses a new reservation.
    async def scenario() -> None:
        async with httpx.AsyncClient() as client:
            rig = Rig(tmp_path, client)
            record, _ = await completed_upload(rig, miners)
            await rig.relay._change(lambda state: state.uploads.clear())
            await rig.relay._change(hold_copies(record, miners * 36))
            async with httpx.AsyncClient(
                transport=httpx.ASGITransport(app=create_app(rig.relay, "t" * 32)),
                base_url="https://relay.test",
            ) as wire:
                answer = await wire.get("/readyz")
            assert answer.status_code == 200, answer.text
            assert answer.json()["journal_fill"] == pytest.approx(fill, abs=0.02)
            assert answer.json()["alarm"] == alarm
            _, manifest, raw, assignment, opened = rig.upload(b"next", nonce=round_nonce(2), w=2)
            opened = with_roster(opened, miners)
            if alarm:
                with pytest.raises(RelayError, match="journal capacity"):
                    await rig.relay.accept(raw, manifest, assignment, opened)
            else:
                await rig.relay.accept(raw, manifest, assignment, opened)

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


def test_recover_when_journal_holds_unassemblable_upload(tmp_path: Path) -> None:
    # Given: a granted manifest whose chunks are valid but concatenate to another hash.
    async def scenario() -> None:
        async with httpx.AsyncClient() as client:
            rig = Rig(tmp_path, client)
            forged = b"x" * (4 << 20) + b"y"
            grant, manifest, _, assignment, opened = rig.upload(forged, nonce="91" * 32, w=1)
            bad = UploadGrant.model_validate({**grant.body(), "delta_hash": H})
            await rig.relay.accept(rig.seal(bad), manifest, assignment, opened)
            await rig.put(bad, forged[: 4 << 20], 0)
            with pytest.raises(RelayError):
                await rig.put(bad, forged[4 << 20 :], 1)
            honest, plain, _, assignment, opened = rig.upload(b"abc", nonce="92" * 32, w=2)
            await rig.relay.accept(rig.seal(honest), plain, assignment, opened)
            await rig.put(honest, b"abc")

            def interrupted(state) -> None:
                state.uploads[honest.digest()].receipt = None

            await rig.relay._change(interrupted)
            # When: a restarted relay recovers the whole journal.
            relay = rig.new_relay()
            await relay.recover()
            completed: list[str] = []
            complete = relay.complete

            async def counted(grant_hash: str):
                completed.append(grant_hash)
                return await complete(grant_hash)

            relay.complete = counted  # type: ignore[method-assign]
            await relay.recover()
            # Then: the poisoned entry is failed once, the honest one still gets its receipt.
            state = await relay.state()
            assert state.uploads[bad.digest()].receipt is None
            assert state.uploads[bad.digest()].forward_error == "AssemblyMismatch"
            assert not rig.store.store._path(H).exists()
            assert state.uploads[honest.digest()].receipt is not None
            assert bad.digest() not in completed

    anyio.run(scenario)


async def interrupted_pair(rig: Rig) -> tuple[UploadGrant, UploadGrant]:
    # Two fully chunked uploads whose receipts were lost: recover() must complete both.
    grants = []
    for data, nonce, w in ((b"abc", "93" * 32, 1), (b"def", "94" * 32, 2)):
        grant, manifest, raw, assignment, opened = rig.upload(data, nonce=nonce, w=w)
        await rig.relay.accept(raw, manifest, assignment, opened)
        await rig.put(grant, data)
        grants.append(grant)

    def interrupted(state) -> None:
        for grant in grants:
            state.uploads[grant.digest()].receipt = None

    await rig.relay._change(interrupted)
    return grants[0], grants[1]


def failing_get(rig: Rig, sha: str, error: Exception) -> None:
    original = rig.store.get

    async def get(key, size, file):
        if key == sha:
            raise error
        await original(key, size, file)

    rig.store.get = get  # type: ignore[method-assign]


def counting(relay) -> list[str]:
    completed: list[str] = []
    complete = relay.complete

    async def counted(grant_hash: str):
        completed.append(grant_hash)
        return await complete(grant_hash)

    relay.complete = counted
    return completed


def test_recover_continues_after_object_not_found(tmp_path: Path) -> None:
    # Given: the first journal entry lost its chunk in the backing store.
    async def scenario() -> None:
        async with httpx.AsyncClient() as client:
            rig = Rig(tmp_path, client)
            lost, honest = await interrupted_pair(rig)
            failing_get(rig, hashlib.sha256(b"abc").hexdigest(), ObjectNotFound("gone"))
            # When: a restarted relay recovers the whole journal.
            relay = rig.new_relay()
            await relay.recover()
            # Then: the honest entry gets its receipt and the lost one is recorded.
            state = await relay.state()
            assert state.uploads[honest.digest()].receipt is not None
            assert state.uploads[lost.digest()].receipt is None
            assert state.uploads[lost.digest()].forward_error == "ObjectNotFound"

    anyio.run(scenario)


def test_recover_does_not_replay_corrupt_object_entry(tmp_path: Path) -> None:
    # Given: the first journal entry's held chunk is rotten.
    async def scenario() -> None:
        async with httpx.AsyncClient() as client:
            rig = Rig(tmp_path, client)
            rotten, honest = await interrupted_pair(rig)
            failing_get(rig, hashlib.sha256(b"abc").hexdigest(), CorruptObjectError("bit rot"))
            relay = rig.new_relay()
            await relay.recover()
            completed = counting(relay)
            # When: recovery runs again.
            await relay.recover()
            # Then: the honest entry recovered once and the rotten one is never retried.
            state = await relay.state()
            assert state.uploads[honest.digest()].receipt is not None
            assert state.uploads[rotten.digest()].forward_error == "CorruptObjectError"
            assert completed.count(rotten.digest()) == 0

    anyio.run(scenario)


def test_recover_retries_after_transient_error(
    tmp_path: Path, caplog: pytest.LogCaptureFixture
) -> None:
    # Given: the first journal entry's backing read times out once.
    async def scenario() -> None:
        async with httpx.AsyncClient() as client:
            rig = Rig(tmp_path, client)
            slow, honest = await interrupted_pair(rig)
            original = rig.store.get
            failing_get(rig, hashlib.sha256(b"abc").hexdigest(), TimeoutError())
            relay = rig.new_relay()
            await relay.recover()
            state = await relay.state()
            assert state.uploads[honest.digest()].receipt is not None
            assert state.uploads[slow.digest()].forward_error is None
            assert "TimeoutError" in caplog.text
            # When: the backing recovers and recovery runs again.
            rig.store.get = original  # type: ignore[method-assign]
            await relay.recover()
            # Then: the transient entry is retried and completes.
            assert (await relay.state()).uploads[slow.digest()].receipt is not None

    anyio.run(scenario)


def replacement(rig: Rig, data: bytes, replaces: str, *, hotkey: str = MINER.ss58):
    # Master-signed grant for the honest chunk list that supersedes `replaces`.
    grant, manifest, _, assignment, opened = rig.upload(data, nonce="61" * 32)
    grant = UploadGrant.model_validate({**grant.body(), "hotkey": hotkey, "replaces": replaces})
    assigned = RelayAssignment.model_validate({**assignment.body, "hotkey": hotkey})
    return grant, manifest, rig.seal(grant), rig.seal(assigned), opened


async def miner_chosen(rig: Rig, data: bytes) -> UploadGrant:
    # Accepted while the master did not hold the original: the list need not match the delta.
    grant, _, _, assignment, opened = rig.upload(data)
    manifest = rig.upload(b"x" * len(data))[1]
    grant = UploadGrant.model_validate(
        {**grant.body(), "chunk_manifest_hash": manifest.chunk_manifest_hash()}
    )
    await rig.relay.accept(rig.seal(grant), manifest, assignment, opened)
    return grant


def test_relay_accepts_replacement_after_old_accept(tmp_path: Path) -> None:
    # Given: the relay reserved the round for a miner-chosen list, then the master
    # (now holding the original) signs a replacement for the honest list.
    async def scenario() -> None:
        async with httpx.AsyncClient() as client:
            rig = Rig(tmp_path, client)
            old = await miner_chosen(rig, b"abc")
            grant, manifest, raw, assignment, opened = replacement(rig, b"abc", old.digest())
            # When: the miner presents the replacement and uploads the original bytes.
            await rig.relay.accept(raw, manifest, assignment, opened)
            await rig.put(grant, b"abc")
            # Then: the upload completes with a receipt and the old reservation is gone.
            state = await rig.relay.state()
            assert state.uploads[grant.digest()].receipt is not None
            assert old.digest() not in state.uploads
            assert old.nonce not in state.nonces

    anyio.run(scenario)


def test_relay_refuses_replacement_after_receipt(tmp_path: Path) -> None:
    async def scenario() -> None:
        async with httpx.AsyncClient() as client:
            rig = Rig(tmp_path, client)
            old, _, _ = await rig.accept(b"abc")
            await rig.put(old, b"abc")
            _, manifest, raw, assignment, opened = replacement(rig, b"abc", old.digest())
            with pytest.raises(RelayError):
                await rig.relay.accept(raw, manifest, assignment, opened)
            assert (await rig.relay.state()).uploads[old.digest()].receipt is not None

    anyio.run(scenario)


def test_relay_refuses_replacement_after_chunk_custody(tmp_path: Path) -> None:
    async def scenario() -> None:
        async with httpx.AsyncClient() as client:
            rig = Rig(tmp_path, client)
            old = await miner_chosen(rig, b"abc")
            with pytest.raises(AssemblyMismatch):  # the chunk is held under signed custody
                await rig.put(old, b"xxx")
            _, manifest, raw, assignment, opened = replacement(rig, b"abc", old.digest())
            with pytest.raises(RelayError, match="custody"):
                await rig.relay.accept(raw, manifest, assignment, opened)
            assert (await rig.relay.state()).uploads[old.digest()].chunks

    anyio.run(scenario)


def test_relay_refuses_replacement_with_wrong_replaces(tmp_path: Path) -> None:
    async def scenario() -> None:
        async with httpx.AsyncClient() as client:
            rig = Rig(tmp_path, client)
            old = await miner_chosen(rig, b"abc")
            _, manifest, raw, assignment, opened = replacement(rig, b"abc", "00" * 32)
            with pytest.raises(RelayError):
                await rig.relay.accept(raw, manifest, assignment, opened)
            assert old.digest() in (await rig.relay.state()).uploads

    anyio.run(scenario)


def test_relay_refuses_replacement_for_other_miner(tmp_path: Path) -> None:
    async def scenario() -> None:
        async with httpx.AsyncClient() as client:
            rig = Rig(tmp_path, client)
            old = await miner_chosen(rig, b"abc")
            _, manifest, raw, assignment, opened = replacement(
                rig, b"abc", old.digest(), hotkey=NEXT.ss58
            )
            with pytest.raises(RelayError):
                await rig.relay.accept(raw, manifest, assignment, opened)
            assert old.digest() in (await rig.relay.state()).uploads

    anyio.run(scenario)
