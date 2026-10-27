"""get_weights over HTTP and over several runs (WGT-09, 15, 16, 20, 23 and AU-06, AU-07).

The HTTP world is the lot's `service_world`: the real v2 network fixture with an app that holds the
internal bearer. Its metagraph is replaced by one the test changes between requests. Live origins
are written the way issuance and maturity leave them (see support_weights).
"""

from __future__ import annotations

import sqlite3
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path
from types import ModuleType

import httpx
import pytest
from hypothesis import given
from hypothesis import strategies as st

from hypertrain.challenge.store import ChallengeError
from hypertrain.ledger import FULL_SHARE_MASS
from hypertrain.ledger.escrow_v2 import EscrowV2

from .support import World, error_code, service_world
from .support_weights import (
    Metagraph,
    add_origin,
    assert_answer_shape,
    events,
    fail_persist,
    live_gate,
    mode_store,
    parse,
    pay_events,
    recover_persist,
)

ORIGIN = "aa" * 32


@contextmanager
def live_world(
    root: Path, li_escrow: ModuleType, origins: list[tuple[str, str, int]], registered: set[str]
) -> Iterator[tuple[World, Metagraph, EscrowV2]]:
    """A service whose run holds matured live origins (id, hotkey, units) and whose master lists
    `registered`. The clock stands four epochs after genesis, so honest epochs are 0..4."""
    with service_world(root) as world:
        world.network.now = 5000
        escrow: EscrowV2 = world.store._services(world.run_id)[0]
        live_gate(world.store._db)
        for origin, hotkey, units in origins:
            add_origin(escrow, origin, hotkey, units, 1, li_escrow.COLD.ss58)
        metagraph = world.app.state.metagraph
        master = Metagraph(metagraph.netuid, registered)
        metagraph.client = httpx.AsyncClient(transport=master.transport())
        yield world, master, escrow


def master_changes(world: World, master: Metagraph, **state: object) -> None:
    """The master's next metagraph answer; the route's 30 s cache is dropped."""
    for name, value in state.items():
        setattr(master, name, value)
    world.app.state.metagraph._cache = None


def now_of(world: World) -> int:
    return int(world.store.clock())


# --- WGT-09 / AU-07: epoch and epoch_at are bounded by the clock ---------------------------------


def test_the_default_epoch_at_is_the_end_of_the_epoch(tmp_path: Path) -> None:
    with service_world(tmp_path) as world:
        world.network.now = 5000
        params = world.store.params
        answer = world.get_weights(3).json()
    assert answer["metadata"]["epoch_at"] == params.genesis_unix + 4 * params.epoch_seconds


@pytest.mark.parametrize("epoch", [0, 3])
def test_an_epoch_of_a_run_without_rewards_is_a_defined_full_burn_and_final(
    tmp_path: Path, epoch: int
) -> None:
    """WGT-10: no live reward, no unit: an explicit empty answer, 200, the same bytes again."""
    with service_world(tmp_path) as world:
        world.network.now = 5000
        first = world.get_weights(epoch, epoch_at=now_of(world))
        again = world.get_weights(epoch, epoch_at=now_of(world) + 9)
    assert first.status_code == 200
    answer = parse(first.content)
    assert_answer_shape(answer)
    assert answer["weights"] == {} and answer["metadata"]["reason"] == "ok"
    assert answer["metadata"]["units_burned_this_epoch"] == FULL_SHARE_MASS
    assert again.content == first.content


@pytest.mark.parametrize("far", ["epoch_at", "epoch"])
@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="WGT-09+RTA-09+AU-07: one request with an epoch or epoch_at far ahead of the clock "
    "never turns every later honest epoch into an out_of_order burn (both are bounded only by "
    "2**63 and raise the append-only watermarks for good)",
)
def test_one_far_ahead_request_never_freezes_the_later_honest_epochs(
    tmp_path: Path, li_escrow: ModuleType, far: str
) -> None:
    with live_world(tmp_path, li_escrow, [(ORIGIN, "hk", 300_000)], {"hk"}) as (world, _, _):
        now = now_of(world)
        # Given: an honest epoch is answered (the control reaches the watermarks).
        control = world.get_weights(0, epoch_at=now).json()
        assert control["metadata"]["reason"] == "ok" and control["weights"] == {"hk": 300_000.0}
        before = world.db_digest()
        # When: one request carries an absurd epoch_at (or epoch), then an honest epoch follows.
        if far == "epoch_at":
            request, honest = world.get_weights(1, epoch_at=2**62), (2, now + 1)
        else:
            request, honest = world.get_weights(2**40, epoch_at=now + 1), (1, now + 1)
        # Then: a refusal changes no state, and the honest epoch is answered in order.
        assert request.status_code < 500
        if request.status_code >= 400:
            assert world.db_digest() == before
        after = world.get_weights(honest[0], epoch_at=honest[1])
        assert after.status_code == 200 and after.json()["metadata"]["reason"] == "ok"


# --- WGT-16: units marked paid reach a registered hotkey ----------------------------------------


@pytest.mark.parametrize("failure", ["deregistered", "metagraph_down"])
@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="WGT-16+WGT-D2: units of a hotkey the master does not list, or whose answer could not "
    "be filtered, stay owed (they are marked paid before the metagraph is asked)",
)
def test_units_that_were_not_delivered_to_a_registered_hotkey_stay_owed(
    tmp_path: Path, li_escrow: ModuleType, failure: str
) -> None:
    origins = [(ORIGIN, "hk-registered", 300_000), ("bb" * 32, "hk-gone", 200_000)]
    registered = {"hk-registered"} if failure == "deregistered" else {"hk-registered", "hk-gone"}
    with live_world(tmp_path, li_escrow, origins, registered) as (world, master, escrow):
        master_changes(world, master, up=failure == "deregistered")
        # When: the first ask of the epoch.
        first = world.get_weights(0, epoch_at=now_of(world))
        # Then: the registered hotkey is paid what it earned (a control for both failures).
        if failure == "deregistered":
            assert first.status_code == 200
            assert first.json()["weights"] == {"hk-registered": 300_000.0}
            owed = {"hk-gone": 200_000}
        else:
            assert first.status_code == 503
            owed = {"hk-registered": 300_000, "hk-gone": 200_000}
        # And what was not delivered stays free for a later epoch.
        free = {
            row["owner_hotkey"]: row["units"]
            for row in escrow.db.execute(
                "SELECT o.hotkey owner_hotkey,u.units FROM escrow_units u JOIN escrow_origins o "
                "ON o.origin=u.origin WHERE u.bucket='available' AND u.units>0 AND o.hotkey "
                "IS NOT NULL"
            )
        }
        assert free == owed


# --- WGT-20 / AU-06: replay of a final answer ----------------------------------------------------


def test_a_stored_answer_replays_without_the_v2_services_and_not_while_the_metagraph_is_down(
    tmp_path: Path, li_escrow: ModuleType
) -> None:
    with live_world(tmp_path, li_escrow, [(ORIGIN, "hk", 300_000)], {"hk"}) as (world, master, _):
        now = now_of(world)
        first = world.get_weights(0, epoch_at=now)
        assert first.status_code == 200 and first.json()["weights"] == {"hk": 300_000.0}

        # Condition kept: with the master up, a replay needs no escrow at all.
        def refuse(run_id: str) -> tuple:
            raise ChallengeError(503, "no v2 service is built for a stored epoch")

        world.store._services = refuse
        assert world.get_weights(0, epoch_at=now + 1).content == first.content
        # Condition broken: the master cannot answer, so a non-empty answer is not replayed.
        stored = world.db_digest()
        master_changes(world, master, up=False)
        down = world.get_weights(0, epoch_at=now)
        assert down.status_code == 503 and world.db_digest() == stored
        # Restored, the very same bytes come back.
        master_changes(world, master, up=True)
        assert world.get_weights(0, epoch_at=now).content == first.content


def test_the_registered_filtered_answer_is_final_even_after_the_hotkey_leaves(
    tmp_path: Path, li_escrow: ModuleType
) -> None:
    """AU-06: the answer the master received for an epoch is the bytes it receives for ever."""
    origins = [(ORIGIN, "hk-stays", 300_000), ("bb" * 32, "hk-leaves", 200_000)]
    with live_world(tmp_path, li_escrow, origins, {"hk-stays", "hk-leaves"}) as (world, master, _):
        now = now_of(world)
        first = world.get_weights(0, epoch_at=now)
        assert first.json()["weights"] == {"hk-leaves": 200_000.0, "hk-stays": 300_000.0}
        # When: one hotkey leaves the subnet and the epoch is asked again, then a new epoch.
        master_changes(world, master, registered={"hk-stays"})
        again = world.get_weights(0, epoch_at=now + 7)
        # Then: the same bytes; the registered filter is not applied a second time.
        assert again.content == first.content
        assert world.count("weights_registered", "epoch=?", (0,)) == 1
        assert_answer_shape(parse(again.content))


# --- WGT-15, WGT-23: several runs, one answer ----------------------------------------------------


@contextmanager
def two_stores(li_escrow: ModuleType) -> Iterator[tuple]:
    """A root store and one hosted child, each with a matured live origin of its own."""
    with mode_store(li_escrow) as root, mode_store(li_escrow) as child:
        root.origin("aa" * 32, "hk-shared", 700_000, 5)
        root.origin("bb" * 32, "hk-root", 100_000, 5)
        child.origin("cc" * 32, "hk-shared", 400_000, 5)
        root.store._children["child-run"] = child.store
        yield root, child
        root.store._children.clear()


def test_a_merged_answer_sums_the_runs_and_each_run_pays_only_its_own_origins(
    li_escrow: ModuleType,
) -> None:
    """WGT-15/WGT-23 (condition kept): one database per run, every additive field summed."""
    with two_stores(li_escrow) as (root, child):
        # When
        first = root.store.weights(0, root.when(20), 1)
        merged = parse(first)
        # Then: the shared hotkey is summed, the mass is one share per run.
        assert merged["weights"] == {"hk-root": 100_000.0, "hk-shared": 1_100_000.0}
        meta = merged["metadata"]
        assert (meta["units_paid"], meta["units_burned_this_epoch"]) == (1_200_000, 800_000)
        assert (merged["full_share_mass"], meta["runs"]) == (2 * FULL_SHARE_MASS, 2)
        assert_answer_shape(merged, mass=2 * FULL_SHARE_MASS)
        # And each escrow paid only the origins of its own database.
        assert [e["origin"] for e in pay_events(root.escrow)] == ["aa" * 32, "bb" * 32]
        assert [e["origin"] for e in pay_events(child.escrow)] == ["cc" * 32]
        assert root.escrow.balances().paid == 800_000 and child.escrow.balances().paid == 400_000
        # And the merged answer is final.
        assert root.store.weights(0, root.when(40), 9) == first


@given(
    runs=st.lists(
        st.dictionaries(st.sampled_from(["a", "b", "c"]), st.integers(1, 400_000), max_size=3),
        min_size=2,
        max_size=4,
    )
)
def test_merging_runs_is_sum_preserving_and_independent_of_the_order_of_the_children(
    runs: list[dict[str, int]],
) -> None:
    """WGT-15 (property): per-hotkey sums, additive metadata and mass add up; the children's order
    changes nothing but the order the answers are listed in."""
    from hypertrain.challenge.store import _merge_answers
    from hypertrain.ledger.journal import canonical

    def answer(weights: dict[str, int]) -> bytes:
        paid = sum(weights.values())
        return canonical(
            {
                "challenge_slug": "hypertrain",
                "epoch": 3,
                "weights": {h: float(u) for h, u in sorted(weights.items())},
                "full_share_mass": FULL_SHARE_MASS,
                "metadata": {
                    "epoch_at": 9,
                    "round_at": 1,
                    "reason": "ok",
                    "units_paid": paid,
                    "units_burned_this_epoch": FULL_SHARE_MASS - paid,
                    "hotkeys_capped": 0,
                    "ledger_minted": paid,
                    "ledger_burned": 0,
                    "ledger_paid": paid,
                    "ledger_pending": 0,
                },
                "computed_at": 5,
            }
        )

    merged = parse(_merge_answers([answer(w) for w in runs]))
    total = {h: sum(w.get(h, 0) for w in runs) for h in {h for w in runs for h in w}}
    assert merged["weights"] == {h: float(total[h]) for h in sorted(total)}
    assert_answer_shape(merged, mass=len(runs) * FULL_SHARE_MASS)
    meta = merged["metadata"]
    assert meta["runs"] == len(runs)
    assert meta["ledger_paid"] == meta["units_paid"] == sum(sum(w.values()) for w in runs)
    tail_reversed = [runs[0], *reversed(runs[1:])]
    assert parse(_merge_answers([answer(w) for w in tail_reversed])) == merged


@pytest.mark.parametrize("retried", [True, False])
def test_a_child_payment_reaches_an_answer_only_when_the_same_epoch_is_retried(
    li_escrow: ModuleType, retried: bool
) -> None:
    """WGT-15 (condition): a child commits its payout before the root persists the merged answer.

    The master must retry the same epoch after a failure; asking another epoch instead leaves the
    child's units marked paid in a stored child answer that no delivered answer ever carried.
    """
    with two_stores(li_escrow) as (root, child):
        fail_persist(root.store._db)
        # Given: the root fails to persist the merged answer after the child committed.
        with pytest.raises(sqlite3.IntegrityError):
            root.store.weights(0, root.when(20), 1)
        recover_persist(root.store._db)
        assert child.escrow.balances().paid == 400_000
        assert root.escrow.balances().paid == 0
        # When: the master retries epoch 0, or moves on to epoch 1.
        epoch = 0 if retried else 1
        merged = parse(root.store.weights(epoch, root.when(21), 2))
        # Then
        if retried:
            assert merged["weights"] == {"hk-root": 100_000.0, "hk-shared": 1_100_000.0}
        else:
            assert merged["weights"] == {"hk-root": 100_000.0, "hk-shared": 700_000.0}
        delivered = int(sum(merged["weights"].values()))
        assert delivered == (1_200_000 if retried else 800_000)
        assert child.escrow.balances().paid == 400_000
        assert len(pay_events(child.escrow)) == 1


def test_a_second_escrow_cannot_be_opened_for_another_run_on_the_same_database(
    li_escrow: ModuleType,
) -> None:
    """WGT-23 (condition broken in a test-owned stand-in): the escrow tables carry no run id, so
    the constructor itself refuses a second run, even under the same economics policy."""
    from dataclasses import replace

    from hypertrain.protocol.messages_v2 import RunManifestV2

    with mode_store(li_escrow) as host:
        host.origin("aa" * 32, "hk", 300_000, 5)
        setup = li_escrow.setup("test")
        body = setup.manifest.body()
        body["network"]["aggregation_policy_hash"] = "22" * 32
        other = replace(setup, manifest=RunManifestV2.model_validate(body))
        assert other.manifest.run_id() != host.escrow.run_id
        assert other.policy == setup.policy
        state, written = host.escrow._snapshot(), events(host.escrow)
        # When
        code = error_code(lambda: li_escrow.ledger(other, host.store._db))
        # Then: refused with its own code and the first run's tables are untouched.
        assert code == "LEDGER_MODE_OR_RUN_CHANGED"
        assert host.escrow._snapshot() == state and events(host.escrow) == written
