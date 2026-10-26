import math
import struct

import numpy as np
import pytest
import torch
from hypothesis import given, settings
from hypothesis import strategies as st

from hypertrain.trainer.codec_v2 import _lp, _z, decode_v2, encode_v2
from hypertrain.trainer.compress import compress, decompress
from hypertrain.trainer.config import CompressConfig

SPARSE = CompressConfig(codec="sparseloco", topk_frac=0.03125, bits=2, ef_beta=0.95)
DENSE = CompressConfig()


def _delta(shapes: dict[str, tuple[int, ...]], seed: int) -> dict[str, torch.Tensor]:
    rng = np.random.default_rng(seed)
    return {n: torch.from_numpy(rng.standard_t(3, s).astype(np.float32)) for n, s in shapes.items()}


SHAPES = {"a.weight": (64, 300), "b.norm": (300,), "c": (1,)}


@pytest.mark.parametrize("cfg", [SPARSE, DENSE])
def test_roundtrip_identical_v1_bytes(cfg: CompressConfig) -> None:
    d = _delta(SHAPES, 0)
    v1, _ = compress(cfg, d, {n: torch.zeros_like(x) for n, x in d.items()})
    v2 = encode_v2(v1)
    assert decode_v2(v2, expected_shapes=SHAPES, max_payload_bytes=len(v1)) == v1
    assert encode_v2(v1) == v2


def test_sparse_v2_smaller_on_proxy() -> None:
    d = {"w": torch.from_numpy(np.random.default_rng(1).standard_t(3, 1 << 20).astype("f4"))}
    v1, _ = compress(SPARSE, d, {"w": torch.zeros(1 << 20)})
    v2 = encode_v2(v1)
    k = math.ceil(0.03125 * (1 << 20))
    p = k / (1 << 20)
    bound = k * (-(1 - p) * math.log2(1 - p) - p * math.log2(p)) / p / 8 + k / 4 + 16
    assert len(v1) / len(v2) > 3.9
    assert len(v2) < 1.05 * bound


@pytest.mark.parametrize("cfg", [SPARSE, DENSE])
def test_rejects_corruption(cfg: CompressConfig) -> None:
    d = _delta(SHAPES, 2)
    v1, _ = compress(cfg, d, {n: torch.zeros_like(x) for n, x in d.items()})
    v2 = encode_v2(v1)
    for bad in (v2[:-1], v2 + b"\0", b"ht-bogus-v2" + v2[12:]):
        with pytest.raises(ValueError):
            decode_v2(bad)
    with pytest.raises(ValueError):
        decode_v2(v2, max_payload_bytes=len(v1) - 1)
    with pytest.raises(ValueError):
        decode_v2(v2, expected_shapes={**SHAPES, "c": (2,)})


def test_rejects_noncanonical_zstd_level() -> None:
    d = _delta({"w": (4096,)}, 3)
    v1, _ = compress(DENSE, d, {"w": torch.zeros(4096)})
    v2 = encode_v2(v1)
    canonical = _lp(_z(v1[-4096:]))
    assert v2.endswith(canonical)
    import zstandard

    other = _lp(zstandard.ZstdCompressor(level=1).compress(v1[-4096:]))
    assert other != canonical
    with pytest.raises(ValueError, match="non-canonical"):
        decode_v2(v2[: -len(canonical)] + other)


def test_rejects_declared_size_bomb() -> None:
    v1, _ = compress(DENSE, {"w": torch.ones(16)}, {"w": torch.zeros(16)})
    v2 = encode_v2(v1)
    bomb = bytearray(v2)
    off = len(b"ht-dense-int8-v2") + 4 + 8 + 1 + 4
    struct.pack_into("<Q", bomb, off, 1 << 40)
    with pytest.raises(ValueError):
        decode_v2(bytes(bomb), max_payload_bytes=1 << 20)


def test_rejects_index_overflow_and_duplicates() -> None:
    v1, _ = compress(SPARSE, {"w": torch.arange(1.0, 65.0)}, {"w": torch.zeros(64)})
    v2 = encode_v2(v1)
    k = struct.unpack_from("<Q", v1, len(b"ht-sparse-v1") + 4 + 8 + 1 + 4 + 8 + 8)[0]
    gaps = np.zeros(k, "<u4")
    head = v2[: v2.index(b"w") + 1 + 4 + 8 + 16]
    tail = _lp(_z(v1[-((k + 3) // 4) :]))
    with pytest.raises(ValueError):
        decode_v2(head + _lp(_z(gaps.reshape(-1, 1).view(np.uint8).T.tobytes())) + tail)


@settings(max_examples=40, deadline=None)
@given(
    shape=st.lists(st.integers(1, 40), min_size=1, max_size=3),
    frac=st.floats(0.01, 1.0),
    seed=st.integers(0, 2**31),
    special=st.sampled_from([None, 0.0, 1e30, -1e-30]),
)
def test_property_roundtrip_and_values(
    shape: list[int], frac: float, seed: int, special: float | None
) -> None:
    x = torch.from_numpy(np.random.default_rng(seed).standard_normal(shape).astype(np.float32))
    if special is not None:
        x.view(-1)[0] = special
    for cfg in (DENSE, CompressConfig(codec="sparseloco", topk_frac=frac, bits=2)):
        v1, _ = compress(cfg, {"w": x}, {"w": torch.zeros_like(x)})
        back = decode_v2(encode_v2(v1), expected_shapes={"w": tuple(shape)})
        assert back == v1
        assert torch.equal(decompress(back)[1]["w"], decompress(v1)[1]["w"])
