"""Coldkey-granted, revocable per-server miner keys (scope "mine" only)."""

from __future__ import annotations

from typing import Annotated, Literal

from pydantic import StringConstraints

from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import KeyError_, decode_hotkey, verify
from hypertrain.protocol.messages import SS58
from hypertrain.protocol.messages_v2 import U64, WireModel

DOMAIN = b"hypertrain/server-key/1|"
Sig = Annotated[str, StringConstraints(pattern=r"^[0-9a-f]{128}$")]
KeyId = Annotated[str, StringConstraints(pattern=r"^[0-9a-f]{32}$")]
Name = Annotated[str, StringConstraints(pattern=r"^[A-Za-z0-9_.:-]{1,64}$")]
# ponytail: "*" grants every run; the server does not scope keys per run yet.
RunId = Annotated[str, StringConstraints(pattern=r"^[A-Za-z0-9_.:*-]{1,128}$")]


class ServerKeyGrant(WireModel):
    run_id: RunId
    coldkey: SS58
    server_pub: SS58
    server_name: Name
    scope: Literal["mine"]
    key_id: KeyId
    issued_at: U64


class ServerKeyRevoke(WireModel):
    key_id: KeyId
    coldkey: SS58
    revoked_at: U64


class SignedGrant(WireModel):
    """cold_sig proves the coldkey grants; server_sig proves possession of the server key."""

    grant: ServerKeyGrant
    cold_sig: Sig
    server_sig: Sig


class SignedRevoke(WireModel):
    revoke: ServerKeyRevoke
    cold_sig: Sig


def signing_message(kind: str, body: WireModel) -> bytes:
    return DOMAIN + kind.encode() + b"|" + canonicalize(body.body(), allow_float=False)


def check_sig(ss58: str, message: bytes, sig: str) -> bool:
    try:
        return verify(decode_hotkey(ss58), message, bytes.fromhex(sig))
    except (KeyError_, ValueError):
        return False
