"""Loading a validator hotkey from a private key file.

The case this exists for: an operator key exported from a Polkadot-style
keystore, which is a 64-byte sr25519 mini-secret and has no mnemonic. The
wallet library cannot load one, so the loader here builds the object the submit
path needs and refuses anything it cannot vouch for.
"""

import json
import os
from pathlib import Path

import pytest
import sr25519

from cortex.protocol import ProtocolError
from cortex.protocol.crypto import encode_hotkey
from cortex.validator.keystore import carries_private_key, load_private_key_wallet

# A fresh key for the fixture. Not a secret: it is generated in the test and
# never leaves it.
_SEED = bytes(range(32))
_PUBLIC, _EXPANDED = sr25519.pair_from_seed(_SEED)
# The 64-byte expanded secret is what a Polkadot keystore exports: scalar ‖ nonce.
# It is *not* seed ‖ secret, which would be 96 bytes.
_SS58 = encode_hotkey(_PUBLIC)


def write(tmp_path: Path, body: dict, *, mode: int = 0o600) -> Path:
    path = tmp_path / "hotkey"
    path.write_text(json.dumps(body))
    path.chmod(mode)
    return path


def test_a_64_byte_private_key_loads_and_signs_as_its_own_account(tmp_path):
    path = write(
        tmp_path, {"privateKey": "0x" + _EXPANDED.hex(), "cryptoType": 1, "ss58Address": _SS58}
    )
    wallet = load_private_key_wallet(path)

    assert wallet.hotkey.public_key == _PUBLIC
    assert wallet.hotkey.ss58_address == _SS58

    # The signature must be one a Bittensor verifier accepts: same primitive,
    # same public key.
    payload = b"commit-mechanism-weights"
    signature = wallet.hotkey.sign(payload)
    assert len(signature) == 64
    assert sr25519.verify(signature, payload, _PUBLIC)
    assert wallet.hotkey.verify(payload, signature)


def test_a_32_byte_seed_loads_too(tmp_path):
    path = write(tmp_path, {"privateKey": "0x" + _SEED.hex(), "cryptoType": 1})
    wallet = load_private_key_wallet(path)
    assert wallet.hotkey.public_key == _PUBLIC


def test_the_hex_prefix_is_optional(tmp_path):
    path = write(tmp_path, {"privateKey": _EXPANDED.hex(), "cryptoType": 1})
    assert load_private_key_wallet(path).hotkey.public_key == _PUBLIC


def test_a_key_that_does_not_match_its_declared_address_is_refused(tmp_path):
    # The check worth having: this file signs as a different account than the
    # operator named, and the chain would reject it at submission instead.
    other = encode_hotkey(sr25519.pair_from_seed(bytes(range(1, 33)))[0])
    path = write(tmp_path, {"privateKey": "0x" + _EXPANDED.hex(), "ss58Address": other})
    with pytest.raises(ProtocolError, match="does not derive"):
        load_private_key_wallet(path)


def test_a_public_or_symlinked_file_is_refused(tmp_path):
    path = write(tmp_path, {"privateKey": "0x" + _EXPANDED.hex()}, mode=0o644)
    with pytest.raises(ProtocolError, match="private regular file"):
        load_private_key_wallet(path)

    secret = write(tmp_path, {"privateKey": "0x" + _EXPANDED.hex()})
    link = tmp_path / "link"
    os.symlink(secret, link)
    with pytest.raises(ProtocolError, match="unreadable"):
        load_private_key_wallet(link)


@pytest.mark.parametrize(
    "body",
    [
        {"privateKey": "0x00"},  # not a key length
        {"privateKey": "zz"},  # not hex
        {"privateKey": 5},  # not a string
        {},  # no key at all
        {"privateKey": "0x" + _EXPANDED.hex(), "cryptoType": 2},  # not sr25519
    ],
)
def test_a_malformed_file_is_refused(tmp_path, body):
    path = write(tmp_path, body)
    with pytest.raises(ProtocolError):
        load_private_key_wallet(path)


def test_a_file_that_is_not_json_is_refused(tmp_path):
    path = tmp_path / "hotkey"
    path.write_text("not json")
    path.chmod(0o600)
    with pytest.raises(ProtocolError, match="not JSON"):
        load_private_key_wallet(path)


def test_an_unknown_attribute_fails_loudly(tmp_path):
    # A wallet with a hotkey and nothing else looks enough like a Wallet for the
    # submit path, but a future call site asking for a coldkey must fail here
    # rather than AttributeError somewhere inside the SDK.
    path = write(tmp_path, {"privateKey": "0x" + _EXPANDED.hex()})
    wallet = load_private_key_wallet(path)
    with pytest.raises(ProtocolError, match="no 'coldkey'"):
        _ = wallet.coldkey


def test_detection_picks_a_keystore_and_leaves_a_mnemonic_wallet_alone(tmp_path):
    # The command line does not change: the file decides.
    keystore = write(tmp_path, {"privateKey": "0x" + _EXPANDED.hex(), "cryptoType": 1})
    assert carries_private_key(keystore) is True

    wallet_like = write(
        tmp_path, {"secretPhrase": "abandon abandon about", "privateKey": "0x" + _EXPANDED.hex()}
    )
    assert carries_private_key(wallet_like) is False

    assert carries_private_key(tmp_path / "absent") is False


def test_detection_refuses_a_keystore_with_no_usable_key(tmp_path):
    path = write(tmp_path, {"privateKey": "0x00"})
    assert carries_private_key(path) is False
