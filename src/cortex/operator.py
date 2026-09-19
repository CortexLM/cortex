"""Offline operator key generation and detached trust-root signing."""

from __future__ import annotations

import os
import secrets
from pathlib import Path

from cortex.config import read_seed
from cortex.protocol.crypto import TRUST_ROOT_DOMAIN, public_key, sign_raw
from cortex.protocol.trust import signing_payload


def _exclusive_write(path: Path, value: bytes, mode: int) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, mode)
    try:
        with os.fdopen(descriptor, "wb", closefd=False) as stream:
            stream.write(value)
            stream.flush()
            os.fsync(stream.fileno())
    finally:
        os.close(descriptor)


def generate_key(seed_path: Path, public_path: Path) -> str:
    """Create a new raw sr25519 seed and its hexadecimal public key without overwrites."""
    seed = secrets.token_bytes(32)
    encoded_public = public_key(seed).hex()
    _exclusive_write(seed_path, seed, 0o600)
    try:
        _exclusive_write(public_path, (encoded_public + "\n").encode(), 0o644)
    except BaseException:
        seed_path.unlink(missing_ok=True)
        raise
    return encoded_public


def sign_trust_document(
    *, input_path: Path, kind: str, seed_path: Path, signature_path: Path
) -> str:
    """Sign one immutable TOML document and create its detached signature."""
    signature = sign_raw(
        read_seed(seed_path), TRUST_ROOT_DOMAIN, signing_payload(input_path, kind)
    ).hex()
    _exclusive_write(signature_path, (signature + "\n").encode(), 0o644)
    return signature
