"""Escrow laws that need a real reward, a dispute service or the HTTP routes.

The real w=0 reward comes from the lot's session fixture. A refusal pins its code and leaves
the unit rows and events unchanged; the control of each is the same step made valid. The random
operation sequences against a bucket model, the open holes of ESC-23 and the recipient rule of
ES-07 close the file.
"""

from __future__ import annotations

import ast
import sqlite3
import tempfile
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path
from types import ModuleType
from typing import Any, NamedTuple

import pytest
from hypothesis import Phase, example, given, settings
from hypothesis import strategies as st

import hypertrain
from hypertrain.auditor.island_bisect import RefereeEvidence
from hypertrain.ledger.escrow_v2 import EscrowV2, Settlement, genesis_allocation_hash
from hypertrain.protocol.envelope_v2 import seal
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import KeyError_, Keypair, decode_hotkey
from hypertrain.protocol.messages_v2 import (
    DisputeV2,
    EscrowLock,
    EscrowRelease,
    EscrowTransfer,
    ResolutionV2,
)

from .support import (
    SERVICE_FIXTURE,
    H,
    Real,
    bare_escrow,
    error_code,
    known_replay,
    load_fixture_module,
    match_settlement,
    mature,
    resign_reward,
    routes,
    service_world,
)
from .support_bounds import genesis_request, repinned
from .support_escrow_laws import (
    OTHER_UNITS,
    bootstrapped,
    dispute_code,
    events,
    fingerprint,
    funded,
    lock_floor,
    lock_op,
    pay_op,
    release_op,
    resolve_contests_like_the_generic_callback,
    round_variant,
    settled,
    signed_by,
    transfer_op,
)

UNFUNDED = "UNFUNDED_OR_UNKNOWN_ORIGIN"
DISPUTES = ("li_disputes_fixture", "challenge/test_disputes_v2.py")
BUDGET = 1_000_000


def rewarded(li_escrow: ModuleType, real: Real, table: dict[str, Settlement]) -> EscrowV2:
    """A production-mode ledger that issued the real round-0 reward; `table` settles it."""
    setup, evidence, reward = real
    escrow = bare_escrow(li_escrow, setup, settlement=table)
    escrow.reward_finalize(reward, evidence)
    return escrow


# ----------------------------------------------------------------- maturity gate (ESC-08)

GOOD = {"unresolved": False, "outcome": "MATCH", "release_beacon": 20}
MATURITY_FAULTS = {
    "before_the_vesting_round": (19, GOOD, "IMMATURE_OR_UNKNOWN_ORIGIN"),
    "settlement_not_resolved": (20, {**GOOD, "unresolved": True}, "DISPUTED_OR_UNVESTED"),
    "settlement_is_fraud": (20, {**GOOD, "outcome": "FRAUD"}, "DISPUTED_OR_UNVESTED"),
    "settlement_not_yet_released": (20, {**GOOD, "release_beacon": 21}, "DISPUTED_OR_UNVESTED"),
    "settlement_of_another_finalize": (20, {**GOOD, "finality_hash": H}, "DISPUTED_OR_UNVESTED"),
    "settlement_not_recorded": (20, None, "UNACCEPTED_SETTLEMENT"),
}


@pytest.mark.parametrize("fault", MATURITY_FAULTS)
def test_a_reward_matures_only_after_vesting_and_a_resolved_hash_bound_settlement(
    li_escrow: ModuleType, real_reward: Real, fault: str
) -> None:
    """ESC-08 (the condition kept): the ledger releases a reward origin on its settlement alone."""
    # Given: the real round-0 reward, issued and pending, and a settlement that is not releasable.
    _, _, reward = real_reward
    table: dict[str, Settlement] = {}
    escrow = rewarded(li_escrow, real_reward, table)
    origin = reward.origin_ids[0]
    beacon, variant, code = MATURITY_FAULTS[fault]
    good = match_settlement(reward.finalize_hash, 20)
    if variant is not None:
        table[origin] = good.model_copy(update=variant)
    before = fingerprint(escrow)
    # When / Then: refused with the code of the failed condition; the units stay pending.
    assert error_code(lambda: escrow.mature("55" * 32, (origin,), beacon)) == code
    assert fingerprint(escrow) == before
    assert escrow.balances().reward_pending == BUDGET
    # Control: the releasable settlement at the vesting round matures the origin, once.
    table[origin] = good
    receipt = escrow.mature("55" * 32, (origin,), 20)
    assert (escrow.balances().available, escrow.balances().reward_pending) == (BUDGET, 0)
    assert escrow.mature("55" * 32, (origin,), 20) == receipt
    assert error_code(lambda: escrow.mature("56" * 32, (origin,), 20)) == "ALREADY_MATURED"
    assert escrow.balances().conserved()


@pytest.mark.parametrize(
    "origins", [(), ("aa" * 32, "aa" * 32), ("ff" * 32,)], ids=["none", "duplicated", "unknown"]
)
def test_maturity_input_is_a_nonempty_list_of_known_distinct_origins(
    li_escrow: ModuleType, origins: tuple[str, ...]
) -> None:
    """ESC-08: an empty, repeated or unknown origin list matures nothing and records nothing."""
    fx = funded(li_escrow, matured=False)
    before = fingerprint(fx.escrow)
    code = error_code(lambda: fx.escrow.mature(H, origins, 1))
    assert code == ("IMMATURE_OR_UNKNOWN_ORIGIN" if origins == ("ff" * 32,) else "MATURITY_INPUT")
    assert fingerprint(fx.escrow) == before
    assert error_code(lambda: fx.escrow.mature(H, (fx.cold,), True)) == "MATURITY_INPUT"  # type: ignore[arg-type]
    assert fx.escrow.balances().reward_pending == 30000


# ------------------------------------------------------------------ issuance (ESC-20)


def test_a_reward_is_issued_once_per_round_and_only_under_the_authority_signature(
    li_escrow: ModuleType, real_reward: Real
) -> None:
    """ESC-20: reward units need the reward authority's signature over this run, once per round."""
    # Given: an empty production ledger and three rewards that are signed wrongly.
    setup, evidence, reward = real_reward
    escrow = bare_escrow(li_escrow, setup)
    forged = {
        "another key": signed_by(reward, li_escrow.OTHER),
        "no signature": reward.model_copy(update={"authority_sig": "0" * 128}),
        "another run": resign_reward(li_escrow, reward, run_id=H),
    }
    # When / Then: none of them issues a unit or records an event.
    for name, bad in forged.items():
        assert (
            error_code(lambda bad=bad: escrow.reward_finalize(bad, evidence)) == "ORIGIN_AUTHORITY"
        )
        assert (escrow.balances().issued, events(escrow)) == (0, 0), name
    # Control: the authority's own signature issues the round budget once.
    receipt = escrow.reward_finalize(reward, evidence)
    assert (escrow.balances().issued, escrow.balances().reward_pending) == (BUDGET, BUDGET)
    assert escrow.reward_finalize(reward, evidence) == receipt
    assert (escrow.balances().issued, events(escrow)) == (BUDGET, 1)
    # A second, differently finalized body for the same round is a conflict, not a second issue.
    other, other_evidence = round_variant(li_escrow, real_reward, 0, entitlements_root="22" * 32)
    before = fingerprint(escrow)
    assert error_code(lambda: escrow.reward_finalize(other, other_evidence)) == "OPERATION_CONFLICT"
    assert fingerprint(escrow) == before
    assert escrow.balances().conserved()


def test_rounds_are_issued_in_order_and_never_beyond_the_pinned_total(
    li_escrow: ModuleType, real_reward: Real
) -> None:
    """ESC-20: round w is issued only after w-1, and the pinned total issuance is a ceiling."""
    # Given: the policy of the real reward caps total issuance at two round budgets.
    setup, evidence, reward = real_reward
    escrow = bare_escrow(li_escrow, setup)
    rounds = {w: round_variant(li_escrow, real_reward, w) for w in (1, 2)}
    # When / Then: round 2 cannot jump the queue, with round 0 missing or with round 1 missing.
    assert error_code(lambda: escrow.reward_finalize(*rounds[2])) == "FINALIZE_ORDER"
    escrow.reward_finalize(reward, evidence)
    assert error_code(lambda: escrow.reward_finalize(*rounds[2])) == "FINALIZE_ORDER"
    # Round 1 fills the cap exactly; round 2 in its turn is refused for the ceiling.
    escrow.reward_finalize(*rounds[1])
    assert escrow.balances().issued == 2 * BUDGET
    before = fingerprint(escrow)
    assert error_code(lambda: escrow.reward_finalize(*rounds[2])) == "TOTAL_ISSUANCE_BUDGET"
    assert fingerprint(escrow) == before
    assert escrow.balances().conserved()


# ------------------------------------------------------------------- pay (ESC-17)


def test_a_pay_operation_id_is_spent_once_and_never_for_another_amount(
    li_escrow: ModuleType,
) -> None:
    """ESC-17: one (epoch, origin, owner) pay id moves units once; a replay or a change cannot."""
    # Given: a pay of 500 units under one operation id.
    fx = funded(li_escrow)
    escrow, cold = fx.escrow, li_escrow.COLD.ss58
    payment = pay_op(li_escrow, fx.cold, operation="55" * 32, units=500)
    receipt = escrow.pay(payment, signer=cold)
    paid = fingerprint(escrow)
    # When / Then: the identical replay returns the receipt and moves nothing.
    assert escrow.pay(payment, signer=cold) == receipt
    assert fingerprint(escrow) == paid
    # The same id with another amount is a conflict and moves nothing either.
    larger = pay_op(li_escrow, fx.cold, operation="55" * 32, units=501)
    assert error_code(lambda: escrow.pay(larger, signer=cold)) == "OPERATION_CONFLICT"
    assert fingerprint(escrow) == paid
    assert escrow.balances().paid == 500
    # Control: the id is the unit of at-most-once; a new id is a new payment.
    escrow.pay(pay_op(li_escrow, fx.cold, operation="56" * 32, units=500), signer=cold)
    assert escrow.balances().paid == 1000 and escrow.balances().conserved()


# ---------------------------------------------------------- caller discipline (ESC-13)


def test_the_only_code_that_slashes_passes_the_coordinator_key_as_authority() -> None:
    """ESC-13 (the condition): authority is a compared string, so who calls slash is the guard.

    Two services slash, each with its own copy of the coordinator key: the dispute settlement
    (`DisputesV2.settle_lock`) and the admission bond (`Admission.seize_bond`, ADR 0010 Part 6).
    A service may slash from more places (the sanction of another reason, another lot), but a
    third module, or any authority other than that key, is a new way to burn units.
    """
    root = Path(hypertrain.__file__).parent
    authorities: dict[str, set[str]] = {}
    for path in sorted(root.rglob("*.py")):
        for node in ast.walk(ast.parse(path.read_text())):
            if isinstance(node, ast.Call) and getattr(node.func, "attr", "") == "slash":
                found = {ast.unparse(k.value) for k in node.keywords if k.arg == "authority"}
                authorities.setdefault(path.relative_to(root).as_posix(), set()).update(
                    found or {"<none>"}
                )
    assert authorities == {
        "challenge/admission.py": {"self.coord.ss58"},
        "challenge/disputes_v2.py": {"self.key.ss58"},
    }


# ------------------------------------------------------------ bond of a dispute (ESC-24, DIA-01)

BOND_FAULTS = {
    "undersized_below_the_referee_cost": ("CONTEST_LOCK_BINDING_OR_COST", {"units": 99}),
    "owned_by_another_coldkey": ("CONTEST_LOCK_BINDING_OR_COST", {"owner": "OTHER"}),
    "bound_to_another_dispute": ("CONTEST_LOCK_BINDING_OR_COST", {"reference": "99" * 32}),
    "an_admission_stake_not_a_bond": ("UNFUNDED_CONTEST", {"kind": "LOCK_ADMISSION"}),
    "no_such_lock": ("UNFUNDED_CONTEST", None),
}


@dataclass
class Contested:
    """The unit dispute service of test_disputes_v2 with COLD's valid 100-unit bond already in."""

    service: Any
    fixture: ModuleType
    referee: Any
    valid: bytes

    def request(self, bond_lock: str, verdict: str = "11" * 32) -> bytes:
        body = DisputeV2(
            hotkey=self.fixture.HOT.ss58,
            verdict_hash=verdict,
            action="contest",
            bond_lock=bond_lock,
        )
        return canonicalize(seal(self.fixture.HOT, "DisputeV2", self.service.run_id, body, 100000))

    def origin(self, owner: str) -> str:
        row = self.service.db.execute(
            "SELECT origin FROM escrow_units WHERE owner=? AND units>0", (owner,)
        ).fetchone()
        return str(row[0])

    def rows(self) -> tuple[int, int]:
        count = self.service.db.execute
        return (
            count("SELECT COUNT(*) FROM disputes_v2").fetchone()[0],
            count("SELECT COUNT(*) FROM dispute_events_v2").fetchone()[0],
        )


def contested_miner(tmp_path: Path) -> Contested:
    module = load_fixture_module(*DISPUTES)
    service, fixture, referee, _, valid = module.service(tmp_path)
    return Contested(service, fixture, referee, valid)


@pytest.mark.parametrize("fault", BOND_FAULTS)
def test_a_dispute_opens_only_on_a_funded_owner_bound_adequately_sized_contest_bond(
    tmp_path: Path, fault: str
) -> None:
    """ESC-24, DIA-01: the bond precedes the dispute, belongs to the miner's coldkey and to it."""
    # Given: the miner's valid bond, and one bond (or none) that breaks a single condition.
    world = contested_miner(tmp_path)
    service, fx = world.service, world.fixture
    valid = EscrowLock.model_validate_json(
        service.db.execute(
            "SELECT request FROM escrow_events WHERE operation_id=?", ("44" * 32,)
        ).fetchone()[0]
    )
    code, change = BOND_FAULTS[fault]
    bond_id = "ee" * 32
    if change is not None:
        owner = getattr(fx, change.get("owner", "COLD")).ss58
        bond = lock_op(
            fx,
            world.origin(owner),
            operation="55" * 32,
            units=change.get("units", 100),
            owner=owner,
            kind=change.get("kind", "LOCK_CONTEST"),
            reference=change.get("reference", valid.dispute_id),
        )
        service.escrow.lock(bond, signer=owner)
        bond_id = bond.operation_id
    before = fingerprint(service.escrow)
    # When / Then: refused with the code of that condition; no dispute, no event, no balance moves.
    assert dispute_code(lambda: service.open(world.request(bond_id), now=1)) == code
    assert world.rows() == (0, 0)
    assert fingerprint(service.escrow) == before
    # Control: the valid bond opens the dispute and stays locked under it.
    turn = service.open(world.valid, now=1)
    assert turn.lock_id == valid.operation_id and world.rows()[0] == 1
    assert service.escrow.balances(fx.COLD.ss58).dispute_locked >= 100


def test_a_miner_has_at_most_one_open_dispute(tmp_path: Path) -> None:
    """ESC-24: a second contest of the same miner is refused while the first is unresolved, and
    the refusal moves nothing (what becomes of its funded bond is DIA-25, package exit_disputes)."""
    # Given: one open dispute, and a second verdict of the same miner with a funded bond.
    world = contested_miner(tmp_path)
    service, fx = world.service, world.fixture
    first = service.open(world.valid, now=1)
    second = service.contest_of("12" * 32).model_copy(update={"verdict_hash": "12" * 32})
    service.contest_of = lambda _: second
    identifier = sha256_hex(canonicalize([service.run_id, second.verdict_hash, second.miner]))
    bond = lock_op(
        fx,
        world.origin(fx.COLD.ss58),
        operation="55" * 32,
        units=100,
        kind="LOCK_CONTEST",
        reference=identifier,
    )
    service.escrow.lock(bond, signer=fx.COLD.ss58)
    before = fingerprint(service.escrow)
    # When / Then
    request = world.request(bond.operation_id, verdict=second.verdict_hash)
    assert (
        dispute_code(lambda: service.open(request, now=1)) == "QUEUE_CAPACITY_EXCLUDE_WITHOUT_FRAUD"
    )
    assert world.rows()[0] == 1 and service.get(first.dispute_id) == first
    assert fingerprint(service.escrow) == before


# ------------------------------------------------------ settlement of a bond (DIA-10)


@pytest.mark.parametrize(
    "closure", ["miner_proven_fraudulent", "auditor_proven_fraudulent", "infrastructure"]
)
def test_a_bond_is_settled_once_whole_and_with_the_production_default_cost(
    tmp_path: Path, closure: str
) -> None:
    """DIA-10, ESC-14: settle_lock burns the whole bond only for a miner proven fraudulent."""
    # Given: a dispute closed one way, settled the way the store does (no referee cost).
    world = contested_miner(tmp_path)
    service, fx = world.service, world.fixture
    accepted = service.contest_of("11" * 32).model_copy(update={"auditor_coldkey": fx.OTHER.ss58})
    service.contest_of = lambda _: accepted
    turn = service.open(world.valid, now=1)
    if closure == "infrastructure":
        turn = service.exclude(turn.dispute_id, now=turn.absolute_deadline)
        settle_at = turn.absolute_deadline
    else:
        loser = fx.HOT.ss58 if closure == "miner_proven_fraudulent" else fx.AUDITOR.ss58
        proof = RefereeEvidence(
            dispute_id=turn.dispute_id,
            transcript_hash=turn.transcript_hash,
            reason="FRAUD",
            loser=loser,
            predecessor_hash="11" * 32,
            inputs_hash="22" * 32,
            output_hash="33" * 32,
            op_spec="verified.actual.op",
        )
        service.register_evidence(proof)
        verdict = ResolutionV2(
            dispute_id=turn.dispute_id,
            transcript_hash=turn.transcript_hash,
            reason="FRAUD",
            loser=loser,
            evidence_hash=proof.digest(),
        )
        body = canonicalize(seal(world.referee, "ResolutionV2", service.run_id, verdict, 100000))
        service.resolution(body, now=3)
        settle_at = 3
    cold = fx.COLD.ss58
    assert service.escrow.balances(cold).dispute_locked == 100
    # When: the settlement is asked twice, at two different beacons.
    service.settle_lock(turn.dispute_id, now=settle_at)
    settled_once = fingerprint(service.escrow)
    service.settle_lock(turn.dispute_id, now=settle_at + 1)
    # Then: one settlement event, nothing moved the second time, every unit accounted for.
    burned = 100 if closure == "miner_proven_fraudulent" else 0
    assert fingerprint(service.escrow) == settled_once
    assert events(service.escrow, "SLASH") == (1 if burned else 0)
    assert events(service.escrow, "RELEASE") == (0 if burned else 1)
    balances = service.escrow.balances()
    assert (balances.burned, balances.dispute_locked) == (burned, 0)
    assert service.escrow.balances(cold).available == 20000 - burned
    assert balances.conserved()
    service.escrow.verify()


# ----------------------------------------------------- HTTP routes (ES-01, ES-03, LCK-10)


def keys() -> ModuleType:
    return load_fixture_module(*SERVICE_FIXTURE)


def test_escrow_routes_refuse_a_foreign_signature_and_change_nothing(tmp_path: Path) -> None:
    """ES-01 (T-ES-01), ESC-02: a key naming another coldkey as owner moves nothing over HTTP."""
    with service_world(tmp_path) as world:
        service, network = keys(), world.network
        owner, foreign = service.COLD[0], service.COLD[1]
        origin = network.origin_ids[0]
        common = {"owner": owner.ss58, "units": 100, "origin_ids": [origin]}
        transfer = EscrowTransfer(
            operation_id="a1" * 32,
            recipient=foreign.ss58,
            admission_id=None,
            dispute_id=None,
            **common,
        )
        release = EscrowRelease(
            operation_id="a2" * 32,
            lock_id="a3" * 32,
            finality_hash=H,
            closed_dispute_root=H,
            admission_id=None,
            dispute_id=None,
            **common,
        )
        contest = EscrowLock(
            operation_id="a4" * 32,
            kind="LOCK_CONTEST",
            admission_id=None,
            dispute_id="bb" * 32,
            **common,
        )

        def post(action: str, key: Any, body: Any, kind: str) -> Any:
            return world.client.post(
                network.url + "/escrow/" + action, json=network.signed(key, kind, body)
            )

        before = world.db_digest()
        # When: the foreign key signs a transfer, a release and a contest lock naming the owner.
        refused = {
            "transfer": post("transfer", foreign, transfer, "EscrowTransfer"),
            "release": post("release", foreign, release, "EscrowRelease"),
            "contest lock": post("lock", foreign, contest, "EscrowLock"),
        }
        # Then: 409 with the ownership refusal, and not one row of the service changed.
        assert {name: (r.status_code, r.json()["detail"]) for name, r in refused.items()} == {
            "transfer": (409, "OWNER_AUTHORITY"),
            "release": (409, "OWNER_AUTHORITY"),
            "contest lock": (409, "signer is not authorized for message role"),
        }
        assert world.db_digest() == before
        # Control: the owner's signature moves the units to the recipient and posts the bond.
        assert post("transfer", owner, transfer, "EscrowTransfer").status_code == 200
        assert post("lock", owner, contest, "EscrowLock").status_code == 200
        escrow = world.store._services(world.run_id)[0]
        assert escrow.balances(foreign.ss58).available == 10_000 + 100
        assert escrow.balances(owner.ss58).available == 10_000 - 200
        assert escrow.balances(owner.ss58).dispute_locked == 100


ADMISSION_LOCK_FAULTS = {
    "another_coldkey_signs_for_the_admission": "signer is not authorized for message role",
    "the_admissions_key_names_another_owner": "LOCK_OWNER_OR_KIND",
    "an_admission_that_does_not_exist": "UNKNOWN_ADMISSION",
}


@pytest.mark.parametrize("fault", ADMISSION_LOCK_FAULTS)
def test_an_admission_stake_is_accepted_only_from_the_coldkey_recorded_at_join(
    tmp_path: Path, fault: str
) -> None:
    """ES-03, LCK-10: a lock names one admission and one coldkey, the one that joined."""
    with service_world(tmp_path) as world:
        service, network = keys(), world.network
        first, second = service.COLD[0], service.COLD[1]
        admission = network.join(0).json()["admission_id"]
        assert network.join(1).status_code == 200

        def stake(owner: Any, admission_id: str, operation: str) -> EscrowLock:
            return EscrowLock(
                operation_id=operation,
                owner=owner.ss58,
                units=1000,
                origin_ids=[network.origin_ids[0 if owner is first else 1]],
                admission_id=admission_id,
                dispute_id=None,
                kind="LOCK_ADMISSION",
            )

        def post(signer: Any, body: EscrowLock) -> Any:
            return world.client.post(
                network.url + "/escrow/lock", json=network.signed(signer, "EscrowLock", body)
            )

        refused = {
            "another_coldkey_signs_for_the_admission": lambda: post(
                second, stake(second, admission, "b1" * 32)
            ),
            "the_admissions_key_names_another_owner": lambda: post(
                first, stake(second, admission, "b2" * 32)
            ),
            "an_admission_that_does_not_exist": lambda: post(
                first, stake(first, "cc" * 32, "b3" * 32)
            ),
        }[fault]
        before = world.db_digest()
        # When / Then
        response = refused()
        assert (response.status_code, response.json()["detail"]) == (
            409,
            ADMISSION_LOCK_FAULTS[fault],
        )
        assert world.db_digest() == before
        # Control: the coldkey that joined stakes for its own admission.
        accepted = post(first, stake(first, admission, "b4" * 32))
        assert accepted.status_code == 200, accepted.text
        escrow = world.store._services(world.run_id)[0]
        assert escrow.locked(admission, first.ss58)[0] == 1000
        assert escrow.balances(second.ss58).admission_locked == 0


def test_clients_reach_only_lock_transfer_and_release_of_the_escrow(tmp_path: Path) -> None:
    """ESC-02: no client route pays, slashes, matures or issues units."""
    with service_world(tmp_path) as world:
        table = routes(world.app)
    base = "/v2/runs/{run_id}/escrow/"
    assert sorted(r.path for r in table if "escrow" in r.path) == [
        base + "lock",
        base + "release",
        base + "transfer",
    ]
    internal = {"slash", "pay", "mature", "mint", "issue", "genesis", "bootstrap"}
    client_routes = [r for r in table if "/admin/" not in r.path and "/internal/" not in r.path]
    assert [r.path for r in client_routes if internal & set(r.path.split("/"))] == []


# --------------------------------------------- random operation sequences (ESC-06, ESC-16, LCK-24)


@dataclass
class OpenLock:
    request: EscrowLock
    left: int
    fraud: bool  # the fate the settlement source will record for it, once it records one


class Plan(NamedTuple):
    call: Callable[[], object]
    code: str | None  # the refusal the ledger must give, or None when it must accept
    apply: Callable[[], None]  # what the bucket model does when the ledger accepts


class Step(NamedTuple):
    kind: str
    pick: int  # which owner, and which open lock a release or slash names
    size: int  # an affordable amount is drawn from it; a lock is FRAUD-fated when it is odd
    over: bool  # ask for one unit more than there is


STRANGER = Keypair(b"\x0a" * 32).ss58  # a real key that no issuance of the run names


class Bank:
    """The model: where the units of the one reward origin must sit after each accepted step.

    The owners it holds units for are the keys the ledger knows: COLD earned the reward and OTHER
    holds the collateral of another origin, which no step names. A transfer goes to them only.
    """

    def __init__(
        self,
        fixture: ModuleType,
        escrow: EscrowV2,
        origin: str,
        registry: dict[str, Settlement],
    ) -> None:
        self.fixture, self.escrow, self.origin, self.registry = fixture, escrow, origin, registry
        self.cold, self.other = fixture.COLD.ss58, fixture.OTHER.ss58
        self.available = {self.cold: BUDGET, self.other: 0}
        self.collateral = {self.cold: 0, self.other: OTHER_UNITS}
        self.locks: list[OpenLock] = []
        self.paid = self.burned = 0

    def held(self, owner: str, kind: str) -> int:
        return sum(
            lock.left
            for lock in self.locks
            if lock.request.owner == owner and lock.request.kind == kind
        )

    @staticmethod
    def amount(step: Step, ceiling: int) -> int:
        """Usually something the ceiling covers, sometimes exactly one unit too much."""
        return ceiling + 1 if step.over else 1 + step.size % max(ceiling, 1)

    def plan(self, step: Step, operation: str) -> Plan | None:
        who, rival = (self.cold, self.other)[step.pick % 2], (self.other, self.cold)[step.pick % 2]
        fx, escrow, origin = self.fixture, self.escrow, self.origin
        # A stake under the smallest lock a ledger takes would be refused for its size.
        smallest = lock_floor(escrow) if step.kind == "lock_admission" else 1
        units = max(self.amount(step, self.available[who]), smallest)
        short = None if units <= self.available[who] else UNFUNDED
        if step.kind in ("lock_admission", "lock_contest"):
            lock = lock_op(
                fx, origin, operation=operation, units=units, owner=who,
                kind="LOCK_" + step.kind.removeprefix("lock_").upper(),
            )  # fmt: skip

            def locked() -> None:
                self.available[who] -= units
                self.locks.append(OpenLock(lock, units, step.size % 2 == 1))

            return Plan(lambda: escrow.lock(lock, signer=who), short, locked)
        if step.kind == "transfer":
            # A pick of 4 or 5 sends to the stranger: refused before any unit is looked at.
            recipient = rival if step.pick < 4 else STRANGER
            gift = transfer_op(
                fx, origin, operation=operation, units=units, owner=who, recipient=recipient
            )

            def gifted() -> None:
                self.available[who] -= units
                self.available[recipient] += units

            known = recipient in self.available
            return Plan(
                lambda: escrow.transfer(gift, signer=who),
                short if known else "UNKNOWN_RECIPIENT",
                gifted,
            )
        if step.kind == "pay":
            payment = pay_op(fx, origin, operation=operation, units=units, owner=who)

            def spent() -> None:
                self.available[who] -= units
                self.paid += units

            return Plan(lambda: escrow.pay(payment, signer=who), short, spent)
        if step.kind == "foreign":
            forged = lock_op(fx, origin, operation=operation, units=1, owner=who)
            return Plan(lambda: escrow.lock(forged, signer=rival), "OWNER_AUTHORITY", lambda: None)
        if not self.locks:
            return None
        return self.settle(step, self.target(step), operation)

    def target(self, step: Step) -> OpenLock:
        """An open lock whose fate fits the step, if there is one; else any lock."""
        fits = [
            lock for lock in self.locks if lock.left > 0 and lock.fraud == (step.kind == "slash")
        ]
        pool = fits or self.locks
        return pool[step.pick % len(pool)]

    def settle(self, step: Step, target: OpenLock, operation: str) -> Plan:
        lock_id, owner = target.request.operation_id, target.request.owner
        units = self.amount(step, target.left)
        fate = self.registry.setdefault(  # the settlement source records the fate on first ask
            lock_id,
            settled("FRAUD", release_beacon=20) if target.fraud else match_settlement(H, 20),
        )
        request = release_op(target.request, operation=operation, units=units)
        short = None if units <= target.left else UNFUNDED
        if step.kind == "release":

            def released() -> None:
                self.available[owner] += units
                target.left -= units

            return Plan(
                lambda: self.escrow.release(request, signer=owner, now_beacon=20),
                "DISPUTED_OR_UNVESTED" if fate.outcome == "FRAUD" else short,
                released,
            )

        def burned() -> None:
            self.burned += units
            target.left -= units

        authority = self.fixture.COORD.ss58
        return Plan(
            lambda: self.escrow.slash(request, authority=authority, now_beacon=20),
            "UNSETTLED_SLASH" if fate.outcome != "FRAUD" else short,
            burned,
        )


KINDS = [
    "lock_admission",
    "lock_contest",
    "transfer",
    "pay",
    "release",
    "release",
    "slash",
    "slash",
    "foreign",
    "replay",
]
STEPS = st.builds(
    Step,
    st.sampled_from(KINDS),
    st.integers(min_value=0, max_value=5),
    st.integers(min_value=0, max_value=10**6),
    st.sampled_from([False, False, False, True]),
)
OPERATIONS = st.lists(STEPS, min_size=8, max_size=20)


# No shrink phase: a failing sequence is reported as drawn instead of being minimised for minutes.
# The draw follows the source of the test (derandomize), so the explicit sequence pins what an edit
# of it must not lose: a lock and a transfer one unit too large, a transfer back from OTHER, the
# slash of OTHER's lock, a replay and a transfer to the stranger.
@settings(phases=[Phase.explicit, Phase.generate])
@given(OPERATIONS)
@example(
    [
        Step("lock_contest", 0, 3, True),
        Step("transfer", 0, 7, True),
        Step("transfer", 0, 250_000, False),
        Step("transfer", 1, 1_000, False),
        Step("lock_contest", 1, 5, False),
        Step("slash", 1, 2, False),
        Step("replay", 0, 0, False),
        Step("transfer", 4, 1, False),
    ]
)
def test_random_operation_sequences_conserve_units_and_follow_a_bucket_model(
    li_escrow: ModuleType, real_reward: Real, sequence: list[Step]
) -> None:
    """ESC-06, ESC-16, LCK-24 (CRD-24), ES-07: after each event the ledger equals a bucket model."""
    # Given: a ledger that issued the real reward, matured to COLD, and vested collateral to
    # OTHER: a transfer goes only to a key the ledger knows, and a key enters it by issuance only.
    (setup, evidence, reward), bootstrap = bootstrapped(
        li_escrow, real_reward, [(li_escrow.OTHER.ss58, OTHER_UNITS, 1)]
    )
    origin = reward.origin_ids[0]
    registry = {origin: match_settlement(reward.finalize_hash, 20)}
    escrow = resolve_contests_like_the_generic_callback(
        bare_escrow(li_escrow, setup, settlement=registry)
    )
    bank = Bank(li_escrow, escrow, origin, registry)
    escrow.production_bootstrap(bootstrap)
    escrow.mature(H, tuple(o.origin_id for o in bootstrap.origins), 1)
    escrow.reward_finalize(reward, evidence)
    mature(escrow, reward)

    def check() -> None:
        balances = escrow.balances()
        assert balances.conserved() and balances.issued == BUDGET + OTHER_UNITS
        assert balances.paid <= balances.issued
        assert escrow.db.execute("SELECT MIN(units) FROM escrow_units").fetchone()[0] >= 0
        for owner in (bank.cold, bank.other):
            mine = escrow.balances(owner)
            assert mine.available == bank.available[owner] + bank.collateral[owner]
            assert mine.admission_locked == bank.held(owner, "LOCK_ADMISSION")
            assert mine.dispute_locked == bank.held(owner, "LOCK_CONTEST")
        assert (balances.paid, balances.burned) == (bank.paid, bank.burned)
        # Only units the earning owner still holds free can ever become weight.
        weight = [(row["owner"], row["hotkey"], row["units"]) for row in escrow.payable(20)]
        assert weight == (
            [(bank.cold, li_escrow.HOT.ss58, bank.available[bank.cold])]
            if bank.available[bank.cold]
            else []
        )
        escrow.verify()

    check()
    accepted: list[tuple[Plan, object]] = []
    for index, step in enumerate(sequence):
        before, count = fingerprint(escrow), events(escrow)
        if step.kind == "replay":
            # An accepted operation sent again returns its receipt and moves nothing.
            if accepted:
                again, receipt = accepted[step.pick % len(accepted)]
                assert again.call() == receipt
                assert fingerprint(escrow) == before
            continue
        plan = bank.plan(step, f"{index + 1:064x}")
        if plan is None:
            continue
        # When
        if plan.code is None:
            accepted.append((plan, plan.call()))
            plan.apply()
            assert events(escrow) == count + 1
        else:
            assert error_code(plan.call) == plan.code
            assert fingerprint(escrow) == before
        # Then
        check()
    # A file-backed reopen reproduces the balances (the chain and the last state both verify).
    with tempfile.TemporaryDirectory() as directory:
        path = Path(directory) / "ledger.db"
        copy = sqlite3.connect(path, isolation_level=None)
        escrow.db.backup(copy)
        copy.close()
        reopened = li_escrow.ledger(setup, path, replay=known_replay(li_escrow))
        for owner in (None, bank.cold, bank.other):
            assert reopened.balances(owner) == escrow.balances(owner)
        reopened.db.close()


# --------------------------------------------------------------------- cost (ESC-23)


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason=(
        "ESC-23: the storage one escrow event adds does not grow with the ledger's unit rows (the "
        "lot encodes the state faster but still stores a full snapshot per event; ADR 0014 4.6, an "
        "event hash over a digest with checkpoints, awaits maintainer approval) "
        "[fix-lot: decision (product choice pending)]"
    ),
)
def test_the_storage_one_event_adds_does_not_grow_with_the_ledger(li_escrow: ModuleType) -> None:
    transfers = 20

    def added_by_transfers(extra_rows: int) -> tuple[int, int]:
        fx = funded(li_escrow)
        escrow, cold = fx.escrow, li_escrow.COLD.ss58
        for index in range(extra_rows):  # every one-unit lock is one more unit row
            lock = lock_op(
                li_escrow, fx.cold, operation=f"{index + 1:064x}", units=lock_floor(escrow)
            )
            escrow.lock(lock, signer=cold)

        def stored() -> int:  # the database in bytes, whichever tables hold the events
            pages = escrow.db.execute("PRAGMA page_count").fetchone()[0]
            size = escrow.db.execute("PRAGMA page_size").fetchone()[0]
            return int(pages * size)

        before, count = stored(), events(escrow)
        for index in range(transfers):  # to another owner: SELF_TRANSFER refuses oneself
            operation = f"ab{index:062x}"
            escrow.transfer(
                transfer_op(li_escrow, fx.cold, operation=operation, units=1), signer=cold
            )
        assert events(escrow) == count + transfers  # the scenario reaches the point under test
        rows = escrow.db.execute("SELECT COUNT(*) FROM escrow_units").fetchone()[0]
        return stored() - before, int(rows)

    (small, few), (large, many) = added_by_transfers(2), added_by_transfers(80)
    assert many > few
    assert large <= 2 * small, (
        f"{transfers} events stored {small} bytes at {few} rows, {large} at {many}"
    )


def lookup_ticks(li_escrow: ModuleType, foreign_locks: int, *, same_owner: bool) -> int:
    """Database work (ticks of 10 SQLite instructions) of the stake lookup of COLD's admission
    `a1..` once `foreign_locks` locks of another admission exist, held by COLD or by OTHER."""
    fx = funded(li_escrow)
    escrow, cold = fx.escrow, li_escrow.COLD.ss58
    holder, origin = (cold, fx.cold) if same_owner else (li_escrow.OTHER.ss58, fx.other)
    mine, theirs = "a1" * 32, "b2" * 32
    unit = lock_floor(escrow)
    escrow.lock(
        lock_op(li_escrow, fx.cold, operation="c1" * 32, units=unit, reference=mine), signer=cold
    )
    for index in range(foreign_locks):
        lock = lock_op(
            li_escrow,
            origin,
            operation=f"{index + 2:064x}",
            units=unit,
            reference=theirs,
            owner=holder,
        )
        escrow.lock(lock, signer=holder)
    ticks = [0]

    def tick() -> int:
        ticks[0] += 1
        return 0

    escrow.db.set_progress_handler(tick, 10)  # one tick per 10 SQLite VM instructions
    try:
        units, _ = escrow.locked(mine, cold)
    finally:
        escrow.db.set_progress_handler(None, 0)
    assert units == unit  # the lookup answers, whatever it costs
    return ticks[0]


def test_the_database_work_of_a_stake_lookup_does_not_grow_with_the_locks_of_other_owners(
    li_escrow: ModuleType,
) -> None:
    small = lookup_ticks(li_escrow, 2, same_owner=False)
    large = lookup_ticks(li_escrow, 80, same_owner=False)
    assert large <= 2 * small + 10, f"one lookup: {small} ticks at 2 foreign locks, {large} at 80"


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason=(
        "ESC-23: the database work of a stake lookup does not grow with the locks the same owner "
        "holds for other admissions (ADR 0014 4.1 indexes the lookup by owner and bounds it by the "
        "cap of 2,048 open locks per coldkey, so its cost grows with the owner's open locks up to "
        "that cap; a flat cost is more than the ADR promises and no lot designs it) "
        "[fix-lot: none (no design yet)]"
    ),
)
def test_the_database_work_of_a_stake_lookup_does_not_grow_with_other_admissions_locks(
    li_escrow: ModuleType,
) -> None:
    small = lookup_ticks(li_escrow, 2, same_owner=True)
    large = lookup_ticks(li_escrow, 120, same_owner=True)
    assert large <= 2 * small + 10, f"one lookup: {small} ticks at 2 foreign locks, {large} at 120"


# ------------------------------------------------------------------ recipient (ES-07)

UNDECODABLE = "5" * 48


def test_a_transfer_to_a_recipient_that_is_no_decodable_key_is_refused(
    li_escrow: ModuleType,
) -> None:
    """ES-07: a transfer to a well-formed recipient that is no decodable key is refused. The rule
    comes after the replay lookup: one that a chain recorded before it still replays."""
    # Given: COLD's matured units, and a first transfer to a real key as the control.
    fx = funded(li_escrow)
    escrow, cold = fx.escrow, li_escrow.COLD.ss58
    escrow.transfer(transfer_op(li_escrow, fx.cold, operation="31" * 32), signer=cold)
    assert escrow.balances(li_escrow.OTHER.ss58).available == OTHER_UNITS + 500
    with pytest.raises(KeyError_):
        decode_hotkey(UNDECODABLE)  # matches the SS58 shape, yet no key signs for it
    before = fingerprint(escrow)
    # When
    gift = transfer_op(li_escrow, fx.cold, operation="32" * 32, recipient=UNDECODABLE)
    code = error_code(lambda: escrow.transfer(gift, signer=cold))
    # Then: the units stay with their owner instead of going to an owner nobody can sign for.
    assert code == "UNKNOWN_RECIPIENT"
    assert fingerprint(escrow) == before
    # And: the same transfer written as the ledger wrote it before the rule keeps its receipt.
    with escrow.tx():
        escrow._move(gift, "available", "available", recipient=gift.recipient, skip_empty=True)
        written = escrow._event(
            gift.operation_id, "TRANSFER", gift.model_dump_json(), gift.owner, server=False
        )
    assert escrow.transfer(gift, signer=cold) == written
    escrow.verify()


def test_a_transfer_goes_only_to_a_key_the_ledger_already_holds(li_escrow: ModuleType) -> None:
    """ES-07 (ADR 0014 5.2): a hop to a key no issuance named is refused, so fresh keys cannot
    multiply a per-key quota; a key whose rows are all empty is still a key the ledger holds."""
    # Given: OTHER gave every unit it had to COLD, and STRANGER was never issued anything.
    fx = funded(li_escrow)
    escrow, cold, other = fx.escrow, li_escrow.COLD.ss58, li_escrow.OTHER.ss58
    back = transfer_op(
        li_escrow, fx.other, operation="33" * 32, units=OTHER_UNITS, owner=other, recipient=cold
    )
    escrow.transfer(back, signer=other)
    assert escrow.balances(other).available == 0
    before = fingerprint(escrow)
    # When: COLD hops to the fresh key
    hop = transfer_op(li_escrow, fx.cold, operation="34" * 32, recipient=STRANGER)
    # Then: refused, nothing moved; the same units still go to OTHER, which holds empty rows only
    assert error_code(lambda: escrow.transfer(hop, signer=cold)) == "UNKNOWN_RECIPIENT"
    assert fingerprint(escrow) == before
    escrow.transfer(transfer_op(li_escrow, fx.cold, operation="35" * 32), signer=cold)
    assert escrow.balances(other).available == 500


def test_a_row_held_by_a_string_nobody_can_sign_for_does_not_make_it_a_recipient(
    li_escrow: ModuleType,
) -> None:
    """ES-07: the recipient must also decode as a public key, even where an issuance named it."""
    # Given: a genesis that also issued one unit to the undecodable string
    cold = li_escrow.COLD.ss58
    allocation = [(cold, 20000, 1), (li_escrow.OTHER.ss58, OTHER_UNITS, 1), (UNDECODABLE, 1, 1)]
    setup = repinned(li_escrow.setup(), genesis_allocation_hash=genesis_allocation_hash(allocation))
    escrow = bare_escrow(li_escrow, setup)
    genesis = genesis_request(li_escrow, escrow.run_id, allocation)
    escrow.test_genesis(genesis)
    escrow.mature(H, tuple(origin.origin_id for origin in genesis.origins), 1)
    assert escrow.balances(UNDECODABLE).available == 1
    before = fingerprint(escrow)
    # When
    gift = transfer_op(
        li_escrow, genesis.origins[0].origin_id, operation="36" * 32, recipient=UNDECODABLE
    )
    # Then
    assert error_code(lambda: escrow.transfer(gift, signer=cold)) == "UNKNOWN_RECIPIENT"
    assert fingerprint(escrow) == before
