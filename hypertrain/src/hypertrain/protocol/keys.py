"""sr25519 keys and Bittensor SS58 hotkeys.

decode_hotkey/encode_hotkey/verify follow OpentypeAI/challenge@4b3ab44 crypto.py (Apache-2.0).
"""

from __future__ import annotations

import hashlib
import hmac

import sr25519

_ALPHABET = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"


class KeyError_(ValueError):
    pass


def decode_hotkey(value: str) -> bytes:
    if not 46 <= len(value) <= 50:
        raise KeyError_("invalid SS58 hotkey length")
    number = 0
    for char in value:
        index = _ALPHABET.find(char)
        if index < 0:
            raise KeyError_("invalid SS58 alphabet")
        number = number * 58 + index
    decoded = number.to_bytes((number.bit_length() + 7) // 8, "big")
    decoded = bytes(len(value) - len(value.lstrip("1"))) + decoded
    if len(decoded) != 35 or decoded[0] != 42:
        raise KeyError_("expected Bittensor SS58 network 42")
    checksum = hashlib.blake2b(b"SS58PRE" + decoded[:-2]).digest()[:2]
    if not hmac.compare_digest(checksum, decoded[-2:]):
        raise KeyError_("invalid SS58 checksum")
    return decoded[1:33]


def encode_hotkey(public: bytes) -> str:
    if len(public) != 32:
        raise KeyError_("public key must be 32 bytes")
    payload = b"\x2a" + public
    payload += hashlib.blake2b(b"SS58PRE" + payload).digest()[:2]
    number = int.from_bytes(payload, "big")
    result = ""
    while number:
        number, remainder = divmod(number, 58)
        result = _ALPHABET[remainder] + result
    return result


class Keypair:
    def __init__(self, seed: bytes) -> None:
        if len(seed) != 32:
            raise KeyError_("seed must be 32 bytes")
        self.public, self._secret = sr25519.pair_from_seed(seed)
        self.ss58 = encode_hotkey(self.public)

    def sign(self, message: bytes) -> bytes:
        return bytes(sr25519.sign((self.public, self._secret), message))

    def __repr__(self) -> str:
        return f"Keypair({self.ss58})"


def verify(public: bytes, message: bytes, signature: bytes) -> bool:
    if len(public) != 32 or len(signature) != 64:
        return False
    try:
        return bool(sr25519.verify(signature, message, public))
    except ValueError:
        return False
