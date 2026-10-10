"""Durable transport custody, shared replica journal, pinned signature authority.

Metadata is an internal journal, never a replacement wire schema. One CAS includes
reservations, original signed acknowledgments, and state changes. No local cache
is authoritative. Missing master routes leave forwarding pending, never accepted.
"""

from __future__ import annotations

import hashlib
import secrets
from collections.abc import AsyncIterator, Awaitable, Callable
from contextlib import asynccontextmanager
from dataclasses import dataclass
from typing import BinaryIO, Literal

import anyio
import httpx
from pydantic import BaseModel, ConfigDict, Field

from hypertrain.data.store import CorruptObjectError, ObjectNotFound
from hypertrain.data.stream_store import StreamStore, receive, spool
from hypertrain.protocol import envelope_v2, relay_envelope
from hypertrain.protocol.envelope import SignatureError
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages_v2 import RoundOpenV2
from hypertrain.protocol.relay_envelope import RelayEnvelope
from hypertrain.protocol.relay_messages import (
    AcceptedUploadAck,
    ChunkCustodyAck,
    CustodyRelease,
    MasterAcceptanceV2,
    RelayAssignment,
    RelayFailureEvidence,
    RelayReceipt,
    RelayRegistryV1,
    RelaySpec,
    RetentionExtension,
    RetentionExtensionAck,
    RetrievalRequest,
    RetrievalResponse,
    UploadChunkManifest,
    UploadGrant,
    WireModel,
    extend_retention,
    has_full_custody,
    validate_release,
)


class RelayError(ValueError):
    """Authenticated contract, authorization, quota or lifecycle rejection."""


@dataclass(frozen=True, slots=True)
class Settlement:
    """Authenticated L0 journal facts, not caller-provided JSON or a wire replacement."""

    finality_hash: str
    finality_beacon: int
    closed_dispute_root: str
    closed_beacon: int | None
    vesting_beacon: int


class Pin(BaseModel):
    horizon: int
    retention_hash: str
    seq: int = 0
    disputes: list[str] = Field(default_factory=list)
    delete_after: int | None = None


class RetrievalRecord(BaseModel):
    request: RelayEnvelope
    response: RelayEnvelope


class ExtensionRecord(BaseModel):
    extension: RelayEnvelope
    acknowledgment: RelayEnvelope


class UploadRecord(BaseModel):
    grant: RelayEnvelope
    assignment: RelayEnvelope
    round_open: envelope_v2.EnvelopeV2
    manifest: UploadChunkManifest
    opportunity: RelayEnvelope
    key_public: str
    chunks: dict[str, RelayEnvelope] = Field(default_factory=dict)
    receipt: RelayEnvelope | None = None
    master_acceptance: RelayEnvelope | None = None
    extensions: dict[str, ExtensionRecord] = Field(default_factory=dict)
    releases: dict[str, RelayEnvelope] = Field(default_factory=dict)
    pins: dict[str, Pin] = Field(default_factory=dict)
    retrievals: dict[str, RetrievalRecord] = Field(default_factory=dict)
    forward_error: str | None = None


class Journal(BaseModel):
    model_config = ConfigDict(extra="forbid")
    uploads: dict[str, UploadRecord] = Field(default_factory=dict)
    nonces: dict[str, str] = Field(default_factory=dict)
    failures: dict[str, RelayEnvelope] = Field(default_factory=dict)
    disabled_keys: list[str] = Field(default_factory=list)
    publications: dict[str, list[str]] = Field(default_factory=dict)
    deletions: dict[str, str] = Field(default_factory=dict)

    @classmethod
    def parse(cls, data: bytes) -> Journal:
        return cls.model_validate_json(data) if data else cls()

    def serialize(self) -> bytes:
        return self.model_dump_json().encode()


class Relay:
    """Mutable service coordinator; durable state always reread through shared CAS."""

    def __init__(
        self,
        *,
        run_id: str,
        master: str,
        registry: RelayRegistryV1,
        network_manifest_hash: str,
        observers: set[str],
        relay_id: str,
        region: str,
        keys: dict[str, Keypair],
        active_key: str,
        backing: StreamStore,
        now: Callable[[], Awaitable[int]],
        forward: Callable[[RelayEnvelope], Awaitable[RelayEnvelope]] | None = None,
        settlement: Callable[[str], Awaitable[Settlement]] | None = None,
        epoch_rounds: int = 100,
        max_uploads: int = 64,
    ) -> None:
        specs = [s for s in registry.specs if s.id == relay_id and s.region == region]
        if len(specs) != 1 or active_key not in keys:
            raise RelayError("relay region/key absent from pinned registry")
        self.run_id, self.master, self.registry = run_id, master, registry
        self.network_manifest_hash = network_manifest_hash
        self.observers = frozenset(observers)
        self.spec: RelaySpec = specs[0]
        self.keys, self.active_key, self.backing = keys, active_key, backing
        self.now, self.forward = now, forward
        self.settlement = settlement
        self.epoch_rounds, self.max_uploads = epoch_rounds, max_uploads
        self.draining = False
        self.inflight = 0
        self.changed = anyio.Event()
        self.journal_key = hashlib.sha256(f"relay-journal|{run_id}|{relay_id}".encode()).hexdigest()
        self.limiter = anyio.CapacityLimiter(4)
        for key in self.spec.pubkeys:
            if key.key_id in keys and keys[key.key_id].ss58 != key.pubkey:
                raise RelayError("private signing key does not match registry")

    async def state(self) -> Journal:
        data, _ = await self.backing.journal(self.journal_key)
        return Journal.parse(data)

    async def _change(self, change: Callable[[Journal], None]) -> Journal:
        state = await self.backing.mutate(
            self.journal_key, Journal.parse, Journal.serialize, change
        )
        event, self.changed = self.changed, anyio.Event()
        event.set()
        return state

    def master_message[T: WireModel](self, raw: RelayEnvelope, cls: type[T], now: int) -> T:
        if (
            raw.run_id != self.run_id
            or raw.signer != self.master
            or raw.exp_drand < now
            or raw.type != cls.__name__
            or not relay_envelope.verify_envelope(raw.model_dump())
        ):
            raise SignatureError("wrong relay master authority/domain/run/type/expiry")
        return cls.model_validate(raw.body)

    def _seal(self, model: WireModel, key_id: str, horizon: int) -> RelayEnvelope:
        return relay_envelope.parse_envelope(
            relay_envelope.seal(
                self.keys[key_id], type(model).__name__, self.run_id, model, horizon
            )
        )

    def _record(self, state: Journal, grant_hash: str) -> UploadRecord:
        try:
            return state.uploads[grant_hash]
        except KeyError:
            raise RelayError("unknown upload grant") from None

    def _key(self, now: int, state: Journal) -> str:
        key_id = self.active_key
        if key_id in state.disabled_keys or not any(
            k.key_id == key_id and k.valid_from_round <= now < k.valid_until_round
            for k in self.spec.pubkeys
        ):
            raise RelayError("active key revoked or outside signed validity interval")
        return key_id

    async def accept(
        self,
        raw: RelayEnvelope,
        manifest: UploadChunkManifest,
        assignment_raw: RelayEnvelope,
        round_raw: envelope_v2.EnvelopeV2,
    ) -> RelayEnvelope:
        now = await self.now()
        grant = self.master_message(raw, UploadGrant, now)
        assignment = self.master_message(assignment_raw, RelayAssignment, now)
        if (
            round_raw.run_id != self.run_id
            or round_raw.signer != self.master
            or round_raw.type != "RoundOpenV2"
            or round_raw.exp_drand < now
            or not envelope_v2.verify_envelope(round_raw.model_dump())
        ):
            raise SignatureError("upload deadline requires signed RoundOpenV2")
        opened = RoundOpenV2.model_validate(round_raw.body)
        manifest.validate_grant(grant)
        if (
            grant.relay_id != self.spec.id
            or grant.assignment_epoch != self.registry.epoch
            or opened.registry_epoch != self.registry.epoch
            or opened.w != grant.w
            or assignment.w != grant.w
            or assignment.hotkey != grant.hotkey
            or assignment.assignment_epoch != grant.assignment_epoch
            or assignment.exp_drand < now
            or assignment.manifest_hash != self.network_manifest_hash
            or self.spec.id not in [assignment.primary_id, *assignment.fallback_ids]
            or grant.size > self.spec.max_object_bytes
        ):
            raise RelayError("grant is not bound to this region/assignment/round/size")
        deadline = min(grant.exp_drand, opened.d_upload)
        if now > deadline:
            raise RelayError("upload deadline passed")
        grant_hash = grant.digest()

        def commit(state: Journal) -> None:
            if grant_hash in state.uploads:
                existing = state.uploads[grant_hash]
                if existing.manifest != manifest or existing.assignment.body != assignment_raw.body:
                    raise RelayError("conflicting immutable upload")
                return
            if self.draining:
                raise RelayError("relay draining; new grants disabled")
            key_id = self._key(now, state)
            if len(state.uploads) >= self.max_uploads:
                raise RelayError("retained upload quota exhausted")
            reserved = sum(
                UploadGrant.model_validate(u.grant.body).size
                for u in state.uploads.values()
                if u.receipt is None
                and AcceptedUploadAck.model_validate(u.opportunity.body).service_deadline >= now
            )
            if reserved + grant.size > self.spec.max_inflight_bytes:
                raise RelayError("shared inflight byte quota exhausted")
            subject = f"{grant.w}|{grant.hotkey}"
            if any(
                f"{u.grant.body['w']}|{u.grant.body['hotkey']}" == subject
                and u.grant.body != raw.body
                for u in state.uploads.values()
            ):
                raise RelayError("conflicting run/round/miner reservation")
            if grant.nonce in state.nonces:
                raise RelayError("grant nonce already reserved")
            state.nonces[grant.nonce] = grant_hash
            ack = AcceptedUploadAck(
                grant_hash=grant_hash,
                w=grant.w,
                hotkey=grant.hotkey,
                relay_id=grant.relay_id,
                assignment_epoch=grant.assignment_epoch,
                delta_hash=grant.delta_hash,
                size=grant.size,
                chunk_manifest_hash=grant.chunk_manifest_hash,
                key_id=key_id,
                accepted_beacon=now,
                service_deadline=deadline,
            )
            state.uploads[grant_hash] = UploadRecord(
                grant=raw,
                assignment=assignment_raw,
                round_open=round_raw,
                manifest=manifest,
                opportunity=self._seal(ack, key_id, grant.retain_until),
                key_public=self.keys[key_id].ss58,
            )

        state = await self._change(commit)
        return state.uploads[grant_hash].opportunity

    @asynccontextmanager
    async def _publication(self, sha: str) -> AsyncIterator[None]:
        """Durable CAS exclusion from before write through custody acknowledgment.

        Abandoned deletion generations never expire: an old physical delete could
        still execute. Recovery requires fencing that operation, not a timed lease.
        """
        generation = secrets.token_hex(32)

        def reserve(state: Journal) -> None:
            if sha in state.deletions:
                raise RelayError("object has an outstanding durable deletion reservation")
            state.publications.setdefault(sha, []).append(generation)

        await self._change(reserve)
        try:
            yield
        finally:

            def release(state: Journal) -> None:
                owners = state.publications[sha]
                owners.remove(generation)
                if not owners:
                    del state.publications[sha]

            with anyio.fail_after(30, shield=True):
                await self._change(release)

    async def chunk(
        self, grant_hash: str, index: int, source: AsyncIterator[bytes]
    ) -> RelayEnvelope:
        record = self._record(await self.state(), grant_hash)
        if not 0 <= index < len(record.manifest.chunks):
            raise RelayError("chunk index invalid")
        async with self._publication(record.manifest.chunks[index].chunk_sha256):
            return await self._chunk(grant_hash, index, source)

    async def _chunk(
        self, grant_hash: str, index: int, source: AsyncIterator[bytes]
    ) -> RelayEnvelope:
        async with self.limiter:
            now = await self.now()
            record = self._record(await self.state(), grant_hash)
            grant = UploadGrant.model_validate(record.grant.body)
            ack = AcceptedUploadAck.model_validate(record.opportunity.body)
            if now > ack.service_deadline or not 0 <= index < len(record.manifest.chunks):
                raise RelayError("chunk index/deadline invalid")
            expected = record.manifest.chunks[index]
            if self.inflight + expected.len > self.spec.max_inflight_bytes:
                raise RelayError("active stream byte quota exhausted")
            with spool() as file:
                self.inflight += expected.len
                try:
                    await receive(source, file, expected.len, expected.chunk_sha256)
                    await self.backing.put(file, expected.chunk_sha256, expected.len)
                finally:
                    self.inflight -= expected.len
            received = await self.now()
            if received > ack.service_deadline:
                raise RelayError("chunk persisted after agreed deadline; no timely custody")
            custody = ChunkCustodyAck(
                index=expected.index,
                off=expected.off,
                len=expected.len,
                chunk_sha256=expected.chunk_sha256,
                upload_ack_hash=ack.digest(),
                grant_hash=grant_hash,
                durable_chunk_key=expected.chunk_sha256,
                key_id=ack.key_id,
                received_beacon=received,
                retain_until=grant.retain_until,
            )
            signed = self._seal(custody, ack.key_id, grant.retain_until)

            def commit(state: Journal) -> None:
                target = self._record(state, grant_hash)
                if str(index) not in target.chunks:
                    target.chunks[str(index)] = signed
                    target.pins[custody.digest()] = Pin(
                        horizon=grant.retain_until, retention_hash=custody.digest()
                    )

            state = await self._change(commit)
            target = state.uploads[grant_hash]
            if len(target.chunks) == len(target.manifest.chunks):
                await self.complete(grant_hash)
            return target.chunks[str(index)]

    async def complete(self, grant_hash: str) -> RelayEnvelope:
        record = self._record(await self.state(), grant_hash)
        grant = UploadGrant.model_validate(record.grant.body)
        async with self._publication(grant.delta_hash):
            return await self._complete(grant_hash)

    async def _complete(self, grant_hash: str) -> RelayEnvelope:
        record = self._record(await self.state(), grant_hash)
        grant = UploadGrant.model_validate(record.grant.body)
        opportunity = AcceptedUploadAck.model_validate(record.opportunity.body)
        if record.receipt is None:
            chunks = [ChunkCustodyAck.model_validate(e.body) for e in record.chunks.values()]
            if not has_full_custody(grant, record.manifest, opportunity, chunks):
                raise RelayError("completion requires exact timely full custody")
            with spool() as whole:
                for chunk in record.manifest.chunks:
                    with spool() as file:
                        await self.backing.get(chunk.chunk_sha256, chunk.len, file)
                        while data := file.read(64 << 10):
                            whole.write(data)
                whole.seek(0)
                await self.backing.put(whole, grant.delta_hash, grant.size)
            now = await self.now()
            if now > opportunity.service_deadline:
                raise RelayError("complete receipt cannot backdate missed upload deadline")
            if now >= grant.retain_until:
                raise RelayError("custody horizon expired before complete receipt")
            receipt = RelayReceipt(
                w=grant.w,
                hotkey=grant.hotkey,
                delta_hash=grant.delta_hash,
                size=grant.size,
                grant_hash=grant_hash,
                chunk_manifest_hash=grant.chunk_manifest_hash,
                durable_object_key=grant.delta_hash,
                key_id=opportunity.key_id,
                received_drand=now,
                retain_until=grant.retain_until,
            )
            signed = self._seal(receipt, opportunity.key_id, grant.retain_until)

            def commit(state: Journal) -> None:
                target = self._record(state, grant_hash)
                if target.receipt is None:
                    target.receipt = signed
                    target.pins[receipt.digest()] = Pin(
                        horizon=grant.retain_until, retention_hash=receipt.digest()
                    )

            record = (await self._change(commit)).uploads[grant_hash]
        assert record.receipt is not None
        if record.master_acceptance is None and self.forward is not None:
            try:
                acceptance_raw = await self.forward(record.receipt)
            except httpx.HTTPError as error:
                failure_name = type(error).__name__

                def pending(state: Journal) -> None:
                    self._record(state, grant_hash).forward_error = failure_name

                await self._change(pending)
                raise
            accepted = self.master_message(acceptance_raw, MasterAcceptanceV2, await self.now())
            receipt = RelayReceipt.model_validate(record.receipt.body)
            if (
                accepted.w != grant.w
                or accepted.hotkey != grant.hotkey
                or accepted.delta_hash != grant.delta_hash
                or accepted.receipt_hash != receipt.digest()
                or accepted.received_drand > opportunity.service_deadline
            ):
                raise RelayError("master acceptance differs from original receipt/deadline")

            def forwarded(state: Journal) -> None:
                target = self._record(state, grant_hash)
                if target.master_acceptance is None:
                    target.master_acceptance = acceptance_raw
                    target.forward_error = None

            await self._change(forwarded)
        return record.receipt

    async def recover(self) -> None:
        """Startup/event-triggered recovery; no invented successful master service."""
        for grant_hash, record in (await self.state()).uploads.items():
            if (
                len(record.chunks) == len(record.manifest.chunks)
                and record.master_acceptance is None
            ):
                try:
                    await self.complete(grant_hash)
                except RelayError as error:
                    # A missed completion deadline must not prevent retained-byte retrieval.
                    failure_name = type(error).__name__

                    def pending(
                        state: Journal,
                        grant_hash: str = grant_hash,
                        failure_name: str = failure_name,
                    ) -> None:
                        self._record(state, grant_hash).forward_error = failure_name

                    await self._change(pending)
                except httpx.HTTPError:
                    # Exact failure recorded by complete; custody remains retrievable.
                    continue

    def _scope(self, record: UploadRecord, hashes: list[str]) -> list[Pin]:
        if (
            not hashes
            or len(set(hashes)) != len(hashes)
            or any(h not in record.pins for h in hashes)
        ):
            raise RelayError("unknown exact custody scope")
        return [record.pins[h] for h in hashes]

    def _lineage(self, pins: list[Pin], hashes: list[str]) -> str:
        if len(pins) > 1 and all(p.seq == 0 for p in pins):
            return hashlib.sha256(canonicalize(sorted(hashes), allow_float=False)).hexdigest()
        first = pins[0]
        if any((p.seq, p.retention_hash) != (first.seq, first.retention_hash) for p in pins):
            raise RelayError("mixed retention lineage")
        return first.retention_hash

    async def retention(self, raw: RelayEnvelope) -> RelayEnvelope:
        now = await self.now()
        extension = self.master_message(raw, RetentionExtension, now)

        def commit(state: Journal) -> None:
            record = self._record(state, extension.grant_hash)
            if extension.digest() in record.extensions:
                return
            pins = self._scope(record, extension.custody_hashes)
            if any(p.delete_after is not None for p in pins):
                raise RelayError("released custody cannot be extended")
            first = pins[0]
            previous_hash = self._lineage(pins, extension.custody_hashes)
            key_id = AcceptedUploadAck.model_validate(record.opportunity.body).key_id
            ack = RetentionExtensionAck(
                extension_hash=extension.digest(), key_id=key_id, accepted_beacon=now
            )
            horizon = extend_retention(
                extension,
                ack,
                grant_hash=extension.grant_hash,
                custody_hashes=extension.custody_hashes,
                key_id=key_id,
                previous_hash=previous_hash,
                previous_seq=first.seq,
                previous_horizon=min(p.horizon for p in pins),
            )
            record.extensions[extension.digest()] = ExtensionRecord(
                extension=raw, acknowledgment=self._seal(ack, key_id, horizon)
            )
            for pin in pins:
                pin.horizon, pin.seq, pin.retention_hash = (
                    horizon,
                    extension.seq,
                    extension.digest(),
                )
                pin.disputes = extension.dispute_ids

        record = (await self._change(commit)).uploads[extension.grant_hash]
        return record.extensions[extension.digest()].acknowledgment

    async def release(self, raw: RelayEnvelope) -> RelayEnvelope:
        """L0 settlement adapter reads authenticated finality/dispute/vesting journal."""
        now = await self.now()
        release = self.master_message(raw, CustodyRelease, now)
        if self.settlement is None:
            raise RelayError("authenticated L0 settlement journal not configured")
        settled = await self.settlement(release.grant_hash)
        if (
            settled.finality_hash != release.finality_hash
            or settled.closed_dispute_root != release.closed_dispute_root
            or settled.vesting_beacon != release.vesting_beacon
        ):
            raise RelayError("release lacks matching authenticated finality/closure evidence")

        def commit(state: Journal) -> None:
            record = self._record(state, release.grant_hash)
            pins = self._scope(record, release.custody_hashes)
            if self._lineage(pins, release.custody_hashes) != release.retention_hash:
                raise RelayError("release mixed retention lineage")
            deadlines = [
                RetrievalRequest.model_validate(r.request.body).deadline_beacon
                for r in record.retrievals.values()
            ]
            bound = validate_release(
                release,
                grant_hash=release.grant_hash,
                custody_hashes=release.custody_hashes,
                retention_hash=release.retention_hash,
                retain_until=max(p.horizon for p in pins),
                finality_beacon=settled.finality_beacon,
                disputes_closed_beacon=settled.closed_beacon,
                epoch_rounds=self.epoch_rounds,
                retrieval_deadlines=deadlines,
            )
            if now < release.release_beacon:
                raise RelayError("release grace has not elapsed")
            record.releases[release.digest()] = raw
            for pin in pins:
                pin.delete_after = bound
                pin.disputes = []

        await self._change(commit)
        return raw

    async def retrieve(self, raw: RelayEnvelope) -> RelayEnvelope:
        now = await self.now()
        request = self.master_message(raw, RetrievalRequest, now)
        record = self._record(await self.state(), request.grant_hash)
        opportunity = AcceptedUploadAck.model_validate(record.opportunity.body)
        if (
            request.relay_id != self.spec.id
            or request.key_id != opportunity.key_id
            or request.assignment_hash
            != RelayAssignment.model_validate(record.assignment.body).digest()
            or not request.requested_beacon <= now <= request.deadline_beacon
        ):
            raise RelayError("retrieval wrong assignment/key/beacon")
        hashes = [*request.custody_ack_hashes]
        if request.receipt_hash is not None:
            hashes.append(request.receipt_hash)
        pins = self._scope(record, hashes)
        if self._lineage(pins, hashes) != request.retention_hash:
            raise RelayError("retrieval wrong signed retention lineage")
        request.validate_horizon(
            min(p.horizon for p in pins), released=any(p.delete_after is not None for p in pins)
        )
        grant = UploadGrant.model_validate(record.grant.body)
        exact = False
        if request.receipt_hash is not None and record.receipt is not None:
            receipt = RelayReceipt.model_validate(record.receipt.body)
            exact = receipt.digest() == request.receipt_hash and (
                request.object_or_chunk_hash,
                request.size,
            ) == (grant.delta_hash, grant.size)
        if request.custody_ack_hashes:
            chunks = [
                ChunkCustodyAck.model_validate(e.body)
                for e in record.chunks.values()
                if ChunkCustodyAck.model_validate(e.body).digest() in request.custody_ack_hashes
            ]
            exact |= (
                len(chunks) == 1
                and (request.object_or_chunk_hash, request.size)
                == (chunks[0].chunk_sha256, chunks[0].len)
            ) or (
                has_full_custody(grant, record.manifest, opportunity, chunks)
                and (request.object_or_chunk_hash, request.size) == (grant.delta_hash, grant.size)
            )
        if not exact:
            raise RelayError("retrieval bytes not covered by exact signed custody")
        previous = record.retrievals.get(request.request_id)
        if previous is not None:
            if previous.request.body != raw.body:
                raise RelayError("conflicting retrieval reservation")
            return previous.response
        status: Literal["SERVED", "NOT_FOUND", "UNAVAILABLE"] = "SERVED"
        error: str | None = None
        try:
            with spool() as file:
                await self.backing.get(request.object_or_chunk_hash, request.size, file)
        except ObjectNotFound:
            status, error = "NOT_FOUND", "missing-custody"
        except CorruptObjectError:
            status, error = "UNAVAILABLE", "corrupt-custody"
        served = await self.now()
        if served > request.deadline_beacon:
            raise RelayError("retrieval deadline elapsed")
        response = RetrievalResponse(
            request_hash=request.digest(),
            nonce=request.nonce,
            key_id=request.key_id,
            status=status,
            object_hash=request.object_or_chunk_hash,
            size=request.size,
            served_beacon=served,
            body_sha256=request.object_or_chunk_hash,
            error_code=error,
        )
        signed = self._seal(response, request.key_id, min(p.horizon for p in pins))

        def commit(state: Journal) -> None:
            target = self._record(state, request.grant_hash)
            current_pins = self._scope(target, hashes)
            if self._lineage(current_pins, hashes) != request.retention_hash:
                raise RelayError("retrieval retention changed during backing read")
            request.validate_horizon(
                min(p.horizon for p in current_pins),
                released=any(p.delete_after is not None for p in current_pins),
            )
            if request.request_id in target.retrievals:
                if target.retrievals[request.request_id].request.body != raw.body:
                    raise RelayError("conflicting retrieval reservation")
                return
            if request.nonce in state.nonces:
                raise RelayError("retrieval nonce replay")
            if any(
                RetrievalRequest.model_validate(r.request.body).object_or_chunk_hash
                == request.object_or_chunk_hash
                and RetrievalRequest.model_validate(r.request.body).deadline_beacon >= served
                for r in target.retrievals.values()
            ):
                raise RelayError("one outstanding challenge per exact object/key")
            if len(target.retrievals) >= 512:
                raise RelayError("retrieval journal quota exhausted")
            state.nonces[request.nonce] = request.digest()
            target.retrievals[request.request_id] = RetrievalRecord(request=raw, response=signed)

        target = (await self._change(commit)).uploads[request.grant_hash]
        return target.retrievals[request.request_id].response

    async def authorize_object(self, sha: str, request_hash: str) -> int:
        """Only a persisted signed nonce challenge opens an object stream."""
        now = await self.now()
        for record in (await self.state()).uploads.values():
            for retrieval in record.retrievals.values():
                request = RetrievalRequest.model_validate(retrieval.request.body)
                response = RetrievalResponse.model_validate(retrieval.response.body)
                if (
                    request.digest() == request_hash
                    and request.object_or_chunk_hash == sha
                    and response.status == "SERVED"
                    and now <= request.deadline_beacon
                ):
                    pins = self._scope(
                        record,
                        [
                            *request.custody_ack_hashes,
                            *([request.receipt_hash] if request.receipt_hash else []),
                        ],
                    )
                    if not any(p.delete_after is not None for p in pins):
                        return request.size
        raise RelayError("object stream requires live authenticated custody challenge")

    async def ready(self) -> bool:
        if self.draining or self.inflight >= self.spec.max_inflight_bytes:
            return False
        now = await self.now()
        state = await self.state()
        self._key(now, state)
        reserved = sum(
            UploadGrant.model_validate(record.grant.body).size
            for record in state.uploads.values()
            if record.receipt is None
            and AcceptedUploadAck.model_validate(record.opportunity.body).service_deadline >= now
        )
        if len(state.uploads) >= self.max_uploads or reserved >= self.spec.max_inflight_bytes:
            return False
        # Persisted CAS proves write privilege; independent object probe proves backing readability.
        await self._change(lambda state: None)
        with spool() as file:
            file.write(b"relay-durable-probe-v1")
            sha = hashlib.sha256(b"relay-durable-probe-v1").hexdigest()
            await self.backing.put(file, sha, 22)
        with spool() as file:
            await self.backing.get(sha, 22, file)
        return True

    async def acks(self, grant_hash: str, cursor: int = 0) -> list[RelayEnvelope]:
        record = self._record(await self.state(), grant_hash)
        return [record.chunks[str(i)] for i in sorted(map(int, record.chunks)) if i >= cursor]

    def _eligible_deletions(self, state: Journal, now: int) -> list[str]:
        """Evidence remains pinned without authenticated release, even past horizon."""
        candidates: set[str] = set()
        protected: set[str] = set()
        for record in state.uploads.values():
            entries = [
                (
                    ChunkCustodyAck.model_validate(e.body).digest(),
                    ChunkCustodyAck.model_validate(e.body).chunk_sha256,
                )
                for e in record.chunks.values()
            ]
            if record.receipt is not None:
                receipt = RelayReceipt.model_validate(record.receipt.body)
                entries.append((receipt.digest(), receipt.delta_hash))
            for digest, sha in entries:
                pin = record.pins[digest]
                if pin.delete_after is not None and now >= pin.delete_after and not pin.disputes:
                    candidates.add(sha)
                else:
                    protected.add(sha)
        return sorted(candidates - protected - state.publications.keys() - state.deletions.keys())

    async def eligible_deletions(self) -> list[str]:
        return self._eligible_deletions(await self.state(), await self.now())

    async def collect(self) -> list[str]:
        """CAS reserves current eligibility through physical delete across replicas."""
        now = await self.now()
        generation = secrets.token_hex(32)

        def reserve(state: Journal) -> None:
            for sha in self._eligible_deletions(state, now):
                state.deletions[sha] = generation

        reserved = await self._change(reserve)
        eligible = [sha for sha, owner in reserved.deletions.items() if owner == generation]
        for sha in eligible:
            await self.backing.delete(sha)

            def finish(state: Journal, sha: str = sha) -> None:
                if state.deletions.get(sha) != generation:
                    raise RelayError("deletion generation changed before physical completion")
                del state.deletions[sha]

            await self._change(finish)
        return eligible

    async def failure(
        self,
        raw: RelayEnvelope,
        artifacts: list[RelayEnvelope],
        observations: list[RelayEnvelope],
        payload: BinaryIO | None = None,
    ) -> RelayFailureEvidence:
        """L0 authenticates observer health. Relay verifies signed custody/equivocation.

        This records key-specific service failures; never malicious intent/miner slash.
        """
        now = await self.now()
        evidence = self.master_message(raw, RelayFailureEvidence, now)
        record = self._record(await self.state(), evidence.grant_hash)
        grant = UploadGrant.model_validate(record.grant.body)
        opportunity = AcceptedUploadAck.model_validate(record.opportunity.body)
        if (
            evidence.run_id != self.run_id
            or evidence.relay_id != self.spec.id
            or evidence.key_id != opportunity.key_id
            or evidence.assignment_hash
            != RelayAssignment.model_validate(record.assignment.body).digest()
            or evidence.upload_ack_hash != opportunity.digest()
            or evidence.chunk_manifest_hash != grant.chunk_manifest_hash
            or max(evidence.observed_beacons) > now
            or evidence.evidence_hash
            != hashlib.sha256(
                canonicalize([e.model_dump(mode="json") for e in artifacts], allow_float=False)
            ).hexdigest()
        ):
            raise RelayError("failure evidence binding mismatch")
        for artifact in artifacts:
            if artifact.run_id != self.run_id or not relay_envelope.verify_envelope(
                artifact.model_dump()
            ):
                raise SignatureError("invalid evidence signature")
        if evidence.result == "CONTRACTUAL_SERVICE_FAILURE":
            authenticated = {
                observation.signer
                for observation in observations
                if observation.signer in self.observers
                and observation.run_id == self.run_id
                and observation.type == "RelayFailureEvidence"
                and observation.body == raw.body
                and relay_envelope.verify_envelope(observation.model_dump())
            }
            if authenticated != set(evidence.observer_ids) or len(authenticated) != 2:
                raise SignatureError("service failure needs two pinned observer signatures")
        verified = False
        match evidence.result:
            case "UNCONFIRMED_AVAILABILITY":
                verified = True
            case "EQUIVOCATION":
                for left in artifacts:
                    for right in artifacts:
                        if (
                            left.signer == right.signer == record.key_public
                            and left.type == right.type
                            and left.type
                            in {"RelayReceipt", "ChunkCustodyAck", "AcceptedUploadAck"}
                            and left.body.get("grant_hash")
                            == right.body.get("grant_hash")
                            == grant.digest()
                            and left.body.get("index") == right.body.get("index")
                            and left.body != right.body
                        ):
                            verified = True
            case "HASH_INCONSISTENCY":
                request, response = evidence.request, evidence.response
                if request is not None and response is not None and payload is not None:
                    signed_response = [
                        e
                        for e in artifacts
                        if e.type == "RetrievalResponse"
                        and e.signer == record.key_public
                        and e.body == response.body()
                    ]
                    served = record.retrievals.get(request.request_id)
                    payload.seek(0)
                    digest, size = hashlib.sha256(), 0
                    while data := payload.read(64 << 10):
                        size += len(data)
                        if size > request.size:
                            break
                        digest.update(data)
                    verified = (
                        bool(signed_response)
                        and served is not None
                        and served.request.body == request.body()
                        and response.status == "SERVED"
                        and response.request_hash == request.digest()
                        and response.nonce == request.nonce
                        and response.key_id == request.key_id
                        and request.requested_beacon
                        <= response.served_beacon
                        <= request.deadline_beacon
                        and (
                            response.body_sha256 != request.object_or_chunk_hash
                            or size != request.size
                            or digest.hexdigest() != response.body_sha256
                        )
                    )
            case "CONTRACTUAL_SERVICE_FAILURE":
                custody_scope = [
                    *evidence.chunk_ack_hashes,
                    *([evidence.receipt_hash] if evidence.receipt_hash else []),
                ]
                obligated = self._scope(record, custody_scope)
                if (
                    self._lineage(obligated, custody_scope) != evidence.retention_hash
                    or any(p.delete_after is not None for p in obligated)
                    or max(evidence.observed_beacons) > min(p.horizon for p in obligated)
                ):
                    raise RelayError("failure observed outside exact signed retention obligation")
                chunks = [
                    ChunkCustodyAck.model_validate(e.body)
                    for e in record.chunks.values()
                    if ChunkCustodyAck.model_validate(e.body).digest() in evidence.chunk_ack_hashes
                ]
                receipt = (
                    RelayReceipt.model_validate(record.receipt.body)
                    if record.receipt is not None
                    and evidence.receipt_hash
                    == RelayReceipt.model_validate(record.receipt.body).digest()
                    else None
                )
                if evidence.request is None:
                    verified = (
                        has_full_custody(grant, record.manifest, opportunity, chunks, receipt)
                        and min(evidence.observed_beacons) > opportunity.service_deadline
                        and record.master_acceptance is None
                    )
                else:
                    request = evidence.request
                    if (
                        request.grant_hash != evidence.grant_hash
                        or request.assignment_hash != evidence.assignment_hash
                        or request.relay_id != evidence.relay_id
                        or request.key_id != evidence.key_id
                        or request.custody_ack_hashes != evidence.chunk_ack_hashes
                        or request.receipt_hash != evidence.receipt_hash
                        or request.retention_hash != evidence.retention_hash
                    ):
                        raise RelayError("retrieval failure differs from exact requested custody")
                    served = record.retrievals.get(request.request_id)
                    verified = (
                        served is not None
                        and served.request.body == request.body()
                        and evidence.response is not None
                        and served.response.body == evidence.response.body()
                        and evidence.response.status != "SERVED"
                        and min(evidence.observed_beacons) > request.deadline_beacon
                    )
        if not verified:
            raise RelayError("failure lacks exact authenticated byte-custody predicate")

        def commit(state: Journal) -> None:
            state.failures[evidence.digest()] = raw
            if (
                evidence.result != "UNCONFIRMED_AVAILABILITY"
                and evidence.key_id not in state.disabled_keys
            ):
                state.disabled_keys.append(evidence.key_id)

        await self._change(commit)
        return evidence
