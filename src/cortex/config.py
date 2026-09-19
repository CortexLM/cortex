"""Master configuration with explicit file credentials and deployed BASE aliases."""

from __future__ import annotations

import os
import stat
from collections.abc import Mapping
from dataclasses import dataclass, field
from math import isfinite
from pathlib import Path
from urllib.parse import urlsplit

from cortex.errors import ServiceError
from cortex.protocol import TrustRoot
from cortex.protocol.crypto import decode_hotkey, public_key
from cortex.protocol.trust import load_trust_root
from cortex.vm.models import Resources


def read_seed(path: Path) -> bytes:
    """Read raw 32 bytes or 64 hex with private regular-file/O_NOFOLLOW checks."""
    try:
        descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
        with os.fdopen(descriptor, "rb") as stream:
            metadata = os.fstat(stream.fileno())
            if not stat.S_ISREG(metadata.st_mode) or metadata.st_mode & 0o077:
                raise ServiceError(503, "signing key file must be private")
            value = stream.read(133)
        if len(value) == 32:
            return value
        decoded = bytes.fromhex(value.decode().strip().removeprefix("0x"))
        if len(decoded) != 32:
            raise ValueError("key length")
        return decoded
    except (OSError, UnicodeError, ValueError):
        raise ServiceError(503, "signing key file unavailable") from None


def _setting(env: Mapping[str, str], name: str, default: str = "") -> str:
    alias = "CORTEX_" + name[5:] if name.startswith("BASE_") else name
    existing = env.get(name)
    alternate = env.get(alias)
    if existing is not None and alternate is not None and existing != alternate:
        raise ValueError(f"conflicting {name} and {alias}")
    return existing if existing is not None else alternate if alternate is not None else default


@dataclass(frozen=True)
class MasterConfig:
    netuid: int
    state_dir: Path
    owner_public_file: Path
    challenges_file: Path
    measurements_file: Path
    gateway_seed_file: Path
    bounty_seed_file: Path
    proof_seed_file: Path
    bounty_session_secret_file: Path
    operator_token_file: Path
    chain_endpoint: str = "finney"
    bounty_backend_url: str | None = None
    emit_poll_seconds: float = 120
    epoch_refresh_seconds: float = 12
    epoch_stale_seconds: float = 60
    minimum_challenges_version: int = 1
    minimum_measurements_version: int = 1
    proof_orchestrator_url: str | None = None
    proof_orchestrator_token_file: Path | None = None
    proof_orchestrator_ca_file: Path | None = None
    proof_vm_resources: Resources = field(default_factory=Resources)

    def __post_init__(self):
        if type(self.netuid) is not int or not 0 <= self.netuid <= 65535:
            raise ValueError("invalid netuid")
        if (
            not all(
                isfinite(interval)
                for interval in (
                    self.epoch_refresh_seconds,
                    self.epoch_stale_seconds,
                    self.emit_poll_seconds,
                )
            )
            or not 0 < self.epoch_refresh_seconds < self.epoch_stale_seconds
            or self.emit_poll_seconds <= 0
        ):
            raise ValueError("invalid epoch refresh/emission intervals")
        if self.minimum_challenges_version < 1 or self.minimum_measurements_version < 1:
            raise ValueError("trust versions must be positive")
        for url in (self.bounty_backend_url, self.proof_orchestrator_url):
            if url:
                parsed = urlsplit(url)
                if (
                    parsed.scheme != "https"
                    or not parsed.hostname
                    or parsed.username
                    or parsed.password
                    or parsed.fragment
                ):
                    raise ValueError("backend URLs must be HTTPS without credentials")
        if self.proof_orchestrator_url and self.proof_orchestrator_token_file is None:
            raise ValueError("Proof orchestrator token file required")
        if self.proof_orchestrator_url and self.proof_orchestrator_ca_file is None:
            raise ValueError("Proof orchestrator CA file required")

    @classmethod
    def from_env(cls, env: Mapping[str, str] | None = None) -> MasterConfig:
        values = os.environ if env is None else env

        def value(name: str, default: str = "") -> str:
            return _setting(values, name, default)

        def required_path(name: str) -> Path:
            result = value(name)
            if not result:
                raise ValueError(f"{name} is required")
            return Path(result)

        def optional_path(name: str) -> Path | None:
            result = value(name)
            return Path(result) if result else None

        if not value("BASE_NETUID"):
            raise ValueError("BASE_NETUID is required")
        return cls(
            netuid=int(value("BASE_NETUID")),
            state_dir=Path(value("BASE_STATE_DIR", "/var/lib/cortex")),
            owner_public_file=Path(value("BASE_OWNER_PUBKEY_FILE", "config/owner.pubkey")),
            challenges_file=Path(value("BASE_CHALLENGES_FILE", "config/challenges.toml")),
            measurements_file=Path(value("BASE_MEASUREMENTS_FILE", "config/measurements.toml")),
            gateway_seed_file=required_path("BASE_GATEWAY_SK_FILE"),
            bounty_seed_file=required_path("BOUNTY_SK_FILE"),
            proof_seed_file=required_path("PROOF_SK_FILE"),
            bounty_session_secret_file=required_path("BOUNTY_SESSION_SECRET_FILE"),
            operator_token_file=required_path("BASE_GATEWAY_ADMIN_TOKEN_FILE"),
            chain_endpoint=value("BASE_CHAIN_ENDPOINT", "finney"),
            bounty_backend_url=value("BOUNTY_BACKEND_PUBLIC_URL") or None,
            emit_poll_seconds=float(
                value("BASE_EMIT_POLL_SECS", value("PROOF_EMIT_POLL_SECS", "120"))
            ),
            epoch_refresh_seconds=float(value("BASE_EPOCH_REFRESH_SECS", "12")),
            epoch_stale_seconds=float(value("BASE_EPOCH_STALE_SECS", "60")),
            minimum_challenges_version=int(value("BASE_CHALLENGES_MIN_VERSION", "1")),
            minimum_measurements_version=int(value("BASE_MEASUREMENTS_MIN_VERSION", "1")),
            proof_orchestrator_url=value("PROOF_VM_ORCHESTRATOR_URL") or None,
            proof_orchestrator_token_file=optional_path("PROOF_VM_ORCHESTRATOR_TOKEN_FILE"),
            proof_orchestrator_ca_file=optional_path("PROOF_VM_ORCHESTRATOR_CA_FILE"),
            proof_vm_resources=Resources(
                vcpus=int(value("PROOF_RLM_VM_VCPUS", "16")),
                mem_mib=int(value("PROOF_RLM_VM_MEM_MIB", "32768")),
                disk_mib=int(value("PROOF_RLM_VM_DISK_MIB", "32768")),
            ),
        )

    def trust_root(self, epoch: int) -> TrustRoot:
        try:
            trust = load_trust_root(
                challenges_path=self.challenges_file,
                challenges_signature=Path(str(self.challenges_file) + ".sig"),
                measurements_path=self.measurements_file,
                measurements_signature=Path(str(self.measurements_file) + ".sig"),
                owner_public=decode_hotkey(self.owner_public_file.read_text().strip()),
                gateway_public=public_key(read_seed(self.gateway_seed_file)),
                epoch=epoch,
                minimum_challenges_version=self.minimum_challenges_version,
                minimum_measurements_version=self.minimum_measurements_version,
            )
            for entry, path in zip(
                trust.challenges, (self.bounty_seed_file, self.proof_seed_file), strict=True
            ):
                if public_key(read_seed(path)) != entry.public_key:
                    raise ServiceError(503, "challenge signing key does not match owner trust")
            return trust
        except (OSError, ValueError):
            raise ServiceError(503, "signed trust root unavailable") from None
