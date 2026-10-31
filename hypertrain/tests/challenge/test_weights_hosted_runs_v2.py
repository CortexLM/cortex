"""`get_weights` over several hosted runs: a retired run is skipped, and nothing is paid unserved.

S21 (run 0ef600b4, TEST): the root `ChallengeStore.weights` asked every hosted store for its
epoch answer. Each child persists its own answer and its `available -> paid` moves in its own
SQLite transaction, so when a later child raised `RunRetired` (its stored manifest is refused
since the env pin is required) the call answered 409 while the children asked before it kept
`weights_runs[epoch]` and the paid units: paid in the ledger, never served.
"""

from __future__ import annotations

import importlib.util
import json
import sqlite3
import sys
from pathlib import Path
from typing import Any

import pytest

from hypertrain.challenge.admission_store import AdmissionStore
from hypertrain.challenge.store import ChallengeStore
from hypertrain.data.store import LocalFSStore
from hypertrain.ledger import FULL_SHARE_MASS, Params
from hypertrain.ledger.escrow_v2 import Settlement

spec = importlib.util.spec_from_file_location(
    "v2_weights_hosted_escrow", Path(__file__).parents[1] / "ledger/test_escrow_v2.py"
)
assert spec is not None and spec.loader is not None
E = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = E
spec.loader.exec_module(E)
actual_reward = E.actual_reward

UNITS = 1_000_000


class CommitFails:
    """A connection whose COMMIT of the epoch answer fails like a full disk: SQLite rolls back."""

    def __init__(self, db: sqlite3.Connection) -> None:
        self.db, self.answered = db, False

    def execute(self, sql: str, *args: Any) -> sqlite3.Cursor:
        if sql.startswith("INSERT INTO weights_runs"):
            self.answered = True
        elif sql == "COMMIT" and self.answered:
            self.db.execute("ROLLBACK")
            raise sqlite3.OperationalError("disk I/O error")
        return self.db.execute(sql, *args)

    def __getattr__(self, name: str) -> Any:
        return getattr(self.db, name)


class Hosted:
    """A root store and the runs it hosts, each with the real signed live w=0 reward matured."""

    def __init__(self, tmp_path: Path, reward_set, monkeypatch) -> None:
        self.setup, self.evidence, self.reward = reward_set
        self.monkeypatch = monkeypatch
        beacon = self.setup.manifest.training.beacon
        self.beacon = beacon
        self.root = ChallengeStore(
            tmp_path / "state",
            # v1 genesis deliberately differs from the drand genesis the maturity round uses.
            Params("hypertrain", beacon.genesis_time - 600, 300, 1, 1),
            None,
            None,
            lambda payload: None,
            LocalFSStore(tmp_path / "objects"),
        )
        self.escrows: dict[str, E.EscrowV2] = {}

    def at(self, rnd: int) -> int:
        return self.beacon.genesis_time + (rnd - 1) * self.beacon.period

    def host(self, store: ChallengeStore, name: str, *, retired: bool = False) -> None:
        """One run in `store` with a matured 1e6-unit origin. A retired run's stored manifest
        lacks the env pin, so the model refuses it: its services cannot be built."""
        escrow = E.ledger(
            self.setup,
            store._db,
            replay=E.verified_shadow,
            settlement=lambda ref: Settlement(
                finality_hash=self.reward.finalize_hash,
                closed_dispute_root=E.H,
                release_beacon=20,
                unresolved=False,
                outcome="MATCH",
            ),
        )
        escrow.mutex = store._lock
        AdmissionStore(escrow)  # the tables the real services create
        escrow.reward_finalize(self.reward, self.evidence)
        escrow.mature(E.H, tuple(self.reward.origin_ids), 20)
        manifest = self.setup.manifest.body()
        if retired:
            del manifest["training"]["reference_spec"]["env"]["pinned"]
        else:
            self.monkeypatch.setattr(store, "_services", lambda run_id: (escrow, None, None))
        store._db.execute(
            "INSERT INTO runs VALUES(?,?,?,'created',NULL)",
            (self.setup.manifest.run_id(), json.dumps(manifest), "{}"),
        )
        self.escrows[name] = escrow

    def retired_beside(self, store: ChallengeStore) -> str:
        """A retired run in `store` next to the live run `host` put there. Only a test can write
        it: a store's first run stays in it, the next one gets a store of its own, and an escrow
        ledger refuses a second run, so this run has none."""
        manifest = self.setup.manifest.body()
        del manifest["training"]["reference_spec"]["env"]["pinned"]
        run_id = "0" * 64  # sorted before the live run
        store._db.execute(
            "INSERT INTO runs VALUES(?,?,?,'created',NULL)", (run_id, json.dumps(manifest), "{}")
        )
        live, real = store._services, ChallengeStore._services.__get__(store)
        self.monkeypatch.setattr(store, "_services", lambda r: real(r) if r == run_id else live(r))
        return run_id

    def idle(self, store: ChallengeStore, *, retired: bool, ledger: bool) -> None:
        """A run in `store` that never finalized a reward, so it owes no v2 weight; `ledger`
        opens its escrow first. Its services cannot be built: a retired manifest is refused,
        and the store holds no policy object for a current one."""
        if ledger:
            E.ledger(self.setup, store._db)
        manifest = self.setup.manifest.body()
        if retired:
            del manifest["training"]["reference_spec"]["env"]["pinned"]
        store._db.execute(
            "INSERT INTO runs VALUES(?,?,?,'created',NULL)",
            (self.setup.manifest.run_id(), json.dumps(manifest), "{}"),
        )

    def child(self, name: str) -> ChallengeStore:
        child = self.root._open_child(self.root.state_dir / "runs" / name)
        self.root._children[name] = child
        return child

    def paid(self, name: str) -> int:
        return self.escrows[name].balances().paid

    def pays(self, name: str) -> int:
        return (
            self.escrows[name]
            .db.execute("SELECT COUNT(*) FROM escrow_events WHERE kind='PAY'")
            .fetchone()[0]
        )


@pytest.fixture
def hosted(tmp_path, actual_reward, monkeypatch):
    h = Hosted(tmp_path, actual_reward, monkeypatch)
    yield h
    h.root.close()


@pytest.fixture
def three_children(hosted):
    """Children a, c are healthy and b (between them, as the sorted names load) is retired."""
    hosted.host(hosted.child("a"), "a")
    hosted.host(hosted.child("b"), "b", retired=True)
    hosted.host(hosted.child("c"), "c")
    return hosted


def weights_runs(store: ChallengeStore) -> list[int]:
    return [r[0] for r in store._db.execute("SELECT epoch FROM weights_runs ORDER BY epoch")]


def test_a_retired_hosted_run_does_not_stop_the_weights_of_the_others(three_children) -> None:
    # Given: two healthy children with 1e6 payable units each and a retired one between them.
    h = three_children
    # When: Cortex asks for the epoch.
    answer = json.loads(h.root.weights(0, h.at(20), 1))
    # Then: both healthy runs are paid in full, as weight to the origin hotkey ...
    assert answer["weights"] == {E.HOT.ss58: 2.0 * UNITS}
    meta = answer["metadata"]
    assert meta["units_paid"] == 2 * UNITS
    # ... the retired run pays nothing and its share burns, never moving to the others
    # (root, a, b, c each own FULL_SHARE_MASS).
    assert (h.paid("a"), h.paid("b"), h.paid("c")) == (UNITS, 0, UNITS)
    assert h.escrows["b"].balances().available == UNITS
    assert answer["full_share_mass"] == 4 * FULL_SHARE_MASS and meta["runs"] == 4
    assert meta["units_burned_this_epoch"] == 2 * FULL_SHARE_MASS
    # And the skip is journaled with the run and the reason, in the retired run's own store.
    row = (
        h.root._children["b"]
        ._db.execute("SELECT data FROM records_v2 WHERE kind='weights-skipped'")
        .fetchone()
    )
    skipped = json.loads(row[0])
    assert skipped["epoch"] == 0
    assert skipped["reason"].startswith("RUN_MANIFEST_RETIRED: ")


def test_a_retired_run_in_the_root_does_not_stop_the_weights_of_its_children(hosted) -> None:
    # Given: the root hosts a retired run whose reward was finalized, and a healthy child.
    hosted.host(hosted.root, "root", retired=True)
    hosted.host(hosted.child("a"), "a")
    # When
    answer = json.loads(hosted.root.weights(0, hosted.at(20), 1))
    # Then: the healthy child is paid, the retired root run is not.
    assert answer["weights"] == {E.HOT.ss58: 1.0 * UNITS}
    assert (hosted.paid("a"), hosted.paid("root")) == (UNITS, 0)
    assert answer["full_share_mass"] == 2 * FULL_SHARE_MASS


def test_a_second_call_for_the_same_epoch_replays_the_first_answer(three_children) -> None:
    # Given: an epoch served once.
    h = three_children
    first = h.root.weights(0, h.at(20), 1)
    # When: it is asked again, later and with another clock, then the next epoch is asked.
    second = h.root.weights(0, h.at(20), 2)
    later = json.loads(h.root.weights(1, h.at(21), 3))
    # Then: the same bytes, one PAY per paid child, and the next epoch has nothing left to pay.
    assert second == first
    assert (h.pays("a"), h.pays("b"), h.pays("c")) == (1, 0, 1)
    assert later["weights"] == {} and later["metadata"]["units_burned_this_epoch"] == 4 * 10**6
    assert (h.paid("a"), h.paid("b"), h.paid("c")) == (UNITS, 0, UNITS)


def test_children_that_already_hold_the_epoch_are_replayed_not_paid_again(three_children) -> None:
    # Given: the S21 state, a call that failed after children a and c committed their epoch
    # answers (each in its own transaction) while the root answer was never served.
    h = three_children
    for name in ("a", "c"):
        h.root._children[name].weights(0, h.at(20), 1)
    committed = {name: h.paid(name) for name in "abc"}
    assert committed == {"a": UNITS, "b": 0, "c": UNITS}
    assert weights_runs(h.root) == []
    # When: the epoch is asked again.
    answer = json.loads(h.root.weights(0, h.at(20), 2))
    # Then: the root answer is rebuilt from those persisted answers, and nothing is paid twice.
    assert answer["weights"] == {E.HOT.ss58: 2.0 * UNITS}
    assert {name: h.paid(name) for name in "abc"} == committed
    assert (h.pays("a"), h.pays("b"), h.pays("c")) == (1, 0, 1)
    assert weights_runs(h.root) == [0]


def test_a_failed_call_leaves_no_child_paid_and_the_retry_pays_each_once(hosted) -> None:
    # Given: two healthy children, and persisting the root answer fails (a crash after the
    # children were asked).
    hosted.host(hosted.child("a"), "a")
    hosted.host(hosted.child("c"), "c")
    hosted.root._db.execute(
        "CREATE TRIGGER fail_persist BEFORE INSERT ON weights_runs "
        "BEGIN SELECT RAISE(ABORT, 'crash'); END"
    )
    with pytest.raises(sqlite3.IntegrityError):
        hosted.root.weights(0, hosted.at(20), 1)
    # Then: no child kept a paid unit or an epoch answer that was never served.
    assert (hosted.paid("a"), hosted.paid("c")) == (0, 0)
    assert (hosted.pays("a"), hosted.pays("c")) == (0, 0)
    assert all(weights_runs(hosted.root._children[name]) == [] for name in "ac")
    # When: the retry (the v1 journals already hold the epoch) persists.
    hosted.root._db.execute("DROP TRIGGER fail_persist")
    retry = json.loads(hosted.root.weights(0, hosted.at(20), 2))
    # Then: each child is paid exactly once, on the retry.
    assert retry["weights"] == {E.HOT.ss58: 2.0 * UNITS}
    assert (hosted.paid("a"), hosted.paid("c")) == (UNITS, UNITS)
    assert (hosted.pays("a"), hosted.pays("c")) == (1, 1)


def test_a_child_that_fails_leaves_the_children_asked_before_it_unpaid(hosted) -> None:
    # Given: three healthy children, the last one fails to persist its epoch answer.
    for name in "abc":
        hosted.host(hosted.child(name), name)
    hosted.root._children["c"]._db.execute(
        "CREATE TRIGGER fail_persist BEFORE INSERT ON weights_runs "
        "BEGIN SELECT RAISE(ABORT, 'crash'); END"
    )
    with pytest.raises(sqlite3.IntegrityError):
        hosted.root.weights(0, hosted.at(20), 1)
    # Then: neither a nor b kept a payment; the call is all or nothing.
    assert [hosted.paid(name) for name in "abc"] == [0, 0, 0]
    assert weights_runs(hosted.root) == []
    # When: the cause is gone and the epoch is asked again.
    hosted.root._children["c"]._db.execute("DROP TRIGGER fail_persist")
    answer = json.loads(hosted.root.weights(0, hosted.at(20), 2))
    # Then: all three are paid once.
    assert answer["weights"] == {E.HOT.ss58: 3.0 * UNITS}
    assert [hosted.pays(name) for name in "abc"] == [1, 1, 1]


def test_a_commit_that_fails_halfway_is_repaired_by_the_replay_and_pays_each_once(hosted) -> None:
    # Given: three healthy children whose answer commits run c, b, a, then the root; a's fails.
    for name in "abc":
        hosted.host(hosted.child(name), name)
    a = hosted.root._children["a"]
    real = a._db
    a._db = CommitFails(real)
    with pytest.raises(sqlite3.OperationalError):
        hosted.root.weights(0, hosted.at(20), 1)
    # Then: c and b hold the epoch they committed, a and the root hold nothing: the root, last,
    # never answers an epoch a child could not keep.
    assert [hosted.paid(name) for name in "abc"] == [0, UNITS, UNITS]
    assert weights_runs(hosted.root) == []
    # When: the epoch is asked again.
    a._db = real
    answer = json.loads(hosted.root.weights(0, hosted.at(20), 2))
    # Then: the stored answers of b and c are merged with the ones computed now, each unit once.
    assert answer["weights"] == {E.HOT.ss58: 3.0 * UNITS}
    assert [hosted.pays(name) for name in "abc"] == [1, 1, 1]
    assert weights_runs(hosted.root) == [0]


def skipped(store: ChallengeStore) -> list[tuple[str, str, int]]:
    """(run, id, epoch) of every `weights-skipped` record of `store`."""
    rows = store._db.execute(
        "SELECT run_id, id, data FROM records_v2 WHERE kind = ? ORDER BY run_id, id",
        ("weights-skipped",),
    )
    return [(r[0], r[1], json.loads(r[2])["epoch"]) for r in rows]


@pytest.mark.parametrize("where", ["root", "child"])
def test_a_retired_run_beside_a_live_one_in_the_same_store_does_not_stop_its_pay(
    hosted, where
) -> None:
    # Given: one store holds a live run with 1e6 payable units and a retired run beside it,
    # and the root hosts a healthy child too.
    store = hosted.root if where == "root" else hosted.child("b")
    hosted.host(store, "live")
    retired = hosted.retired_beside(store)
    hosted.host(hosted.child("a"), "a")
    # When
    answer = json.loads(hosted.root.weights(0, hosted.at(20), 1))
    # Then: the live run is paid in full beside the child; only the retired run is skipped.
    assert answer["weights"] == {E.HOT.ss58: 2.0 * UNITS}
    assert (hosted.paid("live"), hosted.paid("a")) == (UNITS, UNITS)
    assert skipped(store) == [(retired, "0", 0)]


def test_a_skip_is_recorded_once_per_epoch_under_that_epoch(three_children) -> None:
    # Given: the retired child b between two healthy ones.
    h = three_children
    # When: epoch 0 is asked, asked again, then epoch 1 is asked.
    first = h.root.weights(0, h.at(20), 1)
    again = h.root.weights(0, h.at(20), 2)
    h.root.weights(1, h.at(21), 3)
    # Then: the second ask is served, not refused, and wrote nothing; each epoch has one skip
    # under its own id, so a later epoch never overwrites an earlier one.
    assert again == first
    assert skipped(h.root._children["b"]) == [
        (h.setup.manifest.run_id(), "0", 0),
        (h.setup.manifest.run_id(), "1", 1),
    ]


@pytest.mark.parametrize(
    ("retired", "ledger"),
    [(True, False), (True, True), (False, False)],
    ids=["retired-no-ledger", "retired-nothing-finalized", "unbuildable-no-ledger"],
)
def test_a_run_with_nothing_to_pay_is_not_built_by_the_weights_call(
    hosted, retired, ledger
) -> None:
    # Given: a child whose run never finalized a reward and whose services cannot be built
    # (S21: 7b0e20e5, 9981d976 and the root's 42f68b45), and a healthy child.
    idle = hosted.child("b")
    hosted.idle(idle, retired=retired, ledger=ledger)
    hosted.host(hosted.child("a"), "a")
    # When
    answer = json.loads(hosted.root.weights(0, hosted.at(20), 1))
    # Then: the epoch is answered and pays the healthy child; the idle run owes nothing, so it
    # is neither built nor booked as skipped.
    assert answer["weights"] == {E.HOT.ss58: 1.0 * UNITS}
    assert skipped(idle) == []


def dangling_at_commit(store: ChallengeStore) -> None:
    """The next epoch answer of `store` fails at COMMIT and SQLite keeps the transaction open,
    as with SQLITE_BUSY: the answer leaves a deferred foreign key dangling."""
    store._db.execute("PRAGMA foreign_keys=ON")
    store._db.executescript("""
      CREATE TABLE answered(epoch INTEGER PRIMARY KEY);
      CREATE TABLE answer_ref(
        epoch INTEGER REFERENCES answered(epoch) DEFERRABLE INITIALLY DEFERRED);
      CREATE TRIGGER dangling AFTER INSERT ON weights_runs
        BEGIN INSERT INTO answer_ref VALUES(NEW.epoch); END;
    """)


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="_tx: no ROLLBACK after a failed COMMIT (Lens N1, HW4-M4)",
)
def test_a_store_whose_commit_fails_is_left_outside_its_transaction(hosted) -> None:
    # Given: three healthy children; a's answer COMMIT fails and SQLite keeps the transaction.
    for name in "abc":
        hosted.host(hosted.child(name), name)
    a = hosted.root._children["a"]
    dangling_at_commit(a)
    with pytest.raises(sqlite3.IntegrityError):
        hosted.root.weights(0, hosted.at(20), 1)
    # Then: a rolled back what its answer wrote and holds no open transaction, ...
    assert not a._db.in_transaction
    assert [hosted.paid(name) for name in "abc"] == [0, UNITS, UNITS]
    assert weights_runs(a) == [] and weights_runs(hosted.root) == []
    # ... so once the cause is gone the replay pays a, merges b and c, and pays each once.
    a._db.execute("DROP TRIGGER dangling")
    answer = json.loads(hosted.root.weights(0, hosted.at(20), 2))
    assert answer["weights"] == {E.HOT.ss58: 3.0 * UNITS}
    assert [hosted.pays(name) for name in "abc"] == [1, 1, 1]
    assert a.healthy()
