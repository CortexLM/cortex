from dataclasses import replace

import pytest

from cortex.protocol import ProtocolError
from cortex.protocol.consensus import Dissent, DissentReason, RootStatement


def test_root_response_binds_epoch_root_and_hotkey():
    statement = RootStatement.sign(bytes([9]) * 32, 7, bytes([4]) * 32)
    assert statement.payload() == bytes.fromhex("0700000000000000") + bytes([4]) * 32
    assert RootStatement.from_json(statement.to_json()) == statement
    assert RootStatement.decode(statement.encode()) == statement
    for changed in (
        replace(statement, epoch=8),
        replace(statement, merkle_root=bytes(32)),
        replace(statement, hotkey=bytes(32)),
    ):
        with pytest.raises(ProtocolError):
            changed.verify()


def test_dissent_wire_layout_and_signature_are_stable():
    statement = Dissent.sign(
        bytes([9]) * 32,
        7,
        bytes([1]) * 32,
        bytes([2]) * 32,
        bytes([3]) * 32,
        DissentReason.PEER_ROOT_CONFLICT,
    )
    assert statement.payload() == (
        b"\x01\x00\x07" + bytes(7) + bytes([1]) * 32 + bytes([2]) * 32 + bytes([3]) * 32 + b"\x09"
    )
    assert len(statement.encode()) == 203
    assert Dissent.decode(statement.encode()) == statement
    with pytest.raises(ProtocolError):
        replace(statement, reason=10).verify()
