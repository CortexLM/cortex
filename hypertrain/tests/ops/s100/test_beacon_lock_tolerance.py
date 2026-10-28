"""The cadence `challenge_image.sh beacon` writes, against a store lock held for a long time.

`beacon_model` drives the real `Pusher.run` through the outage (docs/adr/0012). What it must show:
no round between the first and the last one the store holds is skipped (a trial's seed is the round
after the one it was finalized in, and the challenge reads that exact round), and the pusher does
not give up while the lock lasts. Which numbers the script writes is read from the compose file it
leaves behind, so an edit of the script moves these tests. Both layouts of the challenge are run:
the run in the root store, and the run in a child store with a lock of its own.

The production `Pusher` pushes the head first. `AscendingPusher` (model only) is the one it
replaced: the tests that explain the old cadence, or reproduce S13, run against it.
"""

from __future__ import annotations

import httpx
import pytest

import hypertrain.beacon.push as push

from .beacon_model import (
    MEASURED,
    TIMEOUT,
    AscendingPusher,
    Costs,
    Relays,
    World,
    simulate,
)
from .sandbox import BEACON

OFFSETS = (0.0, 0.7, 1.4, 2.1, 2.8, 3.5, 5.0, 7.0)  # where in a drand period the lock begins
LAYOUTS = ("root", "child")
# What the previous version of the script left on the TEST stack, with the failure budget S13
# reported ("5 of its 10 allowed failures"); the sandbox compose file has 5.
OLD = {"interval": 0.2, "catch_up": 3, "max_failures": 10}


@pytest.fixture
def shipped(box, backup) -> dict[str, float]:
    box.run("beacon", backup, mutates={BEACON})
    entrypoint = box.live_yaml()["services"]["beacon"]["entrypoint"]
    return {
        "interval": float(entrypoint[9]),
        "catch_up": int(entrypoint[11]),
        "max_failures": int(entrypoint[13]),
    }


@pytest.mark.parametrize("layout", LAYOUTS)
@pytest.mark.parametrize("lock", [13, 19, 43, 60, 150])
def test_the_ascending_pusher_skips_no_round_in_a_lock_of_up_to_two_and_a_half_minutes(
    shipped, lock, layout
):
    for offset in OFFSETS:
        got = simulate(lock, offset=offset, layout=layout, pusher=AscendingPusher, **shipped)
        assert (got.exit_code, got.holes) == (0, 0), (lock, offset, got)
        assert got.final_lag <= 2, (lock, offset, got)  # the gate is open again at the end
        # The price of never skipping: the pusher refills oldest first and the gate stays closed
        # for up to about four times the lock (ADR 0012 section 3).
        assert got.gate_closed_after_lock <= 4 * lock + 20, (lock, offset, got)


@pytest.mark.parametrize("layout", LAYOUTS)
def test_the_pusher_outlasts_a_lock_of_five_minutes(shipped, layout):
    for offset in OFFSETS:
        got = simulate(300, offset=offset, layout=layout, **shipped)
        assert got.exit_code == 0 and got.timeouts < shipped["max_failures"], (offset, got)


@pytest.mark.parametrize("layout", LAYOUTS)
def test_the_model_sees_what_was_wrong_with_the_old_cadence(layout):
    old = {"pusher": AscendingPusher}
    lock = {"root": 13, "child": 60}[layout]
    skipped = [simulate(lock, offset=o, layout=layout, **old, **OLD).holes for o in OFFSETS]
    assert max(skipped) >= 1  # catch-up 3: the pusher never refills what the lock hid
    # each of the two numbers matters on its own
    assert simulate(120, layout="root", **old, **OLD).exit_code == 1  # 10 failures: gone at ~110 s
    assert simulate(60, interval=0.2, catch_up=60, max_failures=5, **old).exit_code == 1
    twenty = simulate(120, interval=0.2, catch_up=20, max_failures=100, layout="root", **old)
    assert twenty.holes > 0


def test_the_model_reproduces_two_measurements_of_s13(shipped):
    old = {"pusher": AscendingPusher}  # the pusher S13 ran
    # "5 of its 10 allowed failures per 60 s of lock"
    timeouts = {simulate(60, offset=o, layout="root", **old, **shipped).timeouts for o in OFFSETS}
    assert timeouts == {5}
    # catch-up 3 after a 13 s lock (cache kept): the first answer came 20 s after the reference
    for layout in LAYOUTS:
        closed = [
            simulate(13, offset=o, layout=layout, **old, **OLD).gate_closed_after_lock
            for o in OFFSETS
        ]
        assert 8 <= sum(closed) / len(closed) <= 28, layout


@pytest.mark.parametrize("pusher", [push.Pusher, AscendingPusher], ids=["head-first", "ascending"])
def test_the_shipped_cadence_keeps_no_hole_when_a_round_costs_more_but_not_when_it_costs_the_period(
    shipped, pusher
):
    """The cadence assumes the pusher keeps up: 2.4 s a round against the 3 s period. The gate
    stays closed much longer at 2.6 s (ascending); neither pusher keeps up at 2.9 s (ADR 0012)."""
    for cost in (1.0, 1.3):  # 2.0 s and 2.6 s a round
        got = simulate(60, costs=Costs(cost, cost), pusher=pusher, **shipped)
        assert (got.exit_code, got.holes, got.final_lag <= 2) == (0, 0, True), cost
    got = simulate(60, costs=Costs(1.45, 1.45), pusher=pusher, **shipped)
    assert got.final_lag > 2 or got.holes > 0  # behind the clock for good, or rounds dropped


@pytest.mark.parametrize("layout", LAYOUTS)
@pytest.mark.parametrize("lock", [13, 19, 43, 60, 100, 150])
def test_the_production_pusher_reopens_the_gate_within_seconds_and_skips_no_round(
    shipped, lock, layout
):
    """The head goes first, then one older round per tick (ADR 0012 section 3)."""
    for offset in OFFSETS:
        got = simulate(lock, offset=offset, layout=layout, **shipped)
        assert (got.exit_code, got.holes) == (0, 0), (lock, offset, got)
        assert got.final_lag <= 2 and got.gate_closed_after_lock <= 10, (lock, offset, got)


@pytest.mark.parametrize("layout", LAYOUTS)
def test_the_production_pusher_covers_the_plan_window_and_no_more(shipped, layout):
    """The plan window is the one cut: a tick that pushes the head and one older round takes about
    5 s while the clock adds 1.7 rounds, so a backlog clears slowly (minutes), but what the first
    plan found is kept until pushed. `--catch-up` bounds what that first plan finds: 60 rounds
    are 180 s, so a lock of 150 s is covered and one of 200 s is not (ADR 0012 section 3)."""

    def holes(lock: int, catch_up: int) -> int:
        cadence = {**shipped, "catch_up": catch_up}
        return max(simulate(lock, offset=o, layout=layout, **cadence).holes for o in OFFSETS)

    assert holes(150, 60) == 0
    assert holes(150, 100) == 0
    if layout == "root":  # the child store's own table is fed by the root's rounds as they come
        assert holes(200, 60) > 0
        assert holes(200, 100) == 0


POST = "/v1/admin/beacon"
ROUND_7 = {"round": 7, "signature": "aa", "randomness": "bb"}


@pytest.mark.parametrize(
    "started", [9.5, 11.0], ids=["verify-ends-in-the-lock", "arrives-in-the-lock"]
)
def test_a_post_that_times_out_is_applied_once_the_lock_is_freed_and_the_next_get_sees_it(started):
    # push_beacon verifies the signature before it takes the lock: a request whose verify is done
    # while the lock is held waits for the lock, not for the lock plus a second verify. The service
    # goes on with a request whose client gave up.
    world = World((10.0, 28.0), MEASURED, 1e9, "root")
    before = world.known(0.0)
    world.now = started
    with pytest.raises(httpx.ReadTimeout):
        world.post(POST, ROUND_7, {})
    assert world.now == pytest.approx(started + TIMEOUT)
    assert world.known(27.9) == before  # the lock is still held: nothing is applied yet
    got = world.get("/v1/beacon/latest")  # waits for the lock, then reads what it holds
    assert got.json() == {"round": 7}
    assert world.now == pytest.approx(28.01)


def test_the_pusher_is_charged_for_each_relay_fetch_and_the_service_for_each_post():
    # the latest round is fetched once and reused, each older one costs a fetch of its own, the
    # GET is nearly free and every POST costs the service's verify
    world = World(None, Costs(pusher=5.0, service=2.0), 1e9, "root")
    world.now = 30.0
    done = AscendingPusher(world, Relays(world), "token", 3).tick()  # type: ignore[arg-type]
    assert len(done["pushed"]) == 3
    assert world.now == pytest.approx(30.0 + 5.0 + 0.01 + 2 * 5.0 + 3 * (2.0 + 0.01))
    assert world.known(world.now) == done["latest"]  # the last POST is applied when answered


def test_in_a_child_store_the_root_keeps_the_round_at_once_and_the_child_gets_it_after_the_lock():
    # the run's store holds the lock: the root answers GET during it and applies a POST's round
    # when its own verify is done, but the store that hosts the run (the gate, the seeds) waits
    world = World((10.0, 28.0), MEASURED, 1e9, "child")
    world.now = 11.0
    with pytest.raises(httpx.ReadTimeout):
        world.post(POST, ROUND_7, {})
    assert world.known(12.3) == 7  # the root has it
    assert world.timeline()(27.9) == world.emitted(0.0) and world.timeline()(28.0) == 7
    world.now = 20.0
    assert world.get("/v1/beacon/latest").json() == {"round": 7}  # not blocked by the lock
    assert world.now == pytest.approx(20.01)


def test_a_tick_pushes_the_head_then_the_oldest_skipped_round_and_nothing_else():
    # the clock at 24 s is in drand round 9 and the service holds round 1: 7, 8 and 9 are due. An
    # unsorted set of {7, 8, 9} iterates 8 first, so only a sorted refill picks 7.
    world = World(None, Costs(pusher=0.1, service=0.1), 1e9, "root")
    world.now = 24.0
    pusher = push.Pusher(world, Relays(world), "token", 3)  # type: ignore[arg-type]
    first = pusher.tick()
    assert (first["latest"], first["known"], first["pushed"]) == (9, 1, [9, 7])
    second = pusher.tick()  # still inside round 9, which the service now holds
    assert (second["known"], second["pushed"]) == (9, [8])
    assert pusher.tick()["pushed"] == []  # nothing is due: nothing is pushed again


def test_the_model_stands_for_the_client_timeout_and_the_gate_bound_of_the_code(
    tmp_path, monkeypatch
):
    """The model copies two constants of the code; a change of either must reach this file."""
    import inspect
    import re
    from types import SimpleNamespace

    import hypertrain.beacon.__main__ as beacon_cli
    from hypertrain.challenge.store import ChallengeStore

    from .beacon_model import LAG_BOUND

    gate = re.search(
        r"now < emitted - (\d+)", inspect.getsource(ChallengeStore._snapshot_current_v2)
    )
    assert gate and int(gate[1]) == LAG_BOUND

    seen: dict[str, object] = {}

    class Client:
        def __init__(self, **options: object) -> None:
            seen.update(options)

        def __enter__(self) -> Client:
            return self

        def __exit__(self, *exc: object) -> None:
            return None

    token = tmp_path / "admin.token"
    token.write_text("t" * 32)
    monkeypatch.setattr(httpx, "Client", Client)
    monkeypatch.setattr(beacon_cli, "Pusher", lambda *a: SimpleNamespace(run=lambda *r: 0))
    argv = ["push", "--api", "http://svc.test", "--admin-token-file", str(token)]
    relays = ["--relay", "https://a.test", "--relay", "https://b.test"]
    assert beacon_cli.main([*argv, *relays, "--once"]) == 0
    assert seen["timeout"] == TIMEOUT
