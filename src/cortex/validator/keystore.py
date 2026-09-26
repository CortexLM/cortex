"""A hotkey loaded from a private key file, in the shape the chain expects.

# Why this exists

`bittensor_wallet.Wallet` derives a hotkey from a BIP-39 mnemonic in
`secretPhrase`; it reads `privateKey`, `publicKey` and `ss58Address` but
overwrites them with whatever the phrase produces. That is fine for a key the
wallet itself generated, and it is the only way to reload it.

It does not work for an operator key exported from a Polkadot-style keystore.
Such a key is a 64-byte sr25519 mini-secret — a seed and its expanded secret —
and no mnemonic produces it. Measured against the wallet library: a file
carrying only `privateKey`, with or without `secretPhrase`, raises
`KeyFileError: Invalid phrase`, and a file carrying a valid phrase alongside a
foreign `privateKey` silently reloads the *phrase's* key instead.

So this module builds the minimal object `sign_and_send_extrinsic` needs:

    .hotkey.public_key   32 bytes, the sr25519 public key
    .hotkey.ss58_address the SS58 form, for the registration lookup
    .hotkey.sign(data)   an sr25519 signature over the payload
    .hotkey.sign_with... the wallet's own signing helpers, if it asks

`sign` goes through `sr25519.sign`, the same primitive `sign_substrate` uses,
so a signature made here is the one a Bittensor verifier accepts.

# Scope

This is a *loader*, not a key generator. It refuses anything that is not a
regular, owner-only file, and it refuses a key whose public half does not match
the declared SS58 address — a mismatch that would otherwise surface as a chain
rejection long after the mistake.
"""

import json
import os
import stat
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import sr25519

from cortex.protocol import ProtocolError
from cortex.protocol.crypto import encode_hotkey

# Bittensor signs and submits under SR25519. Any other scheme here would produce
# a signature the chain rejects, so a file declaring another one is refused
# rather than converted.
_SR25519_CRYPTO_TYPE = 1


def _read_private_file(path: Path) -> bytes:
    """Read a key file that only its owner can open."""
    try:
        descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    except OSError:
        raise ProtocolError("keystore file is unreadable") from None
    with os.fdopen(descriptor, "rb") as stream:
        metadata = os.fstat(stream.fileno())
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_mode & 0o077:
            raise ProtocolError("keystore file must be a private regular file")
        return stream.read(1 << 20)


def _private_bytes(value: object) -> bytes:
    """A hex private key, with or without the 0x prefix."""
    if not isinstance(value, str):
        raise ProtocolError("hotkey privateKey must be a hex string")
    text = value.strip().removeprefix("0x").removeprefix("0X")
    try:
        raw = bytes.fromhex(text)
    except ValueError:
        raise ProtocolError("hotkey privateKey is not hex") from None
    # Two shapes are accepted, and they are different things:
    #
    #   32 bytes  a seed, from which the pair is derived
    #   64 bytes  an *expanded* secret — scalar ‖ nonce, the form a Polkadot
    #             keystore exports. It is NOT a seed ‖ secret mini-secret (that
    #             is 96 bytes), so it must be read as a secret, not as a seed.
    #
    # Anything else is refused rather than guessed at.
    if len(raw) not in (32, 64):
        raise ProtocolError("hotkey privateKey must be 32 or 64 bytes")
    return raw


def _public_from(private: bytes) -> bytes:
    if len(private) == 64:
        return sr25519.public_from_secret_key(private)
    return sr25519.pair_from_seed(private)[0]


@dataclass(frozen=True)
class Hotkey:
    """The three things the submit path asks of a wallet's hotkey."""

    _public: bytes
    _private: bytes

    @property
    def public_key(self) -> bytes:
        return self._public

    @property
    def crypto_type(self) -> int:
        """The substrate signer reads this before signing (the file is checked to be sr25519)."""
        return _SR25519_CRYPTO_TYPE

    @property
    def ss58_address(self) -> str:
        return encode_hotkey(self._public)

    def sign(self, data: bytes) -> bytes:
        if len(self._private) == 64:
            return sr25519.sign((self._public, self._private), data)
        return sr25519.sign(sr25519.pair_from_seed(self._private), data)

    def verify(self, data: bytes, signature: bytes) -> bool:
        try:
            return bool(sr25519.verify(signature, data, self._public))
        except ValueError:
            return False


@dataclass(frozen=True)
class PrivateKeyWallet:
    """The subset of a Bittensor `Wallet` this validator uses.

    Built from a keystore file that carries `privateKey`. Construct it with
    `load_private_key_wallet`, which enforces the file checks.
    """

    hotkey: Hotkey
    name: str

    def __getattr__(self, item: str) -> Any:
        raise ProtocolError(
            f"private-key wallet has no {item!r}: this loader supports signing only"
        )


def load_private_key_wallet(path: Path, *, name: str | None = None) -> PrivateKeyWallet:
    """Load a hotkey from a keystore file carrying `privateKey`.

    Raises `ProtocolError` for a file that is public, a symlink, malformed, or
    whose key does not match the `ss58Address` it declares. The last check is
    the one worth having: a mismatched pair signs as a different account than
    the operator named, and the chain would reject it at submission rather than
    here.
    """
    raw = _read_private_file(path)
    try:
        document = json.loads(raw)
    except ValueError:
        raise ProtocolError("keystore file is not JSON") from None
    if not isinstance(document, dict):
        raise ProtocolError("keystore file is not a JSON object")

    crypto_type = document.get("cryptoType", _SR25519_CRYPTO_TYPE)
    if crypto_type != _SR25519_CRYPTO_TYPE:
        raise ProtocolError("hotkey cryptoType must be 1 (sr25519)")

    private = _private_bytes(document.get("privateKey"))
    public = _public_from(private)

    declared = document.get("ss58Address")
    if isinstance(declared, str) and declared:
        if encode_hotkey(public) != declared:
            raise ProtocolError("hotkey privateKey does not derive the ss58Address it declares")

    return PrivateKeyWallet(
        hotkey=Hotkey(_public=public, _private=private), name=name or "private-key"
    )


def carries_private_key(path: Path) -> bool:
    """Whether a hotkey file is one this loader should read.

    A mnemonic wallet writes `secretPhrase` and leaves `privateKey` absent or
    stale; a Polkadot-style keystore carries a usable `privateKey` and no
    mnemonic. Deciding on the file keeps the command line unchanged, so the
    audited deploy surface stays what the gate checks.

    Unreadable or malformed files answer False: the caller falls back to the
    wallet library, which reports the problem in its own terms. A phrase that does
    not derive the declared ss58Address raises ProtocolError.
    """
    try:
        raw = _read_private_file(path)
        document = json.loads(raw)
    except (ProtocolError, ValueError):
        return False
    if not isinstance(document, dict):
        return False
    # A phrase means the wallet library owns this file: it derives from the phrase
    # and ignores the key (btcli writes both). The phrase must then name the account
    # the file declares, or the validator would sign as someone else.
    phrase = document.get("secretPhrase")
    if isinstance(phrase, str) and phrase.strip():
        declared = document.get("ss58Address")
        if isinstance(declared, str) and declared:
            from bittensor_wallet import Keypair

            try:
                derived = Keypair.create_from_mnemonic(phrase.strip()).ss58_address
            except Exception:
                raise ProtocolError("hotkey secretPhrase is not a valid mnemonic") from None
            if derived != declared:
                raise ProtocolError(
                    "hotkey secretPhrase does not derive the ss58Address it declares"
                )
        return False
    try:
        _private_bytes(document.get("privateKey"))
    except ProtocolError:
        return False
    return True
