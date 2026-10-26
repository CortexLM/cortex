"""Deterministic shard assignment: rendezvous hashing under a per-member capacity.

plan() is a pure function of the round seed, the pending shards and the free capacity of the
live members, so the coordinator's ordered event log replays to the same assignment and an
auditor can recompute it. Only pending shards are planned: a departure moves the departed
member's shards and nothing else.
"""

from __future__ import annotations

import hashlib
import struct
from collections.abc import Iterable, Mapping

ASSIGN_TAG = b"ht-elastic-assign-v1|"


def score(seed: bytes, shard: int, member: str) -> bytes:
    return hashlib.sha256(ASSIGN_TAG + seed + struct.pack(">Q", shard) + member.encode()).digest()


def plan(seed: bytes, pending: Iterable[int], free: Mapping[str, int]) -> dict[int, str]:
    """Each pending shard, in ascending order, goes to the member with the highest score among
    those with capacity left; shards beyond the total free capacity stay pending."""
    left = {member: n for member, n in free.items() if n > 0}
    out: dict[int, str] = {}
    for shard in sorted(set(pending)):
        if not left:
            break
        member = max(left, key=lambda m: (score(seed, shard, m), m))
        out[shard] = member
        left[member] -= 1
        if not left[member]:
            del left[member]
    return out
