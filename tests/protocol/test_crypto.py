import json
from pathlib import Path

import pytest

from cortex.protocol.crypto import (
    BUNDLE_DOMAIN,
    RAW_WEIGHT_DOMAIN,
    decode_hotkey,
    encode_hotkey,
    public_key,
    sign_raw,
    sign_substrate,
    verify_raw,
    verify_substrate,
)
from cortex.protocol.scale import ProtocolError

REFERENCE = json.loads((Path(__file__).parent / "vectors/rust_wire.json").read_text())


@pytest.mark.parametrize(
    "reference",
    REFERENCE["signatures"],
    ids=lambda reference: f"payload-{len(reference['payload']) // 2}",
)
def test_verifies_independent_rust_schnorrkel_vectors(reference):
    key = bytes.fromhex(reference["public"])
    signature = bytes.fromhex(reference["signature"])
    payload = bytes.fromhex(reference["payload"])
    assert public_key(bytes.fromhex(reference["seed"])) == key
    assert verify_raw(key, BUNDLE_DOMAIN, payload, signature)
    assert not verify_raw(key, RAW_WEIGHT_DOMAIN, payload, signature)
    assert not verify_raw(key, BUNDLE_DOMAIN, payload + b"!", signature)


@pytest.mark.parametrize("mutation", ["marker", "scalar", "point", "public", "short"])
def test_rejects_signature_malleability_and_invalid_group_elements(mutation):
    reference = REFERENCE["signatures"][0]
    key = bytes.fromhex(reference["public"])
    signature = bytearray.fromhex(reference["signature"])
    if mutation == "marker":
        signature[63] &= 127
    elif mutation == "scalar":
        scalar = int.from_bytes(signature[32:63] + bytes([signature[63] & 127]), "little")
        scalar += 2**252 + 27742317777372353535851937790883648493
        signature[32:] = scalar.to_bytes(32, "little")
        signature[63] |= 128
    elif mutation == "point":
        signature[:32] = bytes([255]) * 32
    elif mutation == "public":
        key = bytes(32)
    elif mutation == "short":
        signature.pop()
    assert not verify_raw(key, BUNDLE_DOMAIN, b"", bytes(signature))


def test_contexts_cannot_be_silently_substituted():
    seed = bytes([7]) * 32
    message = b"signed payload"
    substrate_signature = sign_substrate(seed, message)
    consensus_signature = sign_raw(seed, BUNDLE_DOMAIN, message)
    assert verify_substrate(public_key(seed), message, substrate_signature)
    assert verify_raw(public_key(seed), BUNDLE_DOMAIN, message, consensus_signature)
    assert not verify_substrate(public_key(seed), message, consensus_signature)
    assert not verify_raw(public_key(seed), BUNDLE_DOMAIN, message, substrate_signature)


def test_python_signature_matches_vector_independently_verified_by_rust(monkeypatch):
    # Fixed RNG is test-only. Rust crypto::verify_raw/schnorrkel 0.11.5 verified
    # these exact signature bytes, with seed [7;32], on 2026-09-16.
    monkeypatch.setattr("cortex.protocol.crypto.os.urandom", lambda size: b"\xab" * size)
    signature = sign_raw(
        bytes([7]) * 32, BUNDLE_DOMAIN, b"independent Rust schnorrkel signature vector"
    )
    assert signature.hex() == (
        "22d93e0325fba78d275d92beccc854feecf37cd9233a3a9d501124d3309a23322"
        "68dc94d9141adfcb0ae0919519b5c6bcc207152ed34d0423037538a430a298d"
    )


def test_ss58_known_alice_and_corrupt_checksum():
    alice = "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY"
    key = bytes.fromhex("d43593c715fdd31c61141abd04a99fd6822c8558854ccde39a5684e7a56da27d")
    assert encode_hotkey(key) == alice
    assert decode_hotkey(alice) == key
    with pytest.raises(ProtocolError):
        decode_hotkey(alice[:-1] + "Z")
