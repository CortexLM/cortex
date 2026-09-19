"""RFC 6962 hashing with promotion of odd nodes (never duplication)."""

from collections.abc import Iterable
from hashlib import sha256

from .models import MetagraphRow
from .scale import ProtocolError, vector

EMPTY_ROOT = sha256(b"").digest()


def merkle_root(preimages: Iterable[bytes]) -> bytes:
    level = [sha256(b"\x00" + value).digest() for value in preimages]
    if not level:
        return EMPTY_ROOT
    while len(level) > 1:
        level = [
            sha256(b"\x01" + level[i] + level[i + 1]).digest() if i + 1 < len(level) else level[i]
            for i in range(0, len(level), 2)
        ]
    return level[0]


def canonical_rows(rows: Iterable[MetagraphRow]) -> tuple[MetagraphRow, ...]:
    result = tuple(sorted(rows, key=lambda row: row.hotkey))
    if len({row.hotkey for row in result}) != len(result) or sorted(
        row.uid for row in result
    ) != list(range(len(result))):
        raise ProtocolError("metagraph hotkeys and contiguous uids must be unique")
    for row in result:
        row.encode()
    return result


def metagraph_root(rows: tuple[MetagraphRow, ...]) -> bytes:
    return sha256(vector(canonical_rows(rows), MetagraphRow.encode)).digest()
