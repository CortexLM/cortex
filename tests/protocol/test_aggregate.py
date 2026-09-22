import json
import struct
from pathlib import Path

import pytest

from cortex.protocol.aggregate import (
    ChallengeWeights,
    aggregate_challenge_weights,
    aggregate_leaves,
)
from cortex.protocol.models import LIVE_SHARES, Leaf, NoScore, Score
from cortex.protocol.scale import ProtocolError

VECTORS = sorted(
    path
    for path in (Path(__file__).parent / "vectors").glob("*.json")
    if path.name != "rust_wire.json"
)


@pytest.mark.parametrize("path", VECTORS, ids=lambda path: path.stem)
def test_upstream_served_vector_parity_bit_for_bit(path):
    vector = json.loads(path.read_text())
    inputs = vector["inputs"]
    results = [
        ChallengeWeights(
            result["slug"], result["emission_percent"], result["weights"], result.get("ok", True)
        )
        for result in inputs["challenge_results"]
    ]
    if "python_error" in vector:
        with pytest.raises(ProtocolError) as error:
            aggregate_challenge_weights(
                results, inputs["hotkey_to_uid"], **inputs.get("kwargs", {})
            )
        assert str(error.value) == vector["python_error"]
        return
    actual = aggregate_challenge_weights(
        results, inputs["hotkey_to_uid"], **inputs.get("kwargs", {})
    )
    assert [list(pair) for pair in actual.final_vector] == vector["expected_vector"]
    expected = vector["python_float_output"]
    assert actual.uids == tuple(expected["uids"])
    assert [struct.pack("d", value) for value in actual.weights] == [
        struct.pack("d", value) for value in expected["weights"]
    ]
    assert list(actual.hotkey_weights) == list(expected["hotkey_weights"])
    assert actual.hotkey_weights == expected["hotkey_weights"]


def test_unavailable_proof_burns_its_eighty_percent_without_reallocating_to_bounty():
    miner = bytes([1]) * 32
    leaves = (
        Leaf(b"bounty", miner, 1, Score(20), bytes(64)),
        Leaf(b"proof", miner, 1, NoScore(), bytes(64)),
    )
    result = aggregate_leaves(leaves, LIVE_SHARES, ((miner, 1),))
    assert result.weights == (0.8, 0.2)
    assert result.final_vector == ((0, 52428), (1, 13107))


def test_no_score_does_not_erase_other_miner_scores():
    miners = (bytes([1]) * 32, bytes([2]) * 32)
    leaves = (
        Leaf(b"proof", miners[0], 1, Score(100), bytes(64)),
        Leaf(b"proof", miners[1], 1, NoScore(), bytes(64)),
    )
    result = aggregate_leaves(leaves, LIVE_SHARES, ((miners[0], 1), (miners[1], 2)))
    assert result.final_vector == ((0, 13107), (1, 52428))


@pytest.mark.parametrize("reports", [0, 1, 5, 9, 10, 20])
def test_v2_bounty_pays_proportionally_up_to_thirty_percent(reports):
    miners = (bytes([1]) * 32, bytes([2]) * 32, bytes([3]) * 32)
    counts = (reports // 2, reports - reports // 2)
    leaves = tuple(
        Leaf(b"bounty", key, 1, Score(count), bytes(64))
        for key, count in zip(miners[:2], counts, strict=True)
    ) + (Leaf(b"proof", miners[2], 1, Score(100), bytes(64)),)
    result = aggregate_leaves(
        leaves,
        ((b"bounty", 3000), (b"proof", 7000)),
        tuple((key, index + 1) for index, key in enumerate(miners)),
        algorithm_version=2,
    )
    weights = dict(zip(result.uids, result.weights, strict=True))
    assert weights[3] == pytest.approx(0.7)
    assert weights.get(0, 0) == pytest.approx(0.3 * (1 - min(reports / 10, 1)))
    for uid, count in enumerate(counts, 1):
        assert weights.get(uid, 0) == pytest.approx(0.3 * count / max(10, reports))


def test_v2_burns_uid0_and_unmapped_authors_without_transferring_their_mass():
    miners = tuple(bytes([index]) * 32 for index in range(3))
    result = aggregate_leaves(
        tuple(Leaf(b"bounty", key, 1, Score(5), bytes(64)) for key in miners),
        ((b"bounty", 3000), (b"proof", 7000)),
        ((miners[0], 0), (miners[1], 1)),
        algorithm_version=2,
    )
    assert result.weights == pytest.approx((0.9, 0.1))


def test_v2_with_no_miner_burns_only_the_declared_proof_share():
    """Zero miners: bounty must not burn as much as proof.

    The operator's rule is that the burn at the end is the proof share, because
    the bounty allocation exists to pay reports. Padding the vector across
    arbitrary uids at equal weight said nothing about that: it burned the same
    amount whichever challenge was empty.
    """
    result = aggregate_leaves(
        (),
        ((b"bounty", 3000), (b"proof", 7000)),
        (),
        algorithm_version=2,
    )
    # The whole declared allocation burns, and proof is the larger part of it.
    assert sum(result.weights) == pytest.approx(0.7)
    assert result.hotkey_weights == {}


def test_v2_with_no_miner_keeps_bounty_below_proof():
    """The bounty side is the smaller burn, whichever uid carries it."""
    result = aggregate_leaves(
        (),
        ((b"bounty", 3000), (b"proof", 7000)),
        ((bytes([9]) * 32, 5),),
        algorithm_version=2,
    )
    # 0.7 spread over the uids the chain needs, never the full 1.0 the old
    # equal-padding produced.
    assert sum(result.weights) == pytest.approx(0.7)


def test_v2_rejects_other_share_profiles():
    with pytest.raises(ProtocolError, match="shares"):
        aggregate_leaves((), LIVE_SHARES, (), algorithm_version=2)


def test_proportional_profile_cannot_silently_use_default_legacy_algorithm():
    miner = bytes([1]) * 32
    with pytest.raises(ProtocolError, match="version 2"):
        aggregate_leaves(
            (Leaf(b"bounty", miner, 1, Score(1), bytes(64)),),
            ((b"bounty", 3000), (b"proof", 7000)),
            ((miner, 1),),
        )
