"""Bounded canonical v2 signing and intake; durable replay receipts belong to L0 storage."""

from __future__ import annotations

import json
from collections.abc import Callable, Mapping
from dataclasses import dataclass, field
from typing import Literal

from pydantic import BaseModel, ConfigDict, Field, JsonValue, ValidationError

from hypertrain.protocol.envelope import (
    ExpiredError,
    MalformedEnvelope,
    ReplayError,
    RunMismatchError,
    SignatureError,
    UnauthorizedSigner,
    body_digest,
)
from hypertrain.protocol.jcs import CanonicalizationError, canonicalize
from hypertrain.protocol.keys import KeyError_, Keypair, decode_hotkey, verify
from hypertrain.protocol.messages import SS58, Hex64
from hypertrain.protocol.messages_v2 import (
    MAX_BODY_BYTES,
    MAX_MANIFEST_BYTES,
    MESSAGE_TYPES_V2,
    AuditJobV2,
    Identifier,
    JoinRequest,
    Positive,
    RunManifestV2,
)

VERSION = "ht/2"
DOMAIN = b"hypertrain/2|"
MAX_DEPTH = 32
MAX_ARRAY = 1_000_000
SIGNER_BOUND_TYPES = {"AcceptV2", "CommitV2", "DeltaManifestV2", "StateServe", "DisputeV2"}


class SignedEnvelope(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)
    v: str
    type: Identifier
    run_id: Hex64
    body: dict[str, JsonValue]
    signer: SS58
    exp_drand: Positive
    sig: str = Field(pattern=r"^[0-9a-f]{128}$")


class EnvelopeV2(SignedEnvelope):
    v: Literal["ht/2"]


def _pairs(pairs: list[tuple[str, JsonValue]]) -> dict[str, JsonValue]:
    result: dict[str, JsonValue] = {}
    for key, value in pairs:
        if key in result:
            raise MalformedEnvelope("duplicate JSON field")
        result[key] = value
    return result


def _reject_number(value: str) -> JsonValue:
    raise MalformedEnvelope(f"raw float/nonfinite JSON number: {value}")


def _bounded(value: JsonValue, depth: int = 0) -> None:
    if depth > MAX_DEPTH:
        raise MalformedEnvelope("message nesting exceeds 32")
    match value:
        case dict():
            if len(value) > 4096:
                raise MalformedEnvelope("object exceeds field bound")
            for key, child in value.items():
                if len(key) > 4096:
                    raise MalformedEnvelope("field name exceeds bound")
                _bounded(child, depth + 1)
        case list():
            if len(value) > MAX_ARRAY:
                raise MalformedEnvelope("array exceeds bound")
            for child in value:
                _bounded(child, depth + 1)
        case str():
            if len(value) > 4096:
                raise MalformedEnvelope("string exceeds bound")
        case float():
            raise MalformedEnvelope("raw floats forbidden; use f32 hex")
        case int():
            if not isinstance(value, bool) and not 0 <= value <= 2**53 - 1:
                raise MalformedEnvelope("integer exceeds canonical unsigned bound")
        case None:
            return


def load_json(raw: bytes | str, *, max_bytes: int = MAX_MANIFEST_BYTES) -> dict[str, JsonValue]:
    """Reject duplicates and floats before schema dispatch, with bounded parser depth."""
    data = raw.encode("utf-8") if isinstance(raw, str) else raw
    if len(data) > max_bytes:
        raise MalformedEnvelope("message exceeds byte bound")
    # Bound nesting before json.loads can recurse; ignore brackets inside strings.
    depth, quoted, escaped = 0, False, False
    for byte in data:
        if quoted:
            if escaped:
                escaped = False
            elif byte == 92:
                escaped = True
            elif byte == 34:
                quoted = False
        elif byte == 34:
            quoted = True
        elif byte in (91, 123):
            depth += 1
            if depth > MAX_DEPTH:
                raise MalformedEnvelope("message nesting exceeds 32")
        elif byte in (93, 125):
            depth -= 1
    try:
        value: JsonValue = json.loads(
            data,
            object_pairs_hook=_pairs,
            parse_float=_reject_number,
            parse_constant=_reject_number,
        )
    except (json.JSONDecodeError, UnicodeDecodeError) as error:
        raise MalformedEnvelope("invalid UTF-8 JSON") from error
    if not isinstance(value, dict):
        raise MalformedEnvelope("message must be an object")
    _bounded(value)
    return value


def _validate_body(
    msg_type: str,
    data: dict[str, JsonValue],
    models: Mapping[str, type[BaseModel]],
) -> BaseModel:
    model_type = models.get(msg_type)
    if model_type is None:
        raise MalformedEnvelope("unsupported message type")
    _bounded(data)
    limit = (
        MAX_MANIFEST_BYTES
        if msg_type in {"RunManifestV2", "DeltaManifestV2", "UploadChunkManifest", "AuditJobV2"}
        else MAX_BODY_BYTES
    )
    if len(canonicalize(data, allow_float=False)) > limit:
        raise MalformedEnvelope("body exceeds message byte bound")
    try:
        model = model_type.model_validate(data)
    except ValidationError as error:
        raise MalformedEnvelope(f"invalid {msg_type} body") from error
    if canonicalize(model.model_dump(mode="json"), allow_float=False) != canonicalize(
        data, allow_float=False
    ):
        raise MalformedEnvelope("body differs from canonical validated model")
    return model


def _signing_message(
    domain: bytes,
    msg_type: str,
    body: Mapping[str, JsonValue],
    exp_drand: int,
    run_id: str,
) -> bytes:
    if type(exp_drand) is not int or not 1 <= exp_drand <= 2**53 - 1:
        raise MalformedEnvelope("expiration must be a strict canonical positive integer")
    return (
        domain
        + msg_type.encode("ascii")
        + b"|"
        + body_digest(body).encode("ascii")
        + b"|"
        + str(exp_drand).encode("ascii")
        + b"|"
        + run_id.encode("ascii")
    )


def signing_message(
    msg_type: str,
    body: Mapping[str, JsonValue],
    exp_drand: int,
    run_id: str,
) -> bytes:
    return _signing_message(DOMAIN, msg_type, body, exp_drand, run_id)


def _seal(
    keypair: Keypair,
    msg_type: str,
    run_id: str,
    body: BaseModel | dict[str, JsonValue],
    exp: int,
    *,
    version: str,
    domain: bytes,
    models: Mapping[str, type[BaseModel]],
) -> dict[str, JsonValue]:
    data = body.model_dump(mode="json") if isinstance(body, BaseModel) else body
    _validate_body(msg_type, data, models)
    env: dict[str, JsonValue] = {
        "v": version,
        "type": msg_type,
        "run_id": run_id,
        "body": data,
        "signer": keypair.ss58,
        "exp_drand": exp,
        "sig": keypair.sign(_signing_message(domain, msg_type, data, exp, run_id)).hex(),
    }
    return env


def seal(
    keypair: Keypair,
    msg_type: str,
    run_id: str,
    body: BaseModel | dict[str, JsonValue],
    exp: int,
) -> dict[str, JsonValue]:
    env = _seal(
        keypair,
        msg_type,
        run_id,
        body,
        exp,
        version=VERSION,
        domain=DOMAIN,
        models=MESSAGE_TYPES_V2,
    )
    _parse(env, EnvelopeV2, MESSAGE_TYPES_V2)
    return env


def _parse(
    raw: bytes | str | dict[str, JsonValue],
    envelope_type: type[SignedEnvelope],
    models: Mapping[str, type[BaseModel]],
) -> tuple[SignedEnvelope, BaseModel]:
    data = (
        load_json(raw, max_bytes=MAX_MANIFEST_BYTES + 1024) if isinstance(raw, bytes | str) else raw
    )
    _bounded(data)
    if len(canonicalize(data, allow_float=False)) > MAX_MANIFEST_BYTES + 1024:
        raise MalformedEnvelope("envelope exceeds bound")
    try:
        env = envelope_type.model_validate(data)
    except ValidationError as error:
        raise MalformedEnvelope("invalid versioned envelope") from error
    return env, _validate_body(env.type, env.body, models)


def parse_envelope(raw: bytes | str | dict[str, JsonValue]) -> EnvelopeV2:
    env, _ = _parse(raw, EnvelopeV2, MESSAGE_TYPES_V2)
    assert isinstance(env, EnvelopeV2)
    return env


def _verify(env: SignedEnvelope, domain: bytes) -> bool:
    return verify(
        decode_hotkey(env.signer),
        _signing_message(domain, env.type, env.body, env.exp_drand, env.run_id),
        bytes.fromhex(env.sig),
    )


def verify_envelope(raw: bytes | str | dict[str, JsonValue]) -> bool:
    try:
        return _verify(parse_envelope(raw), DOMAIN)
    except (MalformedEnvelope, KeyError_, CanonicalizationError):
        return False


def replay_key(msg_type: str, run_id: str, signer: str, model: BaseModel) -> tuple[str, ...]:
    """Semantic reservation ID, independent of signature randomness and expiration."""
    fields: dict[str, tuple[str, ...]] = {
        "DisputeV2": ("verdict_hash",),
        "BisectV2": ("dispute_id", "seq", "party"),
        "ResolutionV2": ("dispute_id",),
        "JoinRequest": ("coldkey", "request_id"),
        "WorkProof": ("admission_id", "challenge_hash"),
        "WorkScreenV2": ("admission_id", "nonce"),
        "JoinChallenge": ("admission_id", "nonce"),
        "StateServe": ("challenge_hash", "t"),
        "EscrowLock": ("operation_id",),
        "EscrowRelease": ("operation_id",),
        "EscrowTransfer": ("operation_id",),
        "EscrowReceipt": ("operation_id",),
        "RotateRequest": ("operation_id",),
        "TestGenesis": ("run_id",),
        "RewardFinalize": ("w",),
        "AuditJobV2": ("job_id", "attempt"),
        "AuditChallengeV2": ("w", "target"),
        "ReplayVerdict": ("challenge_hash",),
        "Receipt": ("w", "commit_hash"),
        "RelayAssignment": ("w", "hotkey", "assignment_epoch"),
        "UploadGrant": ("w", "hotkey", "assignment_epoch"),
        "AcceptedUploadAck": ("grant_hash",),
        "ChunkCustodyAck": ("grant_hash", "index"),
        "RelayReceipt": ("grant_hash",),
        "MasterAcceptanceV2": ("w", "hotkey", "delta_hash"),
        "RetentionExtension": ("grant_hash", "seq"),
        "RetentionExtensionAck": ("extension_hash",),
        "CustodyRelease": ("grant_hash",),
        "RetrievalRequest": ("request_id",),
        "RetrievalResponse": ("request_hash",),
        "RelayFailureEvidence": ("evidence_hash",),
        "RelayRegistryV1": ("epoch",),
    }
    names = fields.get(msg_type)
    if names is None:
        names = tuple(name for name in ("w", "hotkey", "target") if hasattr(model, name))
    subject = tuple(str(getattr(model, name)) for name in names) if names else ("immutable",)
    return (run_id, msg_type, signer, *subject)


@dataclass
class Intake:
    """Mutable in-memory reservations; caller restores/persists them transactionally."""

    run_id: str
    allowed_signers: Mapping[str, Callable[[str], bool]] = field(default_factory=dict)
    reservations: dict[tuple[str, ...], str] = field(default_factory=dict)

    def accept(self, raw: bytes | str | dict[str, JsonValue], now_round: int) -> BaseModel:
        return self._accept(raw, now_round, EnvelopeV2, MESSAGE_TYPES_V2, DOMAIN)

    def _accept(
        self,
        raw: bytes | str | dict[str, JsonValue],
        now_round: int,
        envelope_type: type[SignedEnvelope],
        models: Mapping[str, type[BaseModel]],
        domain: bytes,
    ) -> BaseModel:
        env, model = _parse(raw, envelope_type, models)
        if env.run_id != self.run_id:
            raise RunMismatchError("envelope belongs to another run")
        if type(now_round) is not int or not 1 <= now_round <= 2**53 - 1:
            raise MalformedEnvelope("now_round must be strict positive beacon round")
        if not _verify(env, domain):
            raise SignatureError("sr25519 signature does not verify")
        if now_round > env.exp_drand:
            raise ExpiredError("envelope expired")
        check = self.allowed_signers.get(env.type)
        if check is not None and not check(env.signer):
            raise UnauthorizedSigner("signer is not authorized for message role")
        model_body = model.model_dump(mode="json")
        if env.type in SIGNER_BOUND_TYPES and model_body["hotkey"] != env.signer:
            raise UnauthorizedSigner("hotkey differs from signer")
        if env.type == "BisectV2" and model_body["party"] != env.signer:
            raise UnauthorizedSigner("bisection party differs from signer")
        body_run = model_body.get("run_id", env.run_id)
        if body_run != env.run_id:
            raise RunMismatchError("embedded run ID differs from envelope")
        if isinstance(model, RunManifestV2) and model.run_id() != env.run_id:
            raise RunMismatchError("manifest wrapper hash differs from envelope")
        if isinstance(model, AuditJobV2):
            model.validate_embedded(now_round)
        key = replay_key(env.type, env.run_id, env.signer, model)
        digest = body_digest(env.body)
        existing = self.reservations.get(key)
        if existing is not None and existing != digest:
            raise ReplayError("conflicting body for reserved replay ID")
        self.reservations[key] = digest
        return model


def join_signing_message(request: JoinRequest) -> bytes:
    data = request.model_dump(mode="json", exclude={"hot_sig", "cold_sig"})
    return _signing_message(
        b"hypertrain/join/2|", "JoinRequest", data, request.expires_beacon, request.run_id
    )


def verify_join(request: JoinRequest, run_id: str, now_round: int) -> bool:
    if request.run_id != run_id or not 1 <= now_round <= request.expires_beacon:
        return False
    try:
        msg = join_signing_message(request)
        return verify(
            decode_hotkey(request.hotkey), msg, bytes.fromhex(request.hot_sig)
        ) and verify(decode_hotkey(request.coldkey), msg, bytes.fromhex(request.cold_sig))
    except (KeyError_, CanonicalizationError):
        return False


def tape_signing_message(body: Mapping[str, JsonValue], run_id: str) -> bytes:
    """L3 owns tape schemas/arithmetic; this is the exact approved signing preimage."""
    return b"hypertrain/tape/2|" + body_digest(body).encode("ascii") + b"|" + run_id.encode("ascii")
