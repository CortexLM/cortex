"""Real tiny audit geometry publication binds the accepted lease descriptor."""

from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path

import pytest

from hypertrain.auditor.replay import AnchorCache, optimizer_hash, pack_state
from hypertrain.challenge.store import ChallengeError
from hypertrain.miner.island_launch import launch_island
from hypertrain.protocol import envelope_v2
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages import LeafPreimage, Receipt
from hypertrain.protocol.messages_v2 import (
    AuditChallengeV2,
    AuditJobV2,
    CommitV2,
    IslandJobV1,
    PolicyHashes,
    RoundOpenV2,
    StartStateV2,
)
from hypertrain.trainer.compress import state_hash
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.model import init_params

spec = importlib.util.spec_from_file_location(
    "audit_publication_fixture", Path(__file__).parent / "test_service_network_v2.py"
)
assert spec is not None and spec.loader is not None
fixtures = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = fixtures
spec.loader.exec_module(fixtures)
network = fixtures.network


def test_audit_roster_rejects_unaccepted_round_without_mutation(network):
    store = network.store
    before = store._db.total_changes
    with pytest.raises(ChallengeError, match="missing accepted round: 0"):
        store.require_roster_v2(network.manifest.run_id(), 0)
    assert store._db.total_changes == before


def test_real_audit_publication_completes_with_exact_lease_descriptor(network, monkeypatch):
    store, manifest = network.store, network.manifest
    run = manifest.run_id()
    hot, auditor = fixtures.HOT[0], fixtures.AUDITORS[0]
    # Real signed admission/proof/finality path used by neighboring service integration.
    path = Path(__file__).parents[1] / "e2e/test_service_network_v2_e2e.py"
    spec = importlib.util.spec_from_file_location("audit_round_acceptance_fixture", path)
    assert spec is not None and spec.loader is not None
    rounds = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = rounds
    spec.loader.exec_module(rounds)
    rounds._graduate_four(network)
    cfg = TrainConfig.from_manifest_v2(manifest)
    theta = init_params(cfg.model)
    cache = AnchorCache()
    starts, roster = [], []
    admission = store._services(run)[1]
    for slot, key in enumerate(fixtures.HOT):
        status = admission.status(key.ss58, now=network.now)
        assert status.eligible and status.record.state == "ACTIVE"
        actual = cache.genesis(manifest, key.ss58, theta)
        starts.append(
            StartStateV2(
                run_id=run,
                w=0,
                hotkey=key.ss58,
                theta_hash=state_hash(theta),
                state_object_sha256=sha256_hex(pack_state(theta, actual.state)),
                opt_state_hash=optimizer_hash(actual.state),
                ef_object_sha256=sha256_hex(pack_state(actual.ef)),
                ef_hash=state_hash(actual.ef),
                parent_anchor_hash=actual.anchor_hash,
                global_step0=0,
                anchor_verdict_hash=actual.proof_hash,
            )
        )
        roster.append(
            {
                "hotkey": key.ss58,
                "slot": slot,
                "q_i": "0000803f",
                "admission_id": status.record.admission_id,
                "coldkey_group": status.record.coldkey,
                "state": status.record.state,
                "eligible_weight": 4194304,
            }
        )
    opening = RoundOpenV2(
        w=0,
        prev_final_hash="0" * 64,
        theta_hash=state_hash(theta),
        outer_state_hash="0" * 64,
        center_hash="0" * 64,
        roster_hash=sha256_hex(canonicalize(roster)),
        honeypot_commit="0" * 64,
        d_open=network.now + 1,
        d_assign=network.now + 2,
        d_commit=network.now + 3,
        d_audit=network.now + 4,
        d_upload=network.now + 5,
        d_final=network.now + 7,
        contract_version=2,
        policy_hashes=PolicyHashes.model_validate(
            {name: getattr(manifest.network, name) for name in PolicyHashes.model_fields}
        ),
        registry_epoch=0,
        start_state_index_hash=sha256_hex(canonicalize([s.body() for s in starts])),
        audit_mode="anchored-full",
        roster=roster,
    )
    before = store._db.total_changes
    with pytest.raises(ChallengeError, match="missing accepted round: 0"):
        store.require_roster_v2(run, 0)
    assert store._db.total_changes == before
    signed_opening = network.signed(fixtures.COORD, "RoundOpenV2", opening)
    response = network.client.post(
        network.url + "/admin/rounds", json=signed_opening, headers=fixtures.admin()
    )
    assert response.status_code == 200, response.text
    assert store._record_v2(run, "round", "0") == signed_opening
    assert store.require_roster_v2(run, 0) is None
    anchor = cache.genesis(manifest, hot.ss58, theta)
    store._persist_anchor_v2(manifest, anchor, cache)
    source = store.state_dir / "jobs-v2/0" / hot.ss58
    source.mkdir(parents=True)
    dataset = store._record_v2(run, "dataset", "inputs")
    sample_ids = list(range(manifest.training.batch_samples()))
    width = (cfg.model.seq_len + 1) * (
        2 if manifest.training.dataset.sample_format.startswith("u16") else 4
    )
    raw_samples = store.objects.get(dataset["samples_hash"])
    proofs = json.loads(store.objects.get(dataset["proofs_hash"]))
    inputs = {
        "start_state": pack_state(theta),
        "ef_in": pack_state(anchor.ef),
        "v0": pack_state({}),
        "samples": b"".join(raw_samples[i * width : (i + 1) * width] for i in sample_ids),
        "sample_proofs": canonicalize([proofs[i] for i in sample_ids]),
    }
    hashes = {}
    for name, raw in inputs.items():
        (source / name).write_bytes(raw)
        hashes[name] = store.objects.put(raw)
    original = IslandJobV1(
        job_version=1,
        run_id=run,
        w=0,
        manifest=manifest,
        sample_ids=sample_ids,
        global_step0=0,
        start_state_sha256=hashes["start_state"],
        ef_in_sha256=hashes["ef_in"],
        v0_sha256=hashes["v0"],
        object_paths={k: k for k in inputs},
        deadline=1,
    )
    job_id, nonce = "aa" * 32, "bb" * 32
    created = network.now
    expiry = created + 99
    deadline = manifest.training.beacon.genesis_time + (expiry - 1) * 3
    published = launch_island(
        original.model_copy(update={"deadline": deadline}), source, backend="cpu"
    )
    summary = json.loads((published.directory / "rank-0/summary.json").read_bytes())["commitments"]
    leaves = [LeafPreimage.model_validate(p) for p in json.loads(published.leaves.read_bytes())]
    start_blob = store.objects.put(pack_state(theta, anchor.state))
    start = StartStateV2(
        run_id=run,
        w=0,
        hotkey=hot.ss58,
        theta_hash=state_hash(theta),
        state_object_sha256=start_blob,
        opt_state_hash=optimizer_hash(anchor.state),
        ef_object_sha256=hashes["ef_in"],
        ef_hash=state_hash(anchor.ef),
        parent_anchor_hash=anchor.anchor_hash,
        global_step0=0,
        anchor_verdict_hash=anchor.proof_hash,
    )
    commit = CommitV2(
        w=0,
        hotkey=hot.ss58,
        leaf_scheme="ht-leaf-v1",
        n_leaves=len(leaves),
        metrics_root="11" * 32,
        tokens=len(sample_ids) * cfg.model.seq_len,
        delta_bytes=published.delta.stat().st_size,
        ef_in_hash=state_hash(anchor.ef),
        **{k: summary[k] for k in ("leaves_root", "final_theta_hash", "ef_out_hash", "delta_hash")},
    )
    challenge = AuditChallengeV2(
        w=0,
        target=hot.ss58,
        beacon_round=created,
        beacon_sig_sha256="11" * 32,
        mode="full",
        segments=[],
        reasons=["final"],
        serve_deadline=expiry,
        anchor_hash=start.digest(),
        audit_mode="anchored-full",
    )
    audit = AuditJobV2(
        run_id=run,
        job_id=job_id,
        auditor_id=auditor.ss58,
        attempt=1,
        lease_nonce=nonce,
        lease_expires=expiry,
        absolute_deadline=expiry,
        reservation_id=nonce,
        replay_step_budget=cfg.inner.H,
        anchor_age=0,
        manifest=manifest,
        challenge_envelope=network.signed(fixtures.COORD, "AuditChallengeV2", challenge),
        commit_envelope=network.signed(hot, "CommitV2", commit),
        sample_ids=sample_ids,
        start_state=start,
        preimages=leaves,
        ef_in={"sha256": hashes["ef_in"], "size": len(inputs["ef_in"])},
        v0={"sha256": hashes["v0"], "size": len(inputs["v0"])},
        created_beacon=created,
    )
    with store._tx():
        store._put_record_v2(run, "island-job", "0:" + hot.ss58, original.body())
        store._put_record_v2(run, "audit-job", job_id, audit.body())
        store._put_record_v2(run, "assignment", "0:" + hot.ss58, {"assignment_hash": "11" * 32})
        store._put_record_v2(run, "finality", "0", {"unresolved_audits": 1})
        store._db.execute(
            "INSERT INTO audit_leases_v2(id,run_id,w,hotkey,challenge,created,absolute,"
            "state,attempts,auditor,nonce,expires) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)",
            (
                job_id,
                run,
                0,
                hot.ss58,
                "{}",
                created,
                expiry,
                "running",
                1,
                auditor.ss58,
                nonce,
                expiry,
            ),
        )
    from hypertrain.auditor import worker
    from hypertrain.auditor.bisect import RefereeError
    from hypertrain.miner.island_launch import IslandFailure

    audit_launches = []

    def independent_launch(job, directory, **kwargs):
        audit_launches.append(job)
        assert kwargs["trace"] is True
        assert directory == store.state_dir / "audit-geometry-v2" / job_id / nonce
        assert directory != source
        return launch_island(job, directory, **kwargs)

    monkeypatch.setattr(worker, "launch_island", independent_launch)
    result = store.execute_audit_v2(
        run,
        job_id,
        canonicalize(
            network.signed(
                auditor, "Receipt", Receipt(w=0, commit_hash=nonce, received_round=created)
            )
        ),
    )
    assert audit_launches == [original.model_copy(update={"deadline": deadline})]
    assert result["verdict"]["result"] == "MATCH"
    signed = network.signed(auditor, "ReplayVerdict", result["verdict"])
    record = store._record_v2(run, "audit-geometry", job_id + ":" + nonce)
    descriptor = envelope_v2.load_json(store.objects.get(record["descriptor_hash"]))
    assert descriptor["job"] == original.model_copy(update={"deadline": deadline}).body()
    assert store._record_v2(run, "island-job", "0:" + hot.ss58) == original.body()
    publication = Path(descriptor["directory"]) / "published"
    for name in ("summary.json", "trace.json"):
        path = publication / "rank-0" / name
        original_bytes = path.read_bytes()
        changed = json.loads(original_bytes)
        if name == "summary.json":
            changed["job_hash"] = "00" * 32
        else:
            changed[0]["rank"] = 1
        path.write_bytes(canonicalize(changed))
        with pytest.raises((IslandFailure, RefereeError, ValueError)):
            store.complete_audit_v2(run, job_id, canonicalize(signed))
        assert (
            store._db.execute("SELECT state FROM audit_leases_v2 WHERE id=?", (job_id,)).fetchone()[
                0
            ]
            == "running"
        )
        path.write_bytes(original_bytes)
    for fault in ("deadline", "lease", "signature"):
        bad = dict(record)
        if fault == "signature":
            env = envelope_v2.parse_envelope(record["receipt"])
            bad["receipt"] = network.signed(hot, "Receipt", env.body)
        else:
            changed = json.loads(canonicalize(descriptor))
            if fault == "deadline":
                changed["job"]["deadline"] += 1
            else:
                changed["lease_nonce"] = "cc" * 32
            bad["descriptor_hash"] = store.objects.put(canonicalize(changed))
            bad["receipt"] = network.signed(
                fixtures.COORD,
                "Receipt",
                Receipt(w=0, commit_hash=bad["descriptor_hash"], received_round=created),
            )
        store._put_record_v2(run, "audit-geometry", job_id + ":" + nonce, bad)
        with pytest.raises(ChallengeError, match="geometry authority"):
            store.complete_audit_v2(run, job_id, canonicalize(signed))
        assert (
            store._db.execute("SELECT state FROM audit_leases_v2 WHERE id=?", (job_id,)).fetchone()[
                0
            ]
            == "running"
        )
        store._put_record_v2(run, "audit-geometry", job_id + ":" + nonce, record)
    assert store.complete_audit_v2(run, job_id, canonicalize(signed)) == {"result": "MATCH"}
    assert store.complete_audit_v2(run, job_id, canonicalize(signed)) == {"result": "MATCH"}
    geometry = store._record_v2(run, "trace-geometry", sha256_hex(canonicalize(result["verdict"])))
    assert geometry["job"] == descriptor["job"]
    # Given: genuine accepted historical publication, a live current dispute.
    import time

    from hypertrain.challenge.disputes_v2 import Contest, Turn
    from hypertrain.miner import island_launch
    from hypertrain.miner.island_launch import IslandFailure

    verdict_hash = sha256_hex(canonicalize(result["verdict"]))
    contest = Contest.model_validate(store._record_v2(run, "contest", verdict_hash))
    dispute_id, transcript = "dd" * 32, "ee" * 32
    turn = Turn(
        run_id=run,
        dispute_id=dispute_id,
        contest=contest,
        seq=0,
        level="step",
        ctx=[],
        interval=(0, 1),
        expected_party=hot.ss58,
        transcript_hash=transcript,
        turn_deadline=created + 109,
        absolute_deadline=created + 129,
        lock_id="ff" * 32,
    )
    with store._tx():
        store._db.execute(
            "INSERT INTO disputes_v2 VALUES(?,?,?,?,?)",
            (dispute_id, verdict_hash, hot.ss58, turn.model_dump_json(), "{}"),
        )
    now = created + 100
    monkeypatch.setattr(store, "_now", lambda _: now)
    monkeypatch.setattr(store, "_snapshot_current_v2", lambda _: None)
    monkeypatch.setattr(time, "time", lambda: deadline + 1)
    launches = []

    def replay(job, directory, **kwargs):
        launches.append(job)
        assert job.deadline == manifest.training.beacon.genesis_time + (created + 128) * 3
        accepted = store._record_v2(run, "referee-job", dispute_id + ":" + transcript)
        receipt = envelope_v2.parse_envelope(accepted["receipt"])
        assert receipt.signer == fixtures.COORD.ss58
        assert envelope_v2.verify_envelope(accepted["receipt"])
        frozen = envelope_v2.load_json(store.objects.get(accepted["descriptor_hash"]))
        assert frozen["job"] == job.body()
        assert frozen["historical_geometry_hash"] == sha256_hex(canonicalize(geometry))
        return launch_island(job, directory, **kwargs)

    monkeypatch.setattr(island_launch, "launch_island", replay)
    # When: invalid historical publication or expired current dispute cannot launch.
    wrong = json.loads(canonicalize(geometry))
    wrong["job"]["deadline"] += 1
    store._put_record_v2(run, "trace-geometry", verdict_hash, wrong)
    with pytest.raises(IslandFailure, match="job binding"):
        store.referee_v2(run, dispute_id)
    assert not launches
    store._put_record_v2(run, "trace-geometry", verdict_hash, geometry)
    now = created + 129
    with pytest.raises(ChallengeError, match="absolute horizon"):
        store.referee_v2(run, dispute_id)
    assert not launches
    now = created + 100
    # Then: actual fresh replay validates separate outputs, registers independent evidence.
    evidence = store.referee_v2(run, dispute_id)
    assert evidence["evidence"]["reason"] == "MATCH"
    assert len(launches) == 1
    assert launches[0] == original.model_copy(
        update={"deadline": manifest.training.beacon.genesis_time + (created + 128) * 3}
    )
    assert store._record_v2(run, "trace-geometry", verdict_hash) == geometry
    assert store._record_v2(run, "island-job", "0:" + hot.ss58) == original.body()
    assert (
        store._db.execute(
            "SELECT body FROM dispute_evidence_v2 WHERE hash=?", (evidence["evidence_hash"],)
        ).fetchone()
        is not None
    )
    assert not store._lease_guards_v2
