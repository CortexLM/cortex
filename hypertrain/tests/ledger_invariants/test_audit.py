"""Who audits, what an auditor may sign, and when a lease stops being usable (D39, `audit`).

A round is audited by the master's own replay; an auditor only signs what the master computed.
One real round (five miners, one of them forging its delta hash, three allowlisted auditors) is
prepared once per process by `support_audit.World` and restored per test. Every refusal below is
pinned by its code and by the store being unchanged.
"""

from __future__ import annotations

import json
from typing import Any

import pytest

from hypertrain.protocol import envelope_v2
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages import f32hex
from hypertrain.protocol.messages_v2 import PolicyHashes, RoundOpenV2
from hypertrain.trainer.compress import state_hash
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.model import init_params

from . import support_audit as sa
from .support_audit import audit_kinds, detail, running, settled_state


@pytest.fixture(scope="session", name="world")
def audit_world(tmp_path_factory: pytest.TempPathFactory) -> sa.World:
    return sa.session_world(tmp_path_factory)


# --- A2W-02, AU-04: who may lease --------------------------------------------------------------


def test_a_registered_miner_that_is_not_an_auditor_gets_no_lease(world: sa.World) -> None:
    # Given: round 0 audited by three allowlisted keys; hot[0] is a registered miner, but not
    # one of them.
    with world.at("s1") as (n, cast):
        before = sa.digest_of(n.store)
        # When
        response = sa.lease(n, cast.hot[0])
        # Then: refused, nothing written, no lease taken, no auditor disabled.
        assert detail(response) == (403, "auditor not allowlisted")
        assert sa.digest_of(n.store) == before
        assert {r["state"] for r in sa.lease_rows(n.store).values()} == {"queued"}
        assert not sa.records(n.store, "auditor-disabled")
        # Control: an allowlisted key asking the same way is leased a job.
        assert sa.lease(n, cast.auditors[0]).status_code == 200


def test_an_allowlisted_miner_is_never_leased_its_own_job(world: sa.World) -> None:
    # Given: hot[3] is allowlisted as an auditor and is also a miner of the round; the other four
    # audits are already done (a test-owned stand-in), so its own job is the only queued one.
    with world.at("s1") as (n, cast):
        own = cast.hot[3]
        with n.store._tx():
            n.store._db.execute(
                "UPDATE audit_leases_v2 SET state='completed' WHERE hotkey!=?", (own.ss58,)
            )
        before = audit_kinds(n.store)
        # When
        response = sa.lease(n, own)
        # Then: nothing to lease for it; its job stays queued and untried.
        assert response.status_code == 204
        assert audit_kinds(n.store) == before
        row = sa.lease_rows(n.store)[own.ss58]
        assert (row["state"], row["attempts"], row["tried"], row["auditor"]) == (
            "queued",
            0,
            "[]",
            None,
        )
        # Control: another allowlisted key is leased that very job.
        job = sa.lease(n, cast.auditors[0]).json()["body"]
        assert job["start_state"]["hotkey"] == own.ss58


def test_an_auditor_that_already_tried_a_job_is_not_leased_it_again(world: sa.World) -> None:
    # Given: the forger's job lists the first auditor among those that already tried it (what a
    # lapsed first attempt records; a test-owned stand-in, as no second attempt can be leased).
    with world.at("s1") as (n, cast):
        store = n.store
        first, second = cast.auditors[:2]
        target = sa.lease_rows(store)[cast.forger.ss58]
        with store._tx():
            store._db.execute(
                "UPDATE audit_leases_v2 SET tried=? WHERE id=?",
                (json.dumps([first.ss58]), target["id"]),
            )
        # When / Then: the first auditor is served another job, never the one it tried.
        served = sa.lease(n, first).json()["body"]
        assert served["job_id"] != target["id"]
        # Control: an auditor that did not try it is served it first.
        assert sa.lease(n, second).json()["body"]["job_id"] == target["id"]


def test_an_auditor_holds_one_lease_and_the_store_at_most_two(world: sa.World) -> None:
    # Given: the first two allowlisted keys each hold an executed lease; the third holds none.
    with world.at("s2") as (n, cast):
        first, _, third = cast.auditors
        held = running(n.store)
        assert len(held) == 2
        before = sa.digest_of(n.store)
        # When / Then: a holder asking again is refused.
        assert detail(sa.lease(n, first)) == (409, "one live lease per auditor")
        assert sa.digest_of(n.store) == before
        # And a key without a lease gets none while two are running.
        assert sa.lease(n, third).status_code == 204
        assert set(running(n.store)) == set(held)
        # Control: as soon as a lease completes the same request is served.
        forger = held[cast.forger.ss58]
        executed = n.store._record_v2(n.manifest.run_id(), "executed-verdict", forger["id"])
        assert (
            sa.complete(n, cast.keys[forger["auditor"]], forger["id"], executed).status_code == 200
        )
        served = sa.lease(n, third)
        assert served.status_code == 200
        assert served.json()["body"]["start_state"]["hotkey"] != third.ss58


def test_only_the_pinned_auditor_may_execute_or_complete_a_live_lease(world: sa.World) -> None:
    # Given: the forger's job is leased and executed by the first auditor.
    with world.at("s2") as (n, cast):
        store = n.store
        row = running(store)[cast.forger.ss58]
        job = {"job_id": row["id"], "lease_nonce": row["nonce"]}
        executed = store._record_v2(n.manifest.run_id(), "executed-verdict", row["id"])
        before = sa.digest_of(store)
        # When: a registered non-auditor, then the miner whose job it is, sign each request.
        for stranger in (cast.hot[1], cast.forger):
            # Then: refused before anything is read or written.
            assert detail(sa.lease(n, stranger)) == (403, "auditor not allowlisted")
            assert detail(sa.execute(n, stranger, job)) == (
                403,
                "audit request lacks owned live lease",
            )
            assert detail(sa.complete(n, stranger, row["id"], executed)) == (
                409,
                "signer is not authorized for message role",
            )
        assert sa.digest_of(store) == before
        assert not sa.records(store, "auditor-disabled")
        # Control: the pinned holder completes the very same verdict.
        holder = cast.keys[row["auditor"]]
        assert sa.complete(n, holder, row["id"], executed).status_code == 200


# --- A2W-04: the verdict is the master's -------------------------------------------------------


def test_the_executed_verdict_of_a_forged_commit_is_the_masters_mismatch(world: sa.World) -> None:
    # Given: the forger signed a commit whose delta hash is not the one its training produced;
    # the honest job is the allowlisted miner's own.
    with world.at("s2") as (n, cast):
        store, run = n.store, n.manifest.run_id()
        rows = running(store)
        forged = store._record_v2(run, "executed-verdict", rows[cast.forger.ss58]["id"])
        honest = store._record_v2(run, "executed-verdict", rows[cast.hot[3].ss58]["id"])
        commit = store._record_v2(run, "commit", f"0:{cast.forger.ss58}")["body"]
        # Then: the master replayed it and found a mismatch no auditor chose; the leaves are
        # honest, so no leaf is bad and the recomputed root is the committed one.
        assert forged["result"] == "MISMATCH"
        assert forged["first_bad_leaf"] is None
        assert forged["recomputed_leaves_root"] == commit["leaves_root"]
        assert honest["result"] == "MATCH"


def flipped(**changes: Any) -> Any:
    return lambda verdict: {**verdict, **changes}


def other_environment(verdict: dict[str, Any]) -> dict[str, Any]:
    return {**verdict, "replay_env": {**verdict["replay_env"], "gpu_uuid_sha256": "ab" * 32}}


@pytest.mark.parametrize(
    ("holder_of", "edit"),
    [
        pytest.param("forger", flipped(result="MATCH"), id="fraud-turned-into-a-pass"),
        pytest.param("forger", flipped(first_bad_leaf=0), id="another-first-bad-leaf"),
        pytest.param("forger", flipped(recomputed_leaves_root="ab" * 32), id="another-root"),
        pytest.param("forger", other_environment, id="another-replay-environment"),
        pytest.param("miner", flipped(result="MISMATCH"), id="pass-turned-into-fraud"),
    ],
)
def test_an_auditor_cannot_sign_a_verdict_other_than_the_masters(
    world: sa.World, holder_of: str, edit: Any
) -> None:
    # Given: the holder of an executed lease; the master stored the verdict it computed.
    with world.at("s2") as (n, cast):
        store, run = n.store, n.manifest.run_id()
        target = cast.forger if holder_of == "forger" else cast.hot[3]
        row = running(store)[target.ss58]
        holder = cast.keys[row["auditor"]]
        stored = store._record_v2(run, "executed-verdict", row["id"])
        before = sa.digest_of(store)
        # When: the holder signs a different verdict and posts it.
        response = sa.complete(n, holder, row["id"], edit(stored))
        # Then: refused for its value, the lease is untouched and nothing is recorded.
        assert detail(response) == (422, "signed verdict differs from independent real replay")
        assert sa.digest_of(store) == before
        assert sa.lease_rows(store)[target.ss58]["state"] == "running"
        # Control: the verdict the master computed completes the same lease.
        assert sa.complete(n, holder, row["id"], stored).json() == {"result": stored["result"]}


def test_a_verdict_signed_by_another_allowlisted_auditor_is_refused(world: sa.World) -> None:
    # Given: a lease held by the first auditor with the master's verdict stored for it.
    with world.at("s2") as (n, cast):
        store = n.store
        row = running(store)[cast.forger.ss58]
        stored = store._record_v2(n.manifest.run_id(), "executed-verdict", row["id"])
        before = sa.digest_of(store)
        # When: the right verdict, signed by an allowlisted key that does not hold the lease.
        response = sa.complete(n, cast.auditors[2], row["id"], stored)
        # Then
        assert detail(response) == (409, "signer is not authorized for message role")
        assert sa.digest_of(store) == before


# --- A2W-05: stale holders ---------------------------------------------------------------------


def test_a_lapsed_lease_cannot_be_executed_or_completed_by_its_old_holder(
    world: sa.World,
) -> None:
    # Given: the first auditor executed the forger's job, then let the lease run out.
    with world.at("s2") as (n, cast):
        store, run = n.store, n.manifest.run_id()
        row = running(store)[cast.forger.ss58]
        holder = cast.keys[row["auditor"]]
        stored = store._record_v2(run, "executed-verdict", row["id"])
        n.push(row["expires"])
        lapsed = sa.lease_rows(store)[cast.forger.ss58]
        assert (lapsed["state"], lapsed["attempts"]) == ("queued", 1)
        kind, silence, refusal = sa.silence_of(store, run, row)
        assert store._record_v2(run, kind, holder.ss58) == silence
        before = sa.digest_of(store)
        # When / Then: it can neither run the job again nor settle the verdict it computed, and
        # it is not leased anything while it is held against for the silence.
        job = {"job_id": row["id"], "lease_nonce": row["nonce"]}
        assert detail(sa.execute(n, holder, job)) == (403, "audit request lacks owned live lease")
        assert detail(sa.complete(n, holder, row["id"], stored)) == (
            409,
            "audit result outside owned immutable lease",
        )
        assert detail(sa.lease(n, holder)) == refusal
        assert sa.digest_of(store) == before


# What the sweep does with a lapsed lease: the first attempt of a window is retried, the second
# ends the window. A tree that re-arms an exhausted job opens one more window then and drops the
# miner at the end of the second; an older tree drops it at once.
LAPSES = [
    pytest.param(1, 1, "retried", id="first-attempt"),
    pytest.param(2, 1, "rearmed" if sa.REARMS else "dropped", id="second-attempt-of-the-window"),
    pytest.param(2, 2, "dropped", id="second-attempt-of-the-last-window"),
]


@pytest.mark.parametrize(("attempts", "windows", "outcome"), LAPSES)
def test_a_lapsed_job_is_retried_once_then_given_one_more_window_then_dropped(
    world: sa.World, attempts: int, windows: int, outcome: str
) -> None:
    # Given: the forger's executed lease, recorded as the first or the second attempt of its first
    # or of its last window (a test-owned stand-in, as no run reaches a late window quickly).
    with world.at("s2") as (n, cast):
        store, run = n.store, n.manifest.run_id()
        row = running(store)[cast.forger.ss58]
        identity = f"0:{cast.forger.ss58}"
        with store._tx():
            store._db.execute(
                "UPDATE audit_leases_v2 SET attempts=? WHERE id=?", (attempts, row["id"])
            )
            if windows == 2:
                body = {"windows": 2, "at": n.now, "absolute": row["absolute"]}
                store._put_record_v2(run, "audit-rearm", row["id"], body)
        open_before = int(store._record_v2(run, "finality", "0")["unresolved_audits"])
        # When: the lease runs out.
        n.push(row["expires"])
        # Then: the job is retried, given a fresh window, or dropped with its miner.
        after = sa.lease_rows(store)[cast.forger.ss58]
        status = store._record_v2(run, "finality", "0")
        excluded = sa.records(store, "excluded").get(("excluded", identity))
        if outcome == "dropped":
            assert after["state"] == "exhausted"
            assert json.loads(excluded or "null") == {
                "reason": "INFRASTRUCTURE",
                "evidence_hash": sha256_hex(row["id"].encode()),
            }
            assert (status["unresolved_audits"], status["disposition"]) == (
                open_before - 1,
                sa.DROP_DISPOSITION,
            )
        else:
            assert after["state"] == "queued" and excluded is None
            assert (status["unresolved_audits"], status["disposition"]) == (open_before, "PENDING")
        if outcome == "retried":
            assert (after["attempts"], after["absolute"]) == (attempts, row["absolute"])
        if outcome == "rearmed":
            rearm = store._record_v2(run, "audit-rearm", row["id"])
            assert rearm == {"windows": 2, "at": row["expires"], "absolute": after["absolute"]}
            assert (after["attempts"], after["tried"], after["auditor"]) == (0, "[]", None)
            # The fresh window starts at the lapse and ends with the miner's sealed commit at
            # the latest.
            sealed = envelope_v2.parse_envelope(store._record_v2(run, "commit", identity))
            window = sa.audit_policy(store, run).absolute_rounds()
            assert after["created"] == row["expires"]
            assert after["absolute"] == min(row["expires"] + window, sealed.exp_drand)


def test_a_job_leased_again_cannot_be_settled_with_the_first_attempts_verdict(
    world: sa.World,
) -> None:
    # Given: the first auditor executed the forger's job and let the lease run out; the allowlisted
    # miner of another job is then leased it as the second attempt.
    with world.at("s2") as (n, cast):
        store, run = n.store, n.manifest.run_id()
        row = running(store)[cast.forger.ss58]
        first, second = cast.keys[row["auditor"]], cast.auditors[2]
        stale = store._record_v2(run, "executed-verdict", row["id"])
        n.push(row["expires"])
        leased = sa.lease(n, second)
        assert leased.status_code == 200
        job = leased.json()["body"]
        again = sa.lease_rows(store)[cast.forger.ss58]
        assert (job["job_id"], again["auditor"], again["attempts"]) == (row["id"], second.ss58, 2)
        assert job["lease_nonce"] != row["nonce"]
        before = sa.digest_of(store)
        # When / Then: the first holder can neither run nor settle the job any more.
        old = {"job_id": row["id"], "lease_nonce": row["nonce"]}
        assert detail(sa.execute(n, first, old)) == (403, "audit request lacks owned live lease")
        assert detail(sa.complete(n, first, row["id"], stale)) == (
            409,
            "signer is not authorized for message role",
        )
        # And the new holder cannot settle the first attempt's verdict without a replay of its own.
        refused = sa.complete(n, second, row["id"], stale)
        assert detail(refused) == (
            409,
            f"missing accepted audit-geometry: {row['id']}:{job['lease_nonce']}",
        )
        assert sa.digest_of(store) == before
        # Control: the new holder replays and settles the job under its own lease.
        verdict = sa.execute(n, second, job).json()["verdict"]
        assert sa.complete(n, second, row["id"], verdict).json() == {"result": verdict["result"]}


def test_a_lease_that_was_taken_back_cannot_be_completed_before_it_expires(
    world: sa.World,
) -> None:
    # Given: the forger's executed lease, put back in the queue while its expiry is still ahead
    # (what a revocation that leaves the expiry column alone would do; a test-owned stand-in).
    with world.at("s2") as (n, cast):
        store, run = n.store, n.manifest.run_id()
        row = running(store)[cast.forger.ss58]
        stored = store._record_v2(run, "executed-verdict", row["id"])
        with store._tx():
            store._db.execute("UPDATE audit_leases_v2 SET state='queued' WHERE id=?", (row["id"],))
        before = sa.digest_of(store)
        # When / Then: its old holder signs the master's verdict and it is not taken.
        late = sa.complete(n, cast.keys[row["auditor"]], row["id"], stored)
        assert detail(late) == (409, "audit result outside owned immutable lease")
        assert sa.digest_of(store) == before


def test_an_execute_request_must_carry_the_nonce_of_the_live_lease(world: sa.World) -> None:
    # Given: the first auditor was just leased the forger's job.
    with world.at("s1") as (n, cast):
        store = n.store
        holder = cast.auditors[0]
        job = sa.lease(n, holder).json()["body"]
        before = sa.digest_of(store)
        # When: it signs an execute request that names another lease (the nonce is the lease).
        stale = {"job_id": job["job_id"], "lease_nonce": "ab" * 32}
        # Then: refused before any replay.
        assert detail(sa.execute(n, holder, stale)) == (403, "audit lease nonce differs")
        assert sa.digest_of(store) == before
        # Control: the nonce the lease was issued with is replayed.
        assert sa.execute(n, holder, job).status_code == 200


def test_a_result_at_the_lease_expiry_round_is_refused_even_before_the_sweep(
    world: sa.World, monkeypatch: pytest.MonkeyPatch
) -> None:
    # Given: two executed leases that expire at the same round.
    with world.at("s2") as (n, cast):
        store, run = n.store, n.manifest.run_id()
        rows = running(store)
        forger, honest = rows[cast.forger.ss58], rows[cast.hot[3].ss58]
        assert forger["expires"] == honest["expires"]
        stored = {h: store._record_v2(run, "executed-verdict", r["id"]) for h, r in rows.items()}
        # When: one round before expiry the honest holder completes.
        n.push(honest["expires"] - 1)
        done = sa.complete(n, cast.keys[honest["auditor"]], honest["id"], stored[cast.hot[3].ss58])
        assert done.status_code == 200
        # And at the expiry round, with the expiry sweep switched off, the other holder does not.
        monkeypatch.setattr(store, "_expire_leases_v2", lambda *_: None)
        n.push(forger["expires"])
        assert sa.lease_rows(store)[cast.forger.ss58]["state"] == "running"
        before = sa.digest_of(store)
        late = sa.complete(n, cast.keys[forger["auditor"]], forger["id"], stored[cast.forger.ss58])
        # Then
        assert detail(late) == (409, "audit result outside owned immutable lease")
        assert sa.digest_of(store) == before


# --- A2W-06: one result per job ----------------------------------------------------------------


def test_a_completed_job_is_final_and_a_repeated_completion_changes_nothing(
    world: sa.World,
) -> None:
    # Given: all five audits completed once; the forger's ended as a mismatch.
    with world.at("s3") as (n, cast):
        store, run = n.store, n.manifest.run_id()
        rows = sa.lease_rows(store)
        assert {h: r["result"] for h, r in rows.items()} == {
            h.ss58: "MISMATCH" if h is cast.forger else "MATCH" for h in cast.hot
        }
        assert store._record_v2(run, "finality", "0")["unresolved_audits"] == 0
        before = settled_state(store)
        # When: each completion is posted again, signed again, and so is a different verdict.
        for row in rows.values():
            key = cast.keys[row["auditor"]]
            stored = store._record_v2(run, "executed-verdict", row["id"])
            assert sa.complete(n, key, row["id"], stored).json() == {"result": row["result"]}
            different = {**stored, "result": "MATCH" if row["result"] == "MISMATCH" else "MISMATCH"}
            assert detail(sa.complete(n, key, row["id"], different)) == (
                409,
                "conflicting signed operation nonce",
            )
            # And the master replays nothing and leases nothing for a finished job.
            job = {"job_id": row["id"], "lease_nonce": row["nonce"]}
            assert detail(sa.execute(n, key, job)) == (403, "audit request lacks owned live lease")
        assert all(sa.lease(n, key).status_code == 204 for key in cast.auditors)
        # And the expiry sweep, far past every deadline, does not touch a terminal job.
        n.push(n.now + 1000)
        # Then: no record and no row changed, and the counter was decremented once per job.
        assert settled_state(store) == before
        assert store._record_v2(run, "finality", "0")["unresolved_audits"] == 0


# --- A2W-09: every roster member is audited in full --------------------------------------------


def test_every_roster_member_has_exactly_one_full_audit_job(world: sa.World) -> None:
    # Given: round 0 committed by five miners and its audits scheduled.
    with world.at("s1") as (n, cast):
        store, run = n.store, n.manifest.run_id()
        coordinator = n.manifest.training.coord_pubkey
        rows = sa.lease_rows(store)
        # Then: one job per member, whatever its commit, and each asks for a full audit.
        assert sorted(rows) == sorted(h.ss58 for h in cast.hot)
        for hotkey, row in rows.items():
            assert row["id"] == sa.job_id(run, hotkey)
            assert (row["w"], row["state"], row["attempts"], row["auditor"]) == (
                0,
                "queued",
                0,
                None,
            )
            challenge = json.loads(row["challenge"])
            assert challenge["signer"] == coordinator
            assert {k: challenge["body"][k] for k in ("w", "target", "mode", "segments")} == {
                "w": 0,
                "target": hotkey,
                "mode": "full",
                "segments": [],
            }
            assert (challenge["body"]["reasons"], challenge["body"]["audit_mode"]) == (
                ["random", "final"],
                "anchored-full",
            )
            assert challenge["body"]["serve_deadline"] == row["absolute"]
        assert store._record_v2(run, "finality", "0")["unresolved_audits"] == len(cast.hot)
        # And scheduling again creates nothing.
        before = sa.digest_of(store)
        again = sa.schedule(n)
        assert (again.status_code, again.json()) == (200, {"jobs": []})
        assert sa.digest_of(store) == before


def opening(n: Any, cast: sa.Cast, q_i: str) -> dict[str, Any]:
    """A signed opening of round 0 for the five miners, each asking audit probability `q_i`."""
    manifest, store = n.manifest, n.store
    admission = store._services(manifest.run_id())[1]
    roster = []
    for slot, hot in enumerate(cast.hot):
        record = admission.status(hot.ss58, now=n.now).record
        roster.append(
            {
                "hotkey": hot.ss58,
                "slot": slot,
                "q_i": q_i,
                "admission_id": record.admission_id,
                "coldkey_group": record.coldkey,
                "state": record.state,
                "eligible_weight": 4194304,
            }
        )
    theta = init_params(TrainConfig.from_manifest_v2(manifest).model)
    body = RoundOpenV2(
        w=0,
        prev_final_hash="0" * 64,
        theta_hash=state_hash(theta),
        outer_state_hash="0" * 64,
        center_hash="0" * 64,
        roster_hash=sha256_hex(canonicalize(roster)),
        honeypot_commit="0" * 64,
        d_open=n.now + 1,
        d_assign=n.now + 2,
        d_commit=n.now + 30,
        d_audit=n.now + 31,
        d_upload=n.now + 150,
        d_final=n.now + 200,
        contract_version=2,
        policy_hashes=PolicyHashes.model_validate(
            {k: getattr(manifest.network, k) for k in PolicyHashes.model_fields}
        ),
        registry_epoch=0,
        start_state_index_hash="0" * 64,
        audit_mode="anchored-full",
        roster=roster,
    )
    signed: dict[str, Any] = n.signed(sa.rollback_module().fixtures.COORD, "RoundOpenV2", body)
    return signed


def test_a_round_cannot_be_opened_with_a_sampled_audit_probability(world: sa.World) -> None:
    # Given: five funded identities and no round yet.
    with world.at("s0") as (n, cast):
        store = n.store
        before = sa.digest_of(store)

        def post(q_i: str) -> Any:
            return n.client.post(
                n.url + "/admin/rounds", json=opening(n, cast, q_i), headers=sa.admin()
            )

        # Control: with full audit probability the opening gets past the roster, and stops at the
        # start-state index, which this test does not compute.
        assert detail(post(f32hex(1.0))) == (
            422,
            "start-state index differs from authenticated anchors",
        )
        # When: the same opening asks to audit each member with probability 1/4.
        refused = post(f32hex(0.25))
        # Then: it is refused for the probability, and no round exists.
        assert detail(refused) == (403, "roster lacks current funded admission authority")
        assert sa.digest_of(store) == before
        assert not sa.records(store, "round")
