"""Read and (de)serialize the gateway trust profile; the gateway is the source of truth."""

import json
import tomllib
from dataclasses import asdict
from hashlib import sha256
from pathlib import Path

from .crypto import decode_hotkey
from .models import ChallengeEntry, ParticipantPolicy, TrustRoot
from .scale import ProtocolError, fixed, vector


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


def challenge_entries(document: dict) -> tuple[ChallengeEntry, ...]:
    """Decode the challenge rows (id, share, participant policy)."""
    return tuple(
        ChallengeEntry(
            row["id"].encode(),
            row["emission_share_bps"],
            _policy(row["policy"]),
        )
        for row in document["challenges"]
    )


def measurements_body(document: dict) -> bytes:
    """Encode the measurement allowlist; its sha256 is the profile's measurements digest."""
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


def load_trust(*, challenges_path: Path, measurements_path: Path) -> TrustRoot:
    """Build the profile from the master's own TOML files. No signature: the files are local."""
    challenges_doc = tomllib.loads(challenges_path.read_text())
    measurements_doc = tomllib.loads(measurements_path.read_text())
    trust = TrustRoot(
        challenge_entries(challenges_doc),
        sha256(measurements_body(measurements_doc)).digest(),
        challenges_doc["version"],
        measurements_doc["version"],
        max(challenges_doc.get("introduced_epoch", 0), measurements_doc.get("introduced_epoch", 0)),
    )
    trust.validate()
    return trust


def trust_to_json(trust: TrustRoot) -> dict:
    """Public profile served at /v1/trust and persisted by the gateway store."""
    return json.loads(json.dumps(asdict(trust), default=bytes.hex))


def trust_from_json(values: object) -> TrustRoot:
    try:
        if not isinstance(values, dict):
            raise TypeError("profile must be an object")
        values = dict(values)
        challenges = []
        for entry in values.pop("challenges"):
            policy = dict(entry["policy"])
            policy["hotkeys"] = tuple(bytes.fromhex(key) for key in policy["hotkeys"])
            challenges.append(
                ChallengeEntry(
                    bytes.fromhex(entry["id"]),
                    entry["emission_share_bps"],
                    ParticipantPolicy(**policy),
                )
            )
        values["measurements_digest"] = bytes.fromhex(values["measurements_digest"])
        trust = TrustRoot(challenges=tuple(challenges), **values)
        trust.validate()
        return trust
    except (KeyError, TypeError, ValueError) as error:
        raise ProtocolError("invalid trust profile") from error
