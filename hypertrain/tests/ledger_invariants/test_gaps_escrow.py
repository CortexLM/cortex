"""Gaps of the escrow group, service half: what the first wave left open around the ledger.

GAP-06 (who writes the credit tables), GAP-07 (maturity after a restart), M-05/CL-29 (the hotkey
a rotated admission is paid on), T-ESC-04 (the id of a dispute's settlement), T-ESC-08 (a dispute
or a conviction holds the disputed origin), T-ESC-28/29 (funding at accept and at finalize) and
GAP-08 (a route reads the shared connection under the store lock). Each test states a defence the
service must hold; a hole is a strict xfail whose body asserts the defence after a control that
reaches the point under test.
"""

from __future__ import annotations

import contextlib
import json
import threading
from collections.abc import Iterator, Sequence
from functools import partial
from pathlib import Path
from types import ModuleType
from typing import Any

import pytest
from pydantic import ValidationError

from hypertrain.challenge.disputes_v2 import Turn
from hypertrain.challenge.store import ChallengeError
from hypertrain.ledger.escrow_v2 import EscrowV2
from hypertrain.protocol.keys import Keypair

from . import support_exit_disputes as X
from .support import (
    SERVICE_FIXTURE,
    World,
    load_fixture_module,
    service_world,
)
from .support_gaps_escrow import (
    ask_for_the_challenge,
    at_epoch_four,
    callers,
    client_event_over_http,
    funded_cohort,
    keys_selecting,
    live_world,
    lose_stake,
    record_writers,
    resolve_fraud,
    resolved_and_settled,
    restarted,
    rotate,
    seed_pending_origin,
    seed_round_origin,
    settlement_ids,
    sql_writers,
    squat_refused,
    units_in,
)
from .support_locks import Miner, Rig, units_of, vested
from .support_opids import (
    DefenceMissing,
    DisputeWorld,
    StepRefused,
    client_event,
    dispute_world,
    fresh_id,
    service_step,
)

# --- GAP-06: the credit tables are insert-only and each record kind has one writer (static) ----

UNFUNDED = "UNFUNDED_OR_UNKNOWN_ORIGIN"
CREDIT_TABLES = frozenset(
    {
        "escrow_origins",
        "escrow_events",
        "escrow_snapshots",
        "escrow_finalized",
        "escrow_shadow_finalized",
        "escrow_units",
        "weights_runs",
        "weights_registered",
    }
)
ESCROW = "ledger/escrow_v2.py"
CREDIT_WRITERS = {
    (ESCROW, "_event", "INSERT INTO", "escrow_events"),
    (ESCROW, "_event", "INSERT INTO", "escrow_snapshots"),
    (ESCROW, "_issue", "INSERT INTO", "escrow_origins"),
    (ESCROW, "reward_finalize", "INSERT INTO", "escrow_finalized"),
    (ESCROW, "reward_finalize", "INSERT INTO", "escrow_shadow_finalized"),
    # escrow_units: the upsert of a credit, the decrement every move takes first, and the three
    # zeroings of reward_pending (maturity, the slash of an admission stake, and the lockless burn
    # of a convicted coldkey's unvested rewards).
    (ESCROW, "_credit", "INSERT INTO", "escrow_units"),
    (ESCROW, "_move", "UPDATE", "escrow_units"),
    (ESCROW, "mature", "UPDATE", "escrow_units"),
    (ESCROW, "slash", "UPDATE", "escrow_units"),
    (ESCROW, "burn_pending", "UPDATE", "escrow_units"),
    ("challenge/store.py", "weights", "INSERT INTO", "weights_runs"),
    ("challenge/app.py", "registered_weights", "INSERT INTO", "weights_registered"),
}
CLOSURE_KIND = "replayed-finality"
FINALIZATION_KINDS = frozenset({"finalize", CLOSURE_KIND, "settlement", "finalize-result"})
# records_v2 is a general table (INSERT OR REPLACE in _put_record_v2, CL-38 is a decision); the
# direct statements below write other kinds: shadow reservations, custody and the child copy of
# the qualification records.
RECORD_STATEMENTS = {
    ("challenge/admission.py", "retain_runtime_custody", "INSERT OR IGNORE INTO", "records_v2"),
    ("challenge/admission.py", "retain_trial_publication", "INSERT OR IGNORE INTO", "records_v2"),
    ("challenge/store.py", "_create_child", "INSERT INTO", "records_v2"),
    ("challenge/store.py", "_put_record_v2", "INSERT OR REPLACE INTO", "records_v2"),
    (ESCROW, "reserve_shadow", "INSERT INTO", "records_v2"),
    (ESCROW, "reward_finalize", "UPDATE", "records_v2"),
}


def test_the_credit_tables_are_written_only_by_the_statements_the_ledger_relies_on() -> None:
    # Given: every SQL statement of the package that writes a table the ledger and the weights
    # answer rest on (origins, events, snapshots, finalized rounds, unit rows, epoch answers).
    found = sql_writers(CREDIT_TABLES)
    # Then: no statement deletes or replaces a row of them (the answers and the hash chain are
    # append-only), and the writers are exactly the listed ones: a second place that touches one
    # of these tables is a new way to move or forge credit.
    assert [site for site in found if site[2] not in ("INSERT INTO", "UPDATE")] == []
    assert found == CREDIT_WRITERS


def test_each_finalization_record_kind_is_stored_only_by_finalize() -> None:
    # Given: the callers of the record store and the direct statements on records_v2.
    literal, dynamic = record_writers(FINALIZATION_KINDS)
    # Then: finalize, settlement and finalize-result are written by finalize_v2 alone and
    # replayed-finality by the one closure writer (D70), which only the three functions that
    # close a round call; no other caller picks its kind at run time than training_v2, which is
    # limited to a closed set of actions, none of them a finalization kind, and _put_changed_v2,
    # which forwards the literal kind of each of its callers, none of them a finalization kind.
    assert literal == {
        ("challenge/store.py", "finalize_v2", kind) for kind in FINALIZATION_KINDS - {CLOSURE_KIND}
    } | {("challenge/store.py", "_close_round_v2", CLOSURE_KIND)}
    assert callers("challenge/store.py", "_close_round_v2") == {
        "finalize_v2",
        "_retire_round_v2",
        "rollback_v2",
    }
    assert dynamic == {
        ("challenge/store.py", "training_v2"),
        ("challenge/store.py", "_put_changed_v2"),
    }
    assert record_writers(FINALIZATION_KINDS, "_put_changed_v2") == (set(), set())
    assert keys_selecting("training_v2", "challenge/store.py", "action") == {
        "accept",
        "commit",
        "delta",
    }
    # and the statements that write records_v2 directly are the known ones (other kinds).
    assert sql_writers(frozenset({"records_v2"})) == RECORD_STATEMENTS


# --- GAP-07: due origins mature on the first beacon after a restart -----------------------------


def due_origin_after_a_restart(world: World, origin: str, *, build_services: bool) -> int:
    """A pending origin due at round 5, a restart, optionally a request that builds the services,
    then the beacon of round 5 pushed to the new store. Returns the origin's available units."""
    cold = load_fixture_module(*SERVICE_FIXTURE)
    world.network.push(2)
    seed_pending_origin(world, origin, cold.COLD[0].ss58, 500, mature_at=5)
    assert units_in(world.store, origin, "reward_pending") == 500
    with restarted(world) as store:
        assert store._services_v2 == {}  # nothing builds the services but a request
        if build_services:
            store._services(world.run_id)
        store.push_beacon(cold.fixture_beacon(5))
        return units_in(store, origin, "available")


def test_a_due_origin_matures_on_the_first_beacon_when_a_request_built_the_services(
    tmp_path: Path,
) -> None:
    # Control of the hole below: the origin is due, nothing else holds it, and once the services
    # exist the same beacon vests it.
    with service_world(tmp_path) as world:
        assert due_origin_after_a_restart(world, fresh_id("gap-07"), build_services=True) == 500


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason=(
        "GAP-07: units due at the first beacon after a restart become available without waiting "
        "for an unrelated request to build the per-run services (push_beacon matures only the "
        "services that exist, and nothing builds them at startup) [fix-lot: none (no design yet)]"
    ),
)
def test_a_due_origin_matures_on_the_first_beacon_after_a_restart(tmp_path: Path) -> None:
    with service_world(tmp_path) as world:
        assert due_origin_after_a_restart(world, fresh_id("gap-07"), build_services=False) == 500


# --- M-05 / CL-29: matured units of a rotated admission are emitted to its current hotkey ------

PAID = 300_000


def current_hotkey(world: World, hotkey: str) -> str:
    """The hotkey the admission that `hotkey` was issued to holds now."""
    with world.store._lock:
        row = world.store._db.execute(
            "SELECT a.hotkey FROM admission_history h JOIN admissions_v2 a "
            "ON a.admission_id=h.admission_id WHERE h.hotkey=?",
            (hotkey,),
        ).fetchone()
    assert row is not None, "the hotkey has no admission"
    return str(row[0])


def paid_after(world: World) -> dict[str, float]:
    response = world.get_weights(0, epoch_at=at_epoch_four(world))
    assert response.status_code == 200, response.text
    weights: dict[str, float] = response.json()["weights"]
    return weights


def test_matured_units_are_emitted_to_the_hotkey_that_earned_them_while_it_is_the_current_one(
    tmp_path: Path,
) -> None:
    # Control of the rotation test below: the origin of the first hotkey pays that hotkey, once.
    service = load_fixture_module(*SERVICE_FIXTURE)
    hot, cold = service.HOT[0].ss58, service.COLD[0]
    origins = [(fresh_id("cl-29"), hot, cold.ss58, PAID)]
    with live_world(tmp_path, origins, {hot, service.REFEREE.ss58}) as (world, _):
        assert world.network.join().status_code == 200
        assert current_hotkey(world, hot) == hot
        assert paid_after(world) == {hot: float(PAID)}


# CRD-12+WGT-13+CL-29 closed by escrow-relay-fixes (3d6129d3): _payee_v2 pays the current hotkey.
def test_matured_units_of_a_rotated_admission_are_emitted_to_its_current_hotkey(
    tmp_path: Path,
) -> None:
    service = load_fixture_module(*SERVICE_FIXTURE)
    hot, spare, cold = service.HOT[0].ss58, service.REFEREE, service.COLD[0]
    new = spare.ss58
    origins = [(fresh_id("cl-29"), hot, cold.ss58, PAID)]
    with live_world(tmp_path, origins, {hot, new}) as (world, _):
        # Given: an admission with matured units, whose coldkey asks to rotate its hotkey.
        assert world.network.join().status_code == 200
        rotation = rotate(world, cold, hot, spare)
        assert rotation.status_code in (200, 409), rotation.text
        current = current_hotkey(world, hot)
        assert current == (new if rotation.status_code == 200 else hot)
        # When: the epoch is asked.
        weights = paid_after(world)
        # Then: the whole payment reaches the admission's current hotkey, whichever design.
        if weights != {current: float(PAID)}:
            raise DefenceMissing(f"paid {weights}, the admission now holds {current}")


# --- T-ESC-04: no client event takes the id of a dispute's settlement (slash, beacon, timeout) -


def squatted_ids(escrow: EscrowV2) -> list[str]:
    """Every id a client could sign to take the release or slash of a lock: the key the event is
    stored under, and the id the service derived before the ledger placed it in its own key space.
    The second is a plain 64-hex id any client can sign; where the service has no key space of its
    own the two are one id."""
    rows = escrow.db.execute(
        "SELECT operation_id,receipt FROM escrow_events WHERE kind IN ('RELEASE','SLASH') "
        "ORDER BY seq"
    )
    return list(
        dict.fromkeys(
            i for key, receipt in rows for i in (key, json.loads(receipt)["operation_id"])
        )
    )


@pytest.mark.parametrize("kind", ["transfer", "lock"])
def test_a_client_event_never_blocks_the_dispute_slash_id(
    li_escrow: ModuleType, tmp_path: Path, kind: str
) -> None:
    verdict, bond = "11" * 32, "44" * 32

    def opened(world: DisputeWorld) -> Turn:
        dispute_id = world.add_contest(verdict, li_escrow.HOT)
        world.bond(li_escrow.COLD, bond, dispute_id, world.origins[0])
        return world.open(li_escrow.HOT, verdict, bond, now=1)

    # Given: the control run settles a lost FRAUD contest and shows the id of the slash.
    with dispute_world(tmp_path / "control", li_escrow) as world:
        resolved_and_settled(world, opened(world), resolve_fraud)
        settle_ids = settlement_ids(world.escrow)
        squats = squatted_ids(world.escrow)
        effect = world.escrow.balances(li_escrow.COLD.ss58)
        burned = world.escrow.balances().burned
    assert len(settle_ids) == 1 and effect.dispute_locked == 0 and effect.available == 19900
    assert burned == 100

    # When: a funded bystander signs an event with that id while the dispute is open.
    with dispute_world(tmp_path / "contended", li_escrow) as world:
        turn = opened(world)
        key, origin = li_escrow.OTHER, world.origins[1]
        client_event(world.escrow, kind, key, fresh_id("neutral"), origin)
        for operation_id in squats:
            squat_refused(partial(client_event, world.escrow, kind, key, operation_id, origin))

        # Then: the referee's FRAUD resolution and the slash go through together, as without it.
        resolved_and_settled(world, turn, resolve_fraud, settle=service_step)
        assert world.disputes.get(turn.dispute_id).resolution is not None
        assert world.escrow.balances(li_escrow.COLD.ss58) == effect
        assert world.escrow.balances().burned == burned
        assert len(settlement_ids(world.escrow)) == 1


def settled_world(world: World, dispute_id: str) -> tuple[list[str], dict[str, int], int]:
    """(settlement ids, the miner's coldkey buckets, units burned) after a dispute was closed."""
    escrow = world.store._services(world.run_id)[0]
    cold = load_fixture_module(*SERVICE_FIXTURE).COLD[0].ss58
    assert X.disputes(world).get(dispute_id).resolution is not None
    return settlement_ids(escrow), units_of(escrow, cold), escrow.balances().burned


def exit_failures(world: World) -> int:
    """Disputes the beacon route could not close, as it journals them."""
    return world.count("records_v2", "kind='dispute-exit-failed'")


def squat_over_http(world: World, kind: str, operation_ids: Sequence[str]) -> None:
    """COLD[1], a funded bystander, signs an event under each of `operation_ids` on the public
    route; its first event under a neutral id proves the route takes its events."""
    bystander = load_fixture_module(*SERVICE_FIXTURE).COLD[1]
    origin = world.network.origin_ids[1]
    neutral = client_event_over_http(world, kind, bystander, fresh_id("neutral"), origin)
    assert neutral.status_code == 200, neutral.text
    for operation_id in operation_ids:
        try:
            client_event_over_http(world, kind, bystander, operation_id, origin)
        except ValidationError as error:
            # An id outside the client's own 64-hex space cannot be signed at all: nothing to post.
            assert "operation_id" in str(error)


@pytest.mark.parametrize("kind", ["transfer", "lock"])
def test_a_client_event_never_stops_the_beacon_from_closing_a_blocked_dispute(
    tmp_path: Path, kind: str
) -> None:
    # Given: the control run, where the beacon closes a contest nobody answers and releases its
    # bond; it shows the id of that release.
    with service_world(tmp_path / "control") as world:
        dispute_id = X.open_contest(world, 0)
        close_at = X.disputes(world).get(dispute_id).absolute_deadline + 1
        world.network.push(close_at)
        settle_ids, held, burned = settled_world(world, dispute_id)
        squats = squatted_ids(world.store._services(world.run_id)[0])
    assert len(settle_ids) == 1 and held["dispute_locked"] == 0 and burned == 0

    # When: a funded bystander signs an event with that id while the contest is open.
    with service_world(tmp_path / "contended") as world:
        dispute_id = X.open_contest(world, 0)
        squat_over_http(world, kind, squats)
        world.network.push(close_at)

        # Then: the beacon closed the contest and returned the bond; nothing was journaled as a
        # failure to close.
        resolution = X.disputes(world).get(dispute_id).resolution
        escrow = world.store._services(world.run_id)[0]
        locked = units_of(escrow, load_fixture_module(*SERVICE_FIXTURE).COLD[0].ss58)
        state = (resolution is not None, locked["dispute_locked"], exit_failures(world))
        assert state == (True, 0, 0), (
            f"closed={state[0]}, dispute_locked={state[1]}, dispute-exit-failed={state[2]}"
        )
        assert len(settlement_ids(escrow)) == 1 and escrow.balances().conserved()


def lose_by_referee(world: World, dispute_id: str) -> None:
    """The referee's FRAUD resolution against the contesting miner, posted to the route."""
    hot = load_fixture_module(*SERVICE_FIXTURE).HOT[0].ss58
    turn = X.disputes(world).get(dispute_id)
    response = X.post_resolution(world, X.register(world, turn, reason="FRAUD", loser=hot))
    if response.status_code != 200:
        raise StepRefused(f"the resolution was refused: {response.status_code} {response.text}")


def lose_by_timeout(world: World, dispute_id: str) -> None:
    """The operator's witnessed timeout of a contester who acknowledged the turn and went silent."""
    response = world.client.post(
        world.network.url + f"/admin/disputes/{dispute_id}/timeout",
        json=X.timeout_request(world, dispute_id),
        headers=world.admin,
    )
    if response.status_code != 200:
        raise StepRefused(f"the timeout was refused: {response.status_code} {response.text}")


def prepare_timeout(world: World, dispute_id: str) -> None:
    X.acknowledge_turn(world, dispute_id)
    world.network.push(X.disputes(world).get(dispute_id).turn_deadline + 1)


@pytest.mark.parametrize(
    ("route", "decide", "burns", "prepare"),
    [
        pytest.param(
            lose_by_referee, "FRAUD", X.BOND, lambda world, dispute: None, id="resolution"
        ),
        pytest.param(lose_by_timeout, "PARTY_TIMEOUT", 0, prepare_timeout, id="timeout"),
    ],
)
def test_a_client_event_never_blocks_the_settlement_of_a_convicted_miner_over_http(
    tmp_path: Path, route: Any, decide: str, burns: int, prepare: Any
) -> None:
    hot = load_fixture_module(*SERVICE_FIXTURE).HOT[0].ss58

    # Given: the control run convicts the miner and settles its bond; it shows the id.
    with service_world(tmp_path / "control") as world:
        dispute_id = X.open_contest(world, 0)
        prepare(world, dispute_id)
        route(world, dispute_id)
        settle_ids, held, burned = settled_world(world, dispute_id)
        squats = squatted_ids(world.store._services(world.run_id)[0])
        resolution = X.disputes(world).get(dispute_id).resolution
    assert resolution is not None and (resolution.reason, resolution.loser) == (decide, hot)
    assert len(settle_ids) == 1 and held["dispute_locked"] == 0 and burned == burns

    # When: a funded bystander signs an event with that id while the contest is open.
    with service_world(tmp_path / "contended") as world:
        dispute_id = X.open_contest(world, 0)
        squat_over_http(world, "transfer", squats)
        prepare(world, dispute_id)

        # Then: the conviction is applied and the bond settled, as without the bystander.
        route(world, dispute_id)
        again, locked, burnt = settled_world(world, dispute_id)
        assert (len(again), locked["dispute_locked"], burnt) == (1, 0, burns)


@pytest.fixture
def rig(tmp_path: Path) -> Iterator[Rig]:
    with service_world(tmp_path) as world:
        yield Rig(world)


# --- T-ESC-08: a dispute or a conviction holds the disputed miner's origin, and only that one ----


def two_miners_with_a_round_each(rig: Rig) -> tuple[Miner, Miner, str, str]:
    """Two joined miners of round 0, each with a pending origin vesting at round 5."""
    first, second = rig.join(0), rig.join(1)
    return (
        first,
        second,
        seed_round_origin(rig, first, "first"),
        seed_round_origin(rig, second, "second"),
    )


@pytest.mark.xfail(
    strict=True,
    raises=DefenceMissing,
    reason=(
        "LCK-09+DIB-07: a reward origin does not vest while a dispute naming its miner and round "
        "is open, and vests again once the dispute is resolved for the miner, but the origin's "
        "settlement is a record written at finalize that no dispute ever touches (whether a "
        "miner's own contest holds its own reward is a pending product decision) "
        "[fix-lot: decision (product choice pending)]"
    ),
)
def test_an_open_dispute_holds_the_origin_of_its_own_miner_only_and_a_match_releases_it(
    rig: Rig,
) -> None:
    # Given: two miners with a pending origin each; only the second one contests round 0.
    first, second, free, held = two_miners_with_a_round_each(rig)
    contest = rig.open_contest(second)
    # When: the beacon passes the vesting round, and one round beyond (each push answers 200).
    rig.net.push(5)
    rig.net.push(6)
    # Then: the miner nobody disputed is paid as ever (a fix cannot freeze the whole round),
    assert vested(rig, free) == 500
    # and the contester's origin waits for its dispute.
    if vested(rig, held) != 0:
        raise DefenceMissing("the origin of a miner with an open dispute vested")
    assert rig.disputes.get(contest.dispute_id).resolution is None
    # When the referee rules for the miner, the origin vests at the release round.
    assert rig.refereed(contest.dispute_id, "MATCH", None).status_code == 200
    release = rig.disputes.settlement(contest.dispute_id).release_beacon
    rig.net.push(max(release, rig.net.now) + 1)
    assert vested(rig, held) == 500
    assert rig.escrow.balances().conserved()


def test_a_miner_convicted_of_fraud_on_a_round_is_emitted_nothing_for_that_rounds_origin(
    rig: Rig,
) -> None:
    # Given: two admitted miners, each holding its bond and a pending origin of round 0; only
    # the second one contests the round, and the referee finds its fraud.
    first, second = rig.admit(0), rig.admit(1)
    seed_round_origin(rig, first, "first")
    owed = seed_round_origin(rig, second, "second")
    assert rig.escrow.balances(second.coldkey).admission_locked > 0
    contest = rig.open_contest(second)
    assert rig.refereed(contest.dispute_id, "FRAUD", second.hotkey).status_code == 200
    # When: the beacon passes the vesting round of both origins, and one round beyond.
    rig.net.push(5)
    rig.net.push(6)
    # Then: the miner nobody convicted is paid its origin (a fix cannot hold the whole round),
    # the convicted one is emitted nothing and its origin did not vest, and units are conserved.
    weights = paid_after(rig.world)
    assert weights[first.hotkey] == 500.0
    assert second.hotkey not in weights
    assert units_in(rig.world.store, owed, "available") == 0
    assert rig.escrow.balances(second.coldkey).reward_pending == 0
    assert rig.escrow.balances().conserved()


# --- T-ESC-28: an accept needs the funding the round opening needed ------------------------------


def test_an_accept_is_refused_when_the_miner_lost_its_stake_after_the_opening(rig: Rig) -> None:
    # Given: an opened round of four funded miners, and the assignment window.
    miners = funded_cohort(rig)
    opening = rig.opening(miners)
    assert rig.open_round(opening).status_code == 200
    rig.net.push(opening.d_assign)
    hashes = {m.hotkey: rig.assignment_hash(m) for m in miners}
    assert rig.accept(miners[1], hashes[miners[1].hotkey]).status_code == 200  # control
    # When: another member's stake is gone (its screen and standing are intact).
    lost = miners[0]
    lose_stake(rig, lost)
    assert not rig.status(lost).eligible
    digest = rig.world.db_digest()
    refused = rig.accept(lost, hashes[lost.hotkey])
    # Then: refused for the stale funding, nothing written; a funded member is still accepted.
    assert (refused.status_code, refused.json()["detail"]) == (
        403,
        "screen or funded admission stale",
    )
    assert rig.world.db_digest() == digest
    assert rig.accept(miners[2], hashes[miners[2].hotkey]).status_code == 200


# --- T-ESC-29: a member who lost its stake after the opening is not built into a reward ----------


def test_a_finalize_input_is_refused_for_a_member_who_lost_its_stake_after_the_opening(
    rig: Rig,
) -> None:
    # Given: an opened round of four funded members and nothing accepted yet.
    miners = funded_cohort(rig)
    assert rig.open_round(rig.opening(miners)).status_code == 200
    store = rig.world.store
    # control: with every member funded the check passes and the call stops later, at the first
    # member's missing commit.
    with pytest.raises(ChallengeError) as stopped:
        store.verified_inputs_v2(rig.run_id, 0)
    assert "eligible" not in str(stopped.value)
    # When: the first member's stake is gone.
    lose_stake(rig, miners[0])
    issued, digest = rig.escrow.balances().issued, rig.world.db_digest()
    # Then: no reward input is built from it, and nothing was finalized or credited.
    with pytest.raises(ChallengeError) as refused:
        store.verified_inputs_v2(rig.run_id, 0)
    assert (refused.value.status, str(refused.value)) == (
        403,
        "current admission funding/identity no longer eligible",
    )
    assert rig.world.db_digest() == digest
    assert rig.world.count("escrow_finalized") == 0
    assert rig.world.count("records_v2", "kind='finalize'") == 0
    assert rig.escrow.balances().issued == issued
    assert rig.escrow.balances(miners[0].coldkey).reward_pending == 0


# --- GAP-08: a read that decides on an admission is serialized with the store's transactions ----

UNREGISTERED = Keypair(b"\x66" * 32).ss58


@pytest.mark.xfail(
    strict=True,
    raises=DefenceMissing,
    reason=(
        "GAP-08: the challenge route reads the admission's hotkey on the shared connection "
        "without the store lock, so a transaction another thread has open (and later rolls back) "
        "is visible to it, and the route refuses a registered hotkey for a state that never "
        "existed (sqlite reports the transaction connection-wide) [fix-lot: none (no design yet)]"
    ),
)
def test_a_route_never_decides_on_the_uncommitted_state_of_another_thread(tmp_path: Path) -> None:
    with service_world(tmp_path) as world:
        net = world.network
        admission_id = net.join(0).json()["admission_id"]
        net.push(2)
        owner = load_fixture_module(*SERVICE_FIXTURE).HOT[0]
        # Given: the route answers as it should when nothing else is going on.
        control = ask_for_the_challenge(world, owner, admission_id)
        assert control.status_code == 200, control.text
        _, admission, _ = world.store._services(world.run_id)
        real = admission.store.by_id
        holding, release = threading.Event(), threading.Event()

        def another_thread() -> None:
            # A request that changes the admission's hotkey in a transaction and rolls it back.
            with contextlib.suppress(RuntimeError), world.store._tx():
                world.store._db.execute(
                    "UPDATE admissions_v2 SET hotkey=? WHERE admission_id=?",
                    (UNREGISTERED, admission_id),
                )
                holding.set()
                release.wait(timeout=30)
                raise RuntimeError("the transaction fails")

        writer = threading.Thread(target=another_thread)

        def by_id(identity: str) -> Any:
            # The interleaving is forced here, at the route's own read: another request opens its
            # transaction first. Sharing the lock, that request waits for this read to finish.
            if not writer.is_alive() and not release.is_set():
                writer.start()
                holding.wait(timeout=1)
            try:
                return real(identity)
            finally:
                release.set()

        admission.store.by_id = by_id  # type: ignore[method-assign]

        # When: the route is asked while another thread has a transaction open on the connection.
        answer = ask_for_the_challenge(world, owner, admission_id)
        writer.join(timeout=30)

        # Then: it answers as in the control, and the admission is exactly as it was.
        assert not writer.is_alive()
        if (answer.status_code, answer.json()) != (control.status_code, control.json()):
            raise DefenceMissing(f"the route answered {answer.status_code}: {answer.text}")
        assert (
            admission.store.by_id(admission_id).hotkey
            == load_fixture_module(*SERVICE_FIXTURE).HOT[0].ss58
        )
