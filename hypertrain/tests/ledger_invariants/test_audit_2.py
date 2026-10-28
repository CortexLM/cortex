"""What reaches the tape and the reward after the audits (D39, `audit`): exclusions, the open-audit
counter, late events, the weights, and the write-once records. Same world as test_audit.py.
"""

from __future__ import annotations

import json
from dataclasses import replace
from types import ModuleType
from typing import Any

import pytest
from hypothesis import given
from hypothesis import strategies as st

from hypertrain.aggregator.core import FinalityError, TapeError
from hypertrain.aggregator.tape_v2 import TapeV2
from hypertrain.aggregator.weighted_v2 import (
    Candidate,
    InsufficientEligibleWeight,
    allocate_weights,
)
from hypertrain.challenge.store import ChallengeError
from hypertrain.protocol.envelope import ExpiredError, UnauthorizedSigner, body_digest
from hypertrain.protocol.envelope_v2 import EnvelopeV2, seal
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import ReplayVerdict, Rollback, f32hex
from hypertrain.protocol.messages_v2 import AggregationPolicyV2, CommitV2, RosterEntryV2

from . import support_audit as sa
from .support import Real, bare_escrow, error_code
from .support_audit import detail, running, settled_state
from .test_audit import audit_world  # noqa: F401  (the session fixture of test_audit.py)

# --- A2W-01: a mismatch is an exclusion, never a credit ---------------------------------------


def test_a_mismatch_completes_as_an_exclusion_and_leaves_no_successor_state(
    world: sa.World,
) -> None:
    # Given: the forger's audit completed with the master's mismatch.
    with world.at("s3") as (n, cast):
        store, run = n.store, n.manifest.run_id()
        forger = cast.forger.ss58
        stored = store._record_v2(run, "executed-verdict", sa.lease_rows(store)[forger]["id"])
        # Then: it is recorded and excluded as unverified, with the master's verdict as evidence.
        assert store._record_v2(run, "replay", f"0:{forger}")["result"] == "MISMATCH"
        assert store._record_v2(run, "excluded", f"0:{forger}") == {
            "reason": "UNVERIFIED",
            "evidence_hash": body_digest(stored),
        }
        assert {i for kind, i in sa.records(store, "excluded")} == {f"0:{forger}"}
        # And no anchor (the state the next round starts from) exists for it, while each
        # matched member has one.
        anchors = {i for _, i in sa.records(store, "anchor")}
        assert f"0:{forger}" not in anchors
        assert {f"0:{h.ss58}" for h in cast.honest} <= anchors
        # And the working set of the round is the matched members only.
        inputs = store.verified_inputs_v2(run, 0)
        assert sorted(work.roster.hotkey for work in inputs) == sorted(h.ss58 for h in cast.honest)
        assert {work.replay.result for work in inputs} == {"MATCH"}


GATE_PINS = ("beacon_pins_v2",)


def gate_pins(store: Any) -> set[str]:
    """The operations the freshness gate holds a pinned beacon round for."""
    with store._lock:
        return {r[0] for r in store._db.execute("SELECT operation_id FROM beacon_pins_v2")}


@pytest.mark.parametrize(
    ("field", "value"),
    [
        pytest.param("result", "MISMATCH", id="not-a-match"),
        pytest.param("audit_mode", "sampled", id="not-a-full-audit"),
        pytest.param("leaves_root", "ab" * 32, id="not-the-committed-leaves"),
    ],
)
def test_the_tape_refuses_an_input_whose_replay_is_not_a_full_match_of_its_commit(
    world: sa.World, field: str, value: str
) -> None:
    # Given: a member whose replay record no longer says "full match of the commit" (what a
    # regression of the exclusion above would leave in the working set).
    with world.at("s3") as (n, cast):
        store, run = n.store, n.manifest.run_id()
        identity = f"0:{cast.honest[0].ss58}"
        record = store._record_v2(run, "replay", identity)
        with store._tx():
            store._put_record_v2(run, "replay", identity, {**record, field: value})
        before, pinned = sa.digest_of(store, without=GATE_PINS), gate_pins(store)
        # When / Then: the tape is not built, whatever the store let through.
        with pytest.raises(TapeError, match="own-assignment full replay not VERIFIED"):
            store.aggregate_v2(run, 0)
        assert sa.digest_of(store, without=GATE_PINS) == before
        assert ("applied", "0") not in sa.records(store)
        # The claim commits before the tape is computed, and with it the pin of the freshness gate
        # (`_aggregate_claim_v2`, CHANGELOG: aggregate without the store lock): the one row the
        # refused attempt keeps.
        assert gate_pins(store) - pinned == {f"aggregate:0:{n.now}"}


# --- A2W-10: a partly audited round does not aggregate or finalize -----------------------------


def test_a_round_with_an_unaudited_member_cannot_be_aggregated(world: sa.World) -> None:
    # Given: two audits done (the forger's, excluded, and the allowlisted miner's); the other
    # three members have a job but no verdict.
    with world.at("s2") as (n, cast):
        store, run = n.store, n.manifest.run_id()
        for row in running(store).values():
            executed = store._record_v2(run, "executed-verdict", row["id"])
            assert sa.complete(n, cast.keys[row["auditor"]], row["id"], executed).status_code == 200
        assert store._record_v2(run, "finality", "0")["unresolved_audits"] == 3
        before = sa.digest_of(store)
        # When
        response = n.client.post(n.url + "/admin/rounds/0/aggregate", headers=sa.admin())
        # Then: the first unaudited member of the roster stops the round, nothing is applied.
        assert detail(response) == (409, f"missing accepted replay: 0:{cast.hot[1].ss58}")
        assert sa.digest_of(store) == before
        assert ("applied", "0") not in sa.records(store)


def seed_finalize(n: Any, cast: sa.Cast, included: list[str]) -> dict[str, Any]:
    applied = n.store._record_v2(n.manifest.run_id(), "applied", "0")
    return sa.finalize_request(n, applied["theta_hash"], included)


def test_finalize_refuses_a_member_without_an_audit_record(world: sa.World) -> None:
    # Given: an aggregated round in which one matched member lost the audit verdict its reward is
    # judged on (the replay record is not the one deleted: a tree that pays the inputs of the
    # applied tape no longer reads it at finalize, while every tree still needs the verdict).
    with world.at("s4") as (n, cast):
        store, run = n.store, n.manifest.run_id()
        lost = cast.honest[1].ss58
        with store._tx():
            store._db.execute(
                "DELETE FROM records_v2 WHERE run_id=? AND kind='verdict' AND id=?",
                (run, f"0:{lost}"),
            )
        request = seed_finalize(n, cast, [h.ss58 for h in cast.honest])
        escrow = store._services(run)[0]
        balances, before = escrow.balances(), sa.digest_of(store)
        # When
        response = n.client.post(
            n.url + "/admin/rounds/0/finalize", json=request, headers=sa.admin()
        )
        # Then: refused, no round is settled and nothing is minted.
        assert detail(response) == (409, f"missing accepted verdict: 0:{lost}")
        assert sa.digest_of(store) == before
        assert escrow.balances() == balances
        assert ("finalize", "0") not in sa.records(store)


def test_a_round_cannot_settle_while_an_audit_is_counted_unresolved(world: sa.World) -> None:
    # Given: an aggregated round whose audit counter says one audit is still open.
    with world.at("s4") as (n, cast):
        store, run = n.store, n.manifest.run_id()
        status = store._record_v2(run, "finality", "0")
        with store._tx():
            store._put_record_v2(run, "finality", "0", {**status, "unresolved_audits": 1})
        request = seed_finalize(n, cast, [h.ss58 for h in cast.honest])
        escrow = store._services(run)[0]
        balances, before = escrow.balances(), sa.digest_of(store)
        # When / Then: every record is in place, yet the round cannot settle.
        with pytest.raises(FinalityError, match="round cannot settle"):
            store.finalize_v2(run, 0, sa.canonical(request))
        assert sa.digest_of(store) == before
        assert escrow.balances() == balances


def test_a_second_exclusion_leaves_too_few_members_to_aggregate(world: sa.World) -> None:
    # Given: all audits done, the forger excluded, and a second member excluded as well (a
    # test-owned stand-in for a second mismatch): three members would carry the round.
    with world.at("s3") as (n, cast):
        store, run = n.store, n.manifest.run_id()
        second = cast.honest[0].ss58
        with store._tx():
            store._put_record_v2(
                run, "excluded", f"0:{second}", {"reason": "UNVERIFIED", "evidence_hash": "ab" * 32}
            )
        before = sa.digest_of(store)
        # When / Then: three miners cannot carry one whole update, so nothing is aggregated; a tree
        # that checks the survivors says so by name before any weight is allocated.
        if sa.CHECKS_SURVIVORS:
            with pytest.raises(ChallengeError, match="ROUND_INFEASIBLE") as refused:
                store.aggregate_v2(run, 0)
            assert refused.value.status == 409
            assert isinstance(refused.value.__cause__, InsufficientEligibleWeight)
        else:
            with pytest.raises(InsufficientEligibleWeight, match="INSUFFICIENT_ELIGIBLE_WEIGHT"):
                store.aggregate_v2(run, 0)
        assert sa.digest_of(store) == before
        assert ("applied", "0") not in sa.records(store)


# --- A2W-11: an excluded member is out of the tape, the included set and the origins ----------


def test_an_excluded_member_is_absent_from_the_tape_the_included_set_and_the_origins(
    world: sa.World,
) -> None:
    # Given: the aggregated round: the forger was excluded by its audit.
    with world.at("s4") as (n, cast):
        store, run = n.store, n.manifest.run_id()
        honest = sorted(h.ss58 for h in cast.honest)
        applied = store._record_v2(run, "applied", "0")
        tape = TapeV2.from_bytes(store.objects.get(applied["tape_hash"]))
        # Then: the tape weighs the four matched members equally and does not know the forger.
        assert [i.roster.hotkey for i in tape.body.inputs] == honest
        assert [(e.hotkey, e.weight_units) for e in tape.body.allocation.entries] == [
            (h, 4194304) for h in honest
        ]
        # When: the coordinator signs a Finalize that lists the forger too.
        request = seed_finalize(n, cast, [*honest, cast.forger.ss58])
        escrow = store._services(run)[0]
        balances, before = escrow.balances(), sa.digest_of(store)
        response = n.client.post(
            n.url + "/admin/rounds/0/finalize", json=request, headers=sa.admin()
        )
        # Then: refused for its included set; nothing minted.
        assert detail(response) == (
            422,
            "finalized included set differs from full accepted authority",
        )
        assert sa.digest_of(store) == before and escrow.balances() == balances


def test_a_finalized_round_pays_the_matched_members_and_not_the_excluded_one(
    world: sa.World,
) -> None:
    # Given: the finalized round.
    with world.at("s5") as (n, cast):
        store, run = n.store, n.manifest.run_id()
        final_hash = store._record_v2(run, "replayed-finality", "0")["finalize_hash"]
        escrow = store._services(run)[0]
        # Then: one reward origin per matched member, none for the forger, and the round budget
        # is spent in full across them.
        origins = {
            row["origin"]: (row["owner"], row["units"])
            for row in store._db.execute(
                "SELECT o.origin,u.owner,u.units FROM escrow_origins o JOIN escrow_units u "
                "ON u.origin=o.origin WHERE u.bucket='reward_pending' AND u.units>0"
            )
        }
        share = escrow.policy.round_reward_units // len(cast.honest)
        owners = {
            sa.reward_origin(run, h.ss58, final_hash, escrow=escrow): (n_owner(cast, h), share)
            for h in cast.honest
        }
        assert origins == owners
        forger_origin = sa.reward_origin(run, cast.forger.ss58, final_hash, escrow=escrow)
        assert forger_origin not in origins
        assert escrow.balances().reward_pending == escrow.policy.round_reward_units
        assert escrow.balances().conserved()


def n_owner(cast: sa.Cast, hotkey: Any) -> str:
    return cast.cold[cast.hot.index(hotkey)].ss58


# --- A2W-12: the counter of open audits ---------------------------------------------------------


def open_audits(store: Any) -> int:
    return sum(r["state"] in ("queued", "running") for r in sa.lease_rows(store).values())


@pytest.mark.parametrize(("stage", "expected"), [("s1", 5), ("s2", 5), ("s3", 0)])
def test_the_audit_counter_equals_the_jobs_not_yet_terminal(
    world: sa.World, stage: str, expected: int
) -> None:
    # Given: the round at a stage of its audits (queued; two executed leases; all completed).
    with world.at(stage) as (n, _):
        counter = n.store._record_v2(n.manifest.run_id(), "finality", "0")["unresolved_audits"]
        # Then: the counter is the number of jobs that are neither completed nor exhausted.
        assert counter == open_audits(n.store) == expected


def test_each_job_decrements_the_counter_once_whether_it_completes_or_is_exhausted(
    world: sa.World, monkeypatch: pytest.MonkeyPatch
) -> None:
    # Given: two executed leases, three jobs still queued.
    with world.at("s2") as (n, cast):
        store, run = n.store, n.manifest.run_id()
        sa.without_reaper(monkeypatch, store)

        def counter() -> int:
            return int(store._record_v2(run, "finality", "0")["unresolved_audits"])

        held = running(store)
        # When: both complete.
        for row in held.values():
            stored = store._record_v2(run, "executed-verdict", row["id"])
            assert sa.complete(n, cast.keys[row["auditor"]], row["id"], stored).status_code == 200
            assert counter() == open_audits(store)
        assert counter() == 3
        # And one of the three runs out of windows first (a test-owned stand-in: the jobs of a
        # round share one deadline, so the counter could not be read between two exhaustions).
        first = next(r for h, r in sa.lease_rows(store).items() if h not in held)
        with store._tx():
            store._db.execute(
                "UPDATE audit_leases_v2 SET absolute=? WHERE id=?", (n.now + 5, first["id"])
            )
        sa.drop_job(n, store, first["hotkey"])
        assert (counter(), open_audits(store)) == (2, 2)
        assert store._record_v2(run, "excluded", f"0:{first['hotkey']}") == {
            "reason": "INFRASTRUCTURE",
            "evidence_hash": sha256_hex(first["id"].encode()),
        }
        # And the windows of the two that were never leased run out.
        sa.run_out_the_windows(n, store)
        assert (counter(), open_audits(store)) == (0, 0)
        after = settled_state(store)
        # Then: they are exhausted and excluded for the infrastructure, and a second sweep (or a
        # late one) decrements nothing again.
        queued = [r for h, r in sa.lease_rows(store).items() if h not in held]
        assert {r["state"] for r in queued} == {"exhausted"}
        for row in queued:
            assert store._record_v2(run, "excluded", f"0:{row['hotkey']}") == {
                "reason": "INFRASTRUCTURE",
                "evidence_hash": sha256_hex(row["id"].encode()),
            }
        n.push(n.now + 7)
        assert settled_state(store) == after


def test_without_scheduled_audits_the_counter_stays_at_the_roster_and_the_round_waits(
    world: sa.World, monkeypatch: pytest.MonkeyPatch
) -> None:
    # Given: round 0 committed, and nobody scheduled its audits (the condition of A2W-12 broken).
    with world.at("committed") as (n, cast):
        store, run = n.store, n.manifest.run_id()
        sa.without_reaper(monkeypatch, store)
        assert not sa.lease_rows(store)
        # When: time passes; there is no job to exhaust.
        n.push(n.now + 1000)
        # Then: the counter is still the whole roster, and no member can be aggregated.
        assert store._record_v2(run, "finality", "0")["unresolved_audits"] == len(cast.hot)
        response = n.client.post(n.url + "/admin/rounds/0/aggregate", headers=sa.admin())
        assert response.status_code == 409
        assert detail(response)[1].startswith("missing accepted ")
        assert ("applied", "0") not in sa.records(store)


# --- A2W-13: late events cannot change a settled or excluded round ----------------------------


def rollback_request(n: Any, cast: sa.Cast) -> dict[str, Any]:
    body = Rollback(
        w=0,
        excluded=[cast.forger.ss58],
        old_theta_hash_w2="ab" * 32,
        new_theta_hash_w2="cd" * 32,
        new_outer_state_hash="ef" * 32,
        recomputed=["agg_w", "step_w", "agg_w1", "step_w1"],
        cause_hashes=["12" * 32],
    )
    signed: dict[str, Any] = n.signed(sa.rollback_module().fixtures.COORD, "Rollback", body)
    return signed


def test_a_settled_round_is_untouched_by_late_audit_events(world: sa.World) -> None:
    # Given: the finalized round.
    with world.at("s5") as (n, cast):
        store, run = n.store, n.manifest.run_id()
        escrow = store._services(run)[0]
        keep = ("finality", "finalize", "finalize-result", "replayed-finality", "settlement")

        def minted() -> tuple[int, int]:
            return escrow.balances().issued, escrow.balances().burned

        before = (settled_state(store), sa.records(store, *keep), minted())
        assert store._record_v2(run, "finality", "0")["disposition"] == "SETTLED"
        # When: auditors ask for work, the audits are scheduled and completed again, the clock
        # runs far past every deadline and the coordinator asks to roll the round back.
        assert all(sa.lease(n, key).status_code == 204 for key in cast.auditors)
        assert sa.schedule(n).json() == {"jobs": []}
        for row in sa.lease_rows(store).values():
            stored = store._record_v2(run, "executed-verdict", row["id"])
            done = sa.complete(n, cast.keys[row["auditor"]], row["id"], stored)
            assert done.json() == {"result": row["result"]}
        n.push(n.now + 1000)
        refused = n.client.post(
            n.url + "/admin/rollback", json=rollback_request(n, cast), headers=sa.admin()
        )
        # Then: money-only, and not one record or job moved; the reward vested with time as it
        # would anyway, and nothing was issued or burned on account of the audits.
        assert detail(refused) == (409, "finalized late fraud is money-only")
        assert (settled_state(store), sa.records(store, *keep), minted()) == before


def test_after_the_last_window_nothing_late_can_change_the_dropped_round(
    world: sa.World, monkeypatch: pytest.MonkeyPatch
) -> None:
    # Given: two executed leases and three queued jobs when the last audit window passes.
    with world.at("s2") as (n, cast):
        store, run = n.store, n.manifest.run_id()
        sa.without_reaper(monkeypatch, store)
        held = running(store)
        stored = {h: store._record_v2(run, "executed-verdict", r["id"]) for h, r in held.items()}
        sa.run_out_the_windows(n, store)
        # Then: every job is exhausted with no audit open, and the round is no more settled than
        # before (a tree that voided the round for everybody says so in its disposition). The
        # forger's job is not dropped: nobody signed the master's MISMATCH, so its re-audit failed
        # (ADR 0011 Part 9) and it stays counted until the reaper retires the round.
        assert {r["state"] for r in sa.lease_rows(store).values()} == {"exhausted"}
        status = store._record_v2(run, "finality", "0")
        assert (status["disposition"], status["unresolved_audits"]) == (sa.DROP_DISPOSITION, 1)
        before = settled_state(store)
        # When: the holders deliver what they computed, ask again, the sweep runs again and
        # someone tries to schedule the audits once more.
        for hotkey, row in held.items():
            key = cast.keys[row["auditor"]]
            late = sa.complete(n, key, row["id"], stored[hotkey])
            assert detail(late) == (409, "audit result outside owned immutable lease")
            job = {"job_id": row["id"], "lease_nonce": row["nonce"]}
            assert detail(sa.execute(n, key, job)) == (403, "audit request lacks owned live lease")
        assert all(sa.lease(n, key).status_code == 204 for key in cast.auditors[2:])
        assert detail(sa.schedule(n)) == (503, "audit absolute horizon exhausted")
        n.push(n.now + 30)
        # Then: nothing changed.
        assert settled_state(store) == before


def test_a_rollback_waits_for_every_audit_of_both_rounds(world: sa.World) -> None:
    # Given: round 0 aggregated and round 1 recorded as applied with one audit still open (a
    # test-owned stand-in: a second real round costs minutes).
    with world.at("s4") as (n, cast):
        store, run = n.store, n.manifest.run_id()
        applied = store._record_v2(run, "finality", "0")
        with store._tx():
            store._put_record_v2(run, "round", "1", {"body": {"roster": []}})
            store._put_record_v2(
                run, "finality", "1", {**applied, "w": 1, "applied": True, "unresolved_audits": 1}
            )
        request = rollback_request(n, cast)
        before = sa.digest_of(store)
        # When / Then: refused for the open audit.
        refused = n.client.post(n.url + "/admin/rollback", json=request, headers=sa.admin())
        assert detail(refused) == (409, "rollback requires applied closed audit/dispute work")
        assert sa.digest_of(store) == before
        # Control: with the audit closed the same request gets past that check.
        with store._tx():
            store._put_record_v2(
                run, "finality", "1", {**applied, "w": 1, "applied": True, "unresolved_audits": 0}
            )
        later = n.client.post(n.url + "/admin/rollback", json=request, headers=sa.admin())
        assert detail(later) == (409, "missing accepted applied: 1")


def test_audits_cannot_be_scheduled_once_the_horizon_is_one_round_away(
    world: sa.World, monkeypatch: pytest.MonkeyPatch
) -> None:
    # Given: a committed round whose audits were never scheduled, one round before the horizon.
    with world.at("committed") as (n, cast):
        store = n.store
        sa.without_reaper(monkeypatch, store)
        horizon = store._record_v2(n.manifest.run_id(), "round", "0")["body"]["d_final"]
        n.push(horizon - 1)
        before = sa.digest_of(store)
        # When / Then: a job created now could not be replayed in time; none is created.
        assert detail(sa.schedule(n)) == (503, "audit absolute horizon exhausted")
        assert sa.digest_of(store) == before
        assert not sa.lease_rows(store)


# --- A2W-14: nothing a miner signs is a free parameter -----------------------------------------


POLICY = AggregationPolicyV2(
    arithmetic="flat-cclip-cap-v1",
    order="utf8",
    center="prev_outer_update",
    trust_mode="uniform-verified",
    weight_quantum=16777216,
    miner_cap_units=4194304,
    probation_cap_units=4194304,
    owner_group_caps={},
    preclip_norm=f32hex(1.0),
    cclip_tau=f32hex(1.0),
    cclip_iters=2,
)
ROSTER = [
    RosterEntryV2(
        hotkey=Keypair(bytes([200 + i]) * 32).ss58,
        slot=i,
        q_i=f32hex(1.0),
        admission_id=sha256_hex(f"admission-{i}".encode()),
        coldkey_group=f"owner{i}",
        state="ACTIVE",
        eligible_weight=4194304,
    )
    for i in range(5)
]
LABEL = st.text(alphabet="0123456789abcdef", min_size=64, max_size=64)


@given(
    labels=st.lists(st.tuples(LABEL, LABEL), min_size=5, max_size=5),
    order=st.permutations(range(5)),
)
def test_the_allocation_depends_only_on_the_roster_never_on_what_a_miner_signed(
    labels: list[tuple[str, str]], order: list[int]
) -> None:
    # Given: five members with equal ceilings, so one unit of the quantum cannot be split evenly
    # and goes to the first member in UTF-8 order.
    def weights(candidates: list[Candidate]) -> dict[str, int]:
        allocation = allocate_weights(candidates, POLICY)
        return {e.hotkey: e.weight_units for e in allocation.entries}

    baseline = weights([Candidate(entry, "0" * 64, "0" * 64) for entry in ROSTER])
    assert sum(baseline.values()) == 16777216
    # When: the commit and delta hashes the miners signed take any value, in any order.
    shuffled = [Candidate(ROSTER[i], *labels[i]) for i in order]
    # Then: each member gets the same weight.
    assert weights(shuffled) == baseline


def test_a_commit_cannot_declare_its_own_entitlement(world: sa.World) -> None:
    # Given: a miner's real commit, as stored.
    with world.at("s3") as (n, cast):
        manifest = n.manifest
        samples = manifest.training.batch_samples()
        body = n.store._record_v2(manifest.run_id(), "commit", f"0:{cast.honest[0].ss58}")["body"]
        commit = CommitV2.model_validate(body)
        commit.validate_assignment(manifest, samples)
        # When / Then: tokens, leaf count and sample count are the server's, not the miner's.
        inflated = commit.model_copy(update={"tokens": commit.tokens + 1})
        with pytest.raises(ValueError, match="tokens differ from server-derived entitlement"):
            inflated.validate_assignment(manifest, samples)
        padded = commit.model_copy(update={"n_leaves": commit.n_leaves + 1})
        with pytest.raises(ValueError, match="leaf count differs from manifest"):
            padded.validate_assignment(manifest, samples)
        with pytest.raises(ValueError, match="assignment sample count differs from manifest"):
            commit.validate_assignment(manifest, samples + 1)


# --- A2W-01, A2W-03, A2W-14: what the escrow credits -------------------------------------------


def with_verdict(
    li_escrow: ModuleType, real: Real, signer: Keypair, exp: int = 200, **changes: Any
) -> Any:
    """The real finality evidence, its one verdict replaced by `changes` and sealed by `signer`."""
    setup, evidence, _ = real
    item = evidence.works[0]
    body = ReplayVerdict.model_validate(item.verdict.body).model_dump(mode="json")
    for name, value in changes.items():
        body[name] = {**body[name], **value} if isinstance(value, dict) else value
    envelope = EnvelopeV2.model_validate(
        seal(
            signer,
            "ReplayVerdict",
            setup.manifest.run_id(),
            ReplayVerdict.model_validate(body),
            exp,
        )
    )
    return replace(evidence, works=(replace(item, verdict=envelope),))


def with_commit(li_escrow: ModuleType, real: Real, **changes: Any) -> Any:
    """The real finality evidence, its one commit replaced by `changes` and sealed by the miner."""
    setup, evidence, _ = real
    item = evidence.works[0]
    body = CommitV2.model_validate(item.commit.body).model_copy(update=changes)
    envelope = EnvelopeV2.model_validate(
        seal(li_escrow.HOT, "CommitV2", setup.manifest.run_id(), body, 200)
    )
    return replace(evidence, works=(replace(item, commit=envelope),))


MISSES = {
    "not-a-match": {"result": "MISMATCH"},
    "a-bad-leaf": {"first_bad_leaf": 0},
    "not-the-committed-root": {"recomputed_leaves_root": "11" * 32},
    "not-the-pinned-image": {"replay_env": {"image_digest": "sha256:" + "ab" * 32}},
    "not-an-allowed-driver": {"replay_env": {"driver": "unlisted-driver"}},
}


@pytest.mark.parametrize("fault", MISSES)
def test_a_verdict_that_is_not_an_authorised_match_of_the_commit_mints_nothing(
    li_escrow: ModuleType, real_reward: Real, fault: str
) -> None:
    # Given: the real reward, whose one verdict is replaced by a signed, well-formed verdict of
    # the allowlisted auditor that is not a full match of the commit.
    setup, real, reward = real_reward
    evidence = with_verdict(li_escrow, real_reward, li_escrow.AUDITOR, **MISSES[fault])
    # Control: the untouched evidence mints the whole budget.
    control = bare_escrow(li_escrow, setup)
    control.reward_finalize(reward, real)
    assert control.balances().issued == reward.budget_units == 1_000_000
    escrow = bare_escrow(li_escrow, setup)
    # When / Then: the escrow refuses the work, with the code of that check, and mints nothing.
    assert error_code(lambda: escrow.reward_finalize(reward, evidence)) == "UNVERIFIED_REWARD_WORK"
    assert escrow.balances().issued == 0


def test_a_verdict_signed_by_the_miner_it_judges_mints_nothing_even_if_it_is_allowlisted(
    li_escrow: ModuleType, real_reward: Real
) -> None:
    # Given: the condition of A2W-03 deliberately broken in a test-owned stand-in: the miner's own
    # hotkey is in the auditor set of the ledger.
    setup, _, reward = real_reward
    escrow = bare_escrow(li_escrow, setup)
    escrow.auditors = frozenset({li_escrow.AUDITOR.ss58, li_escrow.HOT.ss58})
    evidence = with_verdict(li_escrow, real_reward, li_escrow.HOT)
    # When / Then: a perfect MATCH signed by the miner itself is still not a verdict on the miner.
    assert error_code(lambda: escrow.reward_finalize(reward, evidence)) == "UNVERIFIED_REWARD_WORK"
    assert escrow.balances().issued == 0


def test_a_verdict_of_a_key_outside_the_auditor_set_or_an_expired_one_mints_nothing(
    li_escrow: ModuleType, real_reward: Real
) -> None:
    setup, _, reward = real_reward
    outsider = with_verdict(li_escrow, real_reward, li_escrow.OTHER)
    # Expired before the earliest beacon a ledger judges stored evidence at: the acceptance
    # beacon of the evidence (a ledger that keeps one) or the finalize beacon.
    expired = with_verdict(li_escrow, real_reward, li_escrow.AUDITOR, exp=1)
    for evidence, error, text in (
        (outsider, UnauthorizedSigner, "signer is not authorized for message role"),
        (expired, ExpiredError, "envelope expired"),
    ):
        escrow = bare_escrow(li_escrow, setup)
        with pytest.raises(error, match=text):
            escrow.reward_finalize(reward, evidence)
        assert escrow.balances().issued == 0


def test_a_commit_that_declares_more_tokens_than_its_assignment_mints_nothing(
    li_escrow: ModuleType, real_reward: Real
) -> None:
    # Given: the real reward, whose one commit is replaced by the miner's own signature over the
    # same commit with one token more than the server derives from its assignment.
    setup, real, reward = real_reward
    declared = CommitV2.model_validate(real.works[0].commit.body).tokens + 1
    inflated = with_commit(li_escrow, real_reward, tokens=declared)
    # Control: the untouched commit mints the whole budget.
    control = bare_escrow(li_escrow, setup)
    control.reward_finalize(reward, real)
    assert control.balances().issued == reward.budget_units == 1_000_000
    escrow = bare_escrow(li_escrow, setup)
    # When / Then: the escrow credits the server's token count only, so the commit is refused.
    with pytest.raises(ValueError, match="tokens differ from server-derived entitlement"):
        escrow.reward_finalize(reward, inflated)
    assert escrow.balances().issued == 0


def test_a_verdict_that_answers_another_challenge_mints_nothing(
    li_escrow: ModuleType, real_reward: Real
) -> None:
    setup, _, reward = real_reward
    escrow = bare_escrow(li_escrow, setup)
    evidence = with_verdict(li_escrow, real_reward, li_escrow.AUDITOR, challenge_hash="22" * 32)
    assert error_code(lambda: escrow.reward_finalize(reward, evidence)) == "REWARD_AUDIT_BINDING"
    assert escrow.balances().issued == 0


# --- A2W-23, AU-04: what withholding a signature achieves --------------------------------------


def test_a_mismatch_the_master_replayed_and_no_auditor_signed_costs_its_miner_the_round_only(
    world: sa.World,
) -> None:
    # Given: the master replayed the forger's job and stored a mismatch; the auditor that holds
    # the lease never signs it. D82 and D84 (ADR 0011 Part 9): that is an infrastructure fault.
    # The job gets a re-audit window for the other auditor; when nobody signs in it, the round is
    # retired and nobody is sanctioned, the forger included: only a mismatch an auditor signed is.
    with world.at("s2") as (n, cast):
        store, run = n.store, n.manifest.run_id()
        forger = cast.forger.ss58
        row = running(store)[forger]
        stored = store._record_v2(run, "executed-verdict", row["id"])
        assert stored["result"] == "MISMATCH"

        def standing() -> tuple[Any, ...]:
            with store._lock:
                admission = store._db.execute(
                    "SELECT state,reason,strikes FROM admissions_v2 WHERE hotkey=?", (forger,)
                ).fetchone()
            return (*admission, store._services(run)[0].balances().burned)

        before = standing()
        # When: the lease runs out and every window of the job passes with nobody signing anything.
        sa.drop_job(n, store, forger)
        # Control: the job is over, with no verdict.
        job = sa.lease_rows(store)[forger]
        assert (job["state"], job["result"]) == ("exhausted", "REAUDIT_FAILED")
        # Then: the round is retired for it, and the forger has no exclusion, no contest record, no
        # sanction row, no strike and no burnt unit.
        assert store._record_v2(run, "retired", "0")["cause"] == "REAUDIT_FAILED"
        assert ("excluded", f"0:{forger}") not in sa.records(store, "excluded")
        assert ("contest", body_digest(stored)) not in sa.records(store, "contest")
        assert sa.records(store, "exit-sanction") == {}
        assert standing() == before


def test_a_completion_is_refused_for_a_lease_that_has_no_owner(world: sa.World) -> None:
    # Given: a live lease whose owner column was lost (a test-owned stand-in: no route leaves
    # one) and the verdict the master stored for it. The signer predicate of the completion is
    # then `None.__eq__`, which accepts every signer; the refusal below comes from the contest
    # record the completion has to build (a contest has an auditor), not from the signer check.
    with world.at("s2") as (n, cast):
        store, run = n.store, n.manifest.run_id()
        row = running(store)[cast.forger.ss58]
        stored = store._record_v2(run, "executed-verdict", row["id"])
        stranger = cast.hot[1]
        # Control: while the lease has its owner, a registered non-auditor cannot complete it.
        assert detail(sa.complete(n, stranger, row["id"], stored)) == (
            409,
            "signer is not authorized for message role",
        )
        with store._tx():
            store._db.execute("UPDATE audit_leases_v2 SET auditor=NULL WHERE id=?", (row["id"],))
        before = sa.digest_of(store)
        # When: that registered non-auditor signs the master's own verdict and posts it.
        response = sa.complete(n, stranger, row["id"], stored)
        # Then: refused, and nothing is recorded: the lease stays running and unsettled.
        assert response.status_code == 409
        assert sa.digest_of(store) == before
        assert store._record_v2(run, "finality", "0")["unresolved_audits"] == 5
        assert ("replay", f"0:{cast.forger.ss58}") not in sa.records(store, "replay")


# --- A2W-07, A2W-15: what a miner signed stays what it signed ----------------------------------


def commit_envelope(store: Any, run: str, hotkey: str) -> dict[str, Any]:
    return store._record_v2(run, "commit", f"0:{hotkey}")


def test_a_commit_is_single_assignment_per_member_and_round(world: sa.World) -> None:
    # Given: round 0 committed and audited by the master; hot[1] signed one commit.
    with world.at("s1") as (n, cast):
        store, run = n.store, n.manifest.run_id()
        member = cast.hot[1]
        signed = commit_envelope(store, run, member.ss58)
        kinds = ("commit", "accept", "delta", "leaves", "assignment", "start", "audit-job")
        before = sa.records(store, *kinds)
        # When: it posts the same commit again.
        again = n.client.post(n.url + "/commit", json=signed)
        # Then: the original receipt comes back and nothing is written.
        assert again.status_code == 200
        assert again.json()["body"]["commit_hash"] == body_digest(signed["body"])
        assert sa.records(store, *kinds) == before
        # When: it signs another commit for the same round (another metrics root).
        other = {**signed["body"], "metrics_root": "ab" * 32}
        refused = n.client.post(n.url + "/commit", json=n.signed(member, "CommitV2", other))
        # Then: refused, and the commit the audit is bound to is unchanged.
        assert detail(refused) == (409, "conflicting signed operation nonce")
        assert sa.records(store, *kinds) == before


def test_a_delta_manifest_must_carry_the_hash_and_size_of_the_commit(world: sa.World) -> None:
    # Given: the forger's commit names a delta hash that none of its uploads has; it posts the
    # delta manifest of an honest member, re-signed as its own, with that member's acceptance.
    with world.at("s1") as (n, cast):
        store, run = n.store, n.manifest.run_id()
        honest = store._record_v2(run, "delta", f"0:{cast.hot[1].ss58}")["body"]
        borrowed = {**honest, "hotkey": cast.forger.ss58}
        before = sa.digest_of(store)
        # When / Then
        response = n.client.post(
            n.url + "/delta", json=n.signed(cast.forger, "DeltaManifestV2", borrowed)
        )
        assert detail(response) == (422, "delta/master acceptance differs from commit")
        assert sa.digest_of(store) == before
        assert ("delta", f"0:{cast.forger.ss58}") not in sa.records(store, "delta")


def test_the_audit_decision_is_bound_to_every_committed_hash(world: sa.World) -> None:
    # Given: an honest miner's audit, replayed by the master (a MATCH), and the same audit
    # re-run by the comparator with one committed hash altered at a time.
    with world.at("s2") as (n, cast):
        row = sa.honest_lease(n.store, cast)
        comparator = sa.Comparator(n, cast.keys[row["hotkey"]], row)
        control = comparator.outcome()
        assert (control.result, control.first_bad_leaf) == ("MATCH", None)
        assert control.recomputed_leaves_root == comparator.commit["leaves_root"]
        # When / Then: a MATCH certifies exactly the committed leaves, delta, final theta and
        # error feedback, and no other value of any of them.
        for field in ("leaves_root", "delta_hash", "final_theta_hash", "ef_in_hash", "ef_out_hash"):
            altered = comparator.outcome(commit={field: "ab" * 32})
            assert (altered.result, altered.first_bad_leaf) == ("MISMATCH", None), field
        # And one altered leaf preimage is found at its index.
        leaf = comparator.outcome(preimage={1: f32hex(123.0)})
        assert (leaf.result, leaf.first_bad_leaf) == ("MISMATCH", 1)


def posted_work_proof(store: Any, hotkey: str) -> dict[str, Any]:
    """The signed WorkProof envelope `hotkey` published the leaves of its commit with."""
    with store._lock:
        rows = store._db.execute(
            "SELECT envelope FROM accepted_v2 WHERE key LIKE 'auth:%WorkProof%'"
        ).fetchall()
    found = [json.loads(r[0]) for r in rows if json.loads(r[0])["signer"] == hotkey]
    assert len(found) == 1
    return found[0]


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="A2W-15: the leaves record an audit job was built from is write-once; publishing the "
    "same leaves as another object after the lease is refused even under another admission id "
    "(T-A2W-17) [fix-lot: none (no design yet)]",
)
def test_the_leaves_an_audit_was_built_from_cannot_be_republished_as_another_object(
    world: sa.World,
) -> None:
    # Given: the forger's audit is leased and executed; its leaves record names one object.
    with world.at("s2") as (n, cast):
        store, run = n.store, n.manifest.run_id()
        forger = cast.forger
        identity = f"0:{forger.ss58}"
        original = store._record_v2(run, "leaves", identity)
        proof = posted_work_proof(store, forger.ss58)
        # Control: publishing the very same proof again is accepted and changes nothing.
        same = n.client.post(n.url + "/leaves", json=proof)
        assert same.status_code == 200
        assert store._record_v2(run, "leaves", identity) == original
        # When: the forger publishes the same leaves, encoded another way (another object).
        leaves = json.loads(store.objects.get(original["object"]))
        encoded = json.dumps(leaves, indent=2).encode()
        reference = {"sha256": store.objects.put(encoded), "size": len(encoded)}
        assert reference["sha256"] != original["object"]
        body = {**proof["body"], "artifact_refs": [*proof["body"]["artifact_refs"][:-1], reference]}
        again = n.client.post(n.url + "/leaves", json=n.signed(forger, "WorkProof", body))
        # Then: refused as a second body for the same proof, the guard that holds.
        assert detail(again) == (409, "conflicting signed operation nonce")
        assert store._record_v2(run, "leaves", identity) == original
        # When: the proof also carries another admission id, which nothing checks against the
        # miner's admission and which is part of the proof's replay identity.
        fresh = {**body, "admission_id": "ab" * 32}
        republished = n.client.post(n.url + "/leaves", json=n.signed(forger, "WorkProof", fresh))
        # Then: refused as well, and the leaves the lease embedded stay the ones on record.
        assert republished.status_code == 409
        assert store._record_v2(run, "leaves", identity) == original
