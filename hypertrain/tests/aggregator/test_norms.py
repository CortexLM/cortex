"""ht-norm-v1 vectorised sqnorm/dot are bitwise equal to the original math.fsum path."""

from __future__ import annotations

import math
import struct

import numpy as np
import pytest

from hypertrain.aggregator.core import (
    centered_clip,
    dot,
    dot_v1_reference,
    dot_v2,
    preclip,
    sqnorm,
    sqnorm_v1_reference,
    sqnorm_v2,
)

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


def _tree_reference(values: list[float]) -> float:
    if not values:
        return 0.0
    n = 1 << (len(values) - 1).bit_length()
    xs = values + [0.0] * (n - len(values))
    while len(xs) > 1:
        xs = [xs[i] + xs[i + 1] for i in range(0, len(xs), 2)]
    return xs[0]


@pytest.mark.parametrize("seed", range(50))
def test_norm_v2_equals_scalar_tree_spec_when_random_inputs(seed: int) -> None:
    rng = np.random.default_rng(seed)
    a = _params(rng)
    b = {n: rng.permutation(x) * np.float32(-0.75) for n, x in a.items()}
    order = sorted(a, key=str.encode)
    sq = [_tree_reference([float(v) * float(v) for v in a[n].tolist()]) for n in order]
    dt = [
        _tree_reference(
            [float(x) * float(y) for x, y in zip(a[n].tolist(), b[n].tolist(), strict=True)]
        )
        for n in order
    ]
    assert _bits(sqnorm_v2(a)) == _bits(math.fsum(sq))
    assert _bits(dot_v2(a, b)) == _bits(math.fsum(dt))


def test_norm_v2_close_to_exact_and_stable_when_repeated() -> None:
    x = {"w": np.random.default_rng(3).standard_normal(1_000_003).astype(np.float32)}
    runs = {_bits(sqnorm_v2(x)) for _ in range(3)}
    assert len(runs) == 1
    exact = sqnorm(x)
    assert abs(sqnorm_v2(x) - exact) <= 64 * math.ulp(exact)


def test_clip_paths_default_to_v1_and_accept_v2() -> None:
    d = {"a": np.full(10, 3.0, dtype=np.float32)}
    assert preclip(d, 1.0)[1] and preclip(d, 1.0, "ht-norm-v2")[1]
    c = {"a": np.zeros(10, dtype=np.float32)}
    v1 = centered_clip([d], [np.float32(1)], c, 1.0, 1)
    v2 = centered_clip([d], [np.float32(1)], c, 1.0, 1, "ht-norm-v2")
    assert v1["a"].tobytes() == v2["a"].tobytes()
