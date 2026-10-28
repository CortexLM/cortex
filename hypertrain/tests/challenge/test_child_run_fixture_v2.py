"""The `child_network` fixture hosts its run in a child store, and the services work there."""
# ruff: noqa: F811  (the fixture parameters shadow the imported fixtures)

from __future__ import annotations

from typing import Any

import pytest

from hypertrain.miner.island_launch import IslandFailure

from .child_run_fixture_v2 import child_network, pending_reference  # noqa: F401
from .test_service_network_v2 import network  # noqa: F401  (pytest fixture import)


def count(store: Any, sql: str, *args: Any) -> int:
    return int(store._db.execute(sql, args).fetchone()[0])


def trials(store: Any) -> dict[str, str]:
    """Outcome of the current trial of every admission the store hosts."""
    return {
        row["admission_id"]: row["outcome"]
        for row in store._db.execute(
            "SELECT a.admission_id, t.outcome FROM admissions_v2 a "
            "JOIN admission_trial_results t ON t.epoch=a.trial_epoch"
        )
    }


def test_the_run_is_hosted_by_a_child_store_next_to_a_root_occupant(child_network):
    n = child_network
    root, child, run_id = n.root, n.store, n.manifest.run_id()
    assert child is not root and root.for_run(run_id) is child
    assert root._children == {run_id: child}
    assert run_id not in root._services_v2 and run_id in child._services_v2
    # The root keeps the first run, and the child shares nothing mutable with it.
    assert list(root._services_v2) == [n.occupant.run_id()] and run_id != n.occupant.run_id()
    assert child._db is not root._db and child._lock is not root._lock
    assert child.objects is not root.objects
    assert child.state_dir == root.state_dir / "runs" / run_id
    # The run is complete in the child: manifest, dataset and genesis origins are its own rows.
    assert count(child, "SELECT COUNT(*) FROM runs WHERE run_id=?", run_id) == 1
    assert count(root, "SELECT COUNT(*) FROM runs WHERE run_id=?", run_id) == 0
    dataset = child._record_v2(run_id, "dataset", "inputs")
    assert child.objects.get(dataset["samples_hash"])
    assert len(n.origin_ids) == 4 and all(
        count(child, "SELECT COUNT(*) FROM escrow_origins WHERE origin=?", origin) == 1
        and count(root, "SELECT COUNT(*) FROM escrow_origins WHERE origin=?", origin) == 0
        for origin in n.origin_ids
    )


def test_a_round_pushed_through_the_root_reaches_the_child_table(child_network):
    n = child_network
    root, child = n.root, n.store
    before = child._now(child._db)
    assert child._db is not root._db and before == n.now
    n.push(before + 1)
    assert root._now(root._db) == child._now(child._db) == before + 1
    row = child._db.execute("SELECT signature FROM beacon WHERE round=?", (before + 1,)).fetchone()
    assert row["signature"] == root._known_sig(root._db, before + 1)


def test_a_join_lock_and_challenge_open_a_trial_in_the_child(child_network):
    n = child_network
    admission_id = pending_reference(n)
    assert trials(n.store) == {admission_id: "OPEN"}
    # The root hosts no admission: the trial exists in the child's tables only.
    assert trials(n.root) == {}
    assert count(n.root, "SELECT COUNT(*) FROM admissions_v2") == 0


def test_a_second_admission_gets_its_own_open_trial_in_the_child(child_network):
    n = child_network
    first, second = pending_reference(n, 0), pending_reference(n, 1)
    assert first != second
    assert trials(n.store) == {first: "OPEN", second: "OPEN"}
    assert trials(n.root) == {}


def test_the_owner_route_opens_the_trial_of_a_child_admission(child_network):
    n = child_network
    # Only the admission's own hotkey may ask: the helper must find it in the child's rows.
    admission_id = pending_reference(n, 1, http=True)
    assert trials(n.store) == {admission_id: "OPEN"} and trials(n.root) == {}


def test_the_helper_serves_the_root_layout_too(network):
    admission_id = pending_reference(network)
    assert network.store._children == {}
    assert trials(network.store) == {admission_id: "OPEN"}


class RecordsTheLaunch:
    """The role launch of an executor that is reached at once and fails the reference."""

    def __init__(self) -> None:
        self.asked: list[tuple[str, Any]] = []
        self.launched: list[tuple[Any, Any, str]] = []

    def __call__(self, kind: str, job: Any, directory: Any, context: Any) -> Any:
        self.asked.append((kind, context["admission_id"]))

        def launch(job: Any, directory: Any, *, backend: str, **options: Any) -> Any:
            self.launched.append((job.run_id, directory, backend))
            raise IslandFailure("the fake executor stops at once")

        return launch


def test_a_reference_on_the_child_reaches_the_launch_set_on_the_child(child_network):
    n = child_network
    root, child, run_id = n.root, n.store, n.manifest.run_id()
    reached, never = RecordsTheLaunch(), RecordsTheLaunch()
    # `_open_child` copied the root's launch when the child was created: the child's own is set.
    root._role_launch, child._role_launch = never, reached
    admission_id = pending_reference(n)
    with pytest.raises(IslandFailure):
        child.trial_reference_v2(run_id, admission_id)
    assert reached.asked == [("reference", admission_id)]
    [(job_run, directory, backend)] = reached.launched
    assert (job_run, backend) == (run_id, "cuda")
    # The trial inputs were staged under the child's state directory, not the root's.
    assert directory.parent == root.state_dir / "runs" / run_id / "trials-v2"
    assert never.asked == [] and never.launched == []
