"""Unsigned operator registry of challenge containers.

The registry decides what runs; only the owner-signed trust root decides emission.
"""

from __future__ import annotations

import re
import tomllib
from dataclasses import dataclass, field
from pathlib import Path
from urllib.parse import urlsplit

from cortex.protocol.models import CHALLENGE_ID

CHANNELS = frozenset({"stable", "edge"})
IMAGE = re.compile(r"ghcr\.io/[a-z0-9][a-z0-9._-]*(?:/[a-z0-9][a-z0-9._-]*)+")
PIN = re.compile(r"sha256:[0-9a-f]{64}")
ENV_NAME = re.compile(r"[A-Z][A-Z0-9_]{0,63}")
RESERVED_ENV = frozenset(
    {
        "CHALLENGE_SLUG",
        "CHALLENGE_STATE_DIR",
        "CHALLENGE_INTERNAL_TOKEN_FILE",
        "CHALLENGE_ADMIN_TOKEN_FILE",
        "CHALLENGE_MASTER_URL",
    }
)
FIELDS = frozenset(
    {
        "id",
        "image",
        "channel",
        "pin",
        "source",
        "attestation",
        "poll_seconds",
        "cpus",
        "memory_mib",
        "pids",
        "proxy_body_limit",
        "proxy_timeout_seconds",
        "env",
    }
)


def container_name(challenge_id: str) -> str:
    return f"cortex-challenge-{challenge_id}"


@dataclass(frozen=True)
class RegistryEntry:
    id: str
    image: str
    source: str
    channel: str = "stable"
    pin: str | None = None
    attestation: bool = True
    poll_seconds: int = 300
    cpus: float = 1.0
    memory_mib: int = 1024
    pids: int = 256
    proxy_body_limit: int = 1024 * 1024
    proxy_timeout_seconds: float = 30.0
    env: dict[str, str] = field(default_factory=dict)

    def __post_init__(self) -> None:
        if not CHALLENGE_ID.fullmatch(self.id) or self.id == "proof":
            raise ValueError("challenge id must be lowercase [a-z0-9-] and not proof")
        if not IMAGE.fullmatch(self.image):
            raise ValueError(f"{self.id}: image must be a ghcr.io repository without tag")
        source = urlsplit(self.source)
        if (
            source.scheme != "https"
            or source.hostname != "github.com"
            or source.query
            or source.fragment
            or len(source.path.strip("/").split("/")) != 2
        ):
            raise ValueError(f"{self.id}: source must be https://github.com/<owner>/<repo>")
        if self.channel not in CHANNELS:
            raise ValueError(f"{self.id}: channel must be stable or edge")
        if self.pin is not None and not PIN.fullmatch(self.pin):
            raise ValueError(f"{self.id}: pin must be sha256:<64 hex>")
        if type(self.attestation) is not bool:
            raise ValueError(f"{self.id}: attestation must be a boolean")
        if (
            not 30 <= self.poll_seconds <= 86400
            or not 0.1 <= self.cpus <= 64
            or not 64 <= self.memory_mib <= 262144
            or not 16 <= self.pids <= 32768
            or not 1024 <= self.proxy_body_limit <= 64 * 1024 * 1024
            or not 1 <= self.proxy_timeout_seconds <= 300
        ):
            raise ValueError(f"{self.id}: resource or proxy limit out of range")
        for name, value in self.env.items():
            if not ENV_NAME.fullmatch(name) or name in RESERVED_ENV or not isinstance(value, str):
                raise ValueError(f"{self.id}: invalid or reserved env {name!r}")

    @property
    def owner_repo(self) -> str:
        return urlsplit(self.source).path.strip("/")

    @property
    def reference(self) -> str:
        """What the supervisor resolves: an explicit digest pin wins over the channel."""
        return f"{self.image}@{self.pin}" if self.pin else f"{self.image}:{self.channel}"

    @property
    def url(self) -> str:
        return f"http://{container_name(self.id)}:8000"


def parse_registry(document: dict) -> dict[str, RegistryEntry]:
    if document.get("version") != 1 or set(document) - {"version", "challenge"}:
        raise ValueError("challenge registry must be version 1 with [[challenge]] rows")
    rows = document.get("challenge", [])
    if not isinstance(rows, list) or len(rows) > 64:
        raise ValueError("challenge registry holds at most 64 rows")
    entries: dict[str, RegistryEntry] = {}
    for row in rows:
        if not isinstance(row, dict) or set(row) - FIELDS:
            raise ValueError("unknown challenge registry field")
        entry = RegistryEntry(**{**row, "env": dict(row.get("env", {}))})
        if entry.id in entries:
            raise ValueError(f"duplicate challenge id {entry.id}")
        entries[entry.id] = entry
    return entries


def load_registry(path: Path | None) -> dict[str, RegistryEntry]:
    """A missing optional registry means no container challenges."""
    if path is None:
        return {}
    try:
        return parse_registry(tomllib.loads(path.read_text()))
    except (OSError, tomllib.TOMLDecodeError, TypeError) as error:
        raise ValueError(f"challenge registry unavailable: {type(error).__name__}") from None
