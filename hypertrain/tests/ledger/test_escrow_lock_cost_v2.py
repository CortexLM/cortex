"""The cost of an admission's funding status does not grow with the ledger, and locks are bounded.

ADR 0014 part 4 (F7a). `EscrowV2.locked()` scanned every lock event of the ledger and ran one
unindexed SUM per lock, under the store lock, for the unauthenticated status route: 1.08 s at
5,000 open locks, 96 s at 40,000. The cost is measured in SQLite virtual-machine steps
(deterministic), not in seconds.
"""

from __future__ import annotations

import importlib.util
import sqlite3
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest

import hypertrain.ledger.escrow_v2 as esc
from hypertrain.ledger.escrow_v2 import EscrowError, EscrowV2
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages_v2 import EscrowLock, EscrowRelease, EscrowTransfer


def load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


HERE = Path(__file__).parent
E = load("lock_cost_ledger_fixture", HERE / "test_escrow_v2.py")
A = load("lock_cost_admission_fixture", HERE.parent / "challenge/test_admission_v2.py")
N = load("lock_cost_network_fixture", HERE.parent / "challenge/test_service_network_v2.py")
H2 = "22" * 32


def ops(db: sqlite3.Connection, call) -> int:
    """SQLite virtual-machine steps spent by `call` (deterministic, unlike wall time)."""
    steps = [0]

    def count() -> int:
        steps[0] += 1
        return 0

    db.set_progress_handler(count, 100)
    try:
        call()
    finally:
        db.set_progress_handler(None, 0)
    return steps[0]


def lock_id(i: int) -> str:
    return sha256_hex(f"lock-{i}".encode())


def lock(origin: str, i: int, units: int = 100, admission: str = E.H, owner=E.COLD) -> EscrowLock:
    return EscrowLock(
        operation_id=lock_id(i),
        owner=owner.ss58,
        units=units,
        origin_ids=[origin],
        admission_id=admission,
        dispute_id=None,
        kind="LOCK_ADMISSION",
    )


def legacy_locked(db: sqlite3.Connection, admission_id: str, owner: str) -> tuple[int, str]:
    """The statement that ran before the index, verbatim: the oracle of the value."""
    units, hashes = 0, []
    for r in db.execute("SELECT * FROM escrow_events WHERE kind='LOCK_ADMISSION'"):
        op = EscrowLock.model_validate_json(r["request"])
        if op.admission_id == admission_id and op.owner == owner:
            amount = db.execute(
                "SELECT COALESCE(SUM(units),0) FROM escrow_units WHERE "
                "bucket='admission_locked' AND ref=? AND owner=?",
                (op.operation_id, owner),
            ).fetchone()[0]
            if amount:
                units += amount
                hashes.append(r["event_hash"])
    return units, sha256_hex(canonicalize(hashes)) if hashes else "0" * 64


def add_foreign_locks(e: EscrowV2, origin: str, count: int) -> None:
    """`count` open 1-unit locks of OTHER, written in the shape lock() stores (no cap, no chain)."""
    seq = e.db.execute("SELECT MAX(seq) FROM escrow_events").fetchone()[0]
    for i in range(count):
        op = lock(origin, 10_000 + i, 1, "33" * 32, E.OTHER)
        e.db.execute(
            "INSERT INTO escrow_events VALUES(?,?,?,?,?,?,?)",
            (
                seq + 1 + i,
                op.operation_id,
                op.kind,
                op.model_dump_json(),
                "00" * 32,
                "11" * 32,
                "{}",
            ),
        )
        e.db.execute(
            "INSERT INTO escrow_units VALUES(?,?,?,?,?)",
            (origin, E.OTHER.ss58, "admission_locked", op.operation_id, 1),
        )


@pytest.fixture
def funded(tmp_path: Path):
    e = E.ledger(E.setup(), tmp_path / "ledger.db")
    cold, other = E.fund(e)
    return e, cold, other


def test_the_funding_status_costs_the_same_whatever_other_owners_hold(funded) -> None:
    e, cold, other = funded
    for i in range(50):
        e.lock(lock(cold, i), signer=E.COLD.ss58)
    alone = ops(e.db, lambda: e.locked(E.H, E.COLD.ss58))
    add_foreign_locks(e, other, 3000)
    crowded = ops(e.db, lambda: e.locked(E.H, E.COLD.ss58))
    assert e.locked(E.H, E.COLD.ss58)[0] == 5000
    # Before the index: 14,500 steps alone, 797,500 with 3,000 foreign locks.
    assert crowded <= 2 * alone + 5_000 and crowded <= 100_000


def test_locked_returns_exactly_what_the_scan_returned(funded) -> None:
    e, cold, other = funded
    two = (cold, other)
    assert e.locked(E.H, E.COLD.ss58) == legacy_locked(e.db, E.H, E.COLD.ss58) == (0, "0" * 64)
    e.lock(lock(cold, 0x91, 100), signer=E.COLD.ss58)
    e.lock(lock(cold, 0x12, 110), signer=E.COLD.ss58)
    e.lock(lock(cold, 0x77, 120, H2), signer=E.COLD.ss58)  # another admission of the same owner
    e.lock(lock(other, 0x05, 130, E.H, E.OTHER), signer=E.OTHER.ss58)  # another owner
    # a lock that lists two origins: one event, two unit rows (COLD first receives units of both)
    e.transfer(
        EscrowTransfer(
            operation_id=lock_id(0xBB),
            owner=E.OTHER.ss58,
            recipient=E.COLD.ss58,
            units=4000,
            origin_ids=[other],
            admission_id=None,
            dispute_id=None,
        ),
        signer=E.OTHER.ss58,
    )
    both = lock(cold, 0x33, 10, E.H).model_copy(update={"origin_ids": sorted(two), "units": 20_000})
    e.lock(both, signer=E.COLD.ss58)
    for owner, admission in ((E.COLD, E.H), (E.COLD, H2), (E.OTHER, E.H), (E.OTHER, H2)):
        assert e.locked(admission, owner.ss58) == legacy_locked(e.db, admission, owner.ss58)
    # closing a lock removes it from both
    status = esc.Settlement(
        finality_hash=E.H,
        closed_dispute_root=E.H,
        release_beacon=1,
        unresolved=False,
        outcome="MATCH",
    )
    e.settlement = e.admission_settlement = lambda _: status
    release = EscrowRelease(
        operation_id=lock_id(0xAA),
        owner=E.COLD.ss58,
        units=110,
        origin_ids=[cold],
        admission_id=E.H,
        dispute_id=None,
        lock_id=lock_id(0x12),
        finality_hash=E.H,
        closed_dispute_root=E.H,
    )
    e.release(release, signer=E.COLD.ss58, now_beacon=10)
    for owner, admission in ((E.COLD, E.H), (E.COLD, H2), (E.OTHER, E.H)):
        assert e.locked(admission, owner.ss58) == legacy_locked(e.db, admission, owner.ss58)
    # the value is read by the signed tape: pin it, as a literal, once
    assert e.locked(E.H, E.COLD.ss58)[0] == 100 + 20_000
    assert e.locked(E.H, E.COLD.ss58)[1] == GOLDEN_HASH


# The hash over the event hashes of COLD's two open locks on admission H, in the order they were
# made. It is also what the scan returned on the parent commit (see the differential above).
GOLDEN_HASH = "179fa83559685abd3528ed5a39993e47b9803529834878f39f0ac38ae08c9c07"


def test_the_open_lock_statement_uses_the_partial_index(funded) -> None:
    e, cold, other = funded
    e.lock(lock(cold, 1), signer=E.COLD.ss58)
    seen: list[str] = []
    e.db.set_trace_callback(seen.append)
    e.locked(E.H, E.COLD.ss58)
    e.db.set_trace_callback(None)
    statement = next(s for s in seen if s.lstrip().startswith("SELECT e.event_hash"))
    plan = " ".join(
        str(row["detail"])
        for row in e.db.execute("EXPLAIN QUERY PLAN " + statement)  # type: ignore[arg-type]
    )
    assert "escrow_open_admission_locks" in plan and "SCAN u" not in plan and "SCAN e" not in plan


def make_locks(e: EscrowV2, origin: str, count: int, start: int = 0, units: int = 100) -> None:
    for i in range(start, start + count):
        e.lock(lock(origin, i, units), signer=E.COLD.ss58)


def events(e: EscrowV2) -> int:
    return e.db.execute("SELECT COUNT(*) FROM escrow_events").fetchone()[0]


def test_a_coldkey_cannot_hold_more_open_locks_than_the_cap(funded, monkeypatch) -> None:
    e, cold, other = funded
    monkeypatch.setattr(esc, "MAX_OPEN_LOCKS_PER_COLDKEY", 3, raising=False)
    make_locks(e, cold, 3)
    before = (events(e), e.balances())
    with pytest.raises(EscrowError, match="OPEN_LOCK_CAP"):
        e.lock(lock(cold, 3), signer=E.COLD.ss58)
    assert (events(e), e.balances()) == before  # nothing was written
    # a replay of an accepted lock still returns its receipt at the cap
    assert e.lock(lock(cold, 0), signer=E.COLD.ss58).operation_id == lock_id(0)
    # another coldkey is not affected
    e.lock(lock(other, 50, 100, E.H, E.OTHER), signer=E.OTHER.ss58)
    # closing one makes room
    status = esc.Settlement(
        finality_hash=E.H,
        closed_dispute_root=E.H,
        release_beacon=1,
        unresolved=False,
        outcome="MATCH",
    )
    e.admission_settlement = lambda _: status
    release = EscrowRelease(
        operation_id=lock_id(0xAA),
        owner=E.COLD.ss58,
        units=100,
        origin_ids=[cold],
        admission_id=E.H,
        dispute_id=None,
        lock_id=lock_id(0),
        finality_hash=E.H,
        closed_dispute_root=E.H,
    )
    e.release(release, signer=E.COLD.ss58, now_beacon=10)
    e.lock(lock(cold, 3), signer=E.COLD.ss58)


def test_a_lock_that_lists_two_origins_counts_once(funded, monkeypatch) -> None:
    e, cold, other = funded
    monkeypatch.setattr(esc, "MAX_OPEN_LOCKS_PER_COLDKEY", 2, raising=False)
    e.transfer(
        EscrowTransfer(
            operation_id=lock_id(0xBB),
            owner=E.OTHER.ss58,
            recipient=E.COLD.ss58,
            units=2000,
            origin_ids=[other],
            admission_id=None,
            dispute_id=None,
        ),
        signer=E.OTHER.ss58,
    )
    origins = sorted((cold, other))
    spans = {"origin_ids": origins, "units": 20_005}  # takes units of both origins: two unit rows
    e.lock(lock(cold, 1).model_copy(update=spans), signer=E.COLD.ss58)
    # one lock, so a second still fits under a cap of 2; the third does not
    e.lock(lock(cold, 2).model_copy(update={"origin_ids": origins}), signer=E.COLD.ss58)
    with pytest.raises(EscrowError, match="OPEN_LOCK_CAP"):
        e.lock(lock(cold, 3).model_copy(update={"origin_ids": origins}), signer=E.COLD.ss58)


def test_the_ledger_holds_a_bounded_number_of_open_locks(funded, monkeypatch) -> None:
    e, cold, other = funded
    monkeypatch.setattr(esc, "MAX_OPEN_LOCKS_LEDGER", 3, raising=False)
    make_locks(e, cold, 2)
    e.lock(lock(other, 90, 100, E.H, E.OTHER), signer=E.OTHER.ss58)
    with pytest.raises(EscrowError, match="LEDGER_LOCK_CAP"):
        e.lock(lock(other, 91, 100, E.H, E.OTHER), signer=E.OTHER.ss58)
    assert e.lock(lock(cold, 0), signer=E.COLD.ss58).operation_id == lock_id(0)  # replays pass


def test_a_lock_is_at_least_a_tenth_of_the_funding_floor(funded) -> None:
    e, cold, _ = funded
    floor = e.policy.S_min_units // 10
    assert floor == 100
    with pytest.raises(EscrowError, match="LOCK_BELOW_MINIMUM"):
        e.lock(lock(cold, 1, floor - 1), signer=E.COLD.ss58)
    e.lock(lock(cold, 2, floor), signer=E.COLD.ss58)
    assert e.balances(E.COLD.ss58).admission_locked == floor


def test_a_standalone_admission_lock_beyond_the_cap_leaves_no_outbox_row(
    tmp_path: Path, monkeypatch
) -> None:
    s = A.setup()
    a = A.service(s, tmp_path)
    admission = A.apply(a, s)
    origin, _ = A.fixtures.fund(a.escrow)
    monkeypatch.setattr(esc, "MAX_OPEN_LOCKS_PER_COLDKEY", 1, raising=False)

    def post(i: int) -> str:
        op = lock(origin, i, 100, admission, A.COLD)
        return a.lock(canonicalize(A.seal(A.COLD, "EscrowLock", a.escrow.run_id, op, 500)), now=2)

    post(1)
    outbox = a.store.db.execute("SELECT COUNT(*) FROM admission_outbox").fetchone()[0]
    with pytest.raises(EscrowError, match="OPEN_LOCK_CAP"):
        post(2)
    assert a.store.db.execute("SELECT COUNT(*) FROM admission_outbox").fetchone()[0] == outbox
    assert a.store.pending_outbox(admission) is False
    post(1)  # the retry of the first lock passes at the cap


@pytest.fixture
def served(tmp_path_factory):
    generator = N.network.__wrapped__(
        tmp_path_factory.mktemp("lock-cost"), SimpleNamespace(param=None)
    )
    n = next(generator)
    try:
        yield n
    finally:
        generator.close()


def test_the_routes_refuse_the_lock_beyond_the_cap_and_replay_the_first(
    served, monkeypatch
) -> None:
    n = served
    monkeypatch.setattr(esc, "MAX_OPEN_LOCKS_PER_COLDKEY", 3, raising=False)
    admission = n.join(0).json()["admission_id"]
    n.push(2)
    origin = n.origin_ids[0]
    ops_ = [lock(origin, i, 100, admission, N.COLD[0]) for i in range(4)]
    post = lambda op: n.client.post(  # noqa: E731
        n.url + "/escrow/lock", json=n.signed(N.COLD[0], "EscrowLock", op)
    )
    assert [post(op).status_code for op in ops_[:3]] == [200, 200, 200]
    counts = lambda: tuple(  # noqa: E731
        n.store._db.execute(f"SELECT COUNT(*) FROM {t}").fetchone()[0]
        for t in ("escrow_events", "accepted_v2", "admission_outbox")
    )
    before = counts()
    refused = post(ops_[3])
    assert refused.status_code == 409 and "OPEN_LOCK_CAP" in refused.text
    assert counts() == before  # neither the nonce nor an outbox row was consumed
    assert post(ops_[0]).status_code == 200  # the retry of an accepted lock
    status = n.client.get(n.url + "/admission/" + N.HOT[0].ss58).json()
    assert status["funding"]["locked_units"] == 300


def test_the_status_route_costs_the_same_in_a_ledger_full_of_other_locks(served) -> None:
    n = served
    admission = n.join(0).json()["admission_id"]
    n.push(2)
    origin = n.origin_ids[0]
    for i in range(5):
        op = lock(origin, i, 100, admission, N.COLD[0])
        posted = n.client.post(n.url + "/escrow/lock", json=n.signed(N.COLD[0], "EscrowLock", op))
        assert posted.status_code == 200, posted.text
    url = n.url + "/admission/" + N.HOT[0].ss58
    db = n.store._db
    alone = ops(db, lambda: n.client.get(url))
    escrow = n.store._services(n.manifest.run_id())[0]
    add_foreign_locks(escrow, n.origin_ids[1], 3000)
    crowded = ops(db, lambda: n.client.get(url))
    assert n.client.get(url).json()["funding"]["locked_units"] == 500
    assert crowded <= 2 * alone  # before the index it was about 55 times as much
