"""Fixed-size u32 shards of u32[seq_len+1] samples, sample Merkle root and shard_sha256_root.

Shard file = raw little-endian u32, shape (samples_per_shard, seq_len+1). Only token ids are hashed.
Merkle leaf i = raw LE bytes of sample i (global index across shards, shard order).
"""

from __future__ import annotations

import hashlib
import json
from collections.abc import Iterable, Iterator, Sequence
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Any, overload

import numpy as np
import numpy.typing as npt

from hypertrain.protocol.hashing import MerkleTree

U32 = np.dtype("<u4")
SAMPLE_FORMAT = "u32[seq_len+1] token ids"


def shard_name(i: int) -> str:
    return f"shard-{i:05d}.u32"


@dataclass
class ShardSetManifest:
    seq_len: int
    samples_per_shard: int
    n_shards: int
    n_samples: int
    depth: int
    merkle_root: str
    shard_sha256s: list[str]
    shard_sha256_root: str
    tokenizer: dict[str, str]
    sample_format: str = SAMPLE_FORMAT
    shard_uri_template: str = "{shard_sha256}"
    extra: dict[str, Any] = field(default_factory=dict)

    def to_json(self) -> str:
        return json.dumps(asdict(self), indent=2, sort_keys=True)

    @classmethod
    def from_json(cls, s: str) -> ShardSetManifest:
        return cls(**json.loads(s))


def pack_samples(
    docs: Iterable[Sequence[int] | npt.NDArray[np.uint32]], seq_len: int, eos_id: int
) -> Iterator[npt.NDArray[np.uint32]]:
    """Concatenate docs (each followed by EOS) and cut non-overlapping u32[seq_len+1] samples.

    ponytail: trailing partial sample is dropped; no doc-boundary masking (add if eval needs it).
    """
    width = seq_len + 1
    eos = np.asarray([eos_id], dtype=U32)
    carry = np.empty(0, dtype=U32)
    for ids in docs:
        buf = np.concatenate([carry, np.asarray(ids, dtype=U32), eos])
        n = len(buf) // width
        for k in range(n):
            yield buf[k * width : (k + 1) * width].copy()
        carry = buf[n * width :]


class ShardWriter:
    """Streams samples into fixed-size shard files; incomplete last shard is discarded on close."""

    def __init__(
        self, out_dir: Path, seq_len: int, samples_per_shard: int, max_shards: int
    ) -> None:
        self.out_dir = Path(out_dir)
        self.out_dir.mkdir(parents=True, exist_ok=True)
        self.width = seq_len + 1
        self.seq_len = seq_len
        self.sps = samples_per_shard
        self.max_shards = max_shards
        self._rows: list[npt.NDArray[np.uint32]] = []
        self.shard_sha256s: list[str] = []

    @property
    def full(self) -> bool:
        return len(self.shard_sha256s) >= self.max_shards

    def add(self, row: npt.NDArray[np.uint32]) -> None:
        if self.full:
            return
        if row.shape != (self.width,) or row.dtype != U32:
            raise ValueError(f"bad sample shape/dtype {row.shape} {row.dtype}")
        self._rows.append(row)
        if len(self._rows) == self.sps:
            raw = np.stack(self._rows).astype(U32, copy=False).tobytes()
            path = self.out_dir / shard_name(len(self.shard_sha256s))
            tmp = path.with_suffix(".tmp")
            tmp.write_bytes(raw)
            tmp.replace(path)
            self.shard_sha256s.append(hashlib.sha256(raw).hexdigest())
            self._rows = []


class ShardSamples(Sequence[bytes]):
    """Lazy view: global sample index -> raw LE bytes, backed by memmapped shard files."""

    def __init__(
        self, shard_dir: Path, n_shards: int, samples_per_shard: int, seq_len: int
    ) -> None:
        self.sps = samples_per_shard
        self.width = seq_len + 1
        self._maps = [
            np.memmap(
                Path(shard_dir) / shard_name(i),
                dtype=U32,
                mode="r",
                shape=(samples_per_shard, self.width),
            )
            for i in range(n_shards)
        ]

    def __len__(self) -> int:
        return len(self._maps) * self.sps

    @overload
    def __getitem__(self, i: int) -> bytes: ...
    @overload
    def __getitem__(self, i: slice) -> Sequence[bytes]: ...
    def __getitem__(self, i: int | slice) -> bytes | Sequence[bytes]:
        if isinstance(i, slice):
            return [self[j] for j in range(*i.indices(len(self)))]
        if not 0 <= i < len(self):
            raise IndexError(i)
        return self._maps[i // self.sps][i % self.sps].tobytes()

    def row(self, i: int) -> npt.NDArray[np.uint32]:
        return np.frombuffer(self[i], dtype=U32)


def merkle_depth(n: int) -> int:
    return max(0, (n - 1).bit_length())


def finalize(
    writer: ShardWriter, tokenizer: dict[str, str], extra: dict[str, Any] | None = None
) -> tuple[ShardSetManifest, MerkleTree]:
    n_shards = len(writer.shard_sha256s)
    if n_shards == 0:
        raise ValueError("no complete shard written")
    samples = ShardSamples(writer.out_dir, n_shards, writer.sps, writer.seq_len)
    tree = MerkleTree(samples)
    shard_root = MerkleTree([bytes.fromhex(h) for h in writer.shard_sha256s]).root
    m = ShardSetManifest(
        seq_len=writer.seq_len,
        samples_per_shard=writer.sps,
        n_shards=n_shards,
        n_samples=len(samples),
        depth=merkle_depth(len(samples)),
        merkle_root=tree.root.hex(),
        shard_sha256s=list(writer.shard_sha256s),
        shard_sha256_root=shard_root.hex(),
        tokenizer=tokenizer,
        extra=extra or {},
    )
    return m, tree


def verify_shards(shard_dir: Path, m: ShardSetManifest) -> list[str]:
    """Return human-readable errors (each naming the shard); empty list = verified."""
    errors: list[str] = []
    for i, want in enumerate(m.shard_sha256s):
        p = Path(shard_dir) / shard_name(i)
        if not p.is_file():
            errors.append(f"{p.name}: missing")
            continue
        h = hashlib.sha256()
        with p.open("rb") as f:
            for chunk in iter(lambda: f.read(1 << 22), b""):
                h.update(chunk)
        if h.hexdigest() != want:
            errors.append(f"{p.name}: sha256 {h.hexdigest()} != manifest {want}")
    if MerkleTree([bytes.fromhex(h) for h in m.shard_sha256s]).root.hex() != m.shard_sha256_root:
        errors.append("manifest: shard_sha256_root mismatch")
    if errors:
        return errors
    samples = ShardSamples(Path(shard_dir), m.n_shards, m.samples_per_shard, m.seq_len)
    if len(samples) != m.n_samples or MerkleTree(samples).root.hex() != m.merkle_root:
        errors.append("manifest: sample merkle_root mismatch")
    return errors


def holdout_commit(holdout_root: bytes, salt: bytes) -> str:
    """sha256(holdout_root || salt); salt stays secret until reveal."""
    return hashlib.sha256(holdout_root + salt).hexdigest()
