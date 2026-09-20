import json
from dataclasses import replace
from hashlib import sha256
from pathlib import Path

import pytest

from cortex.protocol import (
    Bundle,
    ChallengeEntry,
    MetagraphRow,
    NoScore,
    ProtocolError,
    Score,
    TrustRoot,
    build_bundle,
    sign_leaf,
    verify_bundle,
)
from cortex.protocol.bundle import recompute
from cortex.protocol.crypto import BUNDLE_DOMAIN, public_key, sign_raw, verify_raw
from cortex.protocol.merkle import merkle_root
from cortex.protocol.scale import Reader


@pytest.fixture
def network():
    rows = tuple(MetagraphRow(bytes([tag]) * 32, tag) for tag in range(3))
    trust = TrustRoot(
        (
            ChallengeEntry(b"bounty", public_key(bytes([1]) * 32), 2000),
            ChallengeEntry(b"proof", public_key(bytes([2]) * 32), 8000),
        ),
        sha256(b"\x00").digest(),
        public_key(bytes([7]) * 32),
    )
    leaves = tuple(
        sign_leaf(bytes([seed]) * 32, name, row.hotkey, 12, Score(50) if row.uid else NoScore())
        for name, seed in ((b"bounty", 1), (b"proof", 2))
        for row in rows
    )
    bundle = build_bundle(
        gateway_seed=bytes([7]) * 32,
        epoch=12,
        netuid=541,
        block_b=99,
        block_hash=bytes([9]) * 32,
        rows=rows,
        leaves=leaves,
        trust=trust,
    )
    return bundle, dict(rows=rows, block_hash=bytes([9]) * 32, trust=trust, netuid=541, epoch=12)


def test_complete_signed_seal_roundtrip_and_independent_recompute(network):
    bundle, arguments = network
    decoded = Bundle.decode(bundle.encode())
    verified = verify_bundle(decoded, **arguments)
    assert verified.final_vector == ((1, 32768), (2, 32768))
    assert sum(value for _, value in verified.final_vector) == 65536  # No post-round adjustment.
    assert Bundle.decode(bundle.encode()) == bundle


@pytest.fixture
def proportional_network(network):
    previous, arguments = network
    trust = replace(
        arguments["trust"],
        challenges=tuple(
            replace(entry, emission_share_bps=3000 if entry.id == b"bounty" else 7000)
            for entry in arguments["trust"].challenges
        ),
        challenges_version=2,
        introduced_epoch=12,
    )
    bundle = build_bundle(
        gateway_seed=bytes([7]) * 32,
        epoch=12,
        netuid=541,
        block_b=99,
        block_hash=arguments["block_hash"],
        rows=arguments["rows"],
        leaves=previous.body.leaves,
        trust=trust,
    )
    return bundle, {**arguments, "trust": trust}


def test_v2_owner_profile_selects_algorithm_and_cannot_downgrade(proportional_network):
    bundle, arguments = proportional_network
    assert verify_bundle(Bundle.decode(bundle.encode()), **arguments).algorithm_version == 2
    body = replace(bundle.body, algorithm_version=1)
    downgraded = Bundle(body, sign_raw(bytes([7]) * 32, BUNDLE_DOMAIN, body.encode()))
    with pytest.raises(ProtocolError, match="version"):
        verify_bundle(downgraded, **arguments)
    with pytest.raises(ProtocolError, match="version"):
        recompute(body, {b"bounty"})


def test_v2_quarantine_burns_mass_without_changing_the_other_share(proportional_network):
    bundle, _ = proportional_network
    result = recompute(bundle.body, {b"bounty"})
    assert result.weights == pytest.approx((0.3, 0.35, 0.35))
    with pytest.raises(ProtocolError, match="surviving share"):
        recompute(bundle.body, {b"proof"})
    assert recompute(bundle.body, {b"proof"}, minimum_share_mass=0).weights == pytest.approx(
        (0.7, 0.15, 0.15)
    )


def test_v2_profile_cannot_activate_before_its_owner_signed_epoch(proportional_network):
    bundle, arguments = proportional_network
    future_trust = replace(arguments["trust"], introduced_epoch=13)
    with pytest.raises(ProtocolError, match="not active"):
        verify_bundle(bundle, **{**arguments, "trust": future_trust})
    with pytest.raises(ProtocolError, match="not active"):
        build_bundle(
            gateway_seed=bytes([7]) * 32,
            epoch=12,
            netuid=541,
            block_b=99,
            block_hash=arguments["block_hash"],
            rows=arguments["rows"],
            leaves=bundle.body.leaves,
            trust=future_trust,
        )


def test_frozen_rust_wire_fixture_matches_exact_fields_and_signature():
    reference = json.loads((Path(__file__).parent / "vectors/rust_wire.json").read_text())
    raw = bytes.fromhex(reference["bundle_scale"])
    bundle = Bundle.decode(raw)
    assert bundle.encode() == raw
    assert bundle.body.leaves[0].encode().hex() == reference["leaf_scale"]
    assert bundle.body.netuid == 541
    assert bundle.body.epoch == 123
    assert bundle.body.emission_shares == ((b"bounty", 2000), (b"proof", 8000))
    assert bundle.body.merkle_root == merkle_root([bundle.body.leaves[0].encode()])
    assert verify_raw(
        bundle.body.gateway_hotkey, BUNDLE_DOMAIN, bundle.body.encode(), bundle.gateway_sig
    )


@pytest.mark.parametrize(
    "field,value,reason",
    [
        ("final_vector", ((1, 65535),), "final vector"),
        ("block_hash", bytes(32), "block hash"),
        ("metagraph_root", bytes(32), "metagraph root"),
        ("measurements_digest", bytes(32), "measurements"),
        ("emission_shares", ((b"bounty", 8000), (b"proof", 2000)), "emission"),
        ("netuid", 1, "subnet"),
        ("epoch", 13, "epoch"),
    ],
)
def test_rejects_validly_signed_but_dishonest_gateway_inputs(network, field, value, reason):
    bundle, arguments = network
    body = replace(bundle.body, **{field: value})
    forged = Bundle(body, sign_raw(bytes([7]) * 32, BUNDLE_DOMAIN, body.encode()))
    with pytest.raises(ProtocolError, match=reason):
        verify_bundle(forged, **arguments)


@pytest.mark.parametrize(
    "change,reason",
    [
        ("missing", "incomplete"),
        ("duplicate", "unique"),
        ("reorder", "sorted"),
        ("signature", "signature"),
        ("retired", "unknown challenge"),
    ],
)
def test_rejects_incomplete_duplicate_forged_or_retired_challenge_leaves(network, change, reason):
    bundle, arguments = network
    leaves = list(bundle.body.leaves)
    if change == "missing":
        leaves.pop()
    elif change == "duplicate":
        leaves.append(leaves[-1])
    elif change == "reorder":
        leaves.reverse()
    elif change == "signature":
        leaves[0] = replace(leaves[0], challenge_sig=bytes(64))
    else:
        leaves[0] = replace(leaves[0], challenge_id=b"prism")
        leaves.sort(key=lambda leaf: leaf.sort_key)
    body = replace(
        bundle.body, leaves=tuple(leaves), merkle_root=merkle_root(leaf.encode() for leaf in leaves)
    )
    forged = Bundle(body, sign_raw(bytes([7]) * 32, BUNDLE_DOMAIN, body.encode()))
    with pytest.raises(ProtocolError, match=reason):
        verify_bundle(forged, **arguments)


def test_unknown_gateway_signer_cannot_replace_owner_trust(network):
    bundle, arguments = network
    seed = bytes([8]) * 32
    body = replace(bundle.body, gateway_hotkey=public_key(seed))
    forged = Bundle(body, sign_raw(seed, BUNDLE_DOMAIN, body.encode()))
    with pytest.raises(ProtocolError, match="untrusted gateway"):
        verify_bundle(forged, **arguments)


def test_decode_rejects_trailing_bytes_and_compact_malleability(network):
    with pytest.raises(ProtocolError, match="trailing"):
        Bundle.decode(network[0].encode() + b"\x00")
    with pytest.raises(ProtocolError, match="noncanonical"):
        Reader(b"\x01\x00").compact()


def test_rfc6962_odd_leaf_promotion_matches_external_known_answer():
    assert merkle_root([b"", b"\x00", b"\x10"]).hex() == (
        "aeb6bcfe274b70a14fb067a5e5578264db0fa9b51af5e0ba159158f329e06e77"
    )
