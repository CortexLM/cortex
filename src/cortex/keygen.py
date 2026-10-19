"""Generate a raw sr25519 seed and its public key for a challenge leaf or topic key."""

import os
import secrets
from pathlib import Path

from cortex.protocol.crypto import public_key


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
    """Create a new raw seed (0600) and its hexadecimal public key (0644), never overwriting."""
    seed = secrets.token_bytes(32)
    encoded_public = public_key(seed).hex()
    _exclusive_write(seed_path, seed, 0o600)
    try:
        _exclusive_write(public_path, (encoded_public + "\n").encode(), 0o644)
    except BaseException:
        seed_path.unlink(missing_ok=True)
        raise
    return encoded_public
