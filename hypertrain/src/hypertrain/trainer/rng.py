"""Philox4x32-10 counter RNG (pure numpy, host-independent) keyed by (run_id, w, step, layer)."""

from __future__ import annotations

import hashlib
import struct

import numpy as np
import numpy.typing as npt

_M0, _M1 = np.uint64(0xD2511F53), np.uint64(0xCD9E8D57)
_W0, _W1 = np.uint32(0x9E3779B9), np.uint32(0xBB67AE85)
_MASK = np.uint64(0xFFFFFFFF)
U32 = npt.NDArray[np.uint32]
RNG_CTR_MAX = 2**53 - 1


def philox4x32(counters: U32, key: tuple[int, int], rounds: int = 10) -> U32:
    """counters: uint32 [n, 4]; returns uint32 [n, 4]."""
    c = counters.astype(np.uint32, copy=True)
    k0, k1 = np.uint32(key[0]), np.uint32(key[1])
    for _ in range(rounds):
        p0 = _M0 * c[:, 0].astype(np.uint64)
        p1 = _M1 * c[:, 2].astype(np.uint64)
        hi0, lo0 = (p0 >> np.uint64(32)).astype(np.uint32), (p0 & _MASK).astype(np.uint32)
        hi1, lo1 = (p1 >> np.uint64(32)).astype(np.uint32), (p1 & _MASK).astype(np.uint32)
        c = np.stack([hi1 ^ c[:, 1] ^ k0, lo1, hi0 ^ c[:, 3] ^ k1, lo0], axis=1)
        k0 = np.uint32((int(k0) + int(_W0)) & 0xFFFFFFFF)
        k1 = np.uint32((int(k1) + int(_W1)) & 0xFFFFFFFF)
    return c


def rng_ctr(run_id: str, w: int, step: int, layer: int) -> int:
    """53-bit Philox key for (run_id, w, step, layer); fits protocol U (<= 2^53-1) as rng_ctr."""
    msg = b"ht-rng|" + run_id.encode() + struct.pack("<QQq", w, step, layer)
    return int.from_bytes(hashlib.sha256(msg).digest()[:8], "little") & RNG_CTR_MAX


def uniform_f64(n: int, ctr: int) -> npt.NDArray[np.float64]:
    """n uniforms in [0, 1) with 24-bit resolution; exact (no transcendental functions)."""
    blocks = (n + 3) // 4
    counters = np.zeros((blocks, 4), dtype=np.uint32)
    counters[:, 0] = np.arange(blocks, dtype=np.uint64).astype(np.uint32)
    counters[:, 1] = (np.arange(blocks, dtype=np.uint64) >> np.uint64(32)).astype(np.uint32)
    out = philox4x32(counters, (ctr & 0xFFFFFFFF, ctr >> 32)).reshape(-1)[:n]
    return (out >> np.uint32(8)).astype(np.float64) * (2.0**-24)
