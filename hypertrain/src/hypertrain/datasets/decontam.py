"""Offline decontamination (A13): 13-word shingles and 128-permutation numpy MinHash."""

from __future__ import annotations

import re
import unicodedata
from collections.abc import Iterable, Sequence

import numpy as np
import numpy.typing as npt

SHINGLE = 13
N_PERM = 128
_P = (1 << 61) - 1
_PUNCT = re.compile(r"[^\w\s]", re.UNICODE)


def normalize(text: str) -> list[str]:
    """NFKC, lowercase, strip punctuation, collapse whitespace -> words."""
    t = _PUNCT.sub(" ", unicodedata.normalize("NFKC", text).lower())
    return t.split()


def shingles(words: Sequence[str], n: int = SHINGLE) -> set[str]:
    return {" ".join(words[i : i + n]) for i in range(len(words) - n + 1)}


def _h64(s: str) -> int:
    import hashlib

    return int.from_bytes(hashlib.sha256(s.encode()).digest()[:8], "little") % _P


def _perms() -> tuple[npt.NDArray[np.uint64], npt.NDArray[np.uint64]]:
    rng = np.random.Generator(np.random.Philox(key=0x4D494E48))  # fixed: signatures are portable
    a = rng.integers(1, 1 << 31, N_PERM, dtype=np.uint64)
    b = rng.integers(0, 1 << 31, N_PERM, dtype=np.uint64)
    return a, b


_A, _B = _perms()


def minhash(words: Sequence[str], n: int = 3) -> npt.NDArray[np.uint64]:
    """128-permutation MinHash over word n-grams (all-ones signature for empty input)."""
    grams = shingles(words, n) or {" ".join(words)}
    h = np.array([_h64(g) & 0xFFFFFFFF for g in grams], dtype=np.uint64)
    # (a*h + b) mod p with a,h < 2**32 stays inside uint64 for p = 2**61-1 only after the mod
    return ((h[None, :] * _A[:, None] + _B[:, None]) % np.uint64(_P)).min(axis=1)


def jaccard_estimate(x: npt.NDArray[np.uint64], y: npt.NDArray[np.uint64]) -> float:
    return float(np.mean(x == y))


class Decontaminator:
    def __init__(self, eval_texts: Iterable[str], minhash_threshold: float | None = None) -> None:
        self.shingles: set[str] = set()
        self.sigs: list[npt.NDArray[np.uint64]] = []
        self.threshold = minhash_threshold
        for t in eval_texts:
            w = normalize(t)
            self.shingles |= shingles(w)
            if minhash_threshold is not None:
                self.sigs.append(minhash(w))

    def contaminated(self, text: str) -> bool:
        w = normalize(text)
        if not self.shingles.isdisjoint(shingles(w)):
            return True
        if self.threshold is not None and self.sigs:
            sig = minhash(w)
            return any(jaccard_estimate(sig, e) >= self.threshold for e in self.sigs)
        return False
