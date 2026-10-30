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

# Red on the S21 base (e5900c3b); the fix commits remove these marks.
paid_unserved = pytest.mark.xfail(
    strict=True, raises=AssertionError, reason="a child commits its payment before the root answers"
)


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


@paid_unserved
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


@paid_unserved
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
