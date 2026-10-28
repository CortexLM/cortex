"""Expiry and finality: crediting a round must not hang on a value one signer chose.

Every test states a defence the protocol must hold. A test marked xfail is a hole whose body is
the minimal reproducer asserting the DESIRED defence; its CONTROL (a passing assertion or a
sibling test) shows that the scenario reaches the point under test. A control that stops
holding raises ControlFailed: the scenario changed and the test is to be rewritten, which is
neither a pass nor the expected failure.

A "[candidate]" in a reason marks a hole whose desired behaviour is the Lens' reading or one of
several valid remedies; the other holes are reproduced defects asserted on an observable outcome,
and a test that presumes one remedy says so below. The reason of every xfail ends with
[fix-lot: <tag>]: the lot whose merge flips it (the marker is then removed and nothing else
changes), or "none (no design yet)" / "decision (product choice pending)".

CRD-15, A2W-17: the finalize judges the stored commit and verdict envelopes at the beacon where
the service accepted them (ADR 0014 part 3), so a miner or an auditor that sealed a short expiry
cannot freeze the round. The ledger-level test shows the rule in milliseconds; the end-to-end
test (four members really trained and audited, 30 s) holds for any change that lets the round
finalize. A change that refuses the short commit at intake makes its control fail
(ControlFailed), which is the signal to rewrite it. The audit queue half of A2W-17 (a
commit that expired before the lease must not stop the other members' audits) is closed on v3
by ADR 0011 part 2 and pinned by tests/challenge/test_audit_liveness_v2.py.
A2W-16: a round wider than one audit lease stays auditable to its last beacon (closed on v3 by
ADR 0011 part 2, decision 6: an attempt is dated by the round it starts in). FIN-05: a replay of a
finalize returns the stored result while its envelope is live. FIN-23: a stale beacon refuses
a round write with 503 and leaves nothing behind. BND-16: signed genesis, bootstrap and reward
bodies belong to one run.
"""

from __future__ import annotations

import json
from dataclasses import dataclass, replace
from pathlib import Path
from types import ModuleType
from typing import Any

import pytest

from hypertrain.ledger.escrow_v2 import (
    FinalityEvidence,
    authority_message,
    genesis_allocation_hash,
    genesis_origin_id,
)
from hypertrain.protocol.envelope import ExpiredError, RunMismatchError
from hypertrain.protocol.envelope_v2 import EnvelopeV2, parse_envelope, seal
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages import Finalize
from hypertrain.protocol.messages_v2 import OriginAllocation, RunManifestV2, TestGenesis

from .support import (
    Real,
    World,
    bare_escrow,
    error_code,
    load_fixture_module,
    memory_db,
    service_world,
)
from .support_expiry import (
    OFFSETS,
    ControlFailed,
    activate,
    audited,
    control,
    files,
    hot,
    keyed_id,
    last_audit_beacon,
    lease_request,
    open_round,
    post_accept,
    post_commit,
    prepared_write,
    service,
    signed_bootstrap,
    stale,
    tables,
    touched,
)

ISSUED = 1_000_000
NARROW = (1, 2, 10, 11, 12, 111)  # d_final - d_audit = 100, one lease of a tiny model (H <= 30)
WIDE = (1, 2, 10, 11, 12, 150)  # d_final - d_audit = 139


# --- CRD-15, A2W-17: an expiry a signer chose after admission -----------------------------------


def reseal(fixture: ModuleType, real: Real, which: str, expiry: int) -> FinalityEvidence:
    """The evidence of `real` with the commit or the verdict sealed again to expire at `expiry`."""
    setup, evidence, _ = real
    item = evidence.works[0]
    key, kind = {
        "commit": (fixture.HOT, "CommitV2"),
        "verdict": (fixture.AUDITOR, "ReplayVerdict"),
    }[which]
    run_id = setup.manifest.run_id()
    envelope = EnvelopeV2.model_validate(seal(key, kind, run_id, getattr(item, which).body, expiry))
    return replace(evidence, works=(replace(item, **{which: envelope}),))


@pytest.mark.parametrize("which", ["commit", "verdict"])
def test_an_expiry_that_lapsed_after_acceptance_cannot_block_the_reward(
    li_escrow: ModuleType, real_reward: Real, which: str
) -> None:
    setup, _, reward = real_reward
    finalized = real_reward[1].finalized_beacon
    # Control: sealed to expire at the finalize beacon itself, the same reward is issued.
    live = bare_escrow(li_escrow, setup)
    try:
        live.reward_finalize(reward, reseal(li_escrow, real_reward, which, finalized))
    except ExpiredError as error:  # a refusal here is a changed scenario, not the call under test
        raise ControlFailed("an expiry equal to the finalize beacon is already refused") from error
    control(live.balances().issued == ISSUED, "the reward is not issued with a live expiry")
    # One round shorter: every beacon the store could have taken the envelope at is earlier.
    lapsed = bare_escrow(li_escrow, setup)
    lapsed.reward_finalize(reward, reseal(li_escrow, real_reward, which, finalized - 1))
    assert lapsed.balances().issued == ISSUED


def test_a_round_whose_only_fault_is_one_short_lived_commit_still_finalizes(
    tmp_path: Path,
) -> None:
    rounds = load_fixture_module("li_rollback_service", "e2e/test_rollback_service_v2.py")
    with service_world(tmp_path) as world:
        network = world.network
        members = [hot(i) for i in range(4)]
        activate(world, 4)
        lapses = network.now + 31  # d_audit of the opening `execute_round` signs
        sign = network.signed

        def signed(key: Any, kind: str, body: Any, expiry: int = 10_000) -> Any:
            short = kind == "CommitV2" and key.ss58 == members[0].ss58
            return sign(key, kind, body, lapses if short else expiry)

        network.signed = signed
        # Four members train, publish, are audited MATCH and are aggregated: all answered 200.
        # execute_round asserts each answer; one of them failing is a changed scenario (a commit
        # refused at intake, say), not the hole this test is about.
        try:
            applied = rounds.execute_round(network, 0, hotkeys=members)
        except AssertionError as error:
            raise ControlFailed(
                f"the round with a short-lived commit did not run: {error}"
            ) from error
        control(network.now == lapses, "execute_round no longer ends at d_audit")
        network.push(network.now + 1)
        with world.store._lock:
            lapsed = sorted(
                (row["kind"], row["id"])
                for row in world.store._db.execute(
                    "SELECT kind,id,data FROM records_v2 WHERE kind IN ('commit','delta','verdict')"
                )
                if json.loads(row["data"])["exp_drand"] < network.now
            )
        control(lapsed == [("commit", f"0:{members[0].ss58}")], f"expired envelopes: {lapsed}")
        final = Finalize(
            w=0,
            final_theta_hash_w1=applied["theta_hash"],
            included=sorted(key.ss58 for key in members),
            entitlements_root="0" * 64,
        )
        response = network.client.post(
            network.url + "/admin/rounds/0/finalize",
            json=sign(service().COORD, "Finalize", final),
            headers=world.admin,
        )
        assert response.status_code == 200, response.text


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason=(
        "CRD-15+CRD-D02: the store accepts a commit whose expiry is before d_final, which no "
        "finalize at or after d_final could re-read; refusing it at intake is the Lens "
        "remedy that ADR 0014 part 3 declines once evidence is judged at acceptance [candidate] "
        "[fix-lot: decision (product choice pending)]"
    ),
)
def test_a_commit_that_cannot_outlive_the_round_is_refused_at_intake(tmp_path: Path) -> None:
    with service_world(tmp_path) as world:
        activate(world, 4)
        rnd, opened = open_round(world, [0, 1, 2, 3], OFFSETS)
        control(opened.status_code == 200, f"open: {opened.text}")
        world.network.push(rnd.d_assign)
        control(post_commit(rnd, 1, rnd.d_final).status_code == 200, "a d_final expiry is refused")
        control(post_accept(rnd, 0).status_code == 200, "accept")
        before = tables(world)
        short = post_commit(rnd, 0, rnd.d_final - 1, accepted=True)
        assert not short.is_success
        assert touched(before, tables(world)) == []


# --- A2W-16: a round the store opened must be auditable -----------------------------------------


def test_a_round_whose_window_fits_one_lease_is_auditable_up_to_its_last_beacon(
    tmp_path: Path,
) -> None:
    with service_world(tmp_path) as world:
        rnd = audited(world, NARROW)
        last = last_audit_beacon(world)
        world.network.push(last)
        leased = lease_request(world, service().AUDITORS[0])
        assert leased.status_code == 200, leased.text
        assert leased.json()["body"]["lease_expires"] == last + 1 == rnd.d_final


def test_a_round_wider_than_one_lease_is_not_left_unauditable(tmp_path: Path) -> None:
    with service_world(tmp_path) as world:
        audited(world, WIDE)  # control: the narrow twin above leases at its last beacon
        last = last_audit_beacon(world)
        world.network.push(last)
        leased = lease_request(world, service().AUDITORS[0])
        assert leased.status_code == 200, leased.text
        job = leased.json()["body"]
        # An attempt is dated by the round it starts in (ADR 0011 part 2), not by its row.
        assert (job["created_beacon"], job["lease_expires"]) == (last, last + 1)


# --- FIN-23: a stale beacon refuses a round write and leaves nothing behind -----------------------


@pytest.mark.parametrize("route", ["open", "accept", "commit", "audits", "aggregate", "lease"])
def test_a_stale_beacon_refuses_a_round_write_with_503_and_leaves_no_trace(
    tmp_path: Path, route: str
) -> None:
    with service_world(tmp_path) as world:
        send = prepared_write(world, route)
        stale(world, 3)
        before, written = tables(world), files(world)
        refused = send()
        assert (refused.status_code, refused.json()["detail"]) == (
            503,
            "beacon infrastructure paused",
        )
        assert touched(before, tables(world)) == [] and files(world) == written
        # Control: the same request is not a 503 once the beacon is fresh again.
        stale(world, 0)
        resumed = send()
        assert resumed.status_code == (409 if route == "aggregate" else 200), resumed.text


# --- FIN-05: a finalize replay returns the stored result while its envelope is live ---------------


FINAL = Finalize(
    w=0, final_theta_hash_w1="aa" * 32, included=[hot(0).ss58], entitlements_root="bb" * 32
)
LIVE_FOR = 3


@dataclass
class Settled:
    """A hosted run whose round 0 was finalized once: the two records the first call leaves."""

    world: World
    envelope: dict[str, Any]
    result: dict[str, Any]
    expiry: int

    def finalize(self, envelope: dict[str, Any] | None = None) -> Any:
        network = self.world.network
        return network.client.post(
            network.url + "/admin/rounds/0/finalize",
            json=self.envelope if envelope is None else envelope,
            headers=self.world.admin,
        )

    def sign(self, final: Finalize = FINAL, expiry: int | None = None) -> dict[str, Any]:
        signed: dict[str, Any] = self.world.network.signed(
            service().COORD, "Finalize", final, expiry=self.expiry if expiry is None else expiry
        )
        return signed


def settled(world: World) -> Settled:
    """Seed what `finalize_v2` stores at its first success: a full round is 30 s of training and
    audit, and the replay branch reads exactly these two records once the round is open.

    The first success also reserves the signed operation (a nonce row); the first call of a test
    creates it here, so a test that sends another body first reaches the stored-body comparison
    on its own, which a real first finalize makes a second line of defence."""
    network = world.network
    activate(world, 4)
    _, opened = open_round(world, [0, 1, 2, 3], OFFSETS)
    control(opened.status_code == 200, f"open: {opened.text}")
    expiry = network.now + LIVE_FOR
    result = {"status": "SETTLED", "reward_receipt": {"event": "first"}, "theta_hash": "aa" * 32}
    envelope = network.signed(service().COORD, "Finalize", FINAL, expiry=expiry)
    with world.store._tx():
        stored = parse_envelope(canonicalize(envelope)).model_dump(mode="json")
        world.store._put_record_v2(world.run_id, "finalize", "0", stored)
        world.store._put_record_v2(world.run_id, "finalize-result", "0", result)
    return Settled(world, envelope, result, expiry)


def test_an_identical_finalize_replay_returns_the_stored_result_and_changes_nothing(
    tmp_path: Path,
) -> None:
    with service_world(tmp_path) as world:
        run = settled(world)
        first = run.finalize()
        assert (first.status_code, first.json()) == (200, run.result)
        after_first = tables(world)
        again = run.finalize()
        assert (again.status_code, again.json()) == (200, run.result)
        # A fresh signature of the same body: signatures are randomized, the semantics are not.
        resigned = run.finalize(run.sign())
        assert (resigned.status_code, resigned.json()) == (200, run.result)
        assert touched(after_first, tables(world)) == []


def test_a_finalize_replay_past_its_expiry_is_refused_unless_it_is_signed_again(
    tmp_path: Path,
) -> None:
    with service_world(tmp_path) as world:
        run = settled(world)
        assert run.finalize().status_code == 200  # the condition holds: the envelope is live
        world.network.push(run.expiry + 1)
        before = tables(world)
        late = run.finalize()
        assert (late.status_code, late.json()["detail"]) == (409, "envelope expired")
        assert touched(before, tables(world)) == []
        renewed = run.finalize(run.sign(expiry=run.expiry + 50))
        assert (renewed.status_code, renewed.json()) == (200, run.result)


@pytest.mark.parametrize(
    ("replayed_first", "detail"),
    [
        (False, "conflicting finalized origin authority"),
        (True, "conflicting signed operation nonce"),
    ],
)
def test_a_finalize_with_other_content_is_refused_and_the_stored_result_stands(
    tmp_path: Path, replayed_first: bool, detail: str
) -> None:
    other = FINAL.model_copy(update={"entitlements_root": "cc" * 32})
    with service_world(tmp_path) as world:
        run = settled(world)
        if replayed_first:
            assert run.finalize().status_code == 200
        before = tables(world)
        refused = run.finalize(run.sign(other))
        assert (refused.status_code, refused.json()["detail"]) == (409, detail)
        assert touched(before, tables(world)) == []
        assert run.finalize().json() == run.result


# --- BND-16: signed genesis, bootstrap and reward bodies belong to one run -----------------------


def moved_run(setup: Any) -> Any:
    """The same setup under a manifest that differs by its beacon genesis: another run id."""
    body = setup.manifest.body()
    body["training"]["beacon"]["genesis_time"] += 1
    other = replace(setup, manifest=RunManifestV2.model_validate(body))
    control(other.manifest.run_id() != setup.manifest.run_id(), "the manifest did not move")
    return other


def test_a_signed_reward_of_one_run_is_refused_by_the_ledger_of_another(
    li_escrow: ModuleType, real_reward: Real
) -> None:
    setup, evidence, reward = real_reward
    own, other = bare_escrow(li_escrow, setup), bare_escrow(li_escrow, moved_run(setup))
    own.reward_finalize(reward, evidence)
    control(own.balances().issued == ISSUED, "the run's own reward is not issued")
    assert error_code(lambda: other.reward_finalize(reward, evidence)) == "ORIGIN_AUTHORITY"
    assert (other.balances().issued, other.balances().reward_pending) == (0, 0)


def signed_genesis(fixture: ModuleType, escrow: Any) -> TestGenesis:
    run_id = escrow.run_id
    allocation = [(fixture.COLD.ss58, 20000, 1), (fixture.OTHER.ss58, 10000, 1)]
    digest = genesis_allocation_hash(allocation)
    unsigned = TestGenesis(
        run_id=run_id,
        allocation_hash=digest,
        total_units=30000,
        origins=[
            OriginAllocation(
                origin_id=keyed_id(genesis_origin_id, escrow, run_id, digest, i),
                owner=h,
                units=u,
                mature_at=m,
            )
            for i, (h, u, m) in enumerate(allocation)
        ],
        authority_sig="0" * 128,
    )
    signature = fixture.COORD.sign(authority_message(unsigned)).hex()
    return TestGenesis.model_validate({**unsigned.body(), "authority_sig": signature})


def test_a_signed_genesis_of_one_run_is_refused_by_the_ledger_of_another(
    li_escrow: ModuleType,
) -> None:
    setup = li_escrow.setup()
    own, other = bare_escrow(li_escrow, setup), bare_escrow(li_escrow, moved_run(setup))
    genesis = signed_genesis(li_escrow, own)
    own.test_genesis(genesis)
    control(own.balances().issued == 30000, "the run's own genesis is not issued")
    assert error_code(lambda: other.test_genesis(genesis)) == "ORIGIN_AUTHORITY"
    assert other.balances().issued == 0


def test_a_signed_bootstrap_of_one_run_is_refused_by_the_ledger_of_another() -> None:
    fixture = load_fixture_module("li_bootstrap_fixture", "ledger/test_production_bootstrap_v2.py")
    setup = fixture.pinned(fixture.E.setup("production"))
    own = fixture.E.ledger(setup, memory_db())
    other = fixture.E.ledger(moved_run(setup), memory_db())
    bootstrap = fixture.bootstrap(own.run_id)
    own.production_bootstrap(bootstrap)
    control(own.balances().issued == 30000, "the run's own bootstrap is not issued")
    assert error_code(lambda: other.production_bootstrap(bootstrap)) == "ORIGIN_AUTHORITY"
    assert other.balances().issued == 0


def issued(store: Any) -> int:
    """Units issued in a hosted store; a ledger that was never served has issued none."""
    if not store._db.execute("SELECT 1 FROM sqlite_master WHERE name='escrow_origins'").fetchone():
        return 0
    return int(
        store._db.execute("SELECT COALESCE(SUM(issued),0) FROM escrow_origins").fetchone()[0]
    )


@pytest.fixture
def hosted_stores() -> Any:
    opened: list[Any] = []
    yield opened.append
    for store in opened:
        store.close_v2_notifications()
        store.close()


def test_a_production_bootstrap_envelope_of_one_run_is_refused_by_the_store_of_another(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, hosted_stores: Any
) -> None:
    economics = load_fixture_module(
        "li_production_economics", "challenge/test_production_economics_v2.py"
    )
    # Two hosted production runs whose economics differ by one pinned number: two manifests.
    one, run_one = economics.hosted(tmp_path / "one", monkeypatch, cuda=True, G_max_units=1100)
    two, run_two = economics.hosted(tmp_path / "two", monkeypatch, cuda=True, G_max_units=1101)
    for store in (one, two):
        hosted_stores(store)
        economics.beacon(store, 1, bls=True)
    control(run_one != run_two, "the two hosted runs share a run id")
    raw_one = signed_bootstrap(economics, one, run_one)
    with pytest.raises(RunMismatchError, match="another run"):
        two.production_bootstrap_v2(run_two, raw_one)
    assert (issued(one), issued(two)) == (0, 0)
    # Control: each store issues its own bootstrap once.
    own_two = two.production_bootstrap_v2(run_two, signed_bootstrap(economics, two, run_two))
    assert own_two["ledger_mode"] == "production"
    assert one.production_bootstrap_v2(run_one, raw_one)["ledger_mode"] == "production"
    assert (issued(one), issued(two)) == (30000, 30000)
