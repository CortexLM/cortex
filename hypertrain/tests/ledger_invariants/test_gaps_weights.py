"""Gaps of the weights group: what the first wave left open in T-WGT-xx (D39, gap wave).

Each test states a defence of the get_weights path ("refuses a malformed request and leaves no
trace", "replays the stored answer", "pays only what its answer carries"). A hole is a strict xfail
whose body asserts the defence after a control that proves the scenario reaches the point; its
reason starts with the finding id and says [candidate] while the Lens skeptic pass has not
confirmed the finding. A step that makes a scenario meaningful goes through `control`, which
raises ScenarioError (not an AssertionError), so a broken scenario fails loudly instead of hiding
behind the expected failure.

This module holds the store-level and route-level gaps; the gaps that need a real audited round
are in test_gaps_weights_2.py.
"""

from __future__ import annotations

import sqlite3
from collections.abc import Iterator
from pathlib import Path
from types import ModuleType

import pytest

from hypertrain.ledger import FULL_SHARE_MASS

from .support import Hosted, Real, at, hosted_reward, mature, pays, service_world
from .support_gaps_weights import (
    ChallengeError,
    DefenceMissing,
    control,
    held,
    owed,
    register_hotkeys,
    root_and_child,
    seed_v1_entitlement,
    watermarks,
)
from .support_http_auth import quiet_client
from .support_weights import (
    add_origin,
    fail_persist,
    mode_store,
    open_store,
    parse,
    pay_events,
    recover_persist,
)
from .test_weights_2 import live_world, master_changes, now_of

ISSUED = 1_000_000
HOLD_AT = 20  # the drand round the real reward matures at
ORIGIN = "aa" * 32


@pytest.fixture
def finalized(tmp_path: Path, li_escrow: ModuleType, real_reward: Real) -> Iterator[Hosted]:
    """The real reward accepted by finalize with its settlement record, still reward_pending."""
    with hosted_reward(tmp_path, li_escrow, real_reward, real_settlement=True) as hosted:
        hosted.finalize()
        yield hosted


def pay_rows(store_db: sqlite3.Connection) -> int:
    return int(
        store_db.execute("SELECT COUNT(*) FROM escrow_events WHERE kind='PAY'").fetchone()[0]
    )


# --- T-WGT-12 (WGT-09): a malformed request is refused by the route and leaves no trace --------

MALFORMED = (
    ("epoch=-1", "epoch"),
    ("epoch=9223372036854775808", "epoch"),
    ("epoch=nan", "epoch"),
    ("epoch=inf", "epoch"),
    ("epoch=1.5", "epoch"),
    ("epoch=0&epoch_at=-1", "epoch_at"),
    ("epoch=0&epoch_at=9223372036854775808", "epoch_at"),
    ("epoch=0&epoch_at=nan", "epoch_at"),
    ("epoch=0&epoch_at=1.5", "epoch_at"),
)


def test_a_malformed_or_out_of_range_epoch_or_epoch_at_is_refused_with_422_and_leaves_no_trace(
    tmp_path: Path,
) -> None:
    with service_world(tmp_path) as world:
        world.network.now = 5000
        # The default client re-raises a server error inside the test; this one reports it as 500.
        client = quiet_client(world.app)
        before = (world.db_digest(), watermarks(world))
        # When: each malformed value is sent with a valid bearer and slug.
        answers = {
            query: client.get("/internal/v1/get_weights?" + query, headers=world.internal)
            for query, _ in MALFORMED
        }
        # Then: every one is refused by the parameter it is wrong in, and nothing was written.
        assert {query: r.status_code for query, r in answers.items()} == {
            query: 422 for query, _ in MALFORMED
        }
        for query, name in MALFORMED:
            assert answers[query].json()["detail"][0]["loc"] == ["query", name], query
        assert (world.db_digest(), watermarks(world)) == before
        assert world.count("weights_runs") == 0
        # And the ordinary current epoch is answered.
        ordinary = client.get("/internal/v1/get_weights?epoch=3", headers=world.internal)
        assert ordinary.status_code == 200 and ordinary.json()["metadata"]["reason"] == "ok"


# --- T-WGT-18 (WGT-06, WGT-13): an answer that lands between the two phases is replayed --------


def test_an_answer_that_lands_between_the_two_phases_is_replayed_not_failed_or_paid_twice(
    finalized: Hosted, li_escrow: ModuleType
) -> None:
    host = finalized
    mature(host.escrow, host.reward)
    healthy = host.store._services
    winner: list[bytes] = []

    def concurrent(run_id: str) -> tuple:
        """The first call of the loser builds its services; between the loser's two transactions
        another request (the winner) answers the same epoch completely."""
        if not winner:
            winner.append(b"")  # the winner's own service build does not recurse
            winner[0] = host.store.weights(0, at(host.escrow, HOLD_AT), 1)
        return healthy(run_id)

    host.store._services = concurrent
    # When: the loser asks the same new epoch.
    loser = host.store.weights(0, at(host.escrow, HOLD_AT), 2)
    # Then: it returns the winner's bytes, raises nothing, and the unit was paid once.
    control(
        parse(winner[0])["weights"] == {li_escrow.HOT.ss58: float(ISSUED)},
        "the winner paid the reward to its earner",
    )
    assert loser == winner[0]
    assert parse(loser)["computed_at"] == 1
    assert pays(host.escrow) == 1 and host.escrow.balances().paid == ISSUED
    assert host.store._db.execute("SELECT COUNT(*) FROM weights_runs").fetchone()[0] == 1


# --- T-WGT-15 (WGT-15, WGT-08, CL-58): a merged answer does not pay under 'out_of_order' -------


@pytest.mark.xfail(
    strict=True,
    raises=DefenceMissing,
    reason="WGT-15+CL-58: a merged answer never carries reason out_of_order while it lists "
    "weight (the reason and the non-additive metadata are the first store's; a fresh child "
    "answers ok and pays) [candidate] [fix-lot: none (no design yet)]",
)
def test_a_merged_answer_never_pays_weight_under_the_reason_out_of_order(
    li_escrow: ModuleType,
) -> None:
    origins = [(ORIGIN, "hk", ISSUED, 20)]
    with root_and_child(li_escrow, origins, [("cc" * 32, "hk", ISSUED, 5)], attached=False) as (
        root,
        child,
    ):
        # Given: the root answered epoch 1 first (nothing payable at round 19), then a fresh child
        # that holds a matured origin is attached; epoch 0 is asked for the first time.
        root.answer(1, 19)
        root.store._children["child"] = child.store
        # When
        merged = parse(root.store.weights(0, root.when(HOLD_AT), 2))
        # Then (control): the root alone is out of order for epoch 0 while the fresh child pays.
        engine = root.store.ledger._engine.answers[0]["answer"]["metadata"]["reason"]
        control(engine == "out_of_order", "the root's own ledger answered epoch 0 out of order")
        control(child.escrow.balances().paid == ISSUED, "the fresh child paid its origin")
        # Design-neutral: relabel to ok, or stop the child from paying under a global order gate.
        meta = merged["metadata"]
        if meta["reason"] == "out_of_order":
            held(
                merged["weights"] == {} and meta["units_paid"] == 0,
                f"reason out_of_order next to weights {merged['weights']} and units_paid "
                f"{meta['units_paid']}",
            )
        paid = root.escrow.balances().paid + child.escrow.balances().paid
        held(
            int(sum(merged["weights"].values())) == paid,
            f"emitted {sum(merged['weights'].values())}, booked paid {paid}",
        )


# --- T-WGT-10 (WGT-15, WGT-07, CL-57): a failure in one store of a multi-run ask -------------


def test_a_child_that_cannot_build_its_services_fails_the_ask_and_the_retry_pays_each_store_once(
    li_escrow: ModuleType,
) -> None:
    both = [(ORIGIN, "HOT", ISSUED, 5)]
    with root_and_child(li_escrow, both, [("cc" * 32, "HOT", ISSUED, 5)]) as (root, child):
        healthy = child.store._services

        def frozen(run_id: str) -> tuple:
            raise ChallengeError(409, "frozen execution backend cannot change")

        child.store._services = frozen
        # When: the child's service build fails in the middle of the ask.
        with pytest.raises(ChallengeError) as refused:
            root.store.weights(0, root.when(HOLD_AT), 1)
        # Then: the refusal propagates, the root's payout rolled back with it, the child is
        # untouched, and no answer was stored.
        assert refused.value.status == 409
        assert root.escrow.balances().available == ISSUED and root.escrow.balances().paid == 0
        assert pays(root.escrow) == 0 and pays(child.escrow) == 0
        assert child.escrow.balances().paid == 0
        assert root.store._db.execute("SELECT COUNT(*) FROM weights_runs").fetchone()[0] == 0
        assert child.store._db.execute("SELECT COUNT(*) FROM weights_runs").fetchone()[0] == 0
        # And once the child builds again, the retry of the same epoch emits both stores' units.
        child.store._services = healthy
        retry = parse(root.store.weights(0, root.when(HOLD_AT), 2))
        assert retry["weights"] == {"HOT": 2.0 * ISSUED}
        assert (retry["metadata"]["units_paid"], retry["metadata"]["runs"]) == (2 * ISSUED, 2)
        assert (pays(root.escrow), pays(child.escrow)) == (1, 1)


def test_units_a_child_would_pay_when_the_root_fails_to_persist_are_emitted_once_by_the_retry(
    li_escrow: ModuleType,
) -> None:
    both = [(ORIGIN, "HOT", ISSUED, 5)]
    with root_and_child(li_escrow, both, [("cc" * 32, "HOT", ISSUED, 5)]) as (root, child):
        fail_persist(root.store._db)
        # When: the root fails to persist the merged answer after the child computed its own.
        with pytest.raises(sqlite3.IntegrityError):
            root.store.weights(0, root.when(HOLD_AT), 1)
        # Then: the child committed nothing either (docs/findings/v2-weights-bridge.md): no unit is
        # marked paid for an answer nobody was served.
        assert root.escrow.balances().available == ISSUED and root.escrow.balances().paid == 0
        assert pays(root.escrow) == 0
        assert root.store._db.execute("SELECT COUNT(*) FROM weights_runs").fetchone()[0] == 0
        assert child.escrow.balances().available == ISSUED and pays(child.escrow) == 0
        assert child.store._db.execute("SELECT COUNT(*) FROM weights_runs").fetchone()[0] == 0
        # And the retry of the same epoch emits both stores' units, each once.
        recover_persist(root.store._db)
        retry = root.store.weights(0, root.when(HOLD_AT), 2)
        assert parse(retry)["weights"] == {"HOT": 2.0 * ISSUED}
        assert (pays(root.escrow), pays(child.escrow)) == (1, 1)
        assert root.store.weights(0, root.when(HOLD_AT + 5), 3) == retry


# --- T-WGT-13 (WGT-09, WGT-03): an omitted epoch_at is the end of the epoch and vests v2 units --


def test_an_omitted_epoch_at_is_the_end_of_the_epoch_and_the_clock_that_vests_v2_units(
    li_escrow: ModuleType,
) -> None:
    with mode_store(li_escrow) as host:
        host.origin(ORIGIN, "hk", ISSUED, HOLD_AT)
        params, beacon = host.store.params, host.escrow.manifest.training.beacon

        def drand_round(epoch: int) -> int:
            end = params.genesis_unix + (epoch + 1) * params.epoch_seconds
            return (end - beacon.genesis_time) // beacon.period + 1

        control(
            drand_round(1) < HOLD_AT <= drand_round(2),
            "the origin matures after the end of epoch 1 and before the end of epoch 2",
        )
        # When: two consecutive epochs are asked without an epoch_at.
        early = parse(host.store.weights(1, None, 1))
        late = parse(host.store.weights(2, None, 1))
        # Then: each answer is dated at the end of its epoch, and the end of the epoch decides
        # vesting: nothing before the maturity round, the whole origin after it.
        for epoch, answer in ((1, early), (2, late)):
            end = params.genesis_unix + (epoch + 1) * params.epoch_seconds
            assert answer["metadata"]["epoch_at"] == end
        assert early["weights"] == {}
        assert early["metadata"]["units_burned_this_epoch"] == FULL_SHARE_MASS
        assert late["weights"] == {"hk": float(ISSUED)}
        assert [(event["origin"], event["units"]) for event in pay_events(host.escrow)] == [
            (ORIGIN, ISSUED)
        ]


# --- T-WGT-16 (WGT-07, CL-57): a retry with another epoch_at does not pay beyond its answer ------


@pytest.mark.xfail(
    strict=True,
    raises=DefenceMissing,
    reason="WGT-07+CL-57: the retry of a journaled epoch with another epoch_at is refused, or "
    "answered and paid by the epoch_at its answer carries (the journal replays the first "
    "epoch_at while the v2 units vest by the retry's) [candidate] [fix-lot: none (no design yet)]",
)
def test_a_retry_of_a_journaled_epoch_with_another_epoch_at_pays_nothing_its_answer_omits(
    finalized: Hosted,
) -> None:
    host = finalized
    mature(host.escrow, host.reward)
    fail_persist(host.store._db)
    with pytest.raises(sqlite3.IntegrityError):
        host.store.weights(0, at(host.escrow, HOLD_AT - 1), 1)
    recover_persist(host.store._db)
    control(0 in host.store.ledger._engine.answers, "the failed ask journaled epoch 0")
    control(
        pays(host.escrow) == 0 and host.escrow.balances().available == ISSUED,
        "the failed persist rolled the payout back: the matured unit is still free",
    )
    # When: the caller retries epoch 0 at the maturity round.
    try:
        retry = parse(host.store.weights(0, at(host.escrow, HOLD_AT), 2))
    except ChallengeError as refused:
        # Refusal branch: a client error, and nothing was consumed.
        held(400 <= refused.status < 500, f"refused with {refused.status}")
        held(pays(host.escrow) == 0, "a refused retry paid")
        return
    # Then (answer branch): whatever the answer carries is what was paid.
    held(
        retry["metadata"]["epoch_at"] == at(host.escrow, HOLD_AT - 1),
        "the answer is dated at the epoch_at the journal holds",
    )
    held(
        retry["weights"] == {} and pays(host.escrow) == 0,
        f"the answer lists {retry['weights']} and {pays(host.escrow)} unit(s) were paid",
    )


# --- T-WGT-01 (WGT-06, WGT-17, WGT-20): the first answer is final whatever the escrow gains ------


def test_a_first_answer_stays_final_when_the_escrow_gains_a_payable_origin_and_after_a_restart(
    tmp_path: Path, li_escrow: ModuleType, real_reward: Real, finalized: Hosted
) -> None:
    host, second = finalized, "bb" * 32
    mature(host.escrow, host.reward)
    first = host.store.weights(0, at(host.escrow, HOLD_AT), 1)
    control(
        parse(first)["weights"] == {li_escrow.HOT.ss58: float(ISSUED)}, "epoch 0 paid the reward"
    )
    # Given: a state change a recompute would see, a second matured origin of another hotkey.
    add_origin(host.escrow, second, "second", 300_000, 10, li_escrow.COLD.ss58)
    # When: epoch 0 is asked again at a later and at an earlier chain time, with other clocks.
    for rnd, computed_at in ((25, 7), (1, 8)):
        # Then: the first bytes, and the new origin was not paid on this epoch's account.
        assert host.store.weights(0, at(host.escrow, rnd), computed_at) == first
        assert pays(host.escrow) == 1
    # And: the next epoch is the one that pays the new origin, and only it.
    nxt = host.store.weights(1, at(host.escrow, 25), 9)
    assert parse(nxt)["weights"] == {"second": 300_000.0}
    assert pays(host.escrow) == 2 and host.escrow.balances().paid == ISSUED + 300_000
    assert host.escrow.balances().conserved()
    assert host.store.weights(0, at(host.escrow, 26), 10) == first
    # And: a store opened again on the same state directory, with no v2 service at all, returns
    # both stored answers and pays nothing.
    rows = pay_rows(host.store._db)
    host.store.close()
    reopened = open_store(tmp_path, real_reward[0].manifest.training.beacon.genesis_time)
    built: list[str] = []

    def no_service(run_id: str) -> tuple:
        built.append(run_id)
        raise ChallengeError(503, "no v2 service is built for a replay")

    reopened._services = no_service
    try:
        assert reopened.weights(0, at(host.escrow, 40), 11) == first
        assert reopened.weights(1, at(host.escrow, 41), 12) == nxt
        assert built == [] and pay_rows(reopened._db) == rows == 2
    finally:
        reopened.close()


# --- T-WGT-03 (WGT-09, WGT-08, CL-24): an epoch_at beyond the present vests nothing early --------

TEN_YEARS = 10 * 365 * 24 * 3600

FROZEN_FAR_EPOCH_AT = pytest.mark.xfail(
    strict=True,
    raises=DefenceMissing,
    reason="WGT-09+CL-24: an epoch_at beyond the present is refused and changes nothing (it vests "
    "an unvested v1 entitlement early, and its watermark answers every later honest epoch "
    "out_of_order) [candidate] [fix-lot: none (no design yet)]",
)


@pytest.mark.parametrize("far", [False, pytest.param(True, marks=FROZEN_FAR_EPOCH_AT)])
def test_an_epoch_at_beyond_the_present_vests_no_entitlement_early_and_freezes_no_epoch(
    tmp_path: Path, far: bool
) -> None:
    with service_world(tmp_path) as world:
        world.network.now = 5000
        seed_v1_entitlement(world, "hk-v1")
        register_hotkeys(world, {"hk-v1"})
        params, now = world.store.params, now_of(world)
        epoch = params.epoch_of(now)
        control(epoch == 3, "the clock stands in epoch 3")
        before = watermarks(world)
        # When: one request asks the current epoch with an epoch_at ten years ahead (the control
        # asks nothing), then the honest requests follow.
        if far:
            request = world.get_weights(epoch, epoch_at=now + TEN_YEARS)
            refused = request.status_code == 422
            if refused:
                held(watermarks(world) == before, "a refused request changed the state")
            else:
                # A design that clamps instead must not pay what is not vested.
                held(
                    request.status_code == 200 and "hk-v1" not in request.json()["weights"],
                    f"status {request.status_code} paid {request.json().get('weights')}",
                )
        honest = world.get_weights(epoch)
        following = world.get_weights(epoch + 1)
        # Then: the entitlement is unvested for the epoch's own end, nothing is out of order.
        assert honest.status_code == 200 and following.status_code == 200
        answer = honest.json()
        held(answer["metadata"]["reason"] == "ok", f"reason {answer['metadata']['reason']}")
        assert answer["weights"] == {} and answer["metadata"]["units_burned_this_epoch"] == ISSUED
        if not far or refused:
            end = params.genesis_unix + (epoch + 1) * params.epoch_seconds
            assert answer["metadata"]["epoch_at"] == end
        held(following.json()["metadata"]["reason"] == "ok", "the next epoch was frozen")


# --- T-WGT-04 (WGT-09, WGT-08, CL-24): an epoch index beyond the present is refused --------------


FROZEN_FAR_EPOCH = pytest.mark.xfail(
    strict=True,
    raises=DefenceMissing,
    reason="WGT-09+CL-24: an epoch index beyond the present (a timestamp sent as the epoch, or a "
    "value whose default epoch_at overflows) is refused with 422 and changes nothing (today the "
    "first is answered and freezes every honest epoch, the second is an unmapped LedgerError, "
    "HTTP 500) [candidate] [fix-lot: none (no design yet)]",
)


@pytest.mark.parametrize(
    "case",
    [
        "no-faulty-request",
        pytest.param("a-timestamp-as-epoch", marks=FROZEN_FAR_EPOCH),
        pytest.param("an-epoch-whose-default-overflows", marks=FROZEN_FAR_EPOCH),
    ],
)
def test_an_epoch_index_beyond_the_present_is_refused_with_422_and_changes_nothing(
    tmp_path: Path, case: str
) -> None:
    with service_world(tmp_path) as world:
        world.network.now = 5000
        client = quiet_client(world.app)
        before = watermarks(world)
        # When: the faulty index is asked without an epoch_at (the control asks nothing), then
        # the current epoch.
        if case != "no-faulty-request":
            epoch = now_of(world) if case == "a-timestamp-as-epoch" else 2**62
            faulty = client.get(f"/internal/v1/get_weights?epoch={epoch}", headers=world.internal)
            # Then: refused and nothing written.
            held(faulty.status_code == 422, f"HTTP {faulty.status_code}")
            held(watermarks(world) == before, "a refused request changed the state")
        # And: the honest epoch is answered in order.
        honest = client.get("/internal/v1/get_weights?epoch=3", headers=world.internal)
        assert honest.status_code == 200
        held(honest.json()["metadata"]["reason"] == "ok", honest.json()["metadata"]["reason"])


# --- T-WGT-05, T-WGT-06 (WGT-16, CL-25): units are consumed only when a registered hotkey gets them

FROZEN_UNDELIVERED = pytest.mark.xfail(
    strict=True,
    raises=DefenceMissing,
    reason="WGT-16+CL-25: units that no registered hotkey received stay owed and pay in a later "
    "epoch (they are booked paid before the metagraph is asked; docs/findings/v2-weights-bridge.md "
    "documents the burn as a deliberate ponytail, so the decision is the maintainers') [candidate] "
    "[fix-lot: decision (product choice pending)]",
)


@FROZEN_UNDELIVERED
def test_a_matured_earner_missing_from_the_metagraph_keeps_its_units_for_a_later_epoch(
    tmp_path: Path, li_escrow: ModuleType
) -> None:
    with live_world(tmp_path, li_escrow, [(ORIGIN, "hk", ISSUED)], set()) as (
        world,
        master,
        escrow,
    ):
        now = now_of(world)
        # When: the earner is not listed by the master and epoch 0 is asked.
        first = world.get_weights(0, epoch_at=now)
        meta = first.json()["metadata"]
        control(
            first.status_code == 200 and first.json()["weights"] == {},
            "an unlisted hotkey is not in the answer",
        )
        control(
            (meta["units_paid"], meta["units_burned_this_epoch"]) == (0, ISSUED),
            "the answer burns the whole share",
        )
        # Then: no unit was consumed on its account.
        held(
            escrow.balances().paid == 0 and pays(escrow) == 0 and owed(escrow) == {"hk": ISSUED},
            f"paid {escrow.balances().paid}, {pays(escrow)} PAY event(s), owed {owed(escrow)}",
        )
        # And: once the master lists the hotkey, the next epoch pays the units, and epoch 0 stays.
        master_changes(world, master, registered={"hk"})
        later = world.get_weights(1, epoch_at=now + 1)
        held(
            later.status_code == 200 and later.json()["weights"] == {"hk": float(ISSUED)},
            f"epoch 1 paid {later.json()['weights']}",
        )
        held(pays(escrow) == 1, f"{pays(escrow)} PAY event(s) after the retry")
        assert world.get_weights(0, epoch_at=now + 2).content == first.content


@FROZEN_UNDELIVERED
def test_a_metagraph_failure_consumes_no_unit_and_the_retry_of_the_same_epoch_pays_once(
    tmp_path: Path, li_escrow: ModuleType
) -> None:
    with live_world(tmp_path, li_escrow, [(ORIGIN, "hk", ISSUED)], {"hk"}) as (
        world,
        master,
        escrow,
    ):
        now = now_of(world)
        master_changes(world, master, up=False)
        # When: epoch 0 is asked while the master cannot answer.
        down = world.get_weights(0, epoch_at=now)
        control(
            (down.status_code, down.json()["detail"])
            == (503, "the metagraph is unavailable, retry later"),
            "the route fails closed with the documented 503",
        )
        # Then: the failed request consumed nothing.
        held(
            escrow.balances().paid == 0 and pays(escrow) == 0 and owed(escrow) == {"hk": ISSUED},
            f"paid {escrow.balances().paid}, {pays(escrow)} PAY event(s), owed {owed(escrow)}",
        )
        # And: the retry of the same epoch pays the unit once, and the third ask is the same.
        master_changes(world, master, up=True)
        retry = world.get_weights(0, epoch_at=now)
        held(
            retry.status_code == 200 and retry.json()["weights"] == {"hk": float(ISSUED)},
            f"the retry answered {retry.status_code} {retry.json().get('weights')}",
        )
        assert pays(escrow) == 1
        assert world.get_weights(0, epoch_at=now).content == retry.content


def test_an_epoch_with_nothing_to_pay_is_answered_while_the_metagraph_is_down(
    tmp_path: Path, li_escrow: ModuleType
) -> None:
    """The route asks the master only for an answer that lists weight (WGT-16 condition kept)."""
    with live_world(tmp_path, li_escrow, [], set()) as (world, master, escrow):
        master_changes(world, master, up=False)
        # When: an epoch without any payable unit is asked while the metagraph is down.
        empty = world.get_weights(0, epoch_at=now_of(world))
        # Then: a defined full burn, 200, and the escrow is as it was.
        assert empty.status_code == 200 and empty.json()["weights"] == {}
        assert empty.json()["metadata"]["units_burned_this_epoch"] == FULL_SHARE_MASS
        assert pays(escrow) == 0
        # And: an epoch that does list weight fails closed under the same outage.
        add_origin(escrow, ORIGIN, "hk", 300_000, 1, li_escrow.COLD.ss58)
        weighty = world.get_weights(1, epoch_at=now_of(world) + 1)
        assert weighty.status_code == 503
