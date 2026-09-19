"""sr25519 signatures, preserving both Cortex and Substrate signing contexts.

py-sr25519-bindings hardcodes ``substrate``. It is used for Bounty signatures
and seed expansion. Proof and consensus use the frozen ``base-sr25519-v1``
transcript plus libsodium's constant-time Ristretto/scalar primitives.
"""

import ctypes
import ctypes.util
import hashlib
import hmac
import os
from functools import lru_cache

import sr25519

from ._merlin import Transcript
from .scale import ProtocolError, byte_vec, fixed

SIGNING_CONTEXT = b"base-sr25519-v1"
BUNDLE_DOMAIN = b"base-bundle-v1"
RAW_WEIGHT_DOMAIN = b"base-rawweight-v1"
TRUST_ROOT_DOMAIN = b"base-trustroot-v1"
_ORDER = 2**252 + 27742317777372353535851937790883648493
_ALPHABET = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"


@lru_cache(maxsize=1)
def _sodium():
    library = ctypes.util.find_library("sodium")
    if library is None:
        raise RuntimeError(
            "libsodium with Ristretto255 support is required for consensus signatures"
        )
    native = ctypes.CDLL(library)
    native.sodium_init.argtypes = []
    native.sodium_init.restype = ctypes.c_int
    if native.sodium_init() < 0:
        raise RuntimeError("libsodium initialization failed")
    functions = {
        "crypto_core_ristretto255_is_valid_point": (1, ctypes.c_int),
        "crypto_core_ristretto255_scalar_reduce": (2, None),
        "crypto_core_ristretto255_scalar_mul": (3, None),
        "crypto_core_ristretto255_scalar_add": (3, None),
        "crypto_core_ristretto255_add": (3, ctypes.c_int),
        "crypto_scalarmult_ristretto255": (3, ctypes.c_int),
        "crypto_scalarmult_ristretto255_base": (2, ctypes.c_int),
    }
    for name, (arity, result) in functions.items():
        function = getattr(native, name)
        function.argtypes = [ctypes.c_void_p] * arity
        function.restype = result
    return native


def _operation(name: str, *inputs: bytes) -> bytes:
    output = ctypes.create_string_buffer(32)
    result = getattr(_sodium(), name)(output, *inputs)
    if result not in (None, 0):
        raise ProtocolError("invalid Ristretto operation")
    return output.raw


def signing_preimage(domain: bytes, payload: bytes) -> bytes:
    return byte_vec(domain) + byte_vec(payload)


def public_key(seed: bytes) -> bytes:
    return sr25519.pair_from_seed(fixed(seed, 32))[0]


def _challenge(public: bytes, nonce_point: bytes, message: bytes) -> bytes:
    transcript = Transcript(b"SigningContext")
    transcript.append(b"", SIGNING_CONTEXT)
    transcript.append(b"sign-bytes", message)
    transcript.append(b"proto-name", b"Schnorr-sig")
    transcript.append(b"sign:pk", public)
    transcript.append(b"sign:R", nonce_point)
    return _operation("crypto_core_ristretto255_scalar_reduce", transcript.challenge(b"sign:c", 64))


def sign_raw(seed: bytes, domain: bytes, payload: bytes) -> bytes:
    """Sign with a fresh OS-random nonce; scalar arithmetic stays in libsodium."""
    public, secret = sr25519.pair_from_seed(fixed(seed, 32))
    return sign_raw_keypair(public, secret, domain, payload)


def sign_raw_keypair(public: bytes, secret: bytes, domain: bytes, payload: bytes) -> bytes:
    """Sign the frozen transcript with an expanded sr25519 wallet secret."""
    fixed(public, 32)
    fixed(secret, 64)
    if not hmac.compare_digest(public, sr25519.public_from_secret_key(secret)):
        raise ProtocolError("sr25519 public key mismatch")
    nonce = _operation("crypto_core_ristretto255_scalar_reduce", os.urandom(64))
    point = _operation("crypto_scalarmult_ristretto255_base", nonce)
    challenge = _challenge(public, point, signing_preimage(domain, payload))
    product = _operation("crypto_core_ristretto255_scalar_mul", challenge, secret[:32])
    scalar = bytearray(_operation("crypto_core_ristretto255_scalar_add", nonce, product))
    scalar[31] |= 128  # Schnorrkel marker, mandatory for non-legacy signatures.
    return point + bytes(scalar)


def verify_raw(public: bytes, domain: bytes, payload: bytes, signature: bytes) -> bool:
    """Verify the frozen Cortex context; never substitute the Substrate context."""
    if len(public) != 32 or len(signature) != 64 or not signature[63] & 128:
        return False
    point = signature[:32]
    scalar = signature[32:63] + bytes([signature[63] & 127])
    if int.from_bytes(scalar, "little") >= _ORDER:
        return False
    sodium = _sodium()
    if (
        public == bytes(32)
        or point == bytes(32)
        or sodium.crypto_core_ristretto255_is_valid_point(public) != 1
        or sodium.crypto_core_ristretto255_is_valid_point(point) != 1
    ):
        return False
    challenge = _challenge(public, point, signing_preimage(domain, payload))
    try:
        expected = _operation("crypto_scalarmult_ristretto255_base", scalar)
        multiplied = _operation("crypto_scalarmult_ristretto255", challenge, public)
        actual = _operation("crypto_core_ristretto255_add", point, multiplied)
    except ProtocolError:
        return False
    return hmac.compare_digest(expected, actual)


def sign_substrate(seed: bytes, payload: bytes) -> bytes:
    return sr25519.sign(sr25519.pair_from_seed(fixed(seed, 32)), payload)


def verify_substrate(public: bytes, payload: bytes, signature: bytes) -> bool:
    if len(public) != 32 or len(signature) != 64:
        return False
    try:
        return bool(sr25519.verify(signature, payload, public))
    except ValueError:
        return False


def decode_hotkey(value: str | bytes) -> bytes:
    """Decode a raw/hex public key or checksummed Bittensor SS58 (network 42)."""
    if isinstance(value, bytes):
        return fixed(value, 32)
    raw = value.removeprefix("0x")
    if len(raw) == 64:
        try:
            return fixed(bytes.fromhex(raw), 32)
        except ValueError as error:
            raise ProtocolError("invalid hotkey hex") from error
    if not 46 <= len(value) <= 50:
        raise ProtocolError("invalid SS58 hotkey")
    number = 0
    for char in value:
        index = _ALPHABET.find(char)
        if index < 0:
            raise ProtocolError("invalid SS58 alphabet")
        number = number * 58 + index
    decoded = number.to_bytes((number.bit_length() + 7) // 8, "big")
    decoded = bytes(len(value) - len(value.lstrip("1"))) + decoded
    if len(decoded) != 35 or decoded[0] != 42:
        raise ProtocolError("expected Bittensor SS58 network 42")
    checksum = hashlib.blake2b(b"SS58PRE" + decoded[:-2]).digest()[:2]
    if not hmac.compare_digest(checksum, decoded[-2:]):
        raise ProtocolError("invalid SS58 checksum")
    return decoded[1:33]


def encode_hotkey(public: bytes) -> str:
    payload = b"\x2a" + fixed(public, 32)
    payload += hashlib.blake2b(b"SS58PRE" + payload).digest()[:2]
    number = int.from_bytes(payload, "big")
    result = ""
    while number:
        number, remainder = divmod(number, 58)
        result = _ALPHABET[remainder] + result
    return result
