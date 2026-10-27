"""Baseline of the audit-cheaters world: honest rounds are clean, the snapshot is faithful, and the
stalls a cheating roster provokes are recorded as data instead of raised.

Every test below drives the real v2 path (run-v2 -> UPLOADED -> audit -> aggregate -> finalize ->
get_weights) on CPU with the tiny fixture model; the first one pays the one-off graduation of the
five identities. Nothing here talks to a live host.
"""

from __future__ import annotations

import hashlib
import importlib.util
import json
import os
import shutil
import sqlite3
import subprocess
import sys
import threading
import time
from pathlib import Path
from types import SimpleNamespace
from typing import Any

import ac_cheats
import ac_world
import httpx
import pytest
from ac_world import (
    GPU_SKIP,
    GraduatedSnapshot,
    MinerOutcome,
    Settlement,
    Shape,
    World,
    ensure_snapshot,
    peak_rss_mb,
    record,
    require_cpu,
)

slow = pytest.mark.slow
HERE = Path(__file__).resolve().parent
# The ef_out_hash the commit claims is not the one the replay recomputes: a MISMATCH at audit.
WRONG_EF_COMMITMENT = ac_cheats.BY_NAME["commit-lie-ef-out-hash"]


def honest_round(world: World, roster: list[int]) -> tuple[list[MinerOutcome], Settlement]:
    world.open_round(0, roster)
    outcomes = world.run_roster({i: None for i in roster})
    return outcomes, world.settle(0)


def tree(root: Path) -> dict[str, str]:
    """Relative path -> sha256 of every file under `root`; empty when `root` does not exist."""
    return {
        str(path.relative_to(root)): hashlib.sha256(path.read_bytes()).hexdigest()
        for path in sorted(root.rglob("*"))
        if path.is_file()
    }


def table_rows(db: sqlite3.Connection) -> dict[str, list[str]]:
    """Every row of every table, as sorted strings, keyed by table name."""
    names = [
        row[0]
        for row in db.execute(
            "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'"
        )
    ]
    return {
        name: sorted(repr(tuple(row)) for row in db.execute(f"SELECT * FROM {name}"))
        for name in names
    }


# The directories of the master state besides challenge.db: written literally here so that a
# directory dropped from ac_world.STATE_DIRS is noticed instead of silently not compared.
STATE_TREES = ("objects", "trials-v2", "ledger")
MARKER = "marker/state.bin"
BEACON_STEP = 7
GENESIS_AGE = 30 * 86400


def marked_snapshot(
    root: Path, scratch: Path, monkeypatch: pytest.MonkeyPatch
) -> GraduatedSnapshot:
    """Snapshot a network whose graduation is replaced by writing one marker file into each
    state directory and moving the beacon, so that restoring each of them is observable without
    paying for 12 reference trials. The network is built with a genesis 30 days in the past."""
    real_time = time.time

    def mark(svc: Any, n: Any, i: int) -> None:
        for name in STATE_TREES:
            target = n.store.state_dir / name / MARKER
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(f"{name}|{i}".encode())
        n.push(n.now + BEACON_STEP)

    monkeypatch.setattr(ac_world.smoke, "_graduate", mark)
    with monkeypatch.context() as aged:
        aged.setattr(time, "time", lambda: real_time() - GENESIS_AGE)
        return ensure_snapshot(root, Shape(identities=5, graduated=1), scratch)


def new_threads(before: set[threading.Thread]) -> list[threading.Thread]:
    return [t for t in threading.enumerate() if t not in before]


@slow
def test_honest_roster_of_five_uploads_matches_and_is_paid_equally(ac_clone: World) -> None:
    """Defence: honest miners are never excluded, sanctioned or underpaid by the audit path."""
    world = ac_clone
    hotkeys = [world.hotkey(i) for i in range(5)]

    outcomes, settlement = honest_round(world, list(range(5)))
    paid = world.weights()
    seen = world.observe()
    record(
        {
            "test": "honest-roster-of-five",
            "run_miner_seconds": [round(o.seconds, 2) for o in outcomes],
            "settle_seconds": round(settlement.seconds, 2),
            "restore_seconds": round(world.restore_seconds or 0, 2),
            "peak_rss_mb": round(peak_rss_mb()),
        }
    )

    assert [o.status for o in outcomes] == ["UPLOADED"] * 5
    assert settlement.settled, [(s.name, s.status, s.detail) for s in settlement.steps]
    assert settlement.results == dict.fromkeys(hotkeys, "MATCH")
    assert all(v.first_bad_leaf is None and v.completed for v in settlement.verdicts)
    assert list(settlement.included) == sorted(hotkeys)
    assert seen.excluded == {}
    assert {row["result"] for row in seen.leases} == {"MATCH"}
    assert seen.admissions.keys() >= set(hotkeys)
    for hotkey in hotkeys:
        row = seen.admissions[hotkey]
        assert (row["state"], row["strikes"], row["pending_dispute"], row["clean_count"]) == (
            "ACTIVE",
            0,
            0,
            12,
        )
    assert paid.status_code == 200
    assert paid.weights == dict.fromkeys(hotkeys, 200000.0)
    assert seen.escrow["conserved"] is True
    assert {o["admission_locked"] for o in seen.escrow_by_owner.values()} == {1000}


@slow
def test_clone_restored_from_the_snapshot_reproduces_the_fresh_network(
    ac_snapshot: Any, tmp_path: Path
) -> None:
    """Defence: a restored clone is a faithful substitute for the network that graduated.

    Same run_id, and the same honest round gives the same replayed leaves, theta, finalize
    hash and payout on the graduating network and on the clone. It needs the live network that
    graduated, so it skips on a cached snapshot; the file-by-file and row-by-row comparison
    that works on any snapshot is test_clone_holds_every_file_and_row_of_the_snapshot.
    """
    fresh = ac_snapshot.origin
    if fresh is None:
        pytest.skip("snapshot found on disk: the process that graduated it is gone")
    roster = list(range(4))

    with fresh:
        fresh_outcomes, fresh_settlement = honest_round(fresh, roster)
        fresh_paid = fresh.weights()
    clone = ac_snapshot.clone(tmp_path)
    try:
        with clone:
            clone_outcomes, clone_settlement = honest_round(clone, roster)
            clone_paid = clone.weights()
    finally:
        clone.close()

    assert clone.run_id == fresh.run_id
    assert [o.status for o in fresh_outcomes] == ["UPLOADED"] * 4
    assert [o.status for o in clone_outcomes] == ["UPLOADED"] * 4
    assert fresh_settlement.settled and clone_settlement.settled
    assert {v.hotkey: (v.result, v.recomputed_leaves_root) for v in clone_settlement.verdicts} == {
        v.hotkey: (v.result, v.recomputed_leaves_root) for v in fresh_settlement.verdicts
    }
    aggregate = "aggregate"
    assert (
        clone_settlement.step(aggregate).body["theta_hash"]
        == (fresh_settlement.step(aggregate).body["theta_hash"])
    )
    assert clone_settlement.finalize_hash == fresh_settlement.finalize_hash
    assert clone_paid.weights == fresh_paid.weights != {}
    record(
        {
            "test": "snapshot-faithful",
            "graduate_seconds": [round(s, 1) for s in ac_snapshot.meta["graduate_seconds"]],
            "snapshot_seconds": round(ac_snapshot.meta["snapshot_seconds"], 2),
            "restore_seconds": round(clone.restore_seconds or 0, 2),
        }
    )


@slow
def test_roster_member_that_never_commits_is_recorded_as_data_and_never_sanctioned(
    ac_clone: World,
) -> None:
    """Defence: the master's answer to a silent roster member is recorded as data, never raised,
    and nobody is sanctioned for it (honest eviction is not cheating).

    Today the master refuses to schedule the audits and the round stalls: that is the AC-LIVENESS
    hole, pinned in test_absence_and_eviction_rounds.py. A master that excludes the silent member
    and settles the others would be as valid here, so only what holds under both is asserted and
    the observed answer goes to the evidence file."""
    world = ac_clone
    roster = list(range(4))
    silent = world.hotkey(3)

    world.open_round(0, roster)
    outcomes = world.run_roster({i: None for i in roster[:3]})
    settlement = world.settle(0)
    seen = world.observe()
    stalled = settlement.stalled_at
    answer = None if stalled is None else settlement.step(stalled)
    record(
        {
            "test": "silent-roster-member",
            "stalled_at": stalled,
            "status": None if answer is None else answer.status,
            "detail": None if answer is None else answer.detail,
            "settled": settlement.settled,
            "leases": len(seen.leases),
            "excluded": sorted(seen.excluded),
        }
    )

    assert [o.status for o in outcomes] == ["UPLOADED"] * 3
    if settlement.settled:
        assert silent not in settlement.included
    else:
        assert answer is not None and answer.status >= 400 and answer.detail
    assert all(v.result == "MATCH" for v in settlement.verdicts)
    assert {row["strikes"] for row in seen.admissions.values()} == {0}
    assert {row["state"] for row in seen.admissions.values()} == {"ACTIVE"}


@slow
def test_exclusion_in_a_roster_of_four_excludes_only_its_miner_and_records_the_aggregate_answer(
    ac_clone: World,
) -> None:
    """Defence: a non-MATCH verdict excludes only its miner, and the master's answer to
    aggregating the three survivors (capped at Q/4 each, so Q is unreachable) is recorded.

    Today that answer is an uncaught InsufficientEligibleWeight (AC-CLIFF, pinned in
    test_missing_sanctions_and_wedges.py); a handled refusal is the same stop here, so only
    "the aggregate step is the one that fails" is asserted and the observed answer is recorded."""
    world = ac_clone
    roster = list(range(4))
    cheater = world.hotkey(3)

    world.open_round(0, roster)
    outcomes = world.run_roster({0: None, 1: None, 2: None, 3: WRONG_EF_COMMITMENT})
    settlement = world.settle(0)
    seen = world.observe()
    aggregate = settlement.step("aggregate")
    record(
        {
            "test": "roster-of-four-one-exclusion",
            "aggregate_status": None if aggregate is None else aggregate.status,
            "aggregate_exception": None if aggregate is None else aggregate.exception,
            "aggregate_detail": None if aggregate is None else aggregate.detail,
        }
    )

    assert [o.status for o in outcomes] == ["UPLOADED"] * 4
    assert settlement.results == {
        **{world.hotkey(i): "MATCH" for i in range(3)},
        cheater: "MISMATCH",
    }
    assert list(seen.excluded) == [f"0:{cheater}"]
    assert seen.excluded[f"0:{cheater}"]["reason"] == "UNVERIFIED"
    assert list(settlement.included) == sorted(world.hotkey(i) for i in range(3))
    assert settlement.stalled_at == "aggregate"
    assert aggregate is not None and aggregate.status >= 400
    assert settlement.step("finalize") is None and not settlement.settled


@slow
def test_the_two_halves_of_a_round_give_the_same_settlement_as_settle(ac_clone: World) -> None:
    """Defence: audit_round stops before the aggregate and finish_round completes the very
    round it was given, so a scenario can act between the audits and the aggregate."""
    world = ac_clone
    hotkeys = sorted(world.hotkey(i) for i in range(4))

    world.open_round(0, list(range(4)))
    world.run_roster({i: None for i in range(4)})
    audited = world.audit_round(0)

    assert audited.audits_done and not audited.settled
    assert [s.name for s in audited.steps if s.name in ("aggregate", "finalize")] == []
    assert audited.finalize_hash is None
    assert audited.results == dict.fromkeys(hotkeys, "MATCH")

    finished = world.finish_round(audited)

    assert finished.settled
    assert [s.name for s in finished.steps][-2:] == ["aggregate", "finalize"]
    assert finished.steps[:-2] == audited.steps
    assert finished.verdicts == audited.verdicts
    assert list(finished.included) == hotkeys
    assert finished.finalize_hash == world.finalize_hash(0)


@slow
def test_clone_holds_every_file_and_row_of_the_snapshot(
    ac_snapshot: GraduatedSnapshot, tmp_path: Path
) -> None:
    """Defence: restoring a snapshot loses nothing and adds nothing, on a cached snapshot too.

    The objects, trials-v2 and ledger trees of the clone are byte-identical to the snapshot's,
    and every table of challenge.db holds the same rows."""
    clone = ac_snapshot.clone(tmp_path)
    try:
        state = clone.network.store.state_dir
        directories = sorted(path.name for path in state.iterdir() if path.is_dir())
        restored = {name: tree(state / name) for name in STATE_TREES}
        rows = table_rows(clone.network.store._db)
    finally:
        clone.close()

    assert directories == sorted(STATE_TREES)
    for name in STATE_TREES:
        saved = tree(ac_snapshot.root / name)
        assert saved, f"the graduated snapshot carries no file under {name}/"
        assert restored[name] == saved, name
    with sqlite3.connect(ac_snapshot.root / "challenge.db") as source:
        assert rows == table_rows(source)


def test_clone_of_a_marked_network_restores_its_state_trees_and_database(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Defence: a snapshot taken from a live network is restored whole into a clone.

    The network carries a file in each of objects/, trials-v2/ and ledger/ and a beacon pushed
    ahead of its genesis state, none of which a fresh network has; the clone must show all of
    them, with the origin's run_id."""
    snapshot = marked_snapshot(tmp_path / "snapshots", tmp_path / "origin", monkeypatch)
    origin = snapshot.origin
    assert origin is not None
    try:
        clone = snapshot.clone(tmp_path / "clone")
        try:
            state = origin.network.store.state_dir
            assert sorted(p.name for p in state.iterdir() if p.is_dir()) == sorted(STATE_TREES)
            for name in STATE_TREES:
                live = tree(state / name)
                assert MARKER in live, name
                assert tree(snapshot.root / name) == live, f"{name} was not written to the snapshot"
                assert tree(clone.network.store.state_dir / name) == live, f"{name} not restored"
            assert clone.run_id == origin.run_id
            assert clone.network.now == origin.network.now
            assert table_rows(clone.network.store._db) == table_rows(origin.network.store._db)
        finally:
            clone.close()
    finally:
        snapshot.close()


def test_a_failed_snapshot_leaves_no_partial_directory(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Defence: a snapshot that cannot be written is not left half-written in the shared
    snapshot directory."""
    root = tmp_path / "snapshots"
    real_copytree = ac_world.shutil.copytree

    def full_disk(src: Any, dst: Any, *args: Any, **kwargs: Any) -> Any:
        if ".partial-" in str(dst):
            raise OSError(28, "No space left on device")
        return real_copytree(src, dst, *args, **kwargs)

    with monkeypatch.context() as patched:
        patched.setattr(ac_world.shutil, "copytree", full_disk)
        with pytest.raises(OSError, match="No space left"):
            marked_snapshot(root, tmp_path / "origin", monkeypatch)

    assert sorted(path.name for path in root.iterdir()) == []


def test_a_snapshot_published_first_by_another_process_is_used_with_its_own_meta(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Defence: when another process publishes the same shape first, the snapshot we return
    describes what is on disk. Each fixture network stamps its own genesis, hence its own
    run_id, so the winner's snapshot is not ours."""
    root = tmp_path / "snapshots"
    winner_meta: dict[str, Any] = {}
    real_rename = os.rename

    def lost_race(source: Any, target: Any) -> None:
        if Path(target) != root / Shape(identities=5, graduated=1).key:
            return real_rename(source, target)
        shutil.copytree(source, target)
        meta = json.loads((Path(target) / "meta.json").read_text())
        winner_meta.update(meta, run_id="e" * 64, genesis_time=meta["genesis_time"] + 1000)
        (Path(target) / "meta.json").write_text(json.dumps(winner_meta))
        raise OSError(39, "Directory not empty")

    with monkeypatch.context() as patched:
        patched.setattr(os, "rename", lost_race)
        snapshot = marked_snapshot(root, tmp_path / "origin", monkeypatch)
    try:
        assert snapshot.meta == winner_meta
        assert snapshot.origin is None
        assert sorted(path.name for path in root.iterdir()) == [snapshot.root.name]
    finally:
        snapshot.close()


def test_closing_a_world_stops_every_thread_it_started(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Defence: World.close() releases the app's portal, executor and notify threads; a suite of
    hundreds of clones must not accumulate them."""
    before = set(threading.enumerate())
    snapshot = marked_snapshot(tmp_path / "snapshots", tmp_path / "origin", monkeypatch)
    clone = snapshot.clone(tmp_path / "clone")
    started = new_threads(before)
    assert started, "building a world starts no thread: this test would prove nothing"

    clone.close()
    snapshot.close()

    for thread in started:
        thread.join(timeout=10)
        assert not thread.is_alive(), thread.name


@pytest.mark.parametrize("inherited", [None, "/elsewhere/lib"])
def test_an_installed_world_moves_the_clock_of_child_processes_and_restores_everything(
    inherited: str | None, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Defence: `with world:` makes the torchrun workers see the same clock as the master,
    never puts the working directory on their path, and leaves the process as it found it."""
    snapshot = marked_snapshot(tmp_path / "snapshots", tmp_path / "origin", monkeypatch)
    origin = snapshot.origin
    assert origin is not None
    if inherited is None:
        monkeypatch.delenv("PYTHONPATH", raising=False)
    else:
        monkeypatch.setenv("PYTHONPATH", inherited)
    real_time, real_client = time.time, httpx.AsyncClient
    try:
        with origin:
            parents = os.environ["PYTHONPATH"].split(os.pathsep)
            child = subprocess.run(  # noqa: S603 - fixed interpreter and code, no user input
                [sys.executable, "-c", "import time; print(time.time())"],
                capture_output=True,
                text=True,
                check=True,
                timeout=60,
            )
            master_clock = time.time()
            assert time.time is not real_time and httpx.AsyncClient is not real_client
        after = (
            time.time is real_time,
            httpx.AsyncClient is real_client,
            os.environ.get("PYTHONPATH"),
            "HT_FIXTURE_CLOCK_OFFSET" in os.environ,
        )
    finally:
        snapshot.close()

    assert parents[1:] == ([] if inherited is None else [inherited])
    assert abs(float(child.stdout) - master_clock) < 30
    assert master_clock < real_time() - GENESIS_AGE / 2
    assert after == (True, True, inherited, False)


def test_a_step_records_the_answer_or_the_exception_instead_of_raising() -> None:
    """Defence: every scenario reads Step.status, detail and exception, so a refusal, a body
    that is not JSON and an uncaught server error must all come back as data."""
    world = World.__new__(World)

    def explode() -> Any:
        raise ZeroDivisionError("boom")

    refused = world._step("a", lambda: httpx.Response(409, json={"detail": "no roster"}))
    html = world._step("b", lambda: httpx.Response(502, text="Bad Gateway"))
    fine = world._step("c", lambda: httpx.Response(200, json={"x": 1}), "subject")
    crashed = world._step("d", explode)

    assert (refused.status, refused.detail, refused.exception) == (409, "no roster", None)
    assert (html.status, html.detail, html.body) == (502, "Bad Gateway", None)
    assert (fine.ok, fine.detail, fine.body, fine.subject) == (True, None, {"x": 1}, "subject")
    assert (crashed.status, crashed.exception, crashed.detail) == (500, "ZeroDivisionError", "boom")


def test_service_level_tests_are_skipped_without_the_opt_in_and_only_those(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """Defence: without AUDIT_CHEATERS_FULL=1 exactly the tests that need a graduated network
    skip, wherever they sit under this directory; with it nothing is skipped."""
    spec = importlib.util.spec_from_file_location("audit_cheaters_conftest", HERE / "conftest.py")
    assert spec is not None and spec.loader is not None
    gate = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(gate)

    def item(path: Path, *fixtures: str) -> Any:
        marks: list[Any] = []
        return SimpleNamespace(
            fspath=str(path), fixturenames=fixtures, marks=marks, add_marker=marks.append
        )

    def skipped(env: str | None) -> list[str]:
        if env is None:
            monkeypatch.delenv("AUDIT_CHEATERS_FULL", raising=False)
        else:
            monkeypatch.setenv("AUDIT_CHEATERS_FULL", env)
        items = {
            "clone": item(HERE / "test_a.py", "tmp_path", "ac_clone"),
            "snapshot": item(HERE / "test_b.py", "ac_snapshot"),
            "nested": item(HERE / "deeper" / "test_c.py", "ac_clone"),
            "audit-level": item(HERE / "test_d.py", "tmp_path", "ac_device"),
            "elsewhere": item(HERE.parent / "ledger" / "test_e.py", "ac_clone"),
        }
        gate.pytest_collection_modifyitems(list(items.values()))
        return [name for name, it in items.items() if any(m.name == "skip" for m in it.marks)]

    assert skipped(None) == ["clone", "snapshot", "nested"]
    assert skipped("0") == ["clone", "snapshot", "nested"]
    assert skipped("true") == ["clone", "snapshot", "nested"]
    assert skipped("1") == []


def test_evidence_line_is_appended_only_when_the_variable_is_set(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Defence: record() writes exactly one JSON line per call to $AUDIT_CHEATERS_EVIDENCE and
    writes nothing when it is unset."""
    target = tmp_path / "evidence.jsonl"
    monkeypatch.delenv("AUDIT_CHEATERS_EVIDENCE", raising=False)
    record({"unset": True})
    assert not target.exists()

    monkeypatch.setenv("AUDIT_CHEATERS_EVIDENCE", str(target))
    record({"b": 2, "a": 1})
    record({"second": True})
    assert target.read_text().splitlines() == ['{"a": 1, "b": 2}', '{"second": true}']


def test_a_device_other_than_cpu_skips_with_the_gpu_message() -> None:
    """Defence: the CPU suite never pretends to cover the GPU path."""
    assert require_cpu("cpu") == "cpu"
    with pytest.raises(pytest.skip.Exception, match=GPU_SKIP):
        require_cpu("cuda")
