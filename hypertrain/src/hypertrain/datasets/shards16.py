"""u16 LE shards of u16[seq_len+1] samples with per-shard and per-unit sha256 (A14, A15).

Shard file = raw little-endian u16, shape (<= samples_per_shard, seq_len+1); only the last shard may
be short. Unit = `unit` consecutive samples (HTTP-Range-verifiable). Merkle leaf i = raw LE bytes of
sample i. `unit_sha256_root` = MerkleTree over the unit hashes, like `shard_sha256_root`.
"""

from __future__ import annotations

import hashlib
import json
from collections.abc import Iterable, Sequence
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Any, overload

import numpy as np
import numpy.typing as npt

from hypertrain.data.shards import merkle_depth
from hypertrain.protocol.hashing import MerkleTree

U16 = np.dtype("<u2")
SAMPLE_FORMAT_U16 = "u16[seq_len+1] token ids"
MAX_SHARD_BYTES = 64 << 20


def shard_name16(i: int) -> str:
    return f"shard-{i:05d}.u16"


@dataclass
class ShardSet16Manifest:
    """Field-for-field superset of data.shards.ShardSetManifest plus the unit hashes."""

    seq_len: int
    samples_per_shard: int
    n_shards: int
    n_samples: int
    depth: int
    merkle_root: str
    shard_sha256s: list[str]
    shard_sha256_root: str
    tokenizer: dict[str, str]
    unit: int
    unit_sha256s: list[str]
    unit_sha256_root: str
    sample_format: str = SAMPLE_FORMAT_U16
    shard_uri_template: str = "{shard_sha256}"
    extra: dict[str, Any] = field(default_factory=dict)

    def to_json(self) -> str:
        return json.dumps(asdict(self), indent=2, sort_keys=True)

    @classmethod
    def from_json(cls, s: str) -> ShardSet16Manifest:
        return cls(**json.loads(s))


class U16ShardSamples(Sequence[bytes]):
    """Lazy view: global sample index -> raw LE u16 bytes, backed by memmapped shard files."""

    def __init__(
        self, shard_dir: Path, n_shards: int, samples_per_shard: int, seq_len: int
    ) -> None:
        self.sps = samples_per_shard
        self.width = seq_len + 1
        self._maps = [
            np.memmap(Path(shard_dir) / shard_name16(i), dtype=U16, mode="r").reshape(
                -1, self.width
            )
            for i in range(n_shards)
        ]
        self._n = sum(len(m) for m in self._maps)

    def __len__(self) -> int:
        return self._n

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
        """Sample as u32 (the trainer's SampleFn contract); the bytes hashed are the u16 ones."""
        return np.frombuffer(self[i], dtype=U16).astype(np.uint32)


def default_samples_per_shard(seq_len: int, unit: int) -> int:
    """Largest power of two keeping a shard <= 64 MiB, rounded down to a multiple of unit."""
    n = 1 << ((MAX_SHARD_BYTES // ((seq_len + 1) * 2)).bit_length() - 1)
    n -= n % unit
    if n < unit:
        raise ValueError("a unit does not fit in a 64 MiB shard")
    return n


def _unit_hashes(raw: bytes, unit_bytes: int) -> list[str]:
    return [
        hashlib.sha256(raw[o : o + unit_bytes]).hexdigest() for o in range(0, len(raw), unit_bytes)
    ]


def write_shards16(
    samples: Iterable[npt.NDArray[Any]],
    out: Path,
    seq_len: int,
    unit: int = 512,
    *,
    samples_per_shard: int | None = None,
    tokenizer: dict[str, str] | None = None,
    extra: dict[str, Any] | None = None,
) -> ShardSet16Manifest:
    """Write shards + manifest.json. The sample tail is truncated to a multiple of `unit`
    (dropped count in extra["dropped_tail_samples"])."""
    sps = samples_per_shard or default_samples_per_shard(seq_len, unit)
    if unit < 1 or sps % unit:
        raise ValueError("samples_per_shard must be a positive multiple of unit")
    out = Path(out)
    out.mkdir(parents=True, exist_ok=True)
    width = seq_len + 1
    unit_bytes = unit * width * 2
    shard_hashes: list[str] = []
    unit_hashes: list[str] = []
    rows: list[npt.NDArray[np.uint16]] = []

    def flush(n_rows: int) -> None:
        raw = np.stack(rows[:n_rows]).astype(U16, copy=False).tobytes()
        path = out / shard_name16(len(shard_hashes))
        tmp = path.with_suffix(".tmp")
        tmp.write_bytes(raw)
        tmp.replace(path)
        shard_hashes.append(hashlib.sha256(raw).hexdigest())
        unit_hashes.extend(_unit_hashes(raw, unit_bytes))
        del rows[:]

    total = 0
    for s in samples:
        a = np.asarray(s)
        if a.shape != (width,):
            raise ValueError(f"bad sample shape {a.shape}, want ({width},)")
        if a.size and (int(a.max()) > 0xFFFF or int(a.min()) < 0):
            raise ValueError("token id does not fit u16")
        rows.append(a.astype(U16))
        total += 1
        if len(rows) == sps:
            flush(sps)
    keep = len(rows) - len(rows) % unit
    dropped = len(rows) - keep
    if keep:
        flush(keep)
    n_samples = total - dropped
    if n_samples == 0:
        raise ValueError("fewer than one full unit of samples")
    reader = U16ShardSamples(out, len(shard_hashes), sps, seq_len)
    m = ShardSet16Manifest(
        seq_len=seq_len,
        samples_per_shard=sps,
        n_shards=len(shard_hashes),
        n_samples=n_samples,
        depth=merkle_depth(n_samples),
        merkle_root=MerkleTree(reader).root.hex(),
        shard_sha256s=shard_hashes,
        shard_sha256_root=MerkleTree([bytes.fromhex(h) for h in shard_hashes]).root.hex(),
        tokenizer=tokenizer or {},
        unit=unit,
        unit_sha256s=unit_hashes,
        unit_sha256_root=MerkleTree([bytes.fromhex(h) for h in unit_hashes]).root.hex(),
        extra={**(extra or {}), "dropped_tail_samples": dropped},
    )
    (out / "manifest.json").write_text(m.to_json())
    return m


def verify_shards16(data_dir: Path, m: ShardSet16Manifest) -> list[str]:
    """Human-readable errors (each naming the file); empty list = verified."""
    errors: list[str] = []
    unit_bytes = m.unit * (m.seq_len + 1) * 2
    got_units: list[str] = []
    for i, want in enumerate(m.shard_sha256s):
        p = Path(data_dir) / shard_name16(i)
        if not p.is_file():
            errors.append(f"{p.name}: missing")
            continue
        raw = p.read_bytes()
        if hashlib.sha256(raw).hexdigest() != want:
            errors.append(f"{p.name}: sha256 mismatch")
        got_units.extend(_unit_hashes(raw, unit_bytes))
    if errors:
        return errors
    if got_units != m.unit_sha256s:
        bad = next(
            (i for i, (a, b) in enumerate(zip(got_units, m.unit_sha256s, strict=True)) if a != b),
            -1,
        )
        errors.append(f"unit sha256 mismatch (first bad unit {bad})")
    if MerkleTree([bytes.fromhex(h) for h in m.shard_sha256s]).root.hex() != m.shard_sha256_root:
        errors.append("manifest: shard_sha256_root mismatch")
    if MerkleTree([bytes.fromhex(h) for h in m.unit_sha256s]).root.hex() != m.unit_sha256_root:
        errors.append("manifest: unit_sha256_root mismatch")
    if errors:
        return errors
    samples = U16ShardSamples(Path(data_dir), m.n_shards, m.samples_per_shard, m.seq_len)
    if len(samples) != m.n_samples or MerkleTree(samples).root.hex() != m.merkle_root:
        errors.append("manifest: sample merkle_root mismatch")
    return errors
