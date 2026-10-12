"""ht-norm-v1 vectorised sqnorm/dot are bitwise equal to the original math.fsum path."""

from __future__ import annotations

import struct

import numpy as np
import pytest

from hypertrain.aggregator.core import dot, dot_v1_reference, sqnorm, sqnorm_v1_reference

SPECIAL = np.array(
    [0.0, -0.0, 1e-45, -1e-45, 1.17549435e-38, 3.4028235e38, -3.4028235e38, 1.0, -1.0],
    dtype=np.float32,
)


def _bits(x: float) -> bytes:
    return struct.pack("<d", x)


def _params(rng: np.random.Generator) -> dict[str, np.ndarray]:
    out = {}
    for i in range(int(rng.integers(1, 5))):
        n = int(rng.integers(0, 3000))
        scale = 10.0 ** float(rng.integers(-44, 37))
        out[f"t{i}"] = (rng.standard_normal(n) * scale).astype(np.float32)
    if rng.integers(0, 3) == 0:
        out["special"] = rng.permutation(SPECIAL)
    return out


@pytest.mark.parametrize("seed", range(300))
def test_bitwise_equal_to_v1_when_random_inputs(seed: int) -> None:
    rng = np.random.default_rng(seed)
    a = _params(rng)
    b = {n: rng.permutation(x) * np.float32(-0.75) for n, x in a.items()}
    assert _bits(sqnorm(a)) == _bits(sqnorm_v1_reference(a))
    assert _bits(dot(a, b)) == _bits(dot_v1_reference(a, b))


def test_bitwise_equal_to_v1_when_chunk_boundary_crossed() -> None:
    rng = np.random.default_rng(7)
    a = {"big": rng.standard_normal((1 << 22) + 17).astype(np.float32)}
    assert _bits(sqnorm(a)) == _bits(sqnorm_v1_reference(a))


def test_empty_and_zero_inputs_match_v1() -> None:
    assert _bits(sqnorm({})) == _bits(sqnorm_v1_reference({})) == _bits(0.0)
    z = {"z": np.array([-0.0, -0.0], dtype=np.float32)}
    assert _bits(dot(z, z)) == _bits(dot_v1_reference(z, z))
