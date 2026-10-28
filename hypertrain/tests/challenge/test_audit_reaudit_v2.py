"""A withheld auditor signature is re-audited by the other auditor, on real replays.

The late-window shape is the one place where the second auditor is genuinely needed: the first
holds the lease to the last round of the last ordinary window, so before D82 the job was dropped
as an INFRASTRUCTURE exclusion without anybody else being asked. One activated identity, one
seeded round, the master's own replay behind both leases. The sanction path itself is untouched:
only the second lease completing as MISMATCH reaches it.
"""
# ruff: noqa: F811  (the `network` fixture parameter shadows the imported fixture, as in test_multirun.py)

from __future__ import annotations

import json
import time

import pytest

from hypertrain.miner.island_launch import launch_island
from hypertrain.protocol import envelope_v2
from hypertrain.protocol.hashing import MerkleTree
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages_v2 import CommitV2, IslandJobV1

from . import test_service_network_v2 as fixtures
from .audit_lease_fixture_v2 import (
    AUDITOR_A,
    AUDITOR_B,
    D_AUDIT,
    lease,
    miners,
    network,  # noqa: F401  (pytest fixture import)
    schedule,
    seed_round,
)
from .five_identity_network_v2 import activate_identities

A, B = AUDITOR_A, AUDITOR_B


@pytest.fixture
def one_identity(network):
    activate_identities(network, fixtures, range(1))
    return network


def records(network, kind):
    return {
        r[0]: json.loads(r[1])
        for r in network.store._db.execute(
            "SELECT id,data FROM records_v2 WHERE run_id=? AND kind=?",
            (network.manifest.run_id(), kind),
        )
    }


def standing(network, hotkey):
    row = network.store._db.execute(
        "SELECT state,reason,strikes FROM admissions_v2 WHERE hotkey=?", (hotkey,)
    ).fetchone()
    return tuple(row)


def genuine(network, key):
    """Replace the seeded fake leaves of `key` by what an honest miner sealed: its island run."""
    store, run = network.store, network.manifest.run_id()
    ident = f"0:{key.ss58}"
    job = IslandJobV1.model_validate(store._record_v2(run, "island-job", ident))
    directory = store.state_dir / "jobs-v2" / "0" / key.ss58
    artifacts = launch_island(
        job.model_copy(update={"deadline": int(time.time()) + 3600}),
        directory,
        backend="cpu",
        trace=True,
    )
    summary = json.loads((artifacts.directory / "rank-0/summary.json").read_bytes())
    leaves = json.loads(artifacts.leaves.read_bytes())
    sealed = CommitV2.model_validate(
        envelope_v2.parse_envelope(store._record_v2(run, "commit", ident)).body
    )
    commit = sealed.model_copy(
        update={
            "n_leaves": len(leaves),
            "metrics_root": MerkleTree(
                [bytes.fromhex(p["loss_f32"] + p["norm_f32"]) for p in leaves]
            ).root.hex(),
            "delta_bytes": artifacts.delta.stat().st_size,
            **{
                k: summary["commitments"][k]
                for k in ("leaves_root", "final_theta_hash", "ef_out_hash", "delta_hash")
            },
        }
    )
    with store._tx():
        store._put_record_v2(run, "commit", ident, network.signed(key, "CommitV2", commit))
        store._put_record_v2(
            run, "leaves", ident, {"object": store.objects.put(canonicalize(leaves))}
        )


def receipt(network, key, nonce):
    return network.signed(
        key, "Receipt", {"w": 0, "commit_hash": nonce, "received_round": network.now}
    )


def execute(network, key, job):
    path = f"/worker/jobs/{job['job_id']}/execute"
    answer = network.client.post(network.url + path, json=receipt(network, key, job["lease_nonce"]))
    assert answer.status_code == 200, answer.text
    return answer.json()["verdict"]


def complete(network, key, job, verdict):
    path = f"/worker/jobs/{job['job_id']}/complete"
    signed = network.signed(key, "ReplayVerdict", verdict)
    answer = network.client.post(network.url + path, json=signed)
    assert answer.status_code == 200, answer.text
    return answer.json()["result"]


@pytest.mark.parametrize("reaudit", ["MATCH", "MISMATCH"])
def test_a_mismatch_withheld_in_the_last_window_is_decided_by_the_other_auditor(
    one_identity, monkeypatch, reaudit
):
    from hypertrain.auditor import worker
    from hypertrain.auditor.replay import Outcome

    network = one_identity
    store, run = network.store, network.manifest.run_id()
    store.enforce_audit_mismatch = True
    key = miners(1)[0]
    admission = store._db.execute(
        "SELECT admission_id FROM admissions_v2 WHERE hotkey=?", (key.ss58,)
    ).fetchone()[0]
    seed_round(network, n=1, executable=True, gap=100, admission_ids={0: admission})
    if reaudit == "MATCH":
        genuine(network, key)
    escrow = store._services(run)[0]
    burned = escrow.balances().burned
    real, replays = worker.execute_island_audit, []

    def replay(*args, **kwargs):
        outcome, artifacts = real(*args, **kwargs)
        if reaudit == "MATCH" and not replays:  # the master's one-off glitch
            outcome = Outcome("MISMATCH", 0, "ab" * 32)
        replays.append(outcome.result)
        return outcome, artifacts

    monkeypatch.setattr(worker, "execute_island_audit", replay)
    schedule(network)
    network.push(D_AUDIT + 100)  # the first window ends unleased: the second opens
    network.push(D_AUDIT + 250)  # A takes it late, its lease runs to the last round of the window
    first = lease(network, A, 1).json()["body"]
    assert first["lease_expires"] == first["absolute_deadline"] == D_AUDIT + 300
    assert execute(network, A, first)["result"] == "MISMATCH"
    network.push(first["absolute_deadline"])  # A never signs it
    failure = records(network, "audit-attempt-failure")[f"{first['job_id']}:{first['lease_nonce']}"]
    assert (failure["cause"], failure.get("withheld")) == ("AUDITOR_SILENT", "MISMATCH")
    assert (records(network, "excluded"), records(network, "exit-sanction")) == ({}, {})
    assert records(network, "exit-failed") == {}
    # the window began when the sign window ended the lease (round 290), 30 rounds before the push
    # that noticed it, and lasts one lease: its own, not a third ordinary window from the push
    window = records(network, "reaudit")[f"0:{key.ss58}"]
    assert window == {"job_id": first["job_id"], "detected": D_AUDIT + 270, "until": D_AUDIT + 370}
    assert lease(network, A, 2).status_code == 403  # A is cooling, and the job is not A's to retake
    leased = lease(network, B, 3)
    assert leased.status_code == 200, leased.text
    second = leased.json()["body"]
    assert (second["job_id"], second["attempt"]) == (first["job_id"], 1)
    assert second["absolute_deadline"] == D_AUDIT + 370
    verdict = execute(network, B, second)
    assert verdict["result"] == reaudit
    assert complete(network, B, second, verdict) == reaudit
    assert replays == (["MISMATCH", "MATCH"] if reaudit == "MATCH" else ["MISMATCH", "MISMATCH"])
    network.push(network.now + 1)
    due = network.manifest.training.verify.T.dispute_per_level
    if reaudit == "MATCH":
        assert records(network, "excluded") == {} and records(network, "exit-sanction") == {}
        assert [w.roster.hotkey for w in store.verified_inputs_v2(run, 0)] == [key.ss58]
        network.push(network.now + due + 1)
        assert records(network, "exit-sanction") == {}
        assert standing(network, key.ss58) == ("ACTIVE", "JOIN_ACCEPTED", 0)
        assert escrow.balances().burned == burned
    else:
        sanction = records(network, "exit-sanction")[f"0:{key.ss58}"]
        assert (sanction["state"], sanction["auditor"]) == ("PENDING", B.ss58)
        network.push(sanction["first_seen"] + due)
        assert records(network, "exit-sanction")[f"0:{key.ss58}"]["state"] == "APPLIED"
        assert standing(network, key.ss58) == ("SUSPENDED", "AUDIT_MISMATCH", 1)
        assert escrow.balances().burned - burned == 1000
        assert escrow.balances().conserved()
