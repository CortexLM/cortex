"""Relay domain reuses bounded canonical intake, never ht/1 or ht/2 signatures."""

from __future__ import annotations

from typing import Literal

from pydantic import BaseModel, JsonValue

from hypertrain.protocol.envelope import MalformedEnvelope
from hypertrain.protocol.envelope_v2 import Intake as TrainingIntake
from hypertrain.protocol.envelope_v2 import (
    SignedEnvelope,
    _parse,
    _seal,
    _signing_message,
    _verify,
)
from hypertrain.protocol.jcs import CanonicalizationError
from hypertrain.protocol.keys import KeyError_, Keypair
from hypertrain.protocol.relay_messages import RELAY_MESSAGE_TYPES

VERSION = "ht-relay/1"
DOMAIN = b"hypertrain/relay/1|"


class RelayEnvelope(SignedEnvelope):
    v: Literal["ht-relay/1"]


def signing_message(
    msg_type: str,
    body: dict[str, JsonValue],
    exp_drand: int,
    run_id: str,
) -> bytes:
    return _signing_message(DOMAIN, msg_type, body, exp_drand, run_id)


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
        models=RELAY_MESSAGE_TYPES,
    )
    _parse(env, RelayEnvelope, RELAY_MESSAGE_TYPES)
    return env


def parse_envelope(raw: bytes | str | dict[str, JsonValue]) -> RelayEnvelope:
    env, _ = _parse(raw, RelayEnvelope, RELAY_MESSAGE_TYPES)
    assert isinstance(env, RelayEnvelope)
    return env


def verify_envelope(raw: bytes | str | dict[str, JsonValue]) -> bool:
    try:
        return _verify(parse_envelope(raw), DOMAIN)
    except (MalformedEnvelope, KeyError_, CanonicalizationError):
        return False


class Intake(TrainingIntake):
    def accept(self, raw: bytes | str | dict[str, JsonValue], now_round: int) -> BaseModel:
        return self._accept(raw, now_round, RelayEnvelope, RELAY_MESSAGE_TYPES, DOMAIN)
