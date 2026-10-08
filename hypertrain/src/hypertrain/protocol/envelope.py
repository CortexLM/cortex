"""Signed envelopes {v, type, run_id, body, signer, exp_drand, sig} and replay-protected intake."""

from __future__ import annotations

import re
from collections.abc import Callable, Mapping
from dataclasses import dataclass, field
from typing import Any

from pydantic import BaseModel, ValidationError

from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import CanonicalizationError, canonicalize
from hypertrain.protocol.keys import KeyError_, Keypair, decode_hotkey, verify
from hypertrain.protocol.messages import MESSAGE_TYPES

VERSION = "ht/1"
DOMAIN = b"hypertrain/1|"
_HEX64 = re.compile(r"^[0-9a-f]{64}$")
_ENVELOPE_KEYS = {"v", "type", "run_id", "body", "signer", "exp_drand", "sig"}


class EnvelopeError(ValueError):
    pass


class MalformedEnvelope(EnvelopeError):
    pass


class SignatureError(EnvelopeError):
    pass


class ExpiredError(EnvelopeError):
    pass


class ReplayError(EnvelopeError):
    pass


class RunMismatchError(EnvelopeError):
    pass


class UnauthorizedSigner(EnvelopeError):
    pass


def body_digest(body: Mapping[str, Any]) -> str:
    return sha256_hex(canonicalize(body, allow_float=False))


def signing_message(msg_type: str, body: Mapping[str, Any], exp_drand: int, run_id: str) -> bytes:
    # ponytail: run_id appended after the spec'd fields so a body signed for one run never
    # verifies in another (advisory section 6 test 12); spec preimage alone omits run_id.
    return (
        DOMAIN
        + msg_type.encode()
        + b"|"
        + body_digest(body).encode()
        + b"|"
        + str(exp_drand).encode()
        + b"|"
        + run_id.encode()
    )


def seal(
    keypair: Keypair, msg_type: str, run_id: str, body: BaseModel | Mapping[str, Any], exp: int
) -> dict[str, Any]:
    if msg_type not in MESSAGE_TYPES:
        raise MalformedEnvelope(f"unknown message type {msg_type!r}")
    data = body.model_dump(mode="json") if isinstance(body, BaseModel) else dict(body)
    MESSAGE_TYPES[msg_type].model_validate(data)
    sig = keypair.sign(signing_message(msg_type, data, exp, run_id))
    return {
        "v": VERSION,
        "type": msg_type,
        "run_id": run_id,
        "body": data,
        "signer": keypair.ss58,
        "exp_drand": exp,
        "sig": sig.hex(),
    }


def _shape(env: Any) -> None:
    if not isinstance(env, Mapping) or set(env) != _ENVELOPE_KEYS:
        raise MalformedEnvelope("envelope must have exactly the ht/1 fields")
    if env["v"] != VERSION:
        raise MalformedEnvelope("unsupported envelope version")
    if env["type"] not in MESSAGE_TYPES:
        raise MalformedEnvelope("unknown message type")
    if not isinstance(env["run_id"], str) or not _HEX64.match(env["run_id"]):
        raise MalformedEnvelope("run_id must be lowercase sha256 hex")
    if not isinstance(env["body"], Mapping):
        raise MalformedEnvelope("body must be an object")
    if type(env["exp_drand"]) is not int or env["exp_drand"] < 1:
        raise MalformedEnvelope("exp_drand must be a positive integer round")
    if not isinstance(env["sig"], str) or not re.fullmatch(r"[0-9a-f]{128}", env["sig"]):
        raise MalformedEnvelope("sig must be 64-byte lowercase hex")
    if not isinstance(env["signer"], str):
        raise MalformedEnvelope("signer must be ss58")


def verify_envelope(env: Any) -> bool:
    """Signature check only (no expiry/replay/run checks)."""
    try:
        _shape(env)
        public = decode_hotkey(env["signer"])
        msg = signing_message(env["type"], env["body"], env["exp_drand"], env["run_id"])
    except (EnvelopeError, KeyError_, CanonicalizationError):
        return False
    return verify(public, msg, bytes.fromhex(env["sig"]))


SIGNER_BOUND_TYPES = {"Commit", "Accept", "DeltaManifest", "StateServe", "Dispute"}
_DECISION_KEY = {"Dispute": "verdict_hash"}


def replay_key(msg_type: str, run_id: str, signer: str, model: BaseModel) -> tuple[str, ...]:
    """From the validated model: (run_id, w, type, signer, subject); one Dispute per verdict."""
    if msg_type in _DECISION_KEY:
        return (run_id, "-", msg_type, signer, str(getattr(model, _DECISION_KEY[msg_type])))
    w = getattr(model, "w", None)
    if not isinstance(w, int):
        return (run_id, "-", msg_type, signer, body_digest(model.model_dump(mode="json")))
    subject = next(
        (str(getattr(model, a)) for a in ("hotkey", "target", "commit_hash") if hasattr(model, a)),
        "",
    )
    return (run_id, str(w), msg_type, signer, subject)


@dataclass
class Intake:
    """Stateful verifier for one run. `seen` must be persisted by the caller for crash safety."""

    run_id: str
    allowed_signers: Mapping[str, Callable[[str], bool]] = field(default_factory=dict)
    seen: set[tuple[str, ...]] = field(default_factory=set)

    def accept(self, env: Any, now_round: int) -> BaseModel:
        _shape(env)
        if env["run_id"] != self.run_id:
            raise RunMismatchError("envelope run_id does not match this run")
        if not verify_envelope(env):
            raise SignatureError("sr25519 signature does not verify")
        if now_round > env["exp_drand"]:
            raise ExpiredError(f"expired at drand round {env['exp_drand']} (now {now_round})")
        check = self.allowed_signers.get(env["type"])
        if check is not None and not check(env["signer"]):
            raise UnauthorizedSigner(f"{env['signer']} may not sign {env['type']}")
        try:
            model = MESSAGE_TYPES[env["type"]].model_validate(env["body"])
        except ValidationError as error:
            raise MalformedEnvelope(
                f"invalid {env['type']} body: {error.error_count()} errors"
            ) from error
        if canonicalize(model.model_dump(mode="json"), allow_float=False) != canonicalize(
            env["body"], allow_float=False
        ):
            raise MalformedEnvelope("body is not in canonical model form")
        if env["type"] in SIGNER_BOUND_TYPES and model.model_dump()["hotkey"] != env["signer"]:
            raise UnauthorizedSigner("body hotkey must equal the envelope signer")
        key = replay_key(env["type"], env["run_id"], env["signer"], model)
        if key in self.seen:
            raise ReplayError("duplicate (run_id, w, type, signer) message")
        self.seen.add(key)
        return model
