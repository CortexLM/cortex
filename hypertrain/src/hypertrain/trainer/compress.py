"""Canonical delta codecs with committed error feedback (EF).

acc = ef_beta * ef_in + delta ; payload = encode(acc) ; ef_out = acc - decode(payload)

dense-int8 ("ht-dense-int8-v1"): per tensor, blocks of 256, scale = amax/127 (f32), values
  round-half-even(acc/scale) clamped to [-127, 127].
sparseloco ("ht-sparse-v1"): per tensor, k = ceil(topk_frac * numel) entries chosen by canonical
  stable top-k (|x| desc, then index asc; never torch.topk), indices stored ascending (u32),
  2-bit codes = sign bit | magnitude bit, magnitude levels lo + (hi-lo)/4 and lo + 3(hi-lo)/4
  with lo/hi = min/max selected |x| (f32).
All tensors are processed in UTF-8-sorted name order; every field is little-endian.
"""

from __future__ import annotations

import hashlib
import math
import struct
from collections.abc import Mapping

import numpy as np
import torch
from torch import Tensor

from hypertrain.protocol.hashing import tensor_hash
from hypertrain.trainer.config import CompressConfig

Params = dict[str, Tensor]
BLOCK = 256
MAGIC = {"dense-int8": b"ht-dense-int8-v1", "sparseloco": b"ht-sparse-v1"}


def _sorted_names(t: Params) -> list[str]:
    return sorted(t, key=lambda s: s.encode("utf-8"))


def state_hash(t: Params) -> str:
    entries = []
    for name, x in t.items():
        if x.dtype != torch.float32:
            raise ValueError(f"{name}: hashed state must be float32")
        raw = x.detach().cpu().contiguous().numpy().astype("<f4", copy=False).tobytes()
        entries.append((name, "f32", tuple(x.shape), raw))
    return tensor_hash(entries)


def canonical_topk(x: Tensor, k: int) -> Tensor:
    """Indices of the k largest |x| (ties -> lower index), returned in ascending index order."""
    flat = x.reshape(-1)
    if not 1 <= k <= flat.numel():
        raise ValueError("invalid top-k budget")
    if not bool(torch.isfinite(flat).all()):
        raise ValueError("nonfinite compression input")
    mag = flat.detach().cpu().abs().numpy().astype(np.float64)
    order = np.lexsort((np.arange(mag.size), -mag))[:k]
    return torch.from_numpy(np.sort(order).astype(np.int64))


def _int8_encode(acc: Tensor) -> tuple[bytes, Tensor]:
    flat = acc.reshape(-1)
    n = flat.numel()
    blocks = (n + BLOCK - 1) // BLOCK
    pad = torch.zeros(blocks * BLOCK, dtype=torch.float32, device="cpu")
    pad[:n] = flat
    pad = pad.view(blocks, BLOCK)
    scales = pad.abs().amax(1) / 127.0
    scales = torch.where(scales > 0, scales, torch.ones_like(scales))
    q = torch.round(pad / scales[:, None]).clamp_(-127, 127).to(torch.int8)
    deq = (q.float() * scales[:, None]).reshape(-1)[:n].reshape(acc.shape)
    body = (
        struct.pack("<I", BLOCK)
        + scales.numpy().astype("<f4").tobytes()
        + q.reshape(-1)[:n].numpy().tobytes()
    )
    return body, deq


def _int8_decode(body: bytes, shape: tuple[int, ...]) -> Tensor:
    n = math.prod(shape)
    blocks = (n + BLOCK - 1) // BLOCK
    if len(body) != 4 + 4 * blocks + n or struct.unpack_from("<I", body)[0] != BLOCK:
        raise ValueError("malformed int8 tensor body")
    scales = np.frombuffer(body, "<f4", blocks, 4).astype(np.float32)
    q = np.frombuffer(body, np.int8, n, 4 + 4 * blocks).astype(np.float32)
    if not np.isfinite(scales).all() or (scales <= 0).any():
        raise ValueError("invalid int8 scales")
    s = torch.from_numpy(scales.copy()).repeat_interleave(BLOCK)[:n]
    return (torch.from_numpy(q.copy()) * s).reshape(shape)


def _levels(lo: float, hi: float) -> tuple[np.float32, np.float32]:
    lo32, hi32 = np.float32(lo), np.float32(hi)
    span = np.float32(hi32 - lo32)
    return np.float32(lo32 + span * np.float32(0.25)), np.float32(lo32 + span * np.float32(0.75))


def _sparse_encode(acc: Tensor, frac: float) -> tuple[bytes, Tensor]:
    flat = acc.reshape(-1)
    n = flat.numel()
    k = min(n, max(1, math.ceil(frac * n)))
    idx = canonical_topk(flat, k)
    vals = flat.index_select(0, idx).numpy().astype(np.float32)
    mags = np.abs(vals)
    lo, hi = np.float32(mags.min()), np.float32(mags.max())
    l0, l1 = _levels(float(lo), float(hi))
    mid = np.float32(lo + np.float32(hi - lo) * np.float32(0.5))
    mbit = (mags > mid).astype(np.uint8)
    sbit = (vals < 0).astype(np.uint8)
    codes = (sbit << 1) | mbit
    packed = np.zeros((k + 3) // 4, dtype=np.uint8)
    for j in range(4):
        part = codes[j::4]
        packed[: part.size] |= part << (2 * j)
    deq_vals = np.where(mbit == 1, l1, l0).astype(np.float32) * np.where(sbit == 1, -1, 1).astype(
        np.float32
    )
    deq = torch.zeros(n, dtype=torch.float32, device="cpu")
    deq[idx] = torch.from_numpy(deq_vals)
    body = (
        struct.pack("<Q", k)
        + struct.pack("<ff", lo, hi)
        + idx.numpy().astype("<u4").tobytes()
        + packed.tobytes()
    )
    return body, deq.reshape(acc.shape)


def _sparse_decode(body: bytes, shape: tuple[int, ...]) -> Tensor:
    n = math.prod(shape)
    if len(body) < 16:
        raise ValueError("malformed sparse tensor body")
    (k,) = struct.unpack_from("<Q", body)
    lo, hi = struct.unpack_from("<ff", body, 8)
    if not 1 <= k <= n or len(body) != 16 + 4 * k + (k + 3) // 4:
        raise ValueError("malformed sparse tensor body")
    if not (math.isfinite(lo) and math.isfinite(hi) and 0 <= lo <= hi):
        raise ValueError("invalid sparse levels")
    idx = np.frombuffer(body, "<u4", k, 16).astype(np.int64)
    if (idx >= n).any() or (np.diff(idx) <= 0).any():
        raise ValueError("sparse indices must be strictly ascending and in range")
    packed = np.frombuffer(body, np.uint8, (k + 3) // 4, 16 + 4 * k)
    codes = np.empty(k, dtype=np.uint8)
    for j in range(4):
        codes[j::4] = (packed[: codes[j::4].size] >> (2 * j)) & 3
    l0, l1 = _levels(lo, hi)
    vals = np.where(codes & 1, l1, l0).astype(np.float32) * np.where(codes & 2, -1, 1).astype(
        np.float32
    )
    out = torch.zeros(n, dtype=torch.float32, device="cpu")
    out[torch.from_numpy(idx)] = torch.from_numpy(vals)
    return out.reshape(shape)


def _lp(b: bytes) -> bytes:
    return struct.pack("<Q", len(b)) + b


def compress(cfg: CompressConfig, delta: Params, ef_in: Params) -> tuple[bytes, Params]:
    """Return (canonical payload bytes, ef_out)."""
    if sorted(delta) != sorted(ef_in):
        raise ValueError("ef_in must cover exactly the delta tensors")
    out = [MAGIC[cfg.codec], struct.pack("<I", len(delta))]
    ef_out: Params = {}
    for name in _sorted_names(delta):
        d, e = delta[name], ef_in[name]
        if d.dtype != torch.float32 or e.dtype != torch.float32 or d.shape != e.shape:
            raise ValueError(f"{name}: delta/ef must be float32 with equal shapes")
        acc = (e * cfg.ef_beta + d) if cfg.ef_beta != 1.0 else (e + d)
        host = acc.detach().cpu()
        if cfg.codec == "dense-int8":
            body, deq = _int8_encode(host)
        else:
            body, deq = _sparse_encode(host, cfg.topk_frac)
        shape = struct.pack("<I", d.ndim) + b"".join(struct.pack("<Q", s) for s in d.shape)
        out += [_lp(name.encode("utf-8")), shape, _lp(body)]
        ef_out[name] = (host - deq).to(acc.device)
    return b"".join(out), ef_out


def _payload_metadata(
    payload: bytes,
    expected_shapes: Mapping[str, tuple[int, ...]] | None,
    max_payload_bytes: int | None,
) -> tuple[str, dict[str, tuple[tuple[int, ...], int, int]]]:
    """Parse all framing and validate bodies before any tensor allocation."""
    if max_payload_bytes is not None and len(payload) > max_payload_bytes:
        raise ValueError("delta payload exceeds byte limit")
    codec = next((c for c, m in MAGIC.items() if payload.startswith(m)), None)
    if codec is None:
        raise ValueError("unknown delta payload magic")
    off = len(MAGIC[codec])
    try:
        (count,) = struct.unpack_from("<I", payload, off)
        off += 4
        if count > (len(payload) - off) // 20:
            raise ValueError("tensor count exceeds payload framing")
        if expected_shapes is not None and count != len(expected_shapes):
            raise ValueError("tensor count differs from expected shapes")
        out: dict[str, tuple[tuple[int, ...], int, int]] = {}
        for _ in range(count):
            (ln,) = struct.unpack_from("<Q", payload, off)
            if ln > len(payload) - off - 8:
                raise ValueError("truncated tensor name")
            raw_name = payload[off + 8 : off + 8 + ln]
            name = raw_name.decode("utf-8")
            if not name or (out and raw_name <= next(reversed(out)).encode("utf-8")):
                raise ValueError("tensor names must be unique and UTF-8 sorted")
            if expected_shapes is not None and name not in expected_shapes:
                raise ValueError("unexpected tensor name")
            off += 8 + ln
            (ndim,) = struct.unpack_from("<I", payload, off)
            if ndim > (len(payload) - off - 4) // 8:
                raise ValueError("tensor rank exceeds payload framing")
            if expected_shapes is not None and ndim != len(expected_shapes[name]):
                raise ValueError("tensor rank differs from expected shape")
            shape = struct.unpack_from(f"<{ndim}Q", payload, off + 4)
            if expected_shapes is not None and shape != expected_shapes[name]:
                raise ValueError("tensor dimensions differ from expected shape")
            off += 4 + 8 * ndim
            (blen,) = struct.unpack_from("<Q", payload, off)
            if blen > len(payload) - off - 8:
                raise ValueError("truncated tensor body")
            start = off + 8
            off += 8 + blen
            body = memoryview(payload)[start:off]
            n = math.prod(shape)
            if codec == "dense-int8":
                blocks = (n + BLOCK - 1) // BLOCK
                if blen != 4 + 4 * blocks + n or struct.unpack_from("<I", body)[0] != BLOCK:
                    raise ValueError("malformed int8 tensor body")
                if any(
                    not math.isfinite(s) or s <= 0
                    for (s,) in struct.iter_unpack("<f", body[4 : 4 + 4 * blocks])
                ):
                    raise ValueError("invalid int8 scales")
                if 128 in body[4 + 4 * blocks :]:
                    raise ValueError("invalid int8 value")
            else:
                if blen < 16:
                    raise ValueError("malformed sparse tensor body")
                k, lo, hi = struct.unpack_from("<Qff", body)
                if not 1 <= k <= n or blen != 16 + 4 * k + (k + 3) // 4:
                    raise ValueError("malformed sparse tensor body")
                if not (math.isfinite(lo) and math.isfinite(hi) and 0 <= lo <= hi):
                    raise ValueError("invalid sparse levels")
                previous = -1
                for (index,) in struct.iter_unpack("<I", body[16 : 16 + 4 * k]):
                    if not previous < index < n:
                        raise ValueError("sparse indices must be strictly ascending and in range")
                    previous = index
                if k % 4 and body[-1] >> (2 * (k % 4)):
                    raise ValueError("nonzero sparse padding bits")
            out[name] = (tuple(shape), start, off)
    except struct.error as exc:
        raise ValueError("truncated delta payload") from exc
    if off != len(payload):
        raise ValueError("trailing bytes in delta payload")
    return codec, out


def validate_payload(
    payload: bytes,
    expected_shapes: Mapping[str, tuple[int, ...]],
    *,
    max_payload_bytes: int = 65536,
) -> str:
    """Bound raw bytes and validate manifest geometry without decoding tensors."""
    codec, _ = _payload_metadata(payload, expected_shapes, max_payload_bytes)
    return codec


def decompress(
    payload: bytes,
    *,
    expected_shapes: Mapping[str, tuple[int, ...]] | None = None,
    max_payload_bytes: int | None = None,
) -> tuple[str, Params]:
    """Decode only after every tensor passes framing and optional manifest bounds."""
    codec, metadata = _payload_metadata(payload, expected_shapes, max_payload_bytes)
    dec = _int8_decode if codec == "dense-int8" else _sparse_decode
    return codec, {n: dec(payload[start:end], shape) for n, (shape, start, end) in metadata.items()}


def payload_hash(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()
