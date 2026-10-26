import os
import subprocess
import sys

import pytest

from hypertrain.protocol.hashing import MerkleTree, tensor_entry_digest, tensor_hash

STATE = [
    ("w.0", "float32", (2, 3), bytes(range(24))),
    ("b.0", "bfloat16", (3,), b"\x01\x02\x03\x04\x05\x06"),
    ("emb", "float32", (), b"\x00\x00\x80\x3f"),
]
TH_SNIPPET = (
    "from hypertrain.protocol.hashing import tensor_hash\n"
    "s=[('w.0','float32',(2,3),bytes(range(24))),('b.0','bfloat16',(3,),b'\\x01\\x02\\x03\\x04"
    "\\x05\\x06'),('emb','float32',(),b'\\x00\\x00\\x80\\x3f')]\n"
    "print(tensor_hash(reversed(s)))"
)


def test_th_independent_of_insertion_order() -> None:
    assert tensor_hash(STATE) == tensor_hash(list(reversed(STATE)))
    assert tensor_hash(dict((e[0], e) for e in STATE).values()) == tensor_hash(STATE)


def test_th_stable_across_processes() -> None:
    outs = set()
    for seed in ("1", "424242"):
        env = {**os.environ, "PYTHONHASHSEED": seed}
        res = subprocess.run(
            [sys.executable, "-c", TH_SNIPPET], capture_output=True, text=True, env=env, check=True
        )
        outs.add(res.stdout.strip())
    assert outs == {tensor_hash(STATE)}


@pytest.mark.parametrize(
    "mutate",
    [
        lambda e: (e[0], "float16", e[2], e[3]),
        lambda e: (e[0], e[1], (3, 2), e[3]),
        lambda e: (e[0], e[1], e[2], b"\xff" + e[3][1:]),
        lambda e: ("w.1", e[1], e[2], e[3]),
    ],
)
def test_th_sensitive_to_every_field(mutate: object) -> None:
    changed = [mutate(STATE[0]), *STATE[1:]]  # type: ignore[operator]
    assert tensor_hash(changed) != tensor_hash(STATE)


def test_entry_encoding_unambiguous() -> None:
    assert tensor_entry_digest("ab", "c", (1,), b"") != tensor_entry_digest("a", "bc", (1,), b"")
    assert tensor_entry_digest("a", "f", (1, 2), b"") != tensor_entry_digest("a", "f", (12,), b"")


def test_duplicate_tensor_names_rejected() -> None:
    with pytest.raises(ValueError):
        tensor_hash([STATE[0], STATE[0]])


@pytest.mark.parametrize("n", list(range(1, 10)) + [16, 33])
def test_merkle_all_proofs_verify(n: int) -> None:
    leaves = [bytes([i]) * 7 for i in range(n)]
    tree = MerkleTree(leaves)
    for i, leaf in enumerate(leaves):
        assert MerkleTree.verify(leaf, i, tree.proof(i), tree.root, n)


def test_merkle_false_cases() -> None:
    leaves = [bytes([i]) for i in range(5)]
    tree = MerkleTree(leaves)
    p = tree.proof(2)
    assert not MerkleTree.verify(b"\x09", 2, p, tree.root, 5)
    assert not MerkleTree.verify(leaves[2], 3, p, tree.root, 5)
    assert not MerkleTree.verify(leaves[2], 2, p[:-1], tree.root, 5)
    assert not MerkleTree.verify(leaves[2], 2, [*p, tree.root], tree.root, 5)
    assert not MerkleTree.verify(leaves[2], 2, p, tree.root, 4)
    assert not MerkleTree.verify(leaves[2], 2, p, bytes(32), 5)
    assert not MerkleTree.verify(leaves[2], 7, p, tree.root, 5)


def test_merkle_domain_separation_blocks_node_as_leaf() -> None:
    tree = MerkleTree([b"a", b"b", b"c", b"d"])
    inner = tree._levels[1][0] + tree._levels[1][1]
    assert MerkleTree([inner]).root != tree.root


def test_merkle_rejects_index_and_proof_length_mismatch() -> None:
    leaves = [bytes([i]) for i in range(6)]
    tree = MerkleTree(leaves)
    assert not MerkleTree.verify(leaves[5], 6, tree.proof(5), tree.root, 6)
    assert not MerkleTree.verify(leaves[4], 4, tree.proof(4), tree.root, 9)
    assert not MerkleTree.verify(leaves[4], 4, tree.proof(4), tree.root, 5)
    for i in range(6):
        assert len(tree.proof(i)) == MerkleTree.proof_length(i, 6)
