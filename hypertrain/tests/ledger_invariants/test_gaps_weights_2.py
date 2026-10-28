"""Gaps of the weights group that need a real audited round (D39, gap wave): what get_weights
pays in every phase of a round, after a refused Finalize, after a late rollback attempt, and for an
earner whose admission is suspended.

The world is the audit package's: five miners, one forger, three auditors, one real round restored
per test from the stage it needs (`support_audit.World`; the stages are built once per process).
Every test asks the store the way the route does, with the epoch_at the service clock gives.
"""

from __future__ import annotations

from typing import Any

import pytest

from hypertrain.aggregator.core import FinalityError
from hypertrain.ledger import FULL_SHARE_MASS
from hypertrain.ledger.escrow_v2 import EscrowV2

from . import support_audit as sa
from .support import at
from .support_audit import detail
from .support_gaps_weights import DefenceMissing, control, held
from .support_weights import parse, pay_events
from .test_audit import audit_world  # noqa: F401  (the session fixture of test_audit.py)
from .test_audit_2 import rollback_request

ISSUED = 1_000_000
FINAL = "ef" * 32


def run_of(n: Any) -> str:
    run_id: str = n.manifest.run_id()
    return run_id


def escrow_of(n: Any) -> EscrowV2:
    escrow: EscrowV2 = n.store._services(run_of(n))[0]
    return escrow


def ask(n: Any, epoch: int, epoch_at: int | None = None) -> bytes:
    """What the route asks the store: the service clock unless the test names a time."""
    return bytes(n.store.weights(epoch, int(n.store.clock()) if epoch_at is None else epoch_at, 1))


def reward_origins(escrow: EscrowV2) -> list[Any]:
    return list(
        escrow.db.execute(
            "SELECT origin,issued,mature_at,hotkey FROM escrow_origins "
            "WHERE reward_round IS NOT NULL ORDER BY hotkey"
        )
    )


def finalized_rows(escrow: EscrowV2) -> int:
    return int(escrow.db.execute("SELECT COUNT(*) FROM escrow_finalized").fetchone()[0])


def assert_full_burn(answer: dict[str, Any]) -> None:
    meta = answer["metadata"]
    assert answer["weights"] == {} and meta["reason"] == "ok"
    assert (meta["units_paid"], meta["units_burned_this_epoch"]) == (0, FULL_SHARE_MASS)


# --- T-WGT-02 (WGT-02, WGT-10, WGT-03): the phases of a round, from the first audit to payment ---


@pytest.mark.parametrize("stage", ["s2", "s4"])
def test_a_round_with_open_audits_or_no_finalize_burns_the_whole_share(
    world: sa.World, stage: str
) -> None:
    """s2: two audits executed, three queued. s4: every audit done and the round aggregated."""
    with world.at(stage) as (n, _):
        escrow = escrow_of(n)
        control(
            {"s2": "running", "s4": "completed"}[stage]
            in {row["state"] for row in sa.lease_rows(n.store).values()},
            "the round is in the phase the test names",
        )
        # When
        first = ask(n, 0)
        # Then: an explicit full burn, no live gate, no reward origin, nothing paid.
        assert_full_burn(parse(first))
        assert finalized_rows(escrow) == 0 and reward_origins(escrow) == []
        assert pay_events(escrow) == [] and escrow.balances().paid == 0
        # And: the answer is final.
        assert ask(n, 0, int(n.store.clock()) + 30) == first


def test_a_settled_round_pays_nothing_until_the_beacon_vests_it_and_then_each_audited_miner(
    world: sa.World,
) -> None:
    """s5: Finalize accepted, the reward issued reward_pending to the four audited miners."""
    with world.at("s5") as (n, cast):
        escrow = escrow_of(n)
        origins = reward_origins(escrow)
        mature_at = max(row["mature_at"] for row in origins)
        control(
            len(origins) == 4 and escrow.balances().reward_pending == ISSUED and n.now < mature_at,
            "four reward origins are pending and the beacon is before their maturity round",
        )
        # When: epoch 0 is asked at the very time the reward would be due, but before the beacon
        # vested it.
        first = ask(n, 0, at(escrow, mature_at))
        # Then: a full burn; the pending units were neither paid nor moved.
        assert_full_burn(parse(first))
        assert finalized_rows(escrow) == 1 and escrow.balances().reward_pending == ISSUED
        assert pay_events(escrow) == []
        # And: the beacon reaches the maturity round (the service vests the origins) and the next
        # epoch pays each audited miner its quarter, once.
        n.push(mature_at + 1)
        later = parse(ask(n, 1))
        honest = sorted(key.ss58 for key in cast.honest)
        assert later["weights"] == {hotkey: 250_000.0 for hotkey in honest}
        assert (later["metadata"]["units_paid"], later["metadata"]["units_burned_this_epoch"]) == (
            ISSUED,
            0,
        )
        assert sorted(event["units"] for event in pay_events(escrow)) == [250_000] * 4
        assert escrow.balances().paid == ISSUED and escrow.balances().reward_pending == 0
        # And: epoch 0 stays what it was answered, although its units pay now.
        assert ask(n, 0, int(n.store.clock()) + 30) == first


# --- M-08 (WGT-17): a stored answer survives a late rollback attempt ------------------------------


def test_a_late_rollback_of_a_settled_round_moves_no_stored_answer_and_pays_nothing(
    world: sa.World,
) -> None:
    with world.at("s5") as (n, cast):
        store, escrow = n.store, escrow_of(n)
        n.push(max(row["mature_at"] for row in reward_origins(escrow)) + 1)
        zero, one = ask(n, 0), ask(n, 1)
        control(len(pay_events(escrow)) == 4, "the epochs after the vesting paid the four miners")
        stored = store._db.execute(
            "SELECT epoch,answer FROM weights_runs ORDER BY epoch"
        ).fetchall()
        paid = (escrow.balances(), len(pay_events(escrow)))
        # When: the coordinator asks to roll the settled round back.
        refused = n.client.post(
            n.url + "/admin/rollback", json=rollback_request(n, cast), headers=sa.admin()
        )
        # Then: money-only, and the answers the master received are the same bytes with the same
        # payouts behind them.
        assert detail(refused) == (409, "finalized late fraud is money-only")
        assert ask(n, 0, int(n.store.clock()) + 30) == zero and ask(n, 1) == one
        assert (
            store._db.execute("SELECT epoch,answer FROM weights_runs ORDER BY epoch").fetchall()
            == stored
        )
        assert (escrow.balances(), len(pay_events(escrow))) == paid


# --- M-02 (FIN-02, WGT-02): a round whose Finalize was refused pays nothing ---------------------

REFUSED: list[Any] = [
    pytest.param({"unresolved_audits": 1}, "round cannot settle", id="an audit is open"),
    pytest.param({"unresolved_disputes": 1}, "round cannot settle", id="a dispute is open"),
    pytest.param(
        {"disposition": "EXCLUDED", "resolution_hash": FINAL},
        "round cannot settle",
        id="the round is excluded",
    ),
    pytest.param({"applied": False}, "round cannot settle", id="no tape was applied"),
    pytest.param(
        {"resolution_hash": FINAL}, "conflicting finality", id="another finalize was seen"
    ),
]


@pytest.mark.parametrize(("change", "message"), REFUSED)
def test_a_round_whose_finalize_was_refused_pays_nothing_whatever_the_epoch_at(
    world: sa.World, change: dict[str, Any], message: str
) -> None:
    with world.at("s4") as (n, cast):
        store, run, escrow = n.store, run_of(n), escrow_of(n)
        original = store._record_v2(run, "finality", "0")
        with store._tx():
            store._put_record_v2(run, "finality", "0", original | change)
        applied = store._record_v2(run, "applied", "0")
        request = sa.finalize_request(n, applied["theta_hash"], [k.ss58 for k in cast.honest])
        # Given: the coordinator's Finalize is refused by the store for this round.
        with pytest.raises(FinalityError, match=message):
            store.finalize_v2(run, 0, sa.canonical(request))
        # When: the epoch is asked at a time far past any maturity round, on purpose.
        far = int(store.clock()) + 10_000
        first = ask(n, 5, far)
        # Then: a full burn, no live gate and no reward unit exist, whatever the time asked.
        assert_full_burn(parse(first))
        assert finalized_rows(escrow) == 0 and reward_origins(escrow) == []
        assert pay_events(escrow) == [] and escrow.balances().paid == 0
        assert ask(n, 5, far + 500) == first
        # And (control): with the status corrected, the same Finalize settles and issues the
        # reward, so the burn above is the refusal's doing.
        with store._tx():
            store._put_record_v2(run, "finality", "0", original)
        response = n.client.post(
            n.url + "/admin/rounds/0/finalize", json=request, headers=sa.admin()
        )
        assert response.status_code == 200, response.text
        assert finalized_rows(escrow) == 1 and escrow.balances().reward_pending == ISSUED


# --- T-WGT-07 (WGT-05, CL-16): an earner suspended with a pending dispute is not paid -----------

FROZEN_SUSPENDED = pytest.mark.xfail(
    strict=True,
    raises=DefenceMissing,
    reason="WGT-05+CL-16: an earner whose admission is suspended with a pending dispute is not "
    "paid, and its matured units stay owed, neither paid nor burned (payable() reads no "
    "admission state). Decision needed: docs/network-v2.md (rewards mature after vesting and "
    "dispute conditions) and docs/mechanism.md (unvested reward is the collateral) promise it, "
    "while ADR 0011 decision 23 (audit-integrity-2) pays a suspended, unbanned member for the "
    "round it was verified in; if earned audited units are irrevocable, delete this test and "
    "correct those two passages [candidate] [fix-lot: decision (product choice pending)]",
)


@pytest.mark.parametrize("suspended", [False, pytest.param(True, marks=FROZEN_SUSPENDED)])
def test_an_earner_whose_admission_is_suspended_with_a_pending_dispute_is_not_paid(
    world: sa.World, suspended: bool
) -> None:
    with world.at("s5") as (n, cast):
        store, run, escrow = n.store, run_of(n), escrow_of(n)
        victim = cast.honest[2]
        if suspended:
            # The call the dispute route makes when a contest names the miner.
            store._services(run)[1].suspend(
                victim.ss58, reason="PENDING_DISPUTE", now=n.now, pending_dispute=True
            )
        n.push(max(row["mature_at"] for row in reward_origins(escrow)) + 1)
        state = store._db.execute(
            "SELECT state,pending_dispute FROM admissions_v2 WHERE hotkey=?", (victim.ss58,)
        ).fetchone()
        control(
            tuple(state) == (("SUSPENDED", 1) if suspended else ("ACTIVE", 0)),
            f"the victim's admission is {tuple(state)}",
        )
        # When: the first epoch after the vesting is asked.
        answer = parse(ask(n, 0))
        # Then: the control pays the four audited miners; the suspended one is not paid.
        everyone = {key.ss58: 250_000.0 for key in cast.honest}
        if not suspended:
            assert answer["weights"] == everyone
            return
        others = {hotkey: units for hotkey, units in everyone.items() if hotkey != victim.ss58}
        meta = answer["metadata"]
        held(
            victim.ss58 not in answer["weights"],
            f"the suspended earner was paid {answer['weights'].get(victim.ss58)}",
        )
        held(
            answer["weights"] == others
            and (meta["units_paid"], meta["units_burned_this_epoch"]) == (750_000, 250_000),
            f"paid {answer['weights']}, burned {meta['units_burned_this_epoch']}",
        )
        owner = escrow.balances(cast.cold[cast.hot.index(victim)].ss58)
        held(owner.paid == 0 and owner.burned == 0, f"the owner's units: {owner}")
