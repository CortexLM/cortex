"""Print the frozen v2 bundle wire vector (deterministic).

scripts/gen_wire_vector.py > tests/protocol/vectors/wire_v2.json
"""

import json
from hashlib import sha256

from cortex.protocol import (
    ChallengeEntry,
    Leaf,
    MetagraphRow,
    NoScore,
    Score,
    TrustRoot,
    build_bundle,
)


def vector() -> dict[str, object]:
    rows = [MetagraphRow(bytes([tag]) * 32, tag) for tag in range(3)]
    trust = TrustRoot(
        (ChallengeEntry(b"bounty", 2000), ChallengeEntry(b"proof", 8000)), sha256(b"\x00").digest()
    )
    leaves = [
        Leaf(name, row.hotkey, 123, Score(50) if row.uid == 1 else NoScore())
        for name in (b"bounty", b"proof")
        for row in rows
    ]
    bundle = build_bundle(
        epoch=123,
        netuid=541,
        block_b=99,
        block_hash=bytes([9]) * 32,
        rows=rows,
        leaves=leaves,
        trust=trust,
    )
    return {
        "authority": "Python reference encoder, protocol_version 2 (no signatures); fixed inputs",
        "bundle_scale": bundle.encode().hex(),
        "leaf_scale": bundle.body.leaves[0].encode().hex(),
        "merkle_root": bundle.body.merkle_root.hex(),
        "final_vector": [list(pair) for pair in bundle.body.final_vector],
    }


if __name__ == "__main__":
    print(json.dumps(vector(), indent=2))
