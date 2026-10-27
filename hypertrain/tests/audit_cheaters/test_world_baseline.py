"""Baseline of the audit-cheaters world: honest rounds are clean, the snapshot is faithful, and the
stalls a cheating roster provokes are recorded as data instead of raised.

Every test below drives the real v2 path (run-v2 -> UPLOADED -> audit -> aggregate -> finalize ->
get_weights) on CPU with the tiny fixture model; the first one pays the one-off graduation of the
five identities. Nothing here talks to a live host.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import pytest
from ac_world import (
    GPU_SKIP,
    MinerOutcome,
    Settlement,
    World,
    peak_rss_mb,
    record,
    require_cpu,
)

slow = pytest.mark.slow


@dataclass(frozen=True)
class WrongEfCommitment:
    """Stub double: commits to an error-feedback hash the replay will not reproduce."""

    cheat: int = 7
    name: str = "stub-wrong-ef-commitment"

    def launch(self, real: Any, ctx: Any) -> Any:
        def launch(job: Any, directory: Path, **kwargs: Any) -> Any:
            artifacts = real(job, directory, **kwargs)
            summary = artifacts.directory / "rank-0/summary.json"
            data = json.loads(summary.read_bytes())
            data["commitments"]["ef_out_hash"] = "ab" * 32
            summary.write_text(json.dumps(data, sort_keys=True))
            return artifacts

        return launch

    def wire(self, miner: Any, ctx: Any) -> None:
        return None


def honest_round(world: World, roster: list[int]) -> tuple[list[MinerOutcome], Settlement]:
    world.open_round(0, roster)
    outcomes = world.run_roster({i: None for i in roster})
    return outcomes, world.settle(0)


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
        assert seen.admissions[hotkey] == {
            "state": "ACTIVE",
            "strikes": 0,
            "pending_dispute": 0,
            "reason": seen.admissions[hotkey]["reason"],
            "clean_count": 12,
        }
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
    hash and payout on the graduating network and on the clone.
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
def test_roster_member_that_never_commits_stalls_the_audit_step_as_data(ac_clone: World) -> None:
    """Defence: a silent roster member is recorded as a stalled audit step, never raised, and
    nobody is sanctioned for it (honest eviction is not cheating)."""
    world = ac_clone
    roster = list(range(4))
    silent = world.hotkey(3)

    world.open_round(0, roster)
    outcomes = world.run_roster({i: None for i in roster[:3]})
    settlement = world.settle(0)
    seen = world.observe()

    assert [o.status for o in outcomes] == ["UPLOADED"] * 3
    assert settlement.stalled_at == "audits"
    audits = settlement.step("audits")
    assert audits is not None and audits.status == 409
    assert audits.detail == f"missing accepted commit: 0:{silent}"
    assert settlement.verdicts == () and not settlement.settled
    assert seen.leases == [] and seen.excluded == {}
    assert {row["strikes"] for row in seen.admissions.values()} == {0}
    assert {row["state"] for row in seen.admissions.values()} == {"ACTIVE"}


@slow
def test_exclusion_in_a_roster_of_four_is_recorded_as_the_aggregation_cliff(
    ac_clone: World,
) -> None:
    """Defence: a non-MATCH verdict excludes only its miner, and the failure of aggregating the
    three survivors (capped at Q/4 each, so Q is unreachable) is recorded, not hidden."""
    world = ac_clone
    roster = list(range(4))
    cheater = world.hotkey(3)

    world.open_round(0, roster)
    outcomes = world.run_roster({0: None, 1: None, 2: None, 3: WrongEfCommitment()})
    settlement = world.settle(0)
    seen = world.observe()

    assert [o.status for o in outcomes] == ["UPLOADED"] * 4
    assert settlement.results == {
        **{world.hotkey(i): "MATCH" for i in range(3)},
        cheater: "MISMATCH",
    }
    assert list(seen.excluded) == [f"0:{cheater}"]
    assert seen.excluded[f"0:{cheater}"]["reason"] == "UNVERIFIED"
    assert list(settlement.included) == sorted(world.hotkey(i) for i in range(3))
    assert settlement.stalled_at == "aggregate"
    aggregate = settlement.step("aggregate")
    assert aggregate is not None and aggregate.exception == "InsufficientEligibleWeight"
    assert settlement.step("finalize") is None and not settlement.settled


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
