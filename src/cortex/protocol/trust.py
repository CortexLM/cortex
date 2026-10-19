"""Read and (de)serialize the gateway trust profile; the gateway is the source of truth."""

import json
import tomllib
from dataclasses import asdict
from hashlib import sha256
from pathlib import Path

from .crypto import decode_hotkey
from .models import ChallengeEntry, ParticipantPolicy, TrustRoot
from .scale import ProtocolError, fixed, vector


def _policy(value: object) -> ParticipantPolicy:
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
        hotkeys = value.get("hotkeys")
        if not isinstance(hotkeys, list) or not all(isinstance(k, str) for k in hotkeys):
            raise ProtocolError("participant policy hotkeys must be an array of strings")
        return ParticipantPolicy(
            2 if kind == "explicit_allowlist" else 3,
            hotkeys=tuple(decode_hotkey(k) for k in hotkeys),
        )
    raise ProtocolError("unknown participant policy")


def _row(row: object) -> ChallengeEntry:
    if not isinstance(row, dict):
        raise ProtocolError("challenge row must be a table")
    key, share = row.get("id"), row.get("emission_share_bps")
    if not isinstance(key, str):
        raise ProtocolError(f"challenge id must be a string, got {key!r}")
    if type(share) is not int or not 0 <= share <= 10000:
        raise ProtocolError(f"emission_share_bps must be an integer in 0..10000, got {share!r}")
    return ChallengeEntry(key.encode(), share, _policy(row.get("policy")))


def challenge_entries(document: dict) -> tuple[ChallengeEntry, ...]:
    """Decode the challenge rows (id, share, participant policy)."""
    rows = document.get("challenges")
    if not isinstance(rows, list):
        raise ProtocolError("challenges must be an array of tables")
    return tuple(_row(row) for row in rows)


def measurements_body(document: dict) -> bytes:
    """Encode the measurement allowlist; its sha256 is the profile's measurements digest."""
    entries = []
    rows = document.get("measurements", [])
    if not isinstance(rows, list) or not all(isinstance(row, dict) for row in rows):
        raise ProtocolError("measurements must be an array of tables")
    for row in rows:
        fields = []
        for name in ("mr_td", "rtmr0", "rtmr1", "rtmr2", "rtmr3", "compose_hash"):
            if not isinstance(row.get(name), str):
                raise ProtocolError(f"measurement {name} must be a hex string")
            fields.append(
                fixed(
                    bytes.fromhex(row[name].removeprefix("0x")),
                    32 if name == "compose_hash" else 48,
                )
            )
        entries.append(b"".join(fields))
    return vector(entries, lambda value: value)


def _whole(document: dict, name: str, default: int | None = None) -> int:
    value = document.get(name, default)
    if type(value) is not int or value < 0:
        raise ProtocolError(f"{name} must be a non-negative integer, got {value!r}")
    return value


def load_trust(*, challenges_path: Path, measurements_path: Path) -> TrustRoot:
    """Build the profile from the master's own TOML files. No signature: the files are local."""
    challenges_doc = tomllib.loads(challenges_path.read_text())
    measurements_doc = tomllib.loads(measurements_path.read_text())
    trust = TrustRoot(
        challenge_entries(challenges_doc),
        sha256(measurements_body(measurements_doc)).digest(),
        _whole(challenges_doc, "version"),
        _whole(measurements_doc, "version"),
        max(
            _whole(challenges_doc, "introduced_epoch", 0),
            _whole(measurements_doc, "introduced_epoch", 0),
        ),
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
