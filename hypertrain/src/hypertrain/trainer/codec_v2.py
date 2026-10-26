"""Codec v2: canonical lossless container over the exact v1 delta payload (compress.py).

encode_v2(v1) splits each tensor body into streams and zstd-compresses each one:
  dense-int8: f32 scales byte-plane shuffled | int8 values
  sparseloco: u32 index gaps (first gap = first index) byte-plane shuffled | packed 2-bit codes
decode_v2(v2) rebuilds the identical v1 bytes, validated by compress._payload_metadata.
The lossy step is untouched, so ef_out and every dequantised value equal v1 bit for bit.
Canonical form: zstandard==0.25.0 at level 19, content size on, checksum off, no dictionary,
single thread. decode_v2 re-encodes and rejects any other encoding of the same v1 bytes.
"""

from __future__ import annotations

import math
import struct
from collections.abc import Mapping

import numpy as np
import zstandard

from hypertrain.trainer.compress import BLOCK, MAGIC, _payload_metadata

ZSTD_VERSION = "0.25.0"
LEVEL = 19
MAGIC_V2 = {"dense-int8": b"ht-dense-int8-v2", "sparseloco": b"ht-sparse-v2"}

if zstandard.__version__ != ZSTD_VERSION:
    raise ImportError(f"codec v2 requires zstandard=={ZSTD_VERSION}")


def _z(data: bytes) -> bytes:
    c = zstandard.ZstdCompressor(
        level=LEVEL, write_content_size=True, write_checksum=False, threads=0
    )
    return c.compress(data)


def _unz(frame: bytes, size: int) -> bytes:
    if zstandard.frame_content_size(frame) != size:
        raise ValueError("stream size differs from tensor geometry")
    out = zstandard.ZstdDecompressor().decompress(frame, max_output_size=size)
    if len(out) != size:
        raise ValueError("stream size differs from tensor geometry")
    return out


def _shuffle4(raw: bytes) -> bytes:
    return np.frombuffer(raw, np.uint8).reshape(-1, 4).T.tobytes()


def _unshuffle4(raw: bytes) -> bytes:
    return np.frombuffer(raw, np.uint8).reshape(4, -1).T.tobytes()


def _lp(b: bytes) -> bytes:
    return struct.pack("<Q", len(b)) + b


def encode_v2(v1: bytes) -> bytes:
    codec, meta = _payload_metadata(v1, None, None)
    out = [MAGIC_V2[codec], struct.pack("<I", len(meta))]
    for name, (shape, start, end) in meta.items():
        body = v1[start:end]
        out += [_lp(name.encode("utf-8")), struct.pack(f"<I{len(shape)}Q", len(shape), *shape)]
        if codec == "dense-int8":
            split = 4 + 4 * ((math.prod(shape) + BLOCK - 1) // BLOCK)
            out += [_lp(_z(_shuffle4(body[4:split]))), _lp(_z(body[split:]))]
        else:
            (k,) = struct.unpack_from("<Q", body)
            idx = np.frombuffer(body, "<u4", k, 16)
            gaps = np.diff(idx, prepend=np.uint32(0)).astype("<u4")
            out += [body[:16], _lp(_z(_shuffle4(gaps.tobytes()))), _lp(_z(body[16 + 4 * k :]))]
    return b"".join(out)


class _Reader:
    def __init__(self, data: bytes, off: int) -> None:
        self.data, self.off = data, off

    def take(self, n: int) -> bytes:
        if n > len(self.data) - self.off:
            raise ValueError("truncated v2 payload")
        b = self.data[self.off : self.off + n]
        self.off += n
        return b

    def unpack(self, fmt: str) -> tuple[int, ...]:
        return struct.unpack(fmt, self.take(struct.calcsize(fmt)))

    def lp(self) -> bytes:
        return self.take(self.unpack("<Q")[0])


def decode_v2(
    v2: bytes,
    *,
    expected_shapes: Mapping[str, tuple[int, ...]] | None = None,
    max_payload_bytes: int | None = None,
) -> bytes:
    """v2 bytes -> identical v1 bytes; sizes are bounded before any stream is inflated."""
    codec = next((c for c, m in MAGIC_V2.items() if v2.startswith(m)), None)
    if codec is None:
        raise ValueError("unknown v2 payload magic")
    r = _Reader(v2, len(MAGIC_V2[codec]))
    (count,) = r.unpack("<I")
    if count > len(v2) // 20:
        raise ValueError("tensor count exceeds payload framing")
    out = [MAGIC[codec], struct.pack("<I", count)]
    total = sum(map(len, out))
    for _ in range(count):
        name = r.lp()
        (ndim,) = r.unpack("<I")
        if ndim > (len(v2) - r.off) // 8:
            raise ValueError("tensor rank exceeds payload framing")
        shape = r.unpack(f"<{ndim}Q")
        n = math.prod(shape)
        if codec == "dense-int8":
            n_scales = 4 * ((n + BLOCK - 1) // BLOCK)
            sizes = (n_scales, n)
            head = struct.pack("<I", BLOCK)
        else:
            head = r.take(16)
            (k,) = struct.unpack_from("<Q", head)
            if not 1 <= k <= n:
                raise ValueError("malformed sparse tensor body")
            sizes = (4 * k, (k + 3) // 4)
        blen = len(head) + sum(sizes)
        total += 8 + len(name) + 4 + 8 * ndim + 8 + blen
        if max_payload_bytes is not None and total > max_payload_bytes:
            raise ValueError("delta payload exceeds byte limit")
        a, b = _unz(r.lp(), sizes[0]), _unz(r.lp(), sizes[1])
        if codec == "dense-int8":
            body = head + _unshuffle4(a) + b
        else:
            gaps = np.frombuffer(_unshuffle4(a), "<u4")
            idx = np.cumsum(gaps, dtype=np.uint64)
            if idx[-1] >= min(n, 1 << 32):
                raise ValueError("sparse indices must be strictly ascending and in range")
            body = head + idx.astype("<u4").tobytes() + b
        out += [_lp(name), struct.pack(f"<I{ndim}Q", ndim, *shape), _lp(body)]
    if r.off != len(v2):
        raise ValueError("trailing bytes in v2 payload")
    v1 = b"".join(out)
    _payload_metadata(v1, expected_shapes, max_payload_bytes)
    # ponytail: canonicality costs one extra encode per validation; replace with a spec'd
    # stream-by-stream comparison if validation time shows up in round wall time (plan 2.8).
    if encode_v2(v1) != v2:
        raise ValueError("non-canonical v2 encoding")
    return v1
