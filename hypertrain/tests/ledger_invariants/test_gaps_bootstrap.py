"""Gaps of the bootstrap group (D39): what the first wave of tests left open in T-BOOTSTRAP-02, 05,
07, 10, 12, 14, 15, 16, 18, 20 and 22.

Each test states a defence of the run economics: a client cannot stall the service's own steps,
the bootstrap is the owner-pinned allocation to the unit, a reward states exactly the round
budget, and a run that cannot hold its economic bound is never created. Holes are strict xfails
whose body asserts the defence; a control line proves the scenario reaches the point under test.
"""

from __future__ import annotations

import hashlib
import json
from collections.abc import Iterator
from pathlib import Path
from types import ModuleType
from typing import Any

import pytest
from pydantic import ValidationError

from hypertrain.challenge.trust_v2 import economic_bound
from hypertrain.protocol.envelope_v2 import seal
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages import Receipt
from hypertrain.protocol.messages_v2 import EconomicsPolicyV2

from .support import (
    SERVICE_FIXTURE,
    H,
    World,
    error_code,
    load_fixture_module,
    memory_db,
    service_world,
    table_names,
)
from .support_bounds import (
    BASE_POLICY,
    PPM,
    U64_MAX,
    capacity_run,
    creation,
    econ,
    genesis_request,
    ledger_for,
    nothing_issued,
    origin_id_of,
    pinned_bootstrap,
    pinned_production,
    production_run,
    split,
    synthetic_round,
    worker,
)
from .support_credit import credited
from .support_gaps_bootstrap import (
    SmallAuditedRound,
    build_small_audited_round,
    challenge_refusal,
    escrow_of,
    install_production_qualification,
    maturity_id_of,
    open_overdue_trial,
    production_economics,
    push_round,
    refusal_of,
    seed_pending_origin,
    trial_outcome,
    world_economics,
)
from .support_locks import Rig
from .support_opids import (
    client_event_over_http,
    fresh_id,
    sender_over_http,
    squat_id,
    squat_stored_keys,
)

# --- T-BOOTSTRAP-02: a squatted maturity id never stalls the beacon route ------------------------


def derived_maturity_id_of(world: World, origin: str) -> str:
    """The id the service derived for the MATURE event of `origin`, as its receipt states it: the
    id before the ledger places the event in its own key space. It is a plain 64-hex id, so any
    client can sign it, where the key the event is stored under (`maturity_id_of`) may not be."""
    request = '["' + origin + '"]'
    db = escrow_of(world).db
    with world.store._lock:
        rows = db.execute(
            "SELECT receipt FROM escrow_events WHERE kind='MATURE' AND request=?", (request,)
        ).fetchall()
    assert len(rows) == 1, f"no single MATURE event for {origin}: {len(rows)}"
    return str(json.loads(rows[0][0])["operation_id"])


@pytest.mark.parametrize("kind", ["transfer", "lock"])
def test_a_squatted_maturity_id_never_blocks_another_owners_origin_or_the_next_push(
    tmp_path: Path, kind: str
) -> None:
    cold = load_fixture_module(*SERVICE_FIXTURE).COLD
    squatted, other = fresh_id("squatted-origin"), fresh_id("other-origin")

    # Given: the control run matures both origins at round 5 and shows the id of the first one.
    with service_world(tmp_path / "control") as world:
        world.network.push(2)
        seed_pending_origin(world, squatted, cold[0].ss58, 500, mature_at=5)
        seed_pending_origin(world, other, cold[2].ss58, 700, mature_at=5)
        open_overdue_trial(world)
        before = escrow_of(world).balances(cold[2].ss58)
        before_first = escrow_of(world).balances(cold[0].ss58)
        assert trial_outcome(world) == "OPEN"
        world.network.push(5)
        assert trial_outcome(world) == "EXPIRED"
        stored_key = maturity_id_of(world, squatted)
        assert maturity_id_of(world, other) != stored_key
        named_id = derived_maturity_id_of(world, squatted)
        matured = escrow_of(world).balances(cold[2].ss58)
        first = escrow_of(world).balances(cold[0].ss58)
    assert (matured.available, matured.reward_pending) == (before.available + 700, 0)
    assert (first.available, first.reward_pending) == (before_first.available + 500, 0)

    # When: a third funded coldkey signs an event under the id of the first origin's maturity: the
    # id the service named for it and, where a client can name it, the key it is stored under.
    with service_world(tmp_path / "contended") as world:
        world.network.push(2)
        seed_pending_origin(world, squatted, cold[0].ss58, 500, mature_at=5)
        seed_pending_origin(world, other, cold[2].ss58, 700, mature_at=5)
        open_overdue_trial(world)
        origin = world.network.origin_ids[1]
        neutral = client_event_over_http(world, kind, cold[1], fresh_id("neutral"), origin)
        assert neutral.status_code == 200, neutral.text  # control: the route takes its events
        # Control: the ledger decided on each squat, by storing the event or refusing it with its
        # own code. An id the event model refuses never reached it, and the push below would then
        # prove nothing about the squat.
        escrow = escrow_of(world)
        send = sender_over_http(world, kind, cold[1], origin)
        squat_id(escrow, kind, named_id, send)
        squat_stored_keys(escrow, kind, [stored_key], send)

        # Then: the beacon route answers at round 5 and after it, the other owner's origin vests
        # as in the control, the overdue trial is closed by the same push, and no unit is created
        # or lost. The squatted origin vests as well: the squat does not occupy the id the
        # service vests it under, and a push that only isolated the conflict would leave it
        # pending.
        push_round(world, 5)
        push_round(world, 6)
        assert trial_outcome(world) == "EXPIRED"
        vested = escrow.balances(cold[2].ss58)
        assert (vested.available, vested.reward_pending) == (before.available + 700, 0)
        assert escrow.balances(cold[0].ss58) == first
        assert escrow.balances().conserved()


# --- T-BOOTSTRAP-05: the origin ids are bound to their index, not to the set of valid ids --------


def test_a_bootstrap_with_the_pinned_origin_ids_in_the_wrong_order_is_refused_and_issues_nothing(
    li_escrow: ModuleType,
) -> None:
    _, escrow, allocation = pinned_production(li_escrow)
    run, pin = escrow.run_id, escrow.policy.bootstrap_allocation_hash
    assert pin is not None
    # Each id is valid for this run and this pin, but the 20,000 units sit under the id of the
    # second origin and the 10,000 units under the first one.
    swapped = pinned_bootstrap(
        li_escrow,
        run,
        allocation,
        origin_ids=[
            origin_id_of(escrow, "bootstrap", run, pin, 1),
            origin_id_of(escrow, "bootstrap", run, pin, 0),
        ],
    )
    assert [o.units for o in swapped.origins] == [20_000, 10_000]
    assert error_code(lambda: escrow.production_bootstrap(swapped)) == "BOOTSTRAP_ORIGIN_DOMAIN"
    assert nothing_issued(escrow)
    # Control: the same list under its own ids is issued, so the order of the ids was the fault.
    escrow.production_bootstrap(pinned_bootstrap(li_escrow, run, allocation))
    assert escrow.balances().issued == 30_000


def test_a_test_genesis_with_the_pinned_origin_ids_in_the_wrong_order_is_refused(
    li_escrow: ModuleType,
) -> None:
    escrow = li_escrow.ledger(li_escrow.setup("test"), memory_db())
    run = escrow.run_id
    pin = escrow.policy.genesis_allocation_hash
    assert pin is not None
    allocation = [(li_escrow.COLD.ss58, 20000, 1), (li_escrow.OTHER.ss58, 10000, 1)]
    swapped = genesis_request(
        li_escrow,
        run,
        allocation,
        origin_ids={
            0: origin_id_of(escrow, "genesis", run, pin, 1),
            1: origin_id_of(escrow, "genesis", run, pin, 0),
        },
    )
    assert swapped.allocation_hash == pin
    assert error_code(lambda: escrow.test_genesis(swapped)) == "GENESIS_ORIGIN_DOMAIN"
    assert nothing_issued(escrow)
    escrow.test_genesis(genesis_request(li_escrow, run, allocation))
    assert escrow.balances().issued == 30_000


# --- T-BOOTSTRAP-07: a body consistent with its own budget is still not the round budget ---------


@pytest.mark.parametrize("off", [-1, 1])
def test_a_reward_consistent_with_its_own_budget_one_unit_off_the_round_budget_is_refused(
    li_escrow: ModuleType, off: int
) -> None:
    setup = li_escrow.setup("production")
    round_reward = setup.policy.round_reward_units
    team = [worker(0), worker(1)]
    hotkeys = [hot.ss58 for hot, _ in team]
    budget = round_reward + off
    # The origin units, the allocation hash and the signature are made for the stated budget, so
    # that no other check of the reward (its allocation least of all) can be what refuses it.
    consistent = synthetic_round(
        li_escrow, setup, 0, team, budget=budget, units=split(budget, hotkeys)
    )
    honest = synthetic_round(li_escrow, setup, 0, team)
    assert sum(consistent.units.values()) == consistent.reward.budget_units == budget
    assert consistent.reward.allocation_hash != honest.reward.allocation_hash
    assert budget < setup.policy.max_total_issuance  # the issuance cap is not what refuses it
    escrow = ledger_for(li_escrow, setup, [consistent, honest])

    assert error_code(lambda: escrow.reward_finalize(consistent.reward, consistent.evidence)) == (
        "ROUND_BUDGET"
    )
    assert nothing_issued(escrow)
    assert escrow.db.execute("SELECT COUNT(*) FROM escrow_finalized").fetchone()[0] == 0
    # Control: the round is still finalizable with its own budget, and issues exactly that.
    escrow.reward_finalize(honest.reward, honest.evidence)
    assert escrow.balances().issued == round_reward
    assert escrow.balances().conserved()


# --- T-BOOTSTRAP-15: the bound is exact where a floating point evaluation is not -----------------


def exact_bound(policy: EconomicsPolicyV2) -> bool:
    """The inequality written out in big integers, apart from the function under test."""
    return (
        PPM
        * policy.s_lower_ppm
        * policy.beta_ppm
        * (policy.R_collectible_units + policy.S_min_units)
        >= policy.G_max_units * 10**18
    )


# (s_lower_ppm, beta_ppm, R, S, G that holds, G one unit larger). The deficit of the larger goal
# is a fraction of a unit, which a 53-bit mantissa rounds away at these magnitudes.
FLOAT_WITNESSES = {
    # The exact quotient is 1766602602801650.87: a double rounds it up to the larger goal.
    "a deficit of 0.13 units": (
        779_246,
        348_857,
        0,
        6_498_555_653_287_381,
        1_766_602_602_801_650,
        1_766_602_602_801_651,
    ),
    # The exact quotient is 6802879701896397.6: a double rounds it up to the larger goal.
    "a deficit of 0.4 units at the top of the exact range": (
        400_000,
        PPM,
        U64_MAX,
        8_000_000_000_000_003,
        6_802_879_701_896_397,
        6_802_879_701_896_398,
    ),
}


@pytest.mark.parametrize("witness", sorted(FLOAT_WITNESSES))
def test_the_run_bound_is_decided_to_the_unit_where_a_float_evaluation_is_not(
    witness: str,
) -> None:
    s, beta, reward, stake, holds, fails = FLOAT_WITNESSES[witness]
    shape = {
        "s_lower_ppm": s,
        "beta_ppm": beta,
        "R_collectible_units": reward,
        "S_min_units": stake,
    }
    fits, too_big = econ(G_max_units=holds, **shape), econ(G_max_units=fails, **shape)
    # The witnesses are chosen so that exact arithmetic is below the larger goal by a fraction of
    # a unit and a float evaluation of the same quotient is not.
    assert exact_bound(fits) and not exact_bound(too_big)
    assert float(fits.q_floor * s * beta * (reward + stake)) / 1e18 >= fails
    assert economic_bound(fits) is True
    assert economic_bound(too_big) is False


# --- T-BOOTSTRAP-20: a policy fault is refused where it is, not by a neighbouring rule ----------

MODE_RULE_GENESIS = "only test mode requires pinned genesis allocation"
MODE_RULE_BOOTSTRAP = "only production mode pins a bootstrap allocation"

# fault -> (fields of the base policy that change, location of the single error it raises)
FIELD_FAULTS: dict[str, tuple[dict[str, object], tuple[str, ...]]] = {
    "an unknown field": ({"surprise": 1}, ("surprise",)),
    "q_floor below certainty": ({"q_floor": 999_999}, ("q_floor",)),
    "a nonzero gammaV": ({"gammaV_units": 1}, ("gammaV_units",)),
    "eleven shadow rounds": ({"shadow_bootstrap_rounds": 11}, ("shadow_bootstrap_rounds",)),
    "a negative stake": ({"S_min_units": -1}, ("S_min_units",)),
    "a zero beta": ({"beta_ppm": 0}, ("beta_ppm",)),
    "s above one in ppm": ({"s_lower_ppm": PPM + 1}, ("s_lower_ppm",)),
    "an unknown ledger mode": ({"ledger_mode": "staging"}, ("ledger_mode",)),
    "a boolean in an unsigned field": ({"G_max_units": True}, ("G_max_units",)),
    "a boolean in a positive field": ({"beta_ppm": True}, ("beta_ppm",)),
    "a boolean in a ppm field": ({"s_lower_ppm": True}, ("s_lower_ppm",)),
    "a float in a positive field": ({"beta_ppm": 1_000_000.0}, ("beta_ppm",)),
    "a float in a ppm field": ({"s_lower_ppm": 1_000_000.0}, ("s_lower_ppm",)),
    "a string in an integer field": ({"G_max_units": "100"}, ("G_max_units",)),
    **{
        f"{field} at 2^53": ({field: 2**53}, (field,))
        for field in (
            "G_max_units",
            "R_collectible_units",
            "S_min_units",
            "beta_ppm",
            "round_reward_units",
            "max_total_issuance",
        )
    },
}


def errors_of(**changes: object) -> list[dict[str, Any]]:
    try:
        econ(**changes)
    except ValidationError as error:
        return [dict(e) for e in error.errors()]
    raise AssertionError(f"the policy was accepted: {changes}")


@pytest.mark.parametrize("fault", sorted(FIELD_FAULTS))
def test_a_policy_field_fault_raises_one_error_located_at_that_field(fault: str) -> None:
    assert econ().ledger_mode == "test"  # control: the unchanged body is accepted
    changes, location = FIELD_FAULTS[fault]
    assert [e["loc"] for e in errors_of(**changes)] == [location]


@pytest.mark.parametrize(
    ("changes", "message"),
    [
        pytest.param({"genesis_allocation_hash": None}, MODE_RULE_GENESIS, id="test-no-genesis"),
        pytest.param({"ledger_mode": "production"}, MODE_RULE_GENESIS, id="production-genesis"),
        pytest.param({"bootstrap_allocation_hash": H}, MODE_RULE_BOOTSTRAP, id="test-bootstrap"),
    ],
)
def test_a_ledger_mode_fault_is_a_model_error_naming_the_rule_it_breaks(
    changes: dict[str, object], message: str
) -> None:
    (error,) = errors_of(**changes)
    assert error["loc"] == () and message in error["msg"]


@pytest.mark.parametrize("field", sorted(BASE_POLICY))
def test_a_missing_policy_field_raises_one_error_located_at_that_field(field: str) -> None:
    body = {name: value for name, value in BASE_POLICY.items() if name != field}
    with pytest.raises(ValidationError) as refused:
        EconomicsPolicyV2.model_validate(body)
    assert [(e["loc"], e["type"]) for e in refused.value.errors()] == [((field,), "missing")]


@pytest.mark.parametrize(
    "field",
    [
        "G_max_units",
        "R_collectible_units",
        "S_min_units",
        "beta_ppm",
        "round_reward_units",
        "max_total_issuance",
    ],
)
def test_a_policy_amount_may_reach_two_to_the_53_minus_one_and_not_one_unit_more(
    field: str,
) -> None:
    # 1024 * (2^53 - 1) < 2^63 - 1: the ceiling is what keeps a SQLite SUM of the field exact.
    assert econ(**{field: U64_MAX}).body()[field] == U64_MAX
    assert [e["loc"] for e in errors_of(**{field: U64_MAX + 1})] == [(field,)]


# --- T-BOOTSTRAP-10: a production policy that cannot hold its bound is refused when the run is
# created -----------------------------------------------------------------------------------------


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="BND-02+T-BOOTSTRAP-10+CL-23: a production run whose policy fails q*s*beta*(R+S)>=G "
    "is created, burns its run id and is refused first at the economic-bound route, possibly "
    "after units were issued [fix-lot: none (no design yet)]",
)
def test_a_production_policy_that_fails_the_economic_bound_creates_no_run(tmp_path: Path) -> None:
    shape = {"R_collectible_units": 100, "S_min_units": 1000, "s_lower_ppm": PPM, "beta_ppm": PPM}
    with service_world(tmp_path / "world") as world:
        # Control: the capacity is 100 + 1000 = 1100 units, and a goal of exactly 1100 creates once
        # the owner-signed qualification a production run stands on is stored, so a refusal of the
        # larger goal below cannot be about the qualification.
        holds = production_economics(world, G_max_units=1100, **shape)
        assert economic_bound(holds) and not economic_bound(
            production_economics(world, G_max_units=1101, **shape)
        )
        with creation(world, tmp_path / "holds", holds) as ok:
            install_production_qualification(ok)
            assert ok.store.create_run_v2(ok.raw)["status"] == "created"
            assert ok.run_rows() == 1
        # One more unit of goal cannot be collateralised: no run, and the bound is the reason.
        fails = production_economics(world, G_max_units=1101, **shape)
        with creation(world, tmp_path / "fails", fails) as bad:
            install_production_qualification(bad)
            refusal = challenge_refusal(lambda: bad.store.create_run_v2(bad.raw))
            assert refusal is not None, "the run was created"
            assert (refusal.status, "economic bound" in refusal.detail) == (409, True)
            assert bad.run_rows() == 0


# --- T-BOOTSTRAP-16: only the record economic_bound_v2 wrote opens production influence ----------

FORGERIES = [
    "a record without a receipt",
    "a receipt signed by another key",
    "a receipt of another run",
    "a receipt over other evidence",
]


def forged_bound_record(run: Any, forgery: str) -> dict[str, Any]:
    """A row of kind 'economic-bound' that carries the policy hash of the run, as an operator
    with write access to the database could place, but that `economic_bound_v2` did not write."""
    keys = run.eco.E
    evidence = {
        "run_id": run.run_id,
        "economics_policy_hash": run.store._run_v2(run.run_id).network.economics_policy_hash,
        "bound": "q*s*beta*(R+S)>=G",
    }
    if forgery == "a record without a receipt":
        return evidence
    signer, run_id, commit = keys.COORD, run.run_id, sha256_hex(canonicalize(evidence))
    if forgery == "a receipt signed by another key":
        signer = keys.OTHER
    elif forgery == "a receipt of another run":
        run_id = "ab" * 32
    else:
        commit = sha256_hex(b"other evidence")
    receipt = Receipt(w=0, commit_hash=commit, received_round=1)
    return {**evidence, "receipt": seal(signer, "Receipt", run_id, receipt, 10_000)}


@pytest.mark.parametrize("forgery", FORGERIES)
@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="BND-03+T-BOOTSTRAP-16+BND-04: the closure behind production eligibility accepts any "
    "row of kind economic-bound, whoever wrote it; defence in depth, a row needs database write "
    "access [fix-lot: none (no design yet)]",
)
def test_a_stored_bound_row_that_economic_bound_v2_did_not_write_opens_no_influence(
    tmp_path: Path, forgery: str
) -> None:
    with production_run(tmp_path, G_max_units=1100) as run:
        run.bootstrap()
        run.admit_and_lock(1100)
        assert run.status().eligible is False  # control: funded at the floor, no record yet
        with run.store._tx():
            run.store._put_record_v2(
                run.run_id, "economic-bound", "run", forged_bound_record(run, forgery)
            )
        assert run.status().eligible is False
        # Only the record the coordinator signs for this run and this policy opens it.
        with run.store._tx():
            run.store._db.execute("DELETE FROM records_v2 WHERE kind='economic-bound'")
        run.store.economic_bound_v2(run.run_id)
        assert run.status().eligible is True


# --- T-BOOTSTRAP-18: a refused run creation is not a created run ---------------------------------

CREATION_REFUSALS = {
    "a production policy without a CUDA qualification": (
        ("ChallengeError", "409 production CUDA qualification required")
    ),
    "a round reward that is not the epochs budget": ("EscrowError", "MANIFEST_REWARD_BUDGET"),
}


@pytest.mark.parametrize("case", sorted(CREATION_REFUSALS))
@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="BND-25+T-BOOTSTRAP-18+CL-40: the run row is committed before its services are built, "
    "so a refused creation leaves a run that the same envelope can no longer create "
    "[fix-lot: none (no design yet)]",
)
def test_a_refused_run_creation_leaves_no_run_row_and_is_refused_the_same_way_again(
    tmp_path: Path, case: str
) -> None:
    with service_world(tmp_path / "world") as world:
        if case.startswith("a production"):
            policy = production_economics(world)
        else:
            # A test-mode policy whose round reward is one unit short of the manifest's budget.
            policy = world_economics(world, round_reward_units=999_999)
        with creation(world, tmp_path / "refused", policy) as run:
            expected = CREATION_REFUSALS[case]
            assert refusal_of(lambda: run.store.create_run_v2(run.raw)) == expected
            assert run.store._services_v2 == {}
            assert run.run_rows() == 0
            # The same envelope is refused for the same reason, not as a duplicate of a run
            # that was never created.
            assert refusal_of(lambda: run.store.create_run_v2(run.raw)) == expected


# --- T-BOOTSTRAP-22: a capacity profile refused for a round that exists burns nothing -----------


def store_digest(store: Any) -> str:
    """Hash of every row of every table of the store: a refusal must leave it as it was."""
    hasher = hashlib.sha256()
    for name in table_names(store._db):
        rows = sorted(repr(tuple(row)) for row in store._db.execute(f'SELECT * FROM "{name}"'))
        hasher.update(repr((name, rows)).encode())
    return hasher.hexdigest()


def test_a_service_profile_refused_after_a_round_exists_keeps_no_nonce_and_stays_admissible(
    tmp_path: Path,
) -> None:
    with capacity_run(tmp_path, reserve=False) as (capacity, store, manifest):
        run_id = manifest.run_id()
        body, raw = capacity.signed_profile(store, manifest)

        def reservations() -> int:
            return int(store._db.execute("SELECT COUNT(*) FROM accepted_v2").fetchone()[0])

        with store._tx():
            store._put_record_v2(run_id, "round", "0", {"opened": "before the profile"})
        before = store_digest(store)
        assert reservations() == 0

        refusal = challenge_refusal(lambda: store.bootstrap_service_capacity_v2(run_id, body, raw))
        assert refusal is not None, "the profile was admitted after a round was opened"
        assert (refusal.status, refusal.detail) == (409, "SERVICE_PROFILE_BEFORE_OPEN")
        # The owner's receipt is signed once: its nonce is not spent by a refusal, and no row at
        # all was written.
        assert reservations() == 0
        assert store_digest(store) == before

        # Control: with no round the very same signed profile is admitted, and that reserves
        # the nonce this test counts.
        with store._tx():
            store._db.execute("DELETE FROM records_v2 WHERE kind='round'")
        assert store.bootstrap_service_capacity_v2(run_id, body, raw) == sha256_hex(body)
        assert reservations() == 1


# --- T-BOOTSTRAP-12: a roster is only as good as the economic bound behind its funding ----------

NOT_FUNDED = "roster lacks current funded admission authority"


@pytest.fixture
def rig(tmp_path: Path) -> Iterator[Rig]:
    with service_world(tmp_path) as world:
        yield Rig(world)


def test_a_round_does_not_open_while_the_economic_bound_closure_is_false(rig: Rig) -> None:
    # Given: four ACTIVE miners holding their full stake, and an opening that lists them all
    miners = [rig.admit(index) for index in range(4)]
    opening = rig.opening(miners)
    digest = rig.world.db_digest()
    assert all(rig.status(miner).eligible for miner in miners)
    admission = rig.admission
    closure = admission.conservative_bound
    # When: the run's economic bound is not recorded, whatever the stake
    admission.conservative_bound = lambda: False
    try:
        assert not any(rig.status(miner).eligible for miner in miners)
        refused = rig.open_round(opening)
        assert (refused.status_code, refused.json()["detail"]) == (403, NOT_FUNDED)
        # Then: nothing was written, and the very same opening is accepted once it is
        assert rig.world.db_digest() == digest
    finally:
        admission.conservative_bound = closure
    opened = rig.open_round(opening)
    assert opened.status_code == 200, opened.text


# --- T-BOOTSTRAP-14: the split the store builds is the one the ledger recomputes -----------------


@pytest.fixture(scope="module")
def audited_six(tmp_path_factory: pytest.TempPathFactory) -> Iterator[SmallAuditedRound]:
    yield from build_small_audited_round(tmp_path_factory, 6)


@pytest.mark.slow
def test_a_round_of_six_audited_workers_is_finalized_with_the_remainder_to_the_first_hotkeys(
    audited_six: SmallAuditedRound,
) -> None:
    """1,000,000 units between six workers who audited the same sample count: the store builds
    the reward body and the ledger recomputes it, so the two must agree on who holds the four
    extra units. Four workers divide the budget evenly and cannot tell the two apart."""
    round_ = audited_six
    round_.restore()
    escrow = round_.store._services(round_.run_id)[0]
    budget = escrow.policy.round_reward_units
    base, remainder = divmod(budget, 6)
    assert (base, remainder) == (166_666, 4)  # control: the budget does not divide evenly
    finalized = round_.finalize()
    assert finalized.status_code == 200, finalized.text
    owners = {
        key.ss58: cold.ss58
        for key, cold in zip(round_.fixtures.HOT, round_.fixtures.COLD, strict=True)
    }
    hotkeys = sorted(owners, key=lambda h: h.encode())
    assert credited(round_) == [
        (hotkey, base + (1 if rank < remainder else 0), 0, owners[hotkey])
        for rank, hotkey in enumerate(hotkeys)
    ]
    assert sum(units for _, units, _, _ in credited(round_)) == budget
    assert escrow.balances().conserved()


# --- T-BOOTSTRAP-19 (decision gated): what a write-once dataset linkage must still allow ---------


def test_the_dataset_linkage_may_be_repinned_before_the_first_round_and_confirmed_after_it(
    tmp_path: Path,
) -> None:
    """The decision is whether a different pair may replace the linkage once a round exists (the
    strict xfail of test_bounds_2.py). Whatever the owner decides, the operator tool must still be
    able to pin before the run opens and to confirm the pair afterwards, so a fix that forbids all
    re-pinning is a regression of the pin tool."""
    with service_world(tmp_path) as world:
        route = f"/v2/runs/{world.run_id}/admin/dataset"
        original = world.store._record_v2(world.run_id, "dataset", "inputs")
        other = {
            "samples_hash": world.store.objects.put(b"other samples"),
            "proofs_hash": world.store.objects.put(b"other proofs"),
        }
        assert other != original  # control: the second pair is a different linkage
        # Before any round: a different pair replaces the linkage, and the original comes back.
        assert world.client.put(route, json=other, headers=world.admin).status_code == 200
        assert world.store._record_v2(world.run_id, "dataset", "inputs") == other
        assert world.client.put(route, json=original, headers=world.admin).status_code == 200
        assert world.store._record_v2(world.run_id, "dataset", "inputs") == original
        # After a round exists: the pair already pinned is confirmed, whatever else is refused.
        with world.store._tx():
            world.store._put_record_v2(world.run_id, "round", "0", {"opened": "before the change"})
        confirmed = world.client.put(route, json=original, headers=world.admin)
        assert confirmed.status_code == 200, confirmed.text
        assert world.store._record_v2(world.run_id, "dataset", "inputs") == original
