"""Tensor-state hashing (TH) and domain-separated Merkle trees.

Pure stdlib; no torch/numpy. Shared contract consumed by data/ and trainer/.
"""

from __future__ import annotations

import hashlib
import struct
from collections.abc import Iterable, Sequence

_LEAF = b"\x00"
_NODE = b"\x01"


def sha256_hex(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _lp(b: bytes) -> bytes:
    return struct.pack("<Q", len(b)) + b


def tensor_entry_digest(name: str, dtype: str, shape: tuple[int, ...], raw_le: bytes) -> bytes:
    """sha256 of u64-len-prefixed name|dtype|shape(u32 ndim, u64 dims)|raw LE bytes."""
    if any(d < 0 for d in shape):
        raise ValueError(f"negative dim in shape {shape!r}")
    shape_enc = struct.pack("<I", len(shape)) + b"".join(struct.pack("<Q", d) for d in shape)
    h = hashlib.sha256()
    h.update(_lp(name.encode("utf-8")))
    h.update(_lp(dtype.encode("utf-8")))
    h.update(_lp(shape_enc))
    h.update(_lp(raw_le))
    return h.digest()


def tensor_hash(entries: Iterable[tuple[str, str, tuple[int, ...], bytes]]) -> str:
    """TH(state): sha256 hex over concat of entry digests sorted by tensor name."""
    digests: dict[str, bytes] = {}
    for name, dtype, shape, raw in entries:
        if name in digests:
            raise ValueError(f"duplicate tensor name {name!r}")
        digests[name] = tensor_entry_digest(name, dtype, shape, raw)
    # sort by UTF-8 bytes so ordering is locale/runtime independent
    return sha256_hex(
        b"".join(digests[n] for n in sorted(digests, key=lambda s: s.encode("utf-8")))
    )


def _leaf_hash(leaf: bytes) -> bytes:
    return hashlib.sha256(_LEAF + leaf).digest()


def _node_hash(left: bytes, right: bytes) -> bytes:
    return hashlib.sha256(_NODE + left + right).digest()


class MerkleTree:
    """Binary Merkle tree: leaf=H(0x00|x), node=H(0x01|l|r), odd last node promoted."""

    def __init__(self, leaves: Sequence[bytes]) -> None:
        if not leaves:
            raise ValueError("MerkleTree needs at least one leaf")
        level = [_leaf_hash(x) for x in leaves]
        self._levels: list[list[bytes]] = [level]
        while len(level) > 1:
            nxt = [_node_hash(level[i], level[i + 1]) for i in range(0, len(level) - 1, 2)]
            if len(level) % 2:
                nxt.append(level[-1])
            self._levels.append(nxt)
            level = nxt

    @property
    def root(self) -> bytes:
        return self._levels[-1][0]

    @property
    def n_leaves(self) -> int:
        return len(self._levels[0])

    def proof(self, index: int) -> list[bytes]:
        if not 0 <= index < self.n_leaves:
            raise IndexError(index)
        out: list[bytes] = []
        for level in self._levels[:-1]:
            sib = index ^ 1
            if sib < len(level):
                out.append(level[sib])
            index //= 2
        return out

    @staticmethod
    def proof_length(index: int, n_leaves: int) -> int:
        length, width = 0, n_leaves
        while width > 1:
            length += (index ^ 1) < width
            index //= 2
            width = (width + 1) // 2
        return length

    @staticmethod
    def verify(leaf: bytes, index: int, proof: Sequence[bytes], root: bytes, n_leaves: int) -> bool:
        if n_leaves < 1 or not 0 <= index < n_leaves:
            return False
        if len(proof) != MerkleTree.proof_length(index, n_leaves):
            return False
        h = _leaf_hash(leaf)
        width = n_leaves
        it = iter(proof)
        while width > 1:
            sib = index ^ 1
            if sib < width:
                s = next(it, None)
                if s is None or len(s) != 32:
                    return False
                h = _node_hash(h, s) if index % 2 == 0 else _node_hash(s, h)
            index //= 2
            width = (width + 1) // 2
        return next(it, None) is None and h == root
