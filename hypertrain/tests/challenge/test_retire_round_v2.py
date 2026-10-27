"""A round that cannot reach a tape is retired: a null round, its anchors carried, no more work."""
# ruff: noqa: F811  (the `network` fixture parameter shadows the imported fixture)

from __future__ import annotations

import time
from dataclasses import asdict, replace
from pathlib import Path

import pytest

from hypertrain.aggregator.core import OuterState
from hypertrain.auditor.replay import AnchorCache, optimizer_hash
from hypertrain.auditor.worker import LeaseGuard
from hypertrain.challenge.finality_v2 import RoundStatus
from hypertrain.challenge.finality_v2 import retire as close_retired
from hypertrain.challenge.store import ChallengeError, ChallengeStore
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import Finalize, Rollback
from hypertrain.protocol.messages_v2 import AuditPolicyV2
from hypertrain.relay.core import Settlement
from hypertrain.trainer.compress import state_hash
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.model import init_params

from . import test_service_network_v2 as fixtures
from .audit_lease_fixture_v2 import (
    AUDITOR_A,
    AUDITOR_B,
    D_AUDIT,
    lease,
    network,  # noqa: F401  (pytest fixture import)
    schedule,
    seed_round,
)
from .test_round_withdrawal_v2 import (  # noqa: F401  (pytest fixture import: ledger)
    ledger,
    put_finality,
    signed_round,
    unresolved,
    withdrawals,
)

H = "00" * 32


def seeded(network, size, silent=()):
    """Round 0 as an open run holds it: every roster member has its genesis anchor (an honest
    silent member too, the fixture gives it none), the `silent` ones never committed."""
    keys = seed_round(network, n=size, executable=True, silent=silent)
    cache, manifest = AnchorCache(), network.manifest
    theta = init_params(TrainConfig.from_manifest_v2(manifest).model)
    with network.store._tx():
        for index in silent:
            anchor = cache.genesis(manifest, keys[index].ss58, theta)
            network.store._persist_anchor_v2(manifest, anchor, cache)
    return keys


def retire(network, survivors, *, w=0, cause="INFEASIBLE"):
    """What the reaper does: one retirement inside the transaction of a beacon push."""
    with network.store._tx():
        return network.store._retire_round_v2(network.manifest.run_id(), w, cause, survivors)


def snapshot(store, run):
    """Every record of the run and every audit lease: what a refused retirement must not touch."""
    records = store._db.execute(
        "SELECT kind,id,data FROM records_v2 WHERE run_id=? ORDER BY kind,id", (run,)
    ).fetchall()
    leases = store._db.execute("SELECT * FROM audit_leases_v2 ORDER BY id").fetchall()
    return [tuple(r) for r in records], [tuple(r) for r in leases]


def genesis_theta(network):
    return init_params(TrainConfig.from_manifest_v2(network.manifest).model)


def second_round(network):
    """Round 1 as the operator opens it after round 0: the same roster and schedule, w = 1."""
    store, run = network.store, network.manifest.run_id()
    opening = store._record_v2(run, "round", "0")
    audits = len(opening["body"]["roster"])
    pending = asdict(unresolved(run, 1, applied=False))
    with store._tx():
        store._put_record_v2(run, "round", "1", {**opening, "body": {**opening["body"], "w": 1}})
        store._put_record_v2(run, "finality", "1", {**pending, "unresolved_audits": audits})


@pytest.mark.parametrize(
    ("size", "silent", "retired"),
    [
        (4, (3,), True),
        (5, (3, 4), True),
        (5, (0, 1, 2, 3, 4), True),
        (5, (4,), False),
        (6, (1, 4), False),
    ],
    ids=["4-1", "5-2", "5-all", "5-1-kept", "6-2-kept"],
)
def test_a_round_is_retired_exactly_when_its_survivors_cannot_carry(network, size, silent, retired):
    # Given: a round whose absent members never committed; the commit window is over
    store, run = network.store, network.manifest.run_id()
    keys = seeded(network, size, silent)
    survivors = [k.ss58 for i, k in enumerate(keys) if i not in silent]
    network.push(D_AUDIT)
    before = snapshot(store, run)
    if retired:
        # When: `audits` already refuses these survivors, and the reaper retires the round
        with pytest.raises(ChallengeError, match="ROUND_INFEASIBLE"):
            store.schedule_audits_v2(run, 0)
        done = retire(network, survivors)
        # Then: the round is closed for good and `audits` says why
        status = store._record_v2(run, "finality", "0")
        assert (status["disposition"], status["applied"]) == ("RETIRED", True)
        assert (done["cause"], done["survivors"]) == ("INFEASIBLE", sorted(survivors))
        with pytest.raises(ChallengeError, match="ROUND_RETIRED") as late:
            store.schedule_audits_v2(run, 0)
        assert late.value.status == 409
    else:
        # When / Then: four survivors can carry the round; the primitive cannot void it
        with pytest.raises(
            ChallengeError, match="ROUND_RETIRE_REFUSED: the survivors can carry the round"
        ) as refused:
            retire(network, survivors)
        assert refused.value.status == 409
        assert snapshot(store, run) == before


def test_a_retirement_writes_the_null_round_the_next_opening_reads(network):
    store, run = network.store, network.manifest.run_id()
    keys = seeded(network, 5, silent=(3, 4))
    survivors = [k.ss58 for k in keys[:3]]
    network.push(D_AUDIT)
    store._put_record_v2(run, "round-meta", "0", {"base": 0})
    theta = genesis_theta(network)
    roster_hash = store._record_v2(run, "round", "0")["body"]["roster_hash"]
    evidence = {
        "v": "ht-round-retired-v1",
        "run_id": run,
        "w": 0,
        "cause": "INFEASIBLE",
        "roster_hash": roster_hash,
        "survivors": sorted(survivors),
        "theta_hash": state_hash(theta),
    }
    evidence_hash = sha256_hex(canonicalize(evidence))

    done = retire(network, survivors)

    # the evidence is a pure function of the facts of the round, and anybody can re-derive it
    assert done == {**evidence, "evidence_hash": evidence_hash, "retired_beacon": D_AUDIT}
    assert store._record_v2(run, "retired", "0") == done
    assert store.objects.get(evidence_hash) == canonicalize(evidence)
    # the model did not move: the next round reads the initial outer state as the output of this one
    applied = store._record_v2(run, "applied", "0")
    assert applied == {
        "kind": "null",
        "tape_hash": evidence_hash,
        "out_state": applied["prev_state"],
        "prev_state": applied["prev_state"],
        "theta_hash": state_hash(theta),
        "predecessor_tape_hash": "0" * 64,
    }
    initial = OuterState.init({k: v.detach().cpu().numpy() for k, v in theta.items()})
    assert store.objects.get(applied["out_state"]) == initial.to_bytes()
    # finality closes it with the evidence, and the ledger reads that closure
    status = RoundStatus(**store._record_v2(run, "finality", "0"))
    assert status == RoundStatus(
        run, 0, True, "RETIRED", 0, 0, False, state_hash(theta), evidence_hash
    )
    assert status.closed and store._round_closure_v2(run, 0) == evidence_hash
    # the data window is consumed, not rewound
    assert store._record_v2(run, "round-meta", "0") == {"base": 0}
    # a second call is the same retirement, not another one
    after = snapshot(store, run)
    assert retire(network, survivors) == done
    assert snapshot(store, run) == after


def test_every_roster_member_restarts_the_next_round_from_the_state_it_started_this_one(network):
    store, run, manifest = network.store, network.manifest.run_id(), network.manifest
    keys = seeded(network, 5, silent=(3, 4))
    network.push(D_AUDIT)
    sources = {k.ss58: store._record_v2(run, "anchor", f"-1:{k.ss58}") for k in keys}

    retire(network, [k.ss58 for k in keys[:3]])

    for key in keys:  # the members that never committed are carried too
        hotkey = key.ss58
        source, carried = sources[hotkey], store._record_v2(run, "anchor", f"0:{hotkey}")
        assert store._record_v2(run, "anchor", f"-1:{hotkey}") == source
        assert (carried["state_root"], carried["ef_hash"]) == (
            source["state_root"],
            source["ef_hash"],
        )
        assert carried["anchor_hash"] != source["anchor_hash"]
        assert carried["proof_hash"] != source["proof_hash"]
        assert (carried["w"], carried["carried_from"]) == (0, -1)
        for name in ("state", "ef"):  # the tensors are linked, never read or rewritten
            assert Path(carried["path"], name).samefile(Path(source["path"], name))
        # the carried anchor authenticates against the roots the next opening will pin
        restored = AnchorCache().restore(
            Path(carried["path"]),
            manifest,
            carried["anchor_hash"],
            carried["proof_hash"],
            carried["state_root"],
            carried["ef_hash"],
            expected_hotkey=hotkey,
            expected_round=0,
            expected_backend=carried["backend"],
        )
        start = store._record_v2(run, "start", f"0:{hotkey}")
        assert optimizer_hash(restored.state) == start["opt_state_hash"]
        assert state_hash(restored.ef) == start["ef_hash"]


@pytest.mark.parametrize("route", ["audits", "aggregate", "finalize", "rollback"])
def test_a_retired_round_answers_409_round_retired_to_new_work(network, route):
    store, run = network.store, network.manifest.run_id()
    keys = seeded(network, 4, silent=(3,))
    network.push(D_AUDIT)
    retire(network, [k.ss58 for k in keys[:3]])
    before = snapshot(store, run)
    theta_hash = state_hash(genesis_theta(network))
    post = network.client.post
    if route == "finalize":
        body = Finalize(w=0, final_theta_hash_w1=theta_hash, included=[], entitlements_root=H)
        response = post(
            network.url + "/admin/rounds/0/finalize",
            json=network.signed(fixtures.COORD, "Finalize", body),
            headers=fixtures.admin(),
        )
    elif route == "rollback":
        body = Rollback(
            w=0,
            excluded=[keys[0].ss58],
            old_theta_hash_w2=theta_hash,
            new_theta_hash_w2=theta_hash,
            new_outer_state_hash=H,
            recomputed=["agg_w", "step_w", "agg_w1", "step_w1"],
            cause_hashes=[H],
        )
        response = post(
            network.url + "/admin/rollback",
            json=network.signed(fixtures.COORD, "Rollback", body),
            headers=fixtures.admin(),
        )
    else:
        response = post(network.url + f"/admin/rounds/0/{route}", headers=fixtures.admin())

    assert response.status_code == 409, response.text
    assert response.json()["detail"].startswith("ROUND_RETIRED"), response.text
    assert snapshot(store, run) == before


def test_no_late_anchor_replaces_a_carried_one(network):
    store, run, manifest = network.store, network.manifest.run_id(), network.manifest
    keys = seeded(network, 4, silent=(3,))
    network.push(D_AUDIT)
    retire(network, [k.ss58 for k in keys[:3]])
    hotkey = keys[0].ss58
    carried = store._record_v2(run, "anchor", f"0:{hotkey}")
    # a MATCH audit that finishes after the retirement would persist the real post-round anchor
    cache = AnchorCache()
    late = replace(cache.genesis(manifest, hotkey, genesis_theta(network)), w=0)
    cache.entries[(run, hotkey, 0, cache.layout_hash(manifest))] = late

    with store._tx():
        with pytest.raises(ChallengeError, match="ROUND_RETIRED") as refused:
            store._persist_anchor_v2(manifest, late, cache)

    assert refused.value.status == 409
    assert store._record_v2(run, "anchor", f"0:{hotkey}") == carried


def test_retiring_cancels_the_open_audits_and_keeps_the_verdicts(network):
    store = network.store
    keys = seeded(network, 5)
    jobs = schedule(network)
    assert len(jobs) == 5
    running = lease(network, AUDITOR_A, 1).json()["body"]["job_id"]
    guard = LeaseGuard(10**6, 10**6, time.time() + 3600)  # what execute_audit_v2 registers
    store._lease_guards_v2[running] = guard
    done = next(j for j in jobs if j != running)
    with store._tx():
        store._db.execute(
            "UPDATE audit_leases_v2 SET state='completed',result='MATCH' WHERE id=?", (done,)
        )

    # the round is dead before any audit finishes: three survivors cannot carry it
    retire(network, [k.ss58 for k in keys[:3]])

    rows = {
        r["id"]: (r["state"], r["result"])
        for r in store._db.execute("SELECT id,state,result FROM audit_leases_v2")
    }
    assert rows.pop(done) == ("completed", "MATCH")  # a finished audit stays as evidence
    assert set(rows.values()) == {("exhausted", "ROUND_RETIRED")}
    assert guard.cancelled.is_set() and running not in store._lease_guards_v2
    assert lease(network, AUDITOR_B, 2).status_code == 204


def test_a_failing_carry_leaves_the_round_as_it_was(network, monkeypatch):
    store, run = network.store, network.manifest.run_id()
    keys = seeded(network, 5, silent=(3, 4))
    survivors = [k.ss58 for k in keys[:3]]
    network.push(D_AUDIT)
    before = snapshot(store, run)
    real, carried = ChallengeStore._carry_anchor_v2, []

    def third_member_fails(self, *args):
        if len(carried) == 2:
            raise OSError("no space left on device")
        carried.append(args)
        return real(self, *args)

    monkeypatch.setattr(ChallengeStore, "_carry_anchor_v2", third_member_fails)
    with store._tx():
        with pytest.raises(OSError, match="no space"):
            store._retire_round_v2(run, 0, "INFEASIBLE", survivors)
        # the reaper goes on in the same transaction: the two anchors carried so far are not kept
        assert len(carried) == 2
        assert snapshot(store, run) == before
    assert snapshot(store, run) == before

    # and the next push retires it
    monkeypatch.undo()
    assert retire(network, survivors)["cause"] == "INFEASIBLE"


@pytest.mark.parametrize(
    ("hold", "refusal"),
    [
        (None, None),
        ("early", "ROUND_RETIRE_REFUSED: the commit window is still open"),
        ("stranger", "ROUND_RETIRE_REFUSED: a survivor is not on the roster"),
        ("service", "ROUND_RETIRE_REFUSED: a service-admitted run keeps its roster complete"),
    ],
)
def test_a_retirement_is_refused_unless_every_precondition_holds(network, hold, refusal):
    store, run = network.store, network.manifest.run_id()
    keys = seeded(network, 4, silent=(3,))
    survivors = [k.ss58 for k in keys[:3]]
    if hold != "early":
        network.push(D_AUDIT)
    if hold == "stranger":
        survivors = [*survivors[:2], Keypair(b"\xee" * 32).ss58]
    if hold == "service":
        with store._tx():
            store._put_record_v2(run, "service-admission", "run", {})
    before = snapshot(store, run)

    if refusal is None:
        assert retire(network, survivors)["cause"] == "INFEASIBLE"
    else:
        with pytest.raises(ChallengeError, match=refusal) as refused:
            retire(network, survivors)
        assert refused.value.status == 409
        assert snapshot(store, run) == before


@pytest.mark.parametrize("predecessor", ["applied", "retired"])
def test_a_round_is_retired_only_once_every_round_before_it_is_closed(network, predecessor):
    store, run = network.store, network.manifest.run_id()
    keys = seeded(network, 5, silent=(3, 4))
    survivors = [k.ss58 for k in keys[:3]]
    network.push(D_AUDIT)
    first = retire(network, survivors)
    second_round(network)
    if predecessor == "applied":  # round 0 was aggregated instead: applied, not yet finalized
        with store._tx():
            status = store._record_v2(run, "finality", "0")
            store._put_record_v2(
                run, "finality", "0", {**status, "disposition": "PENDING", "resolution_hash": None}
            )
    before = snapshot(store, run)

    if predecessor == "applied":
        with pytest.raises(
            ChallengeError, match="ROUND_RETIRE_REFUSED: an earlier round is not closed"
        ):
            retire(network, survivors, w=1)
        assert snapshot(store, run) == before
    else:
        second = retire(network, survivors, w=1)
        # the second null round continues the first: same model, the lineage hash chains
        applied = store._record_v2(run, "applied", "1")
        assert applied["predecessor_tape_hash"] == first["evidence_hash"]
        assert applied["out_state"] == store._record_v2(run, "applied", "0")["out_state"]
        assert applied["tape_hash"] == second["evidence_hash"] != first["evidence_hash"]
        # and the anchors chain: the genesis tensors, three names later
        for key in keys:
            origin = store._record_v2(run, "anchor", f"-1:{key.ss58}")
            chained = store._record_v2(run, "anchor", f"1:{key.ss58}")
            assert (chained["state_root"], chained["ef_hash"]) == (
                origin["state_root"],
                origin["ef_hash"],
            )
            assert chained["carried_from"] == 0
            assert (
                chained["anchor_hash"]
                != store._record_v2(run, "anchor", f"0:{key.ss58}")["anchor_hash"]
            )


@pytest.mark.parametrize(
    ("cause", "scheduled", "lag", "retired"),
    [
        ("AUDIT_WINDOW_MISSED", False, 1, True),
        ("AUDIT_WINDOW_MISSED", False, 2, False),
        ("AUDIT_WINDOW_MISSED", True, 1, False),
        ("INFEASIBLE", False, 1, False),
    ],
    ids=["missed", "still-open", "audits-were-scheduled", "five-can-carry"],
)
def test_a_round_nobody_audited_is_retired_when_its_audit_window_is_gone(
    network, cause, scheduled, lag, retired
):
    # Given: five members committed (a feasible round) and the operator never called `audits`
    store, run = network.store, network.manifest.run_id()
    keys = seeded(network, 5)
    survivors = [k.ss58 for k in keys]
    policy = AuditPolicyV2.model_validate_json(
        store.objects.get(network.manifest.network.audit_policy_hash)
    )
    absolute = min(D_AUDIT + 200, D_AUDIT + policy.absolute_rounds())
    network.push(D_AUDIT)
    if scheduled:
        store.schedule_audits_v2(run, 0)
    network.push(absolute - lag)
    before = snapshot(store, run)

    if retired:
        # When: `audits` answers 503 for ever, the reaper retires the round
        with pytest.raises(ChallengeError) as horizon:
            store.schedule_audits_v2(run, 0)
        assert horizon.value.status == 503
        done = retire(network, survivors, cause=cause)
        # Then: it is closed, and `audits` now says so instead of 503
        assert (done["cause"], done["retired_beacon"]) == (cause, absolute - lag)
        assert store._record_v2(run, "finality", "0")["disposition"] == "RETIRED"
        with pytest.raises(ChallengeError, match="ROUND_RETIRED"):
            store.schedule_audits_v2(run, 0)
    else:
        with pytest.raises(ChallengeError, match="ROUND_RETIRE_REFUSED") as refused:
            retire(network, survivors, cause=cause)
        assert refused.value.status == 409
        assert snapshot(store, run) == before


def test_the_round_after_a_retired_one_is_reward_finalized(network, ledger):
    # Given: round 0 retired by the store, round 1 applied and offered to the ledger
    store, run = network.store, network.manifest.run_id()
    keys = seeded(network, 5, silent=(3, 4))
    network.push(D_AUDIT)
    done = retire(network, [k.ss58 for k in keys[:3]])
    _, reward, evidence = signed_round(network, ledger, 1)
    put_finality(store, unresolved(run, 1))
    issued = ledger.balances().issued

    # When: the reward of round 1 is finalized
    ledger.reward_finalize(reward, evidence)

    # Then: it is paid, and round 0 was recorded once as closed without a reward
    assert ledger.balances().issued - issued == ledger.policy.round_reward_units
    assert [(w["w"], w["closure_hash"]) for w in withdrawals(ledger)] == [
        (0, done["evidence_hash"])
    ]
    ledger.verify()


@pytest.mark.parametrize(
    ("state", "pinned"),
    [("retired", False), ("retired-with-a-dispute", True), ("pending", True)],
)
def test_relay_custody_of_a_retired_round_can_be_settled(network, state, pinned):
    # Given: a grant of round 0, whose finality is as `state` says
    store, run = network.store, network.manifest.run_id()
    evidence_hash, grant_hash, beacon = "ab" * 32, "cd" * 32, 77
    status = unresolved(run, 0, applied=False)
    if state != "pending":
        status = close_retired(status, state_hash(genesis_theta(network)), evidence_hash)
    if state == "retired-with-a-dispute":
        status = replace(status, unresolved_disputes=1)
    with store._tx():
        store._put_record_v2(run, "grant", grant_hash, {"body": {"w": 0}})
        store._put_record_v2(run, "finality", "0", asdict(status))
        store._put_record_v2(
            run, "retired", "0", {"evidence_hash": evidence_hash, "retired_beacon": beacon}
        )

    # When / Then: the retirement is the finality of the custody, and an open dispute or an
    # unfinished round still pins it
    if pinned:
        with pytest.raises(ChallengeError, match="pinned by unresolved finality") as refused:
            store.relay_settlement_v2(run, grant_hash)
        assert refused.value.status == 409
    else:
        vesting = beacon + network.manifest.training.verify.E_vest_rounds
        assert store.relay_settlement_v2(run, grant_hash) == asdict(
            Settlement(evidence_hash, beacon, sha256_hex(canonicalize([])), beacon, vesting)
        )
