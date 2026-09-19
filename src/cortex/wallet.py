"""Bittensor hotkeys with the distinct Cortex and Substrate signing contexts."""

from __future__ import annotations

import hmac
import json
from dataclasses import dataclass, field
from pathlib import Path

import sr25519

from cortex.errors import ServiceError
from cortex.http import read_private_file
from cortex.protocol.crypto import encode_hotkey, sign_raw_keypair
from cortex.protocol.scale import ProtocolError, fixed


@dataclass(frozen=True)
class HotkeySigner:
    public_key: bytes
    _secret: bytes = field(repr=False)

    def __post_init__(self) -> None:
        fixed(self.public_key, 32)
        fixed(self._secret, 64)
        if not hmac.compare_digest(self.public_key, sr25519.public_from_secret_key(self._secret)):
            raise ProtocolError("sr25519 public key mismatch")

    @classmethod
    def from_dev_seed(cls, seed: bytes) -> HotkeySigner:
        """Raw seeds are only for deterministic local development fixtures."""
        public, secret = sr25519.pair_from_seed(fixed(seed, 32))
        return cls(public, secret)

    @property
    def ss58_address(self) -> str:
        return encode_hotkey(self.public_key)

    def sign(self, domain: bytes, payload: bytes) -> bytes:
        return sign_raw_keypair(self.public_key, self._secret, domain, payload)

    def sign_substrate(self, payload: bytes) -> bytes:
        return sr25519.sign((self.public_key, self._secret), payload)


def load_wallet_hotkey(
    *,
    name: str,
    hotkey: str,
    path: str | Path = "~/.bittensor/wallets",
    password_file: Path | None = None,
) -> HotkeySigner:
    """Unlock the selected hotkey only; never create or modify wallet files."""
    try:
        from bittensor_wallet import Wallet
        from bittensor_wallet.keyfile import serialized_keypair_to_keyfile_data
    except ImportError:
        raise ServiceError(503, "install cortex-subnet[chain] for Bittensor wallets") from None
    password = read_private_file(password_file) if password_file is not None else None
    try:
        wallet = Wallet(name=name, hotkey=hotkey, path=str(Path(path).expanduser()))
        if wallet.hotkey_file.is_encrypted() and password is None:
            raise ServiceError(503, "encrypted Bittensor hotkey password file required")
        keypair = wallet.get_hotkey(password=password)
        if keypair.crypto_type != 1:
            raise ServiceError(503, "Bittensor hotkey must use sr25519")
        public = keypair.public_key
        if public is None:
            raise ServiceError(503, "Bittensor hotkey unavailable")
        # The SDK serializer supports derived hotkeys without an exportable mini-seed.
        # Expanded secret material stays in memory and never enters a diagnostic.
        data = json.loads(serialized_keypair_to_keyfile_data(keypair))
        secret = bytes.fromhex(data["privateKey"].removeprefix("0x"))
        return HotkeySigner(public, secret)
    except ServiceError:
        raise
    except Exception:
        raise ServiceError(503, "Bittensor hotkey unavailable") from None
