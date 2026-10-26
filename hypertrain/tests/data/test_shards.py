import hashlib
from pathlib import Path

import numpy as np
import pytest

from hypertrain.data.shards import (
    ShardSamples,
    ShardSetManifest,
    ShardWriter,
    finalize,
    holdout_commit,
    pack_samples,
    shard_name,
    verify_shards,
)
from hypertrain.data.tokenizer import ByteTokenizer, HFTokenizer, TokenizerPinError
from hypertrain.protocol.hashing import MerkleTree


def _build(tmp: Path) -> tuple[ShardSetManifest, MerkleTree]:
    tok = ByteTokenizer()
    docs = (tok.encode(f"doc {i} " + "héllo wörld " * (i % 7 + 1)) for i in range(400))
    w = ShardWriter(tmp, seq_len=15, samples_per_shard=32, max_shards=3)
    for row in pack_samples(docs, 15, tok.eos_id):
        w.add(row)
        if w.full:
            break
    return finalize(w, {"name": tok.name, "sha256": tok.sha256})


def test_pack_is_concatenation_with_eos() -> None:
    rows = list(pack_samples([[1, 2, 3], [4, 5], [6]], seq_len=3, eos_id=257))
    assert [r.tolist() for r in rows] == [[1, 2, 3, 257], [4, 5, 257, 6]]
    assert all(r.dtype == np.dtype("<u4") and r.shape == (4,) for r in rows)


def test_manifest_and_merkle_proofs(tmp_path: Path) -> None:
    m, tree = _build(tmp_path)
    assert (m.n_shards, m.n_samples) == (3, 96)
    assert m.depth == 7
    samples = ShardSamples(tmp_path, 3, 32, 15)
    for i in range(m.n_samples):
        leaf = samples[i]
        assert len(leaf) == 16 * 4
        proof = tree.proof(i)
        assert MerkleTree.verify(leaf, i, proof, bytes.fromhex(m.merkle_root), m.n_samples)
    bad = bytearray(samples[5])
    bad[0] ^= 1
    assert not MerkleTree.verify(bytes(bad), 5, tree.proof(5), tree.root, m.n_samples)
    assert not MerkleTree.verify(samples[5], 6, tree.proof(5), tree.root, m.n_samples)
    assert m.shard_sha256s[0] == hashlib.sha256((tmp_path / shard_name(0)).read_bytes()).hexdigest()
    assert ShardSetManifest.from_json(m.to_json()) == m


def test_build_is_deterministic(tmp_path: Path) -> None:
    a, _ = _build(tmp_path / "a")
    b, _ = _build(tmp_path / "b")
    assert a == b


def test_verify_detects_tampered_shard(tmp_path: Path) -> None:
    m, _ = _build(tmp_path)
    assert verify_shards(tmp_path, m) == []
    p = tmp_path / shard_name(1)
    raw = bytearray(p.read_bytes())
    raw[100] ^= 0xFF
    p.write_bytes(bytes(raw))
    errs = verify_shards(tmp_path, m)
    assert len(errs) == 1 and errs[0].startswith(shard_name(1))
    p.unlink()
    assert verify_shards(tmp_path, m) == [f"{shard_name(1)}: missing"]


def test_holdout_commit_binds_salt() -> None:
    r = b"\x01" * 32
    assert holdout_commit(r, b"s1") != holdout_commit(r, b"s2")
    assert holdout_commit(r, b"s1") == hashlib.sha256(r + b"s1").hexdigest()


def test_byte_tokenizer_pin() -> None:
    t = ByteTokenizer()
    assert t.vocab_size == 259 and len(t.sha256) == 64
    assert t.decode(t.encode("ünï")) == "ünï"


def test_hf_tokenizer_rejects_wrong_pin(tmp_path: Path) -> None:
    p = tmp_path / "tokenizer.json"
    p.write_text("{}")
    with pytest.raises(TokenizerPinError):
        HFTokenizer(p, "0" * 64, "x")
