"""Read, sign and verify owner-pinned trust roots."""

import tomllib
from hashlib import sha256
from pathlib import Path

from .crypto import TRUST_ROOT_DOMAIN, decode_hotkey, verify_raw
from .models import ChallengeEntry, ParticipantPolicy, TrustRoot
from .scale import ProtocolError, byte_vec, fixed, uint, vector


def _signature(path: Path) -> bytes:
    raw = path.read_bytes()
    if len(raw) == 64:
        return raw
    try:
        return fixed(bytes.fromhex(raw.decode().strip().removeprefix("0x")), 64)
    except (ValueError, UnicodeDecodeError) as error:
        raise ProtocolError("invalid detached signature") from error


def _policy(value: str | dict) -> ParticipantPolicy:
    if value == "all_metagraph_hotkeys":
        return ParticipantPolicy()
    if not isinstance(value, dict):
        raise ProtocolError("invalid participant policy")
    kind = value.get("type")
    if kind == "all_metagraph_hotkeys":
        return ParticipantPolicy()
    if kind == "stake_at_least":
        return ParticipantPolicy(1, min_stake=value["min_stake"])
    if kind in ("explicit_allowlist", "all_except_deny_list"):
        return ParticipantPolicy(
            2 if kind == "explicit_allowlist" else 3,
            hotkeys=tuple(decode_hotkey(k) for k in value["hotkeys"]),
        )
    raise ProtocolError("unknown participant policy")


def trust_payload(version: int, introduced_epoch: int, body: bytes) -> bytes:
    return uint(version, 4) + uint(introduced_epoch, 8) + byte_vec(body)


def challenge_entries(document: dict) -> tuple[ChallengeEntry, ...]:
    """Decode the challenge body exactly as it is signed on the wire."""
    return tuple(
        ChallengeEntry(
            row["id"].encode(),
            decode_hotkey(row["public_key"]),
            row["emission_share_bps"],
            _policy(row["policy"]),
        )
        for row in document["challenges"]
    )


def measurements_body(document: dict) -> bytes:
    """Encode the measurement allowlist exactly as it is signed on the wire."""
    entries = []
    for row in document.get("measurements", []):
        fields = []
        for name in ("mr_td", "rtmr0", "rtmr1", "rtmr2", "rtmr3", "compose_hash"):
            fields.append(
                fixed(
                    bytes.fromhex(row[name].removeprefix("0x")),
                    32 if name == "compose_hash" else 48,
                )
            )
        entries.append(b"".join(fields))
    return vector(entries, lambda value: value)


def signing_payload(path: Path, kind: str) -> bytes:
    """Return the detached-signature preimage for an operator document."""
    document = tomllib.loads(path.read_text())
    if kind == "challenges":
        challenges = challenge_entries(document)
        body = vector(challenges, ChallengeEntry.encode)
    elif kind == "measurements":
        body = measurements_body(document)
    else:
        raise ProtocolError("unknown trust document kind")
    return trust_payload(document["version"], document.get("introduced_epoch", 0), body)


def load_trust_root(
    *,
    challenges_path: Path,
    challenges_signature: Path,
    measurements_path: Path,
    measurements_signature: Path,
    owner_public: bytes,
    gateway_public: bytes,
    epoch: int,
    minimum_challenges_version: int = 1,
    minimum_measurements_version: int = 1,
) -> TrustRoot:
    """Owner key and minimum versions are local pins, never fetched from the gateway."""
    challenges_doc = tomllib.loads(challenges_path.read_text())
    measurements_doc = tomllib.loads(measurements_path.read_text())
    challenges = challenge_entries(challenges_doc)
    measurement_bytes = measurements_body(measurements_doc)
    trust = TrustRoot(
        challenges,
        sha256(measurement_bytes).digest(),
        gateway_public,
        challenges_doc["version"],
        measurements_doc["version"],
        max(challenges_doc.get("introduced_epoch", 0), measurements_doc.get("introduced_epoch", 0)),
    )
    for document, body, signature_path, minimum in (
        (challenges_doc, trust.challenges_body(), challenges_signature, minimum_challenges_version),
        (measurements_doc, measurement_bytes, measurements_signature, minimum_measurements_version),
    ):
        version = document["version"]
        introduced = document.get("introduced_epoch", 0)
        if version < minimum or introduced > epoch:
            raise ProtocolError("trust root rollback or future activation")
        if not verify_raw(
            owner_public,
            TRUST_ROOT_DOMAIN,
            trust_payload(version, introduced, body),
            _signature(signature_path),
        ):
            raise ProtocolError("invalid trust root owner signature")
    return trust
