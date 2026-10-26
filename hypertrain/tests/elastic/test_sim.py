"""The eviction simulator: every schedule ends in the state of the uninterrupted run."""

import random
from collections import Counter
from dataclasses import replace

import pytest
from hypothesis import given, settings
from hypothesis import strategies as st

from hypertrain.elastic.checkpoint import WorkUnit
from hypertrain.elastic.sim import (
    POLICY,
    RUN_ID,
    SCENARIOS,
    Fault,
    Schedule,
    SimConfig,
    Simulation,
    genesis,
    main,
    random_config,
    simulate,
)

CFG = SimConfig()
LOSSY = ("kill", "loss", "partition", "store-partition", "region-loss", "drought", "churn")


@pytest.mark.parametrize("seed", range(4))
@pytest.mark.parametrize("name", SCENARIOS)
def test_every_scenario_ends_in_the_uninterrupted_state(name: str, seed: int) -> None:
    result = simulate(CFG, name, seed)
    assert result.violations == ()
    assert result.completed and result.final_ok
    assert result.accepted_steps == CFG.rounds * CFG.shards * CFG.steps


@settings(max_examples=300, deadline=None)
@given(seed=st.integers(0, 2**32 - 1), name=st.sampled_from(SCENARIOS))
def test_random_runs_keep_every_invariant(seed: int, name: str) -> None:
    assert simulate(random_config(random.Random(seed)), name, seed).violations == ()


@pytest.mark.parametrize("name", LOSSY)
def test_each_loss_wastes_at_most_one_checkpoint_interval(name: str) -> None:
    for seed in range(5):
        result = simulate(CFG, name, seed)
        lost = sum(loss.steps for loss in result.losses)
        assert result.max_loss <= CFG.ckpt_every
        assert result.wasted_steps <= lost <= len(result.losses) * CFG.ckpt_every


def test_a_drained_notice_loses_no_step() -> None:
    drained = 0
    for seed in range(6):
        result = simulate(CFG, "notice", seed)
        assert result.exits.get("notice", 0) == result.drains
        assert result.wasted_steps == 0 and result.max_loss == 0
        assert result.drain_ms <= result.drains * CFG.ckpt_ms
        drained += result.drains
    assert drained >= 4


def test_a_notice_too_short_to_flush_leaves_at_once_and_once() -> None:
    drains = 0
    for seed in range(10):
        result = simulate(CFG, "short-notice", seed)
        assert result.violations == ()
        assert result.exits.get("notice", 0) == result.drains and result.drain_ms == 0
        drains += result.drains
    assert drains >= 3


def test_lapses_and_notices_never_penalise_an_honest_miner() -> None:
    exits: Counter[str] = Counter()
    for name in (*LOSSY, "notice", "region-notice"):
        for seed in range(4):
            result = simulate(CFG, name, seed)
            assert result.penalized == ()
            exits.update(result.exits)
    assert exits["lapse"] > 0 and exits["notice"] > 0 and exits["stalled"] > 0
    assert exits["replaced"] == 0
    restarts = replace(CFG, restart_ms=5_000)  # back before the lease runs out: a replacement
    assert sum(simulate(restarts, "kill", seed).exits.get("replaced", 0) for seed in range(4))
    assert all(simulate(restarts, "kill", seed).penalized == () for seed in range(4))


def test_a_cheater_is_penalised_and_its_forged_link_never_counts() -> None:
    result = simulate(replace(CFG, cheaters=(1,)), "none", 0)
    assert result.penalized == ("miner-01",)
    assert result.exits.get("revoked") == 1
    assert result.final_ok and result.violations == ()


def test_resuming_the_same_heads_again_and_again_changes_nothing() -> None:
    # One host killed after each accepted link (61 s) and again inside the next restore (7 s):
    # every replacement resumes from a head that is not the base, some resumes twice.
    cfg = replace(SimConfig(hosts=1, regions=1, shards=1, steps=12, rounds=1), ckpt_every=3)
    cfg = replace(cfg, restart_ms=5_000)
    kills = [Fault(3_000 + 68_000 * i + gap, "kill", 0) for i in range(8) for gap in (0, 61_000)]
    sim = Simulation(cfg, Schedule("kill-storm", tuple(kills)), 0)
    result = sim.run()
    assert result.violations == () and result.final_ok
    assert result.exits.get("replaced", 0) >= 4 and result.resumes >= 4
    unit = WorkUnit(RUN_ID, POLICY, 0, 0, sim.store.put(genesis()), cfg.steps)
    chain = sim.checkpoints.chain(unit, sim.coord.settle().heads[0])
    assert [link.step for link in chain] == [3, 6, 9, 12]
    assert len({link.lease for link in chain}) == 4  # each link from another host


def test_a_short_notice_inside_a_partition_is_not_a_failed_drain() -> None:
    # The notice lands while the host cannot reach the coordinator and the kill follows a
    # second later: the leave cannot go out, and that is the partition's doing, not the drain's.
    faults = (Fault(9_000, "partition", 0, 34_000), Fault(16_000, "notice", 0, 17_000))
    result = Simulation(CFG, Schedule("notice-in-partition", faults), 0).run()
    assert result.violations == () and result.final_ok
    assert result.exits.get("lapse") == 1 and result.exits.get("notice", 0) == 0


def test_a_slow_flush_keeps_its_lease_and_leaves_with_a_notice() -> None:
    cfg = replace(CFG, ckpt_ms=30_000, ttl_ms=20_000)  # the flush outlasts one lease
    for seed in range(4):
        result = simulate(cfg, "notice", seed)
        assert result.violations == () and result.wasted_steps == 0
        assert result.exits.get("notice", 0) == result.drains


def test_the_simulator_is_deterministic() -> None:
    assert simulate(CFG, "churn", 3) == simulate(CFG, "churn", 3)


def test_the_table_reports_every_scenario(capsys: pytest.CaptureFixture[str]) -> None:
    assert main(["table", "--seed", "1"]) == 0
    rows = [line for line in capsys.readouterr().out.splitlines() if line.startswith("| ")]
    names = [row.split(" | ")[0].removeprefix("| ") for row in rows[1:]]
    assert names == [*SCENARIOS, "churn + cheater"]
    assert all(row.split(" | ")[1:3] == ["yes", "yes"] for row in rows[1:])
