"""Economic bound, stake floor and ledger (BND-01 .. BND-20): policy, audit floor and ledger.

Each test states a defence the economics of a run must hold: the bound is exact integer
arithmetic, the stake floor is rounded up and identical in its three implementations, every
roster entry is audited with probability one, a production bootstrap is the owner-pinned
allocation issued once, a run keeps one policy and one ledger mode for life, total issuance
stays under the pinned cap, and a round issues exactly its budget. The store and HTTP half of
the package is in test_bounds_2.py.
"""

from __future__ import annotations

import hashlib
import json
from dataclasses import replace
from functools import partial
from types import ModuleType, SimpleNamespace

import pytest
from hypothesis import given
from hypothesis import strategies as st

from hypertrain.challenge import trust_v2
from hypertrain.challenge.trust_v2 import (
    FundedStatus,
    TrustError,
    audit_floor,
    economic_bound,
    select_audit,
)
from hypertrain.data.store import LocalFSStore
from hypertrain.ledger.escrow_v2 import genesis_allocation_hash
from hypertrain.miner.coldkey_ops import required_units
from hypertrain.protocol.example import example_manifest
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import f32hex
from hypertrain.protocol.messages_v2 import (
    EconomicsPolicyV2,
    ProductionBootstrap,
    RosterEntryV2,
    RunManifestV2,
)

from .support import H, error_code, memory_db, table_names
from .support_bounds import (
    BASE_POLICY,
    PPM,
    U64_MAX,
    Floors,
    K,
    econ,
    genesis_request,
    ledger_for,
    nothing_issued,
    origin_id_of,
    pinned_bootstrap,
    pinned_production,
    refuses,
    repinned,
    split,
    synthetic_round,
    worker,
)

# BND-01: the run-level bound q*s*beta*(R+S) >= G is exact integer arithmetic.

S_FACTOR = st.one_of(st.just(PPM), st.integers(0, PPM))
BETA_FACTOR = st.one_of(st.just(PPM), st.integers(1, PPM))
UNITS = st.integers(0, 10**9)


@given(s=S_FACTOR, beta=BETA_FACTOR, reward=UNITS, stake=UNITS)
def test_the_run_bound_holds_up_to_its_capacity_and_not_one_unit_beyond(
    s: int, beta: int, reward: int, stake: int
) -> None:
    # Capacity in units: q*s*beta*(R+S) / 10^18 with q the pinned 10^6 ppm floor. Equality passes.
    ceiling = PPM * s * beta * (reward + stake) // 10**18
    shape = {
        "s_lower_ppm": s,
        "beta_ppm": beta,
        "R_collectible_units": reward,
        "S_min_units": stake,
    }
    assert economic_bound(econ(G_max_units=ceiling, **shape))
    assert not economic_bound(econ(G_max_units=ceiling + 1, **shape))
    # Monotone: a bound that holds still holds with one more unit of any factor, and a goal one
    # unit smaller.
    for factor, limit in (
        ("s_lower_ppm", PPM),
        ("beta_ppm", PPM),
        ("R_collectible_units", U64_MAX),
        ("S_min_units", U64_MAX),
    ):
        if shape[factor] < limit:
            assert economic_bound(econ(G_max_units=ceiling, **{**shape, factor: shape[factor] + 1}))
    if ceiling:
        assert economic_bound(econ(G_max_units=ceiling - 1, **shape))


def test_the_run_bound_edges_zero_detection_zero_goal_and_every_field_at_its_maximum() -> None:
    # The shipped numbers: 10^6 x 10^6 x 10^6 x (100 + 1000) / 10^18 = 1100 exactly.
    assert economic_bound(econ(G_max_units=1100))
    assert not economic_bound(econ(G_max_units=1101))
    # No detection (s = 0) satisfies only a zero goal; admission refuses s = 0 on its own.
    assert economic_bound(econ(G_max_units=0, s_lower_ppm=0))
    assert not economic_bound(econ(G_max_units=1, s_lower_ppm=0))
    # The stake, reward, goal and cap at 2^53-1 stay exact big-integer arithmetic: no rounding.
    top = {
        "G_max_units": U64_MAX,
        "R_collectible_units": U64_MAX,
        "S_min_units": U64_MAX,
        "beta_ppm": PPM,
        "max_total_issuance": U64_MAX,
    }
    assert economic_bound(econ(**top))
    assert not economic_bound(econ(**{**top, "beta_ppm": 1, "s_lower_ppm": 1}))


# BND-02 (candidate defect BND-D04): nothing ties the factors of the bound to a sane range.


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="BND-02+BND-D04: no factor of the bound may exceed the fraction one (beta_ppm <= 10^6) "
    "so a single policy field cannot make the inequality vacuous [candidate] "
    "[fix-lot: decision (product choice pending)]",
)
def test_beta_cannot_exceed_one_so_one_field_cannot_make_the_bound_vacuous() -> None:
    # Control: with every fraction at one, a goal a billion times the stake is out of reach.
    modest = {"G_max_units": 10**9, "S_min_units": 1, "R_collectible_units": 0}
    assert not economic_bound(econ(**modest))
    # Raising beta alone, 9e9 times, makes the same goal pass: the policy must refuse that.
    assert refuses(lambda: econ(**modest, beta_ppm=U64_MAX))


# BND-06: the per-miner stake floor is rounded up and identical in all three implementations.


@pytest.fixture(scope="module")
def objects(tmp_path_factory: pytest.TempPathFactory) -> LocalFSStore:
    return LocalFSStore(tmp_path_factory.mktemp("bounds-objects"))


@given(
    goal=st.integers(0, 1000),
    s=st.integers(200_000, PPM),
    beta=st.integers(200_000, PPM),
    stake=st.integers(0, 5000),
    reward=st.integers(0, 500),
    probe=st.sampled_from([-1, 0, 1]),
)
def test_the_stake_floor_is_rounded_up_and_the_same_in_status_gate_and_operator_tool(
    li_escrow: ModuleType,
    objects: LocalFSStore,
    goal: int,
    s: int,
    beta: int,
    stake: int,
    reward: int,
    probe: int,
) -> None:
    floors = Floors(
        li_escrow,
        objects,
        G_max_units=goal,
        s_lower_ppm=s,
        beta_ppm=beta,
        S_min_units=stake,
        R_collectible_units=reward,
    )
    multiple = floors.setup.manifest.training.verify.s_min_reward_multiple
    # The documented floor: max(S_min, multiple * R, ceil(G * 10^18 / (s * beta * 10^6))).
    required = max(stake, multiple * reward, -(-goal * 10**18 // (s * beta * PPM)))
    locked = required + probe
    if not 0 <= locked <= 20_000:  # the fixture's genesis funds 20,000 units for this coldkey
        return
    floors.lock_to(locked)
    eligible, funding = floors.status()
    allowed, gate_requirement = floors.tape_gate(funding)
    assert funding.locked_units == locked
    # The same floor to the unit in the tape gate and the operator tool, rounded up ...
    assert gate_requirement == required
    assert required_units(floors.escrow.policy, floors.escrow.manifest, locked) == required
    # ... and the same verdict at the boundary: required - 1 refused, required and more pass.
    assert eligible == allowed == (locked >= required)


def test_the_stake_floor_rounds_up_where_the_exact_quotient_is_fractional(
    li_escrow: ModuleType, objects: LocalFSStore
) -> None:
    # ceil(101 * 10^12 / (500_000 * 333_333)) = 607, not 606: truncation would under-collateralise.
    floors = Floors(
        li_escrow,
        objects,
        G_max_units=101,
        s_lower_ppm=500_000,
        beta_ppm=333_333,
        S_min_units=0,
        R_collectible_units=0,
    )
    floors.lock_to(606)
    eligible, funding = floors.status()
    assert (eligible, floors.tape_gate(funding)) == (False, (False, 607))
    floors.lock_to(607)
    eligible, funding = floors.status()
    assert (eligible, floors.tape_gate(funding)) == (True, (True, 607))
    assert required_units(floors.escrow.policy, floors.escrow.manifest, 0) == 607


def test_no_stake_satisfies_the_floor_when_detection_is_zero_in_any_implementation(
    li_escrow: ModuleType, objects: LocalFSStore
) -> None:
    floors = Floors(li_escrow, objects, G_max_units=100, s_lower_ppm=0)
    floors.lock_to(20_000)
    eligible, funding = floors.status()
    allowed, gate_requirement = floors.tape_gate(funding)
    assert (eligible, allowed) == (False, False)
    assert gate_requirement > funding.locked_units
    assert required_units(floors.escrow.policy, floors.escrow.manifest, 20_000) > 20_000


# BND-07 and BND-08: every live roster entry is audited with probability one; the draw is exact.


@given(
    state=st.sampled_from(["APPLIED", "PROBATION", "ACTIVE", "SUSPENDED", "BANNED"]),
    clean=st.integers(0, 20),
    anomaly=st.integers(0, PPM),
    locked=st.integers(0, 5000),
    collectible=st.integers(0, 500),
)
def test_the_audit_floor_is_certainty_for_every_state_of_a_roster_entry(
    state: str, clean: int, anomaly: int, locked: int, collectible: int
) -> None:
    roster = RosterEntryV2(
        hotkey=K,
        slot=0,
        q_i=f32hex(1),
        admission_id=H,
        coldkey_group="owner",
        state=state,  # type: ignore[arg-type]
        eligible_weight=1 << 22,
    )
    floor = audit_floor(
        econ(),
        example_manifest().verify,
        roster,
        FundedStatus(H, K, H, "test", locked, collectible, H, True),
        run_id=H,
        clean_finalizations=clean,
        reference_reward_units=100,
        anomaly_ppm=anomaly,
    )
    # The nominal 1.0 / 0.5 / 0.25 schedule never lowers the pinned floor of 100 percent.
    assert floor.q_ppm == PPM


def test_a_policy_cannot_pin_an_audit_floor_below_certainty() -> None:
    assert econ(q_floor=PPM).q_floor == PPM
    for other in (0, 250_000, 999_999, PPM + 1):
        assert refuses(lambda other=other: econ(q_floor=other))


class FixedDigest:
    def __init__(self, value: int) -> None:
        self.value = value

    def digest(self) -> bytes:
        return self.value.to_bytes(32, "big")


def draw(patch: pytest.MonkeyPatch, value: int, q_ppm: int) -> bool:
    """select_audit when the sha256 of the draw seed is `value`."""
    patch.setattr(trust_v2, "hashlib", SimpleNamespace(sha256=lambda _: FixedDigest(value)))
    return select_audit(H, 0, H, K, q_ppm).selected


@given(q_ppm=st.one_of(st.sampled_from([0, 1, 250_000, 999_999, PPM]), st.integers(0, PPM)))
def test_the_audit_draw_selects_exactly_the_digests_below_ceil_q_of_the_space(q_ppm: int) -> None:
    # T = ceil(q * 2^256 / 10^6): rounding favours auditing; q = 0 never, q = 10^6 always.
    threshold = -(-q_ppm * 2**256 // PPM)
    with pytest.MonkeyPatch.context() as patch:
        if threshold:
            assert draw(patch, threshold - 1, q_ppm)
        if threshold < 2**256:
            assert not draw(patch, threshold, q_ppm)
        assert draw(patch, 0, q_ppm) == (q_ppm > 0)
        assert draw(patch, 2**256 - 1, q_ppm) == (q_ppm == PPM)


def test_the_audit_draw_is_a_function_of_the_run_the_round_the_beacon_and_the_hotkey() -> None:
    def smallest_selecting(run: str, w: int, beacon: str, hotkey: str) -> int:
        seed = f"ht-audit-v2|{run}|{w}|{beacon}|{hotkey}".encode()
        value = int.from_bytes(hashlib.sha256(seed).digest(), "big")
        return value * PPM // 2**256 + 1  # least q with value * 10^6 < q * 2^256

    other_key = Keypair(bytes([0x31]) * 32).ss58
    base = ("ab" * 32, 7, "cd" * 32, K)
    variants = [
        base,
        ("ef" * 32, 7, "cd" * 32, K),
        ("ab" * 32, 8, "cd" * 32, K),
        ("ab" * 32, 7, "12" * 32, K),
        ("ab" * 32, 7, "cd" * 32, other_key),
    ]
    thresholds = [smallest_selecting(*v) for v in variants]
    assert len(set(thresholds)) == len(thresholds)
    for variant, q_ppm in zip(variants, thresholds, strict=True):
        assert select_audit(*variant[:3], variant[3], q_ppm).selected
        assert not select_audit(*variant[:3], variant[3], q_ppm - 1).selected


@pytest.mark.parametrize(
    ("w", "q_ppm"),
    [(0, -1), (0, PPM + 1), (0, True), (0, 0.5), (-1, PPM), (True, PPM), (1.0, PPM)],
)
def test_the_audit_draw_refuses_out_of_range_or_non_integer_bounds(
    w: object, q_ppm: object
) -> None:
    with pytest.raises(TrustError, match="invalid audit draw bounds"):
        select_audit(H, w, H, K, q_ppm)  # type: ignore[arg-type]


# BND-17: a policy with a missing, unknown or out-of-range field is refused.

POLICY_FAULTS = {
    "an unknown field": {"surprise": 1},
    "q_floor below certainty": {"q_floor": 999_999},
    "a nonzero gammaV": {"gammaV_units": 1},
    "eleven shadow rounds": {"shadow_bootstrap_rounds": 11},
    "G at 2^53": {"G_max_units": 2**53},
    "the issuance cap at 2^63": {"max_total_issuance": 2**63},
    "R at int64 max": {"R_collectible_units": 2**63 - 1},
    "a negative stake": {"S_min_units": -1},
    "a zero beta": {"beta_ppm": 0},
    "s above one in ppm": {"s_lower_ppm": PPM + 1},
    "a float where an integer is due": {"G_max_units": 100.0},
    "test mode without a genesis pin": {"genesis_allocation_hash": None},
    "production mode with a genesis pin": {"ledger_mode": "production"},
    "test mode with a bootstrap pin": {"bootstrap_allocation_hash": H},
    "an unknown ledger mode": {"ledger_mode": "staging"},
}


@pytest.mark.parametrize("fault", sorted(POLICY_FAULTS))
def test_a_policy_with_an_unknown_or_out_of_range_field_is_refused(fault: str) -> None:
    assert econ().ledger_mode == "test"  # control: the unchanged body is accepted
    assert refuses(lambda: econ(**POLICY_FAULTS[fault]))


@pytest.mark.parametrize("field", sorted(BASE_POLICY))
def test_a_policy_without_one_of_its_fields_is_refused_none_has_a_default(field: str) -> None:
    body = {name: value for name, value in BASE_POLICY.items() if name != field}
    assert refuses(lambda: EconomicsPolicyV2.model_validate(body))


# BND-09, BND-10, BND-13 and BND-14: the production bootstrap and the test genesis. The ledger
# half of "the bootstrap is issued once and a replay returns the stored receipt" is already pinned
# by test_bootstrap_idempotent_per_operation_and_once_per_run in
# tests/ledger/test_production_bootstrap_v2.py; the store half is in test_bounds_2.py.


def test_the_test_genesis_has_one_operation_a_replay_returns_it_other_content_conflicts(
    li_escrow: ModuleType,
) -> None:
    escrow = li_escrow.ledger(li_escrow.setup("test"), memory_db())
    pin = escrow.policy.genesis_allocation_hash
    allocation = [(li_escrow.COLD.ss58, 20_000, 1), (li_escrow.OTHER.ss58, 10_000, 1)]
    request = genesis_request(li_escrow, escrow.run_id, allocation, pin=pin)
    first = escrow.test_genesis(request)
    balances = escrow.balances()
    assert escrow.test_genesis(request) == first and escrow.balances() == balances
    other = genesis_request(li_escrow, escrow.run_id, [(li_escrow.COLD.ss58, 30_000, 1)], pin=pin)
    assert error_code(lambda: escrow.test_genesis(other)) == "OPERATION_CONFLICT"
    assert escrow.balances() == balances


BOOTSTRAP_FAULTS = {
    "allocation hash other than the pin": "BOOTSTRAP_ALLOCATION",
    "the pinned hash over a different list": "BOOTSTRAP_ALLOCATION",
    "an origin id of the genesis domain": "BOOTSTRAP_ORIGIN_DOMAIN",
    "an origin id of another run": "BOOTSTRAP_ORIGIN_DOMAIN",
    "an authority signature by another signer": "ORIGIN_AUTHORITY",
    "a body for another run": "ORIGIN_AUTHORITY",
}


@pytest.mark.parametrize("fault", sorted(BOOTSTRAP_FAULTS))
def test_a_bootstrap_other_than_the_owner_pinned_allocation_is_refused_and_issues_nothing(
    li_escrow: ModuleType, fault: str
) -> None:
    fixture = li_escrow
    _, escrow, allocation = pinned_production(fixture)
    run, pin = escrow.run_id, escrow.policy.bootstrap_allocation_hash
    swapped = [(allocation[0][0], 10_000, 1), (allocation[1][0], 20_000, 1)]
    refused = {
        "allocation hash other than the pin": lambda: pinned_bootstrap(
            fixture, run, [(fixture.COLD.ss58, 30_000, 1)]
        ),
        "the pinned hash over a different list": lambda: pinned_bootstrap(
            fixture, run, swapped, pin=pin
        ),
        "an origin id of the genesis domain": lambda: pinned_bootstrap(
            fixture,
            run,
            allocation,
            origin_ids=[
                origin_id_of(escrow, "bootstrap", run, pin, 0),
                origin_id_of(escrow, "genesis", run, pin, 1),
            ],
        ),
        "an origin id of another run": lambda: pinned_bootstrap(
            fixture,
            run,
            allocation,
            origin_ids=[origin_id_of(escrow, "bootstrap", "ab" * 32, pin, i) for i in range(2)],
        ),
        "an authority signature by another signer": lambda: pinned_bootstrap(
            fixture, run, allocation, signer=fixture.OTHER
        ),
        "a body for another run": lambda: pinned_bootstrap(fixture, "ab" * 32, allocation),
    }[fault]()
    assert error_code(lambda: escrow.production_bootstrap(refused)) == BOOTSTRAP_FAULTS[fault]
    assert nothing_issued(escrow)
    # Control: the issuer can still issue exactly the pinned list afterwards.
    escrow.production_bootstrap(pinned_bootstrap(fixture, run, allocation))
    assert escrow.balances().issued == 30_000


def test_a_bootstrap_conserves_its_total_and_its_origins_are_distinct(
    li_escrow: ModuleType,
) -> None:
    body = pinned_bootstrap(li_escrow, "ab" * 32, [(li_escrow.COLD.ss58, 20_000, 1)]).body()
    assert ProductionBootstrap.model_validate(body).total_units == 20_000
    assert refuses(lambda: ProductionBootstrap.model_validate({**body, "total_units": 20_001}))
    # The repeated origin states a total that matches its sum, so only distinctness refuses it.
    repeated = {**body, "origins": body["origins"] * 2, "total_units": 40_000}
    assert refuses(lambda: ProductionBootstrap.model_validate(repeated))


def test_a_test_genesis_is_refused_once_the_ledger_has_any_event(li_escrow: ModuleType) -> None:
    setup = li_escrow.setup("test")
    rounds = [synthetic_round(li_escrow, setup, 0, [worker(0)])]
    escrow = ledger_for(li_escrow, setup, rounds)
    escrow.reward_finalize(rounds[0].reward, rounds[0].evidence)
    balances = escrow.balances()
    allocation = [(li_escrow.COLD.ss58, 20_000, 1), (li_escrow.OTHER.ss58, 10_000, 1)]
    late = genesis_request(
        li_escrow, escrow.run_id, allocation, pin=escrow.policy.genesis_allocation_hash
    )
    assert error_code(lambda: escrow.test_genesis(late)) == "TEST_GENESIS_FORBIDDEN"
    assert escrow.balances() == balances


def test_units_never_cross_ledger_modes_a_run_has_one_mode_for_life(
    li_escrow: ModuleType,
) -> None:
    # A production run refuses a test genesis and a test run refuses a production bootstrap.
    _, escrow, allocation = pinned_production(li_escrow)
    genesis = genesis_request(li_escrow, escrow.run_id, allocation)
    assert error_code(lambda: escrow.test_genesis(genesis)) == "TEST_GENESIS_FORBIDDEN"
    assert nothing_issued(escrow)
    test_escrow = li_escrow.ledger(li_escrow.setup("test"), memory_db())
    attempt = pinned_bootstrap(li_escrow, test_escrow.run_id, allocation)
    code = error_code(lambda: test_escrow.production_bootstrap(attempt))
    assert code == "PRODUCTION_BOOTSTRAP_FORBIDDEN"
    assert nothing_issued(test_escrow)
    # The pins of the two modes cannot be combined in one policy.
    assert refuses(lambda: econ(bootstrap_allocation_hash=H))
    assert refuses(lambda: econ(ledger_mode="production"))


def test_a_ledger_database_belongs_to_one_policy_and_one_run_for_life(
    li_escrow: ModuleType,
) -> None:
    production, escrow, allocation = pinned_production(li_escrow)
    escrow.production_bootstrap(pinned_bootstrap(li_escrow, escrow.run_id, allocation))
    before, database = escrow.balances(), escrow.db
    # Another policy (hence another run id) or the other mode cannot reinterpret the same units.
    other = repinned(production, G_max_units=production.policy.G_max_units + 1)
    assert other.manifest.run_id() != production.manifest.run_id()
    changed = "LEDGER_MODE_OR_RUN_CHANGED"
    assert error_code(lambda: li_escrow.ledger(other, database)) == changed
    assert error_code(lambda: li_escrow.ledger(li_escrow.setup("test"), database)) == changed
    # The same policy under another manifest is another run and is refused too.
    body = production.manifest.body()
    body["network"]["aggregation_policy_hash"] = "22" * 32
    elsewhere = replace(production, manifest=RunManifestV2.model_validate(body))
    assert elsewhere.manifest.run_id() != production.manifest.run_id()
    assert error_code(lambda: li_escrow.ledger(elsewhere, database)) == changed
    # An out-of-band edit of the pinned policy text is refused by the next start, not honoured.
    stored = json.loads(database.execute("SELECT policy FROM escrow_config").fetchone()[0])
    stored["G_max_units"] += 1
    database.execute("UPDATE escrow_config SET policy=?", (json.dumps(stored),))
    assert error_code(lambda: li_escrow.ledger(production, database)) == changed
    database.execute("UPDATE escrow_config SET policy=?", (production.policy.model_dump_json(),))
    assert li_escrow.ledger(production, database).balances() == before


def test_a_receipt_of_another_ledger_mode_fails_the_event_chain_at_the_next_start(
    li_escrow: ModuleType,
) -> None:
    production, escrow, allocation = pinned_production(li_escrow)
    escrow.production_bootstrap(pinned_bootstrap(li_escrow, escrow.run_id, allocation))
    row = escrow.db.execute("SELECT operation_id, receipt FROM escrow_events").fetchone()
    receipt = json.loads(row["receipt"])
    assert receipt["ledger_mode"] == "production"
    receipt["ledger_mode"] = "test"
    escrow.db.execute(
        "UPDATE escrow_events SET receipt=? WHERE operation_id=?",
        (json.dumps(receipt), row["operation_id"]),
    )
    assert error_code(lambda: li_escrow.ledger(production, escrow.db)) == "EVENT_CHAIN"


# BND-04: the policy of a run is the one object its manifest pins.


def test_the_ledger_refuses_a_policy_other_than_the_one_the_manifest_pins(
    li_escrow: ModuleType,
) -> None:
    setup = li_escrow.setup("production")
    forged = replace(setup, policy=setup.policy.model_copy(update={"G_max_units": 101}))
    database = memory_db()
    assert error_code(lambda: li_escrow.ledger(forged, database)) == "ECONOMICS_POLICY_HASH"
    assert "escrow_events" not in table_names(database)


def test_the_ledger_refuses_a_round_reward_that_is_not_epochs_per_round_times_a_million(
    li_escrow: ModuleType,
) -> None:
    setup = li_escrow.setup("production")
    assert li_escrow.ledger(setup, memory_db()).policy.round_reward_units == 1_000_000
    # The manifest pins the new policy digest, so only the budget relation is violated, one unit
    # above or one unit below.
    for off_by_one in (1_000_001, 999_999):
        database = memory_db()
        pinned = repinned(setup, round_reward_units=off_by_one)
        refused = error_code(lambda p=pinned, d=database: li_escrow.ledger(p, d))
        assert refused == "MANIFEST_REWARD_BUDGET"
        assert "escrow_events" not in table_names(database)
    consistent = repinned(setup, epochs_per_round=2, round_reward_units=2_000_000)
    assert li_escrow.ledger(consistent, memory_db()).policy.round_reward_units == 2_000_000


# BND-11: total issuance never exceeds the pinned cap, counting bootstrap and live rewards.


@given(
    bootstrap=st.integers(1, 2_000_000),
    rewards_that_fit=st.integers(0, 3),
    slack=st.sampled_from([-1, 0, 1]),
    position=st.integers(0, 4),
)
def test_bootstrap_and_live_rewards_never_issue_more_than_the_cap_and_an_exact_fit_passes(
    li_escrow: ModuleType, bootstrap: int, rewards_that_fit: int, slack: int, position: int
) -> None:
    # A cap that holds the bootstrap and `rewards_that_fit` rewards, exactly or one unit off.
    cap = bootstrap + rewards_that_fit * 1_000_000 + slack
    allocation = [(li_escrow.COLD.ss58, bootstrap, 1)]
    setup = repinned(
        li_escrow.setup("production"),
        max_total_issuance=cap,
        bootstrap_allocation_hash=genesis_allocation_hash(allocation),
    )
    rounds = [synthetic_round(li_escrow, setup, w, [worker(0)]) for w in range(4)]
    escrow = ledger_for(li_escrow, setup, rounds)
    issued, finalized = 0, 0
    for step in range(5):
        # Step `position` is the bootstrap, every other step the next round reward.
        if step == position:
            amount = bootstrap
            run = partial(
                escrow.production_bootstrap, pinned_bootstrap(li_escrow, escrow.run_id, allocation)
            )
        else:
            amount = 1_000_000
            run = partial(
                escrow.reward_finalize, rounds[finalized].reward, rounds[finalized].evidence
            )
        if issued + amount <= cap:  # an exact fit passes
            run()
            issued += amount
            if step != position:
                finalized += 1
        else:
            assert error_code(run) == "TOTAL_ISSUANCE_BUDGET"
        # Accepted or refused, the ledger issued exactly the accepted amounts and no more.
        assert escrow.balances().issued == issued <= cap
        assert escrow.balances().conserved()


# BND-20: each finalized round issues exactly its budget, split conservingly.


@given(workers=st.integers(1, 9), epochs=st.integers(1, 3))
def test_a_round_issues_exactly_its_budget_floor_split_with_the_remainder_to_the_first_hotkeys(
    li_escrow: ModuleType, workers: int, epochs: int
) -> None:
    setup = repinned(
        li_escrow.setup("production"),
        epochs_per_round=epochs,
        round_reward_units=epochs * 1_000_000,
    )
    team = [worker(i) for i in range(workers)]
    round_ = synthetic_round(li_escrow, setup, 0, team)
    escrow = ledger_for(li_escrow, setup, [round_])
    escrow.reward_finalize(round_.reward, round_.evidence)
    hotkeys = sorted((hot.ss58 for hot, _ in team), key=lambda h: h.encode())
    issued = {
        row["origin"]: row["issued"]
        for row in escrow.db.execute("SELECT origin, issued FROM escrow_origins")
    }
    by_hotkey = {h: issued[o] for h, o in zip(hotkeys, round_.reward.origin_ids, strict=True)}
    assert by_hotkey == split(epochs * 1_000_000, hotkeys)
    assert sum(issued.values()) == escrow.balances().issued == epochs * 1_000_000
    # The remainder is below the number of workers: no two workers differ by more than a unit.
    assert max(issued.values()) - min(issued.values()) <= 1


@pytest.mark.parametrize(
    "variant", ["the remainder to the last hotkey", "floor only", "everyone rounded up"]
)
def test_a_reward_body_that_states_another_split_than_the_documented_one_is_refused(
    li_escrow: ModuleType, variant: str
) -> None:
    setup = li_escrow.setup("production")
    team = [worker(i) for i in range(3)]
    hotkeys = sorted((hot.ss58 for hot, _ in team), key=lambda h: h.encode())
    # Control: the documented split of 1,000,000 between three is 333,334 / 333,333 / 333,333.
    honest = synthetic_round(li_escrow, setup, 0, team)
    assert [honest.units[h] for h in hotkeys] == [333_334, 333_333, 333_333]
    accepting = ledger_for(li_escrow, setup, [honest])
    accepting.reward_finalize(honest.reward, honest.evidence)
    assert accepting.balances().issued == 1_000_000
    stated = {
        "the remainder to the last hotkey": [333_333, 333_333, 333_334],
        "floor only": [333_333, 333_333, 333_333],
        "everyone rounded up": [333_334, 333_334, 333_334],
    }[variant]
    units = dict(zip(hotkeys, stated, strict=True))
    wrong = synthetic_round(li_escrow, setup, 0, team, units=units)
    escrow = ledger_for(li_escrow, setup, [wrong])
    assert error_code(lambda: escrow.reward_finalize(wrong.reward, wrong.evidence)) == (
        "REWARD_ALLOCATION"
    )
    assert nothing_issued(escrow)


@pytest.mark.parametrize("budget", [999_999, 1_000_001])
def test_a_reward_whose_budget_is_not_the_round_reward_is_refused_with_round_budget(
    li_escrow: ModuleType, budget: int
) -> None:
    setup = li_escrow.setup("production")
    off = synthetic_round(li_escrow, setup, 0, [worker(0), worker(1)], budget=budget)
    escrow = ledger_for(li_escrow, setup, [off])
    assert error_code(lambda: escrow.reward_finalize(off.reward, off.evidence)) == "ROUND_BUDGET"
    assert nothing_issued(escrow)
