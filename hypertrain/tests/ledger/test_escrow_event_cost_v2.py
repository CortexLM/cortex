"""An escrow event is cheap to encode, and a transfer cannot add rows nobody can use.

ADR 0014 part 4 (F7b). Every event wrote and hashed the whole ledger state through the pure-Python
JCS encoder, one interpreter step per character, twice, and `verify()` did it again for every stored
snapshot: 37 ms per event at 2,006 rows, 17.5 s to verify them, 106 s at 5,006. The bytes must not
change: the state and the event hash feed the signed tape.
"""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

import pytest
from hypothesis import given, settings
from hypothesis import strategies as st

import hypertrain.ledger.escrow_v2 as esc
import hypertrain.protocol.jcs as jcs
from hypertrain.ledger.escrow_v2 import EscrowError, EscrowV2
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages_v2 import EscrowLock, EscrowRelease, EscrowTransfer


def load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


E = load("event_cost_ledger_fixture", Path(__file__).parent / "test_escrow_v2.py")


def op_id(i: int) -> str:
    return sha256_hex(f"event-cost-{i}".encode())


def transfer(owner: Keypair, recipient: Keypair, origins: list[str], units: int, i: int):
    return EscrowTransfer(
        operation_id=op_id(i),
        owner=owner.ss58,
        recipient=recipient.ss58,
        units=units,
        origin_ids=origins,
        admission_id=None,
        dispute_id=None,
    )


@pytest.fixture
def funded(tmp_path: Path):
    e = E.ledger(E.setup(), tmp_path / "ledger.db")
    cold, other = E.fund(e)
    return e, cold, other


def units_of(e: EscrowV2, owner: Keypair) -> int:
    return e.db.execute(
        "SELECT COUNT(*) FROM escrow_units WHERE owner=?", (owner.ss58,)
    ).fetchone()[0]


def add_rows(e: EscrowV2, origin: str, owner: Keypair, count: int) -> None:
    """`count` empty rows of the shape a ledger grows, each with its own ref."""
    for i in range(count):
        e.db.execute(
            "INSERT INTO escrow_units VALUES(?,?,?,?,0)",
            (origin, owner.ss58, "available", f"ref-{i:05d}"),
        )


@given(
    st.recursive(
        st.none() | st.integers(-(2**53 - 1), 2**53 - 1) | st.text(),
        lambda inner: st.lists(inner, max_size=4),
        max_leaves=12,
    )
)
@settings(max_examples=500, deadline=None)
def test_the_state_encoder_gives_the_bytes_of_the_reference_jcs(value) -> None:
    # Quotes, backslashes, control characters, DEL, U+2028 and non-BMP characters are all in
    # `st.text()`; the bytes are what the signed tape hashes, so one differing byte is a fork.
    assert esc._jcs([value]).encode() == canonicalize([value])


def test_an_event_does_not_run_the_python_encoder_over_every_row(funded, monkeypatch) -> None:
    e, cold, other = funded
    add_rows(e, other, E.OTHER, 200)
    pushed = [0]
    string = jcs._string

    def counting(s: str) -> str:
        pushed[0] += len(s)
        return string(s)

    monkeypatch.setattr(jcs, "_string", counting)
    e.transfer(transfer(E.OTHER, E.COLD, [other], 1, 1), signer=E.OTHER.ss58)
    e.verify()
    # Before the C encoder: 83,163 characters at 200 rows, twice per event, and again per stored
    # snapshot in verify().
    assert pushed[0] < 5_000


def test_a_value_outside_the_safe_integer_range_refuses_the_event(funded) -> None:
    e, cold, other = funded
    # Another owner's row: the signer's own balances are already refused by the receipt model.
    big = 2**60
    e.db.execute("UPDATE escrow_origins SET issued=? WHERE origin=?", (big, other))
    e.db.execute(
        "UPDATE escrow_units SET units=? WHERE origin=? AND owner=? AND bucket='available'",
        (big, other, E.OTHER.ss58),
    )
    before = e.db.execute("SELECT COUNT(*) FROM escrow_events").fetchone()[0]
    with pytest.raises(EscrowError, match="STATE_RANGE"):
        e.transfer(transfer(E.COLD, E.OTHER, [cold], 1, 2), signer=E.COLD.ss58)
    assert e.db.execute("SELECT COUNT(*) FROM escrow_events").fetchone()[0] == before


LIMIT = esc.STATE_INT_LIMIT


@pytest.mark.parametrize(
    ("table", "column", "bad", "edge"),
    [
        ("escrow_origins", "mature_at", LIMIT + 1, LIMIT),
        # `abs()` of the smallest 64-bit integer is an SQLite error, not a number to compare.
        ("escrow_origins", "mature_at", -(2**63), -LIMIT),
        ("escrow_origins", "mature_at", 1.5, 1),
        ("escrow_origins", "reward_round", 1.5, 1),
        ("escrow_origins", "reward_round", 2**60, LIMIT),
        ("escrow_origins", "reward_round", -(LIMIT + 1), -LIMIT),
        ("escrow_finalized", "w", LIMIT + 1, LIMIT),
        ("escrow_finalized", "w", -(LIMIT + 1), -LIMIT),
        ("escrow_shadow_finalized", "ordinal", LIMIT + 1, LIMIT),
        ("escrow_shadow_finalized", "ordinal", -(LIMIT + 1), -LIMIT),
        ("escrow_shadow_finalized", "trial_epoch", 1.5, 1),
        ("escrow_shadow_finalized", "trial_epoch", 2**60, LIMIT),
        ("escrow_shadow_finalized", "trial_epoch", -(LIMIT + 1), -LIMIT),
    ],
)
def test_every_number_the_state_serialises_is_kept_in_the_safe_range(
    funded, table: str, column: str, bad: float, edge: int
) -> None:
    # `json.dumps` writes 1.5 or 2**60 without complaint where `canonicalize()` raises, so the state
    # hash of a ledger holding one would differ from the one an independent verifier computes.
    e, cold, other = funded
    e.db.execute("INSERT INTO escrow_finalized VALUES(0, NULL)")
    e.db.execute("INSERT INTO escrow_shadow_finalized(ordinal) VALUES(0)")
    where, args = ("WHERE origin=?", (other,)) if table == "escrow_origins" else ("", ())

    def put(value: float) -> None:
        e.db.execute(f"UPDATE {table} SET {column}=? {where}", (value, *args))

    put(edge)
    e.transfer(transfer(E.COLD, E.OTHER, [cold], 1, 10), signer=E.COLD.ss58)
    put(bad)
    before = e.db.execute("SELECT COUNT(*) FROM escrow_events").fetchone()[0]
    with pytest.raises(EscrowError, match="STATE_RANGE"):
        e.transfer(transfer(E.COLD, E.OTHER, [cold], 1, 11), signer=E.COLD.ss58)
    assert e.db.execute("SELECT COUNT(*) FROM escrow_events").fetchone()[0] == before


def test_verify_refuses_the_smallest_64_bit_integer_as_out_of_range(funded) -> None:
    # `verify()` takes the snapshot before it checks conservation, and `abs(-2**63)` is an SQLite
    # error: the restart check must answer STATE_RANGE like every other value it refuses.
    e, cold, other = funded
    e.db.execute("UPDATE escrow_origins SET issued=? WHERE origin=?", (-(2**63), other))
    with pytest.raises(EscrowError, match="STATE_RANGE"):
        e.verify()


def test_a_transfer_writes_no_row_for_a_listed_origin_it_does_not_spend(funded) -> None:
    e, cold, other = funded
    # The recipient must be an owner the ledger knows, so it holds the row of its own origin: COLD's
    # row of `cold` takes the unit, and COLD holds none of `other`, listed and not spent.
    e.transfer(transfer(E.COLD, E.OTHER, [cold], 4000, 3), signer=E.COLD.ss58)
    held = "SELECT COUNT(*) FROM escrow_units WHERE owner=? AND origin=?"
    assert e.db.execute(held, (E.COLD.ss58, other)).fetchone()[0] == 0
    rows = units_of(e, E.COLD)
    listed = sorted([cold, other])
    assert listed == [cold, other]  # the lowest id pays first: the unit comes from `cold`
    e.transfer(transfer(E.OTHER, E.COLD, listed, 1, 4), signer=E.OTHER.ss58)
    # Both origins are listed and must be held; the first covers the unit, the second adds nothing.
    assert units_of(e, E.COLD) == rows
    assert e.balances(E.COLD.ss58).available == 20000 - 4000 + 1
    e.verify()


def test_a_lock_listing_an_origin_it_does_not_spend_can_still_be_released(funded) -> None:
    # Guards the skip above against over-reaching: `settle_lock` rebuilds a release from the
    # original lock, including the origin that took no units, and `_move` needs a row for each.
    e, cold, other = funded
    e.transfer(transfer(E.OTHER, E.COLD, [other], 4000, 5), signer=E.OTHER.ss58)
    listed = sorted([cold, other])
    lock = EscrowLock(
        operation_id=op_id(6),
        owner=E.COLD.ss58,
        units=100,
        origin_ids=listed,
        admission_id=E.H,
        dispute_id=None,
        kind="LOCK_ADMISSION",
    )
    e.lock(lock, signer=E.COLD.ss58)
    status = esc.Settlement(
        finality_hash=E.H,
        closed_dispute_root=E.H,
        release_beacon=1,
        unresolved=False,
        outcome="MATCH",
    )
    e.admission_settlement = lambda _: status
    release = EscrowRelease(
        operation_id=op_id(7),
        owner=E.COLD.ss58,
        units=100,
        origin_ids=listed,
        admission_id=E.H,
        dispute_id=None,
        lock_id=lock.operation_id,
        finality_hash=E.H,
        closed_dispute_root=E.H,
    )
    e.release(release, signer=E.COLD.ss58, now_beacon=10)
    assert e.balances(E.COLD.ss58).admission_locked == 0


def test_a_transfer_to_oneself_is_refused_and_an_old_one_replays(funded) -> None:
    e, cold, other = funded
    own = transfer(E.COLD, E.COLD, [cold], 1, 8)
    before = (e.db.execute("SELECT COUNT(*) FROM escrow_events").fetchone()[0], e.balances())
    with pytest.raises(EscrowError, match="SELF_TRANSFER"):
        e.transfer(own, signer=E.COLD.ss58)
    assert (
        e.db.execute("SELECT COUNT(*) FROM escrow_events").fetchone()[0],
        e.balances(),
    ) == before
    # A self-transfer written before the refusal keeps its receipt: the chain is not rewritten.
    with e.tx():
        e._move(own, "available", "available", recipient=own.recipient)
        written = e._event(
            own.operation_id, "TRANSFER", own.model_dump_json(), own.owner, server=False
        )
    assert e.transfer(own, signer=E.COLD.ss58) == written
    e.verify()
