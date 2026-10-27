"""The eviction simulator: every schedule ends in the state of the uninterrupted run."""

import random
from collections import Counter
from dataclasses import replace

import pytest
from hypothesis import given, settings
from hypothesis import strategies as st

from hypertrain.elastic.checkpoint import WorkUnit
from hypertrain.elastic.coordinator import Coordinator, Holder, Move, Settlement
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
    schedule,
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


def test_aggregate_reports_each_scenario_over_its_seeds(capsys: pytest.CaptureFixture[str]) -> None:
    assert main(["aggregate", "--seeds", "3", "--scenarios", "none", "loss"]) == 0
    rows = [line for line in capsys.readouterr().out.splitlines() if line.startswith("| ")]
    cells = [row.split(" | ") for row in rows[1:]]
    assert [(c[0].removeprefix("| "), c[1], c[2], c[3]) for c in cells] == [
        ("none", "3", "3", "0"),
        ("loss", "3", "3", "0"),
    ]


# the first two are commits refused after their shard moved, the others grants that no longer
# list it; rescan random_config(Random(seed)) x the schedules if either generator changes
HANDED_OVER = [
    (408, "region-notice"),
    (546, "region-notice"),
    (32, "store-partition"),
    (33, "notice"),
]


@pytest.mark.parametrize(("seed", "name"), HANDED_OVER)
def test_work_in_flight_when_its_shard_is_handed_over_is_not_a_refusal(
    seed: int, name: str
) -> None:
    # Capacity 2 and an idle host: the coordinator hands a shard over while its holder's commit
    # is on the way, and the refused commit is the hand-over's cost, not a fault of the core.
    cfg = random_config(random.Random(seed))
    sim = Simulation(cfg, schedule(name, cfg, seed), seed)
    result = sim.run()
    assert [loss.kind for loss in result.losses].count("handed over") == 1
    assert result.violations == () and result.final_ok
    assert all(loss.steps <= cfg.ckpt_every for loss in result.losses)


def test_the_oracle_still_flags_a_hand_over_the_coordinator_did_not_record(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    class Forgetful(Coordinator):
        def settle(self) -> Settlement:
            return replace(super().settle(), moves=())

    monkeypatch.setattr("hypertrain.elastic.sim.Coordinator", Forgetful)
    result = simulate(random_config(random.Random(14)), "none", 14)
    assert any("moved from a live holder" in v for v in result.violations)


@pytest.mark.parametrize(
    ("seed", "name"),
    [(8, "churn"), (14, "none"), (17, "none"), (329, "none"), (477, "none"), (605, "none")],
)
def test_runs_with_hand_overs_keep_every_invariant(seed: int, name: str) -> None:
    # Capacity 2 and idle hosts: two to five hand-overs a run, each to a host that held nothing.
    class Counting(Simulation):
        moves = 0

        def _close_round(self) -> None:
            self.moves += len(self.coord.settle().moves)
            super()._close_round()

    cfg = random_config(random.Random(seed))
    sim = Counting(cfg, schedule(name, cfg, seed), seed)
    result = sim.run()
    assert sim.moves >= 2
    assert result.violations == () and result.final_ok


def test_the_oracle_flags_a_hand_over_to_a_member_that_held_a_shard(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    cfg = SimConfig(hosts=2, regions=1, shards=2, steps=12, rounds=1)
    sim = Simulation(cfg, schedule("none", cfg, 0), 0)
    for slot in range(cfg.hosts):
        sim._boot(slot)
    sim._open(0)
    for _ in range(3):  # both hosts join and take a shard each
        sim.clock.advance(cfg.tick_ms)
        sim.coord.tick()
        for host in sim.hosts.values():
            host.tick()
    sim._check()  # records who holds what
    (first, a), (second, b) = [(shard, sim.coord.holder(shard)) for shard in range(2)]
    assert a is not None and b is not None and a.member != b.member and sim.violations == []
    # a hand-over of the first shard to the member that already holds the second
    fake = Move(first, a.member, a.token, b.member, sim.clock.now_ms())
    real = sim.coord.settle()
    monkeypatch.setattr(sim.coord, "settle", lambda: replace(real, moves=(fake,)))
    sim._check()
    assert any("handed to a busy member" in v for v in sim.violations)


def test_a_hand_over_excuses_a_move_of_its_own_round_only() -> None:
    cfg = SimConfig(hosts=2, regions=1, shards=2, steps=12, rounds=2)
    sim = Simulation(cfg, schedule("none", cfg, 0), 0)
    sim.handed.add((0, 1))  # a Move of the round before, which no displacement used
    sim.moves_seen.add(Move(0, "a", 1, "b", 0))
    sim._open(0)
    assert not sim.handed and not sim.moves_seen


def test_a_hand_over_explains_one_move_of_its_shard_and_no_other() -> None:
    cfg = SimConfig(hosts=2, regions=1, shards=2, steps=12, rounds=1)
    sim = Simulation(cfg, schedule("none", cfg, 0), 0)
    for slot in range(cfg.hosts):
        sim._boot(slot)
    sim._open(0)
    for _ in range(3):  # both hosts join and take a shard each
        sim.clock.advance(cfg.tick_ms)
        sim.coord.tick()
        for host in sim.hosts.values():
            host.tick()
    sim._check()
    gone = Holder("gone", 10_000)  # a live lease the shard is taken from, twice
    sim.handed.add((0, gone.token))  # the coordinator recorded a Move of shard 0 from it
    sim._holders[0] = gone
    sim._check()
    assert sim.violations == [] and (0, gone.token) not in sim.handed
    sim._holders[0] = gone  # the shard leaves that lease again, and nothing recorded a Move
    sim._check()
    assert any("moved from a live holder" in v for v in sim.violations)


def test_a_host_that_loses_work_to_a_hand_over_is_not_stalled_for_it() -> None:
    # Seed 2000019 at capacity 2: m02 comes back from a kill and a hand-over takes the shard it
    # is training. Without a fresh window it was stalled later with no fault in its own window.
    cfg = random_config(random.Random(2000019))
    result = simulate(cfg, "churn", 2000019)
    assert [loss.kind for loss in result.losses].count("handed over") == 1
    assert result.violations == () and result.final_ok


def test_a_host_that_trains_three_shards_in_turn_is_not_robbed_by_a_hand_over() -> None:
    # Seed 1400008 at capacity 3: m00 trains shards 0 to 3 one after the other while an idle host
    # waits. A hand-over used to take the shard m00 was training, twice (51 steps lost), and
    # then stalled it; no shard moves now, since the one m00 trains is never the one taken.
    cfg = replace(random_config(random.Random(1400008)), capacity=3)
    result = simulate(cfg, "loss", 1400008)
    assert result.violations == () and result.final_ok
    assert [loss for loss in result.losses if loss.kind == "handed over"] == []
