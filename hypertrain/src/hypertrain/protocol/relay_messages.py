"""Transport-only relay contracts; opportunity acceptance never implies byte custody."""

from __future__ import annotations

from typing import Annotated, Literal
from urllib.parse import urlsplit

from pydantic import Field, StrictBool, StrictInt, model_validator

from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages import SS58, Hex64
from hypertrain.protocol.messages_v2 import (
    MAX_OBJECT_BYTES,
    U64,
    Hashes,
    Identifier,
    Positive,
    Text,
    WireModel,
)

CHUNK_BYTES = 4 << 20
ObjectSize = Annotated[StrictInt, Field(ge=1, le=MAX_OBJECT_BYTES)]


class RelayKey(WireModel):
    key_id: Identifier
    pubkey: SS58
    valid_from_round: U64
    valid_until_round: Positive

    @model_validator(mode="after")
    def _validity(self) -> RelayKey:
        if self.valid_from_round >= self.valid_until_round:
            raise ValueError("empty relay key validity interval")
        return self


class RelaySpec(WireModel):
    id: Identifier
    region: Identifier
    https_url: Text
    pubkeys: Annotated[list[RelayKey], Field(min_length=1, max_length=16)]
    max_object_bytes: ObjectSize
    max_inflight_bytes: Positive
    codecs: Annotated[
        list[Literal["ht-sparse-v1", "ht-dense-int8-v1"]], Field(min_length=1, max_length=2)
    ]
    mode: Literal["transport"]

    @model_validator(mode="after")
    def _endpoint(self) -> RelaySpec:
        url = urlsplit(self.https_url)
        if (
            url.scheme != "https"
            or not url.hostname
            or url.username
            or url.password
            or url.query
            or url.fragment
        ):
            raise ValueError("relay endpoint must be credential-free HTTPS")
        if len({k.key_id for k in self.pubkeys}) != len(self.pubkeys):
            raise ValueError("duplicate relay key ID")
        if len(set(self.codecs)) != len(self.codecs):
            raise ValueError("duplicate codec")
        if self.max_inflight_bytes < self.max_object_bytes:
            raise ValueError("inflight limit cannot be less than object limit")
        return self


class RelayRegistryV1(WireModel):
    registry_version: Literal[1]
    epoch: U64
    previous_registry_hash: Hex64 | None
    specs: Annotated[list[RelaySpec], Field(min_length=1, max_length=128)]

    @model_validator(mode="after")
    def _ids(self) -> RelayRegistryV1:
        if len({s.id for s in self.specs}) != len(self.specs):
            raise ValueError("duplicate relay ID")
        if (self.epoch == 0) != (self.previous_registry_hash is None):
            raise ValueError("successor registry must reference predecessor")
        return self


class RelayNetworkManifest(WireModel):
    run_id: Hex64
    base_manifest_hash: Hex64
    registry_hash: Hex64
    specs_hash: Hex64
    assignment_policy: Literal["master-observed-median3-v1"]


class RelayAssignment(WireModel):
    w: U64
    hotkey: SS58
    primary_id: Identifier
    fallback_ids: Annotated[list[Identifier], Field(max_length=128)]
    assignment_epoch: U64
    manifest_hash: Hex64
    exp_drand: Positive

    @model_validator(mode="after")
    def _fallbacks(self) -> RelayAssignment:
        if self.primary_id in self.fallback_ids or len(set(self.fallback_ids)) != len(
            self.fallback_ids
        ):
            raise ValueError("relay assignment IDs must be unique")
        return self


class UploadGrant(WireModel):
    w: U64
    hotkey: SS58
    relay_id: Identifier
    assignment_epoch: U64
    delta_hash: Hex64
    size: ObjectSize
    chunk_manifest_hash: Hex64
    retain_until: Positive
    nonce: Hex64
    exp_drand: Positive

    @model_validator(mode="after")
    def _retention(self) -> UploadGrant:
        if self.retain_until <= self.exp_drand:
            raise ValueError("grant retention must outlive upload expiration")
        return self


class UploadChunk(WireModel):
    index: Annotated[StrictInt, Field(ge=0, le=511)]
    off: U64
    len: Annotated[StrictInt, Field(ge=1, le=CHUNK_BYTES)]
    chunk_sha256: Hex64


class UploadChunkManifest(WireModel):
    size: ObjectSize
    chunks: Annotated[list[UploadChunk], Field(min_length=1, max_length=512)]

    @model_validator(mode="after")
    def _coverage(self) -> UploadChunkManifest:
        offset = 0
        for index, chunk in enumerate(self.chunks):
            if chunk.index != index or chunk.off != offset:
                raise ValueError("chunks must be indexed contiguous exact coverage")
            if index < len(self.chunks) - 1 and chunk.len != CHUNK_BYTES:
                raise ValueError("nonterminal chunk must contain 4MiB")
            offset += chunk.len
        if offset != self.size:
            raise ValueError("chunk coverage does not match object size")
        return self

    def chunk_manifest_hash(self) -> str:
        """Grant binds the canonical expected chunk LIST, not a second object wrapper."""
        return sha256_hex(canonicalize([c.body() for c in self.chunks], allow_float=False))

    def validate_grant(self, grant: UploadGrant) -> None:
        if self.size != grant.size or self.chunk_manifest_hash() != grant.chunk_manifest_hash:
            raise ValueError("chunk list differs from signed grant")


class AcceptedUploadAck(WireModel):
    grant_hash: Hex64
    w: U64
    hotkey: SS58
    relay_id: Identifier
    assignment_epoch: U64
    delta_hash: Hex64
    size: ObjectSize
    chunk_manifest_hash: Hex64
    key_id: Identifier
    accepted_beacon: Positive
    service_deadline: Positive

    @model_validator(mode="after")
    def _deadline(self) -> AcceptedUploadAck:
        if self.accepted_beacon > self.service_deadline:
            raise ValueError("upload opportunity accepted after deadline")
        return self


class ChunkCustodyAck(UploadChunk):
    upload_ack_hash: Hex64
    grant_hash: Hex64
    durable_chunk_key: Text
    key_id: Identifier
    received_beacon: Positive
    retain_until: Positive

    @model_validator(mode="after")
    def _horizon(self) -> ChunkCustodyAck:
        if self.received_beacon >= self.retain_until:
            raise ValueError("chunk custody requires future retention")
        return self


class RelayReceipt(WireModel):
    w: U64
    hotkey: SS58
    delta_hash: Hex64
    size: ObjectSize
    grant_hash: Hex64
    chunk_manifest_hash: Hex64
    durable_object_key: Text
    key_id: Identifier
    received_drand: Positive
    retain_until: Positive

    @model_validator(mode="after")
    def _horizon(self) -> RelayReceipt:
        if self.received_drand >= self.retain_until:
            raise ValueError("receipt requires future retention")
        return self


class MasterAcceptanceV2(WireModel):
    w: U64
    hotkey: SS58
    delta_hash: Hex64
    receipt_hash: Hex64
    received_drand: Positive


class RetentionExtension(WireModel):
    grant_hash: Hex64
    custody_hashes: Annotated[list[Hex64], Field(min_length=1, max_length=512)]
    seq: Positive
    previous_retention_hash: Hex64
    dispute_ids: Hashes
    issued_beacon: Positive
    retain_until: Positive

    @model_validator(mode="after")
    def _horizon(self) -> RetentionExtension:
        if self.issued_beacon >= self.retain_until:
            raise ValueError("extension requires future horizon")
        if len(set(self.custody_hashes)) != len(self.custody_hashes):
            raise ValueError("duplicate extension custody scope")
        return self


class RetentionExtensionAck(WireModel):
    extension_hash: Hex64
    key_id: Identifier
    accepted_beacon: Positive


class CustodyRelease(WireModel):
    grant_hash: Hex64
    custody_hashes: Annotated[list[Hex64], Field(min_length=1, max_length=512)]
    retention_hash: Hex64
    finality_hash: Hex64
    vesting_beacon: Positive
    closed_dispute_root: Hex64
    release_beacon: Positive

    @model_validator(mode="after")
    def _vesting(self) -> CustodyRelease:
        if self.release_beacon < self.vesting_beacon:
            raise ValueError("release precedes vesting")
        if len(set(self.custody_hashes)) != len(self.custody_hashes):
            raise ValueError("duplicate release custody scope")
        return self


class RetrievalRequest(WireModel):
    request_id: Hex64
    nonce: Hex64
    relay_id: Identifier
    key_id: Identifier
    assignment_hash: Hex64
    grant_hash: Hex64
    custody_ack_hashes: Hashes
    receipt_hash: Hex64 | None
    retention_hash: Hex64
    object_or_chunk_hash: Hex64
    size: ObjectSize
    requested_beacon: Positive
    deadline_beacon: Positive

    @model_validator(mode="after")
    def _deadline(self) -> RetrievalRequest:
        if not self.requested_beacon < self.deadline_beacon <= self.requested_beacon + 10:
            raise ValueError("retrieval deadline must be in (requested, requested+10]")
        if not self.custody_ack_hashes and self.receipt_hash is None:
            raise ValueError("retrieval requires authenticated byte custody")
        if len(set(self.custody_ack_hashes)) != len(self.custody_ack_hashes):
            raise ValueError("duplicate retrieval custody scope")
        return self

    def validate_horizon(self, retain_until: int, *, released: bool = False) -> None:
        """Upload cutoff is intentionally absent from retrieval authorization."""
        if released or self.deadline_beacon != min(self.requested_beacon + 10, retain_until):
            raise ValueError("retrieval exceeds effective signed custody obligation")


class RetrievalResponse(WireModel):
    request_hash: Hex64
    nonce: Hex64
    key_id: Identifier
    status: Literal["SERVED", "NOT_FOUND", "UNAVAILABLE"]
    object_hash: Hex64
    size: ObjectSize
    served_beacon: Positive
    body_sha256: Hex64
    error_code: Identifier | None

    @model_validator(mode="after")
    def _status(self) -> RetrievalResponse:
        if (self.status == "SERVED") != (self.error_code is None):
            raise ValueError("retrieval status/error code mismatch")
        return self

    def validate_request(self, request: RetrievalRequest, payload: bytes | None) -> None:
        if (
            self.request_hash != request.digest()
            or self.nonce != request.nonce
            or self.key_id != request.key_id
            or self.object_hash != request.object_or_chunk_hash
            or self.size != request.size
            or self.served_beacon > request.deadline_beacon
            or self.served_beacon < request.requested_beacon
        ):
            raise ValueError("retrieval response differs from authenticated request")
        if self.status == "SERVED":
            if (
                payload is None
                or len(payload) != self.size
                or sha256_hex(payload) != self.body_sha256
                or self.body_sha256 != self.object_hash
            ):
                raise ValueError("served bytes fail independent hash/size verification")
        elif payload is not None:
            raise ValueError("failed retrieval must not return object bytes")


class AvailabilityContext(WireModel):
    master_healthy: StrictBool
    beacon_healthy: StrictBool
    reference_healthy: StrictBool
    observer_healthy: StrictBool


class RelayFailureEvidence(WireModel):
    run_id: Hex64
    relay_id: Identifier
    key_id: Identifier
    assignment_hash: Hex64
    grant_hash: Hex64
    upload_ack_hash: Hex64
    chunk_manifest_hash: Hex64
    chunk_ack_hashes: Hashes
    receipt_hash: Hex64 | None
    retention_hash: Hex64
    request: RetrievalRequest | None
    response: RetrievalResponse | None
    observer_ids: Annotated[list[SS58], Field(min_length=1, max_length=2)]
    observed_beacons: Annotated[list[Positive], Field(min_length=1, max_length=2)]
    result: Literal[
        "EQUIVOCATION",
        "HASH_INCONSISTENCY",
        "CONTRACTUAL_SERVICE_FAILURE",
        "UNCONFIRMED_AVAILABILITY",
    ]
    availability_context: AvailabilityContext
    evidence_hash: Hex64

    @model_validator(mode="after")
    def _observers(self) -> RelayFailureEvidence:
        if len(set(self.observer_ids)) != len(self.observer_ids):
            raise ValueError("observers must be independent identities")
        if len(self.observer_ids) != len(self.observed_beacons):
            raise ValueError("each observer needs its verified beacon observation")
        if self.result == "CONTRACTUAL_SERVICE_FAILURE":
            if self.receipt_hash is None and not self.chunk_ack_hashes:
                raise ValueError("opportunity ack cannot establish byte custody")
            if len(self.observer_ids) != 2 or not all(self.availability_context.body().values()):
                raise ValueError("attribution requires healthy independent observers")
        return self


def has_full_custody(
    grant: UploadGrant,
    manifest: UploadChunkManifest,
    opportunity: AcceptedUploadAck,
    chunks: list[ChunkCustodyAck],
    receipt: RelayReceipt | None = None,
) -> bool:
    """Exact, timely same-key coverage only. Caller verifies all signed envelopes first."""
    if (
        manifest.size != grant.size
        or manifest.chunk_manifest_hash() != grant.chunk_manifest_hash
        or opportunity.grant_hash != grant.digest()
        or opportunity.w != grant.w
        or opportunity.hotkey != grant.hotkey
        or opportunity.relay_id != grant.relay_id
        or opportunity.assignment_epoch != grant.assignment_epoch
        or opportunity.delta_hash != grant.delta_hash
        or opportunity.size != grant.size
        or opportunity.chunk_manifest_hash != grant.chunk_manifest_hash
        or opportunity.service_deadline > grant.exp_drand
    ):
        return False
    if receipt is not None:
        return (
            receipt.grant_hash == grant.digest()
            and receipt.w == grant.w
            and receipt.hotkey == grant.hotkey
            and receipt.delta_hash == grant.delta_hash
            and receipt.size == grant.size
            and receipt.chunk_manifest_hash == grant.chunk_manifest_hash
            and receipt.key_id == opportunity.key_id
            and opportunity.accepted_beacon
            <= receipt.received_drand
            <= opportunity.service_deadline
            and receipt.retain_until >= grant.retain_until
        )
    if len(chunks) != len(manifest.chunks):
        return False
    ordered = sorted(chunks, key=lambda c: c.index)
    return all(
        (ack.index, ack.off, ack.len, ack.chunk_sha256)
        == (expected.index, expected.off, expected.len, expected.chunk_sha256)
        and ack.upload_ack_hash == opportunity.digest()
        and ack.grant_hash == grant.digest()
        and ack.key_id == opportunity.key_id
        and opportunity.accepted_beacon <= ack.received_beacon <= opportunity.service_deadline
        and ack.retain_until >= grant.retain_until
        for ack, expected in zip(ordered, manifest.chunks, strict=True)
    )


def extend_retention(
    extension: RetentionExtension,
    ack: RetentionExtensionAck,
    *,
    grant_hash: str,
    custody_hashes: list[str],
    key_id: str,
    previous_hash: str,
    previous_seq: int,
    previous_horizon: int,
) -> int:
    """Apply only an acknowledged, scoped successor signed before prior expiry."""
    if (
        extension.grant_hash != grant_hash
        or set(extension.custody_hashes) != set(custody_hashes)
        or extension.previous_retention_hash != previous_hash
        or extension.seq != previous_seq + 1
        or extension.retain_until <= previous_horizon
        or not extension.issued_beacon <= ack.accepted_beacon < previous_horizon
        or ack.extension_hash != extension.digest()
        or ack.key_id != key_id
    ):
        raise ValueError("unacknowledged, late or conflicting retention extension")
    return extension.retain_until


def validate_release(
    release: CustodyRelease,
    *,
    grant_hash: str,
    custody_hashes: list[str],
    retention_hash: str,
    retain_until: int,
    finality_beacon: int,
    disputes_closed_beacon: int | None,
    epoch_rounds: int,
    retrieval_deadlines: list[int],
) -> int:
    """Return earliest deletion beacon; future lanes persist finality and release evidence."""
    if (
        disputes_closed_beacon is None
        or epoch_rounds < 1
        or release.grant_hash != grant_hash
        or release.retention_hash != retention_hash
        or set(release.custody_hashes) != set(custody_hashes)
        or release.release_beacon
        < max(finality_beacon, release.vesting_beacon, disputes_closed_beacon) + epoch_rounds
    ):
        raise ValueError("release requires closed disputes, finality, vesting and epoch grace")
    return max(release.release_beacon, retain_until, *retrieval_deadlines)


RELAY_MESSAGE_TYPES: dict[str, type[WireModel]] = {
    cls.__name__: cls
    for cls in (
        RelayRegistryV1,
        RelayNetworkManifest,
        RelayAssignment,
        UploadGrant,
        UploadChunkManifest,
        AcceptedUploadAck,
        ChunkCustodyAck,
        RelayReceipt,
        MasterAcceptanceV2,
        RetentionExtension,
        RetentionExtensionAck,
        CustodyRelease,
        RetrievalRequest,
        RetrievalResponse,
        RelayFailureEvidence,
    )
}
