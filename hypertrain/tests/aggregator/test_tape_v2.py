"""Ordinary tape compatibility remains independent of rollback evidence."""

from pathlib import Path

from test_weighted_v2 import KEY, economics, fixture, policy

from hypertrain.aggregator.tape_v2 import TapeV2, make_tape, replay_tape
from hypertrain.data.store import LocalFSStore
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize


def test_ordinary_tape_bytes_and_output_remain_original(tmp_path: Path) -> None:
    # Given: accepted ordinary payloads; no repair version/domain injected.
    store = LocalFSStore(tmp_path)
    manifest, works, previous = fixture(store)
    tape = make_tape(
        store,
        manifest,
        policy(),
        canonicalize(economics().body()),
        KEY,
        w=0,
        prev_state=previous,
        predecessor_tape_hash="11" * 32,
        inputs=works,
        reference_reward_units=100,
    )
    raw = tape.to_bytes()
    # When
    state = replay_tape(
        store,
        TapeV2.from_bytes(raw),
        manifest,
        policy(),
        canonicalize(economics().body()),
        signer=KEY.ss58,
        w=0,
        prev_state=previous,
        predecessor_tape_hash="11" * 32,
        inputs=works,
        reference_reward_units=100,
    )
    # Then
    assert raw == canonicalize(tape.body_json(), allow_float=False)
    assert set(tape.body_json()) == {"body", "signer", "sig"}
    assert tape.body.v == "ht-tape-v2"
    assert sha256_hex(state.to_bytes()) == tape.body.out_state
