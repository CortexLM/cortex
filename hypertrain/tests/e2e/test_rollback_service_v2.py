"""Real signed HTTP two-round carry repair, distinct anchors and fresh checkpoint replay."""

from __future__ import annotations

import importlib.util
import json
import os
import shutil
import sqlite3
import sys
import time
from pathlib import Path

import pytest

from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize

spec = importlib.util.spec_from_file_location(
    "rollback_service_fixture",
    Path(__file__).parents[1] / "challenge/test_service_network_v2.py",
)
assert spec is not None and spec.loader is not None
fixtures = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = fixtures
spec.loader.exec_module(fixtures)


@pytest.fixture(scope="module")
def active_network(tmp_path_factory):
    from hypertrain.auditor.replay import unpack_state
    from hypertrain.data.trial_assignment import trial_samples
    from hypertrain.miner.island_launch import validate_artifacts
    from hypertrain.protocol.keys import Keypair
    from hypertrain.protocol.messages import Finalize
    from hypertrain.protocol.messages_v2 import (
        EscrowLock,
        JoinChallenge,
        RunManifestV2,
        WorkProof,
        WorkScreenV2,
    )
    from hypertrain.trainer.compress import state_hash

    patch = pytest.MonkeyPatch()
    fixtures.HOT = [Keypair(bytes([80 + i]) * 32) for i in range(5)]
    fixtures.COLD = [Keypair(bytes([90 + i]) * 32) for i in range(5)]
    original = fixtures.fixture.setup
    cache = Path("/tmp/hypertrain-rollback-active-carry")
    baseline_time = int(time.time())
    if (cache / "challenge.db").exists():
        with sqlite3.connect(cache / "challenge.db") as db:
            cached_manifest = json.loads(db.execute("SELECT manifest FROM runs").fetchone()[0])
        baseline_time = cached_manifest["training"]["beacon"]["genesis_time"] + 1
    real_time = time.time
    offset = real_time() - baseline_time
    patch.setattr(time, "time", lambda: real_time() - offset)
    directory = tmp_path_factory.mktemp("service-rollback")
    clock = directory / "clock"
    clock.mkdir()
    (clock / "sitecustomize.py").write_text(
        "import os,time\n_real=time.time\n"
        "_offset=float(os.environ['HT_FIXTURE_CLOCK_OFFSET'])\n"
        "time.time=lambda:_real()-_offset\n",
    )
    patch.setenv("HT_FIXTURE_CLOCK_OFFSET", str(offset))
    patch.setenv("PYTHONPATH", str(clock) + os.pathsep + os.environ.get("PYTHONPATH", ""))

    def carry_setup():
        setup = original()
        body = setup.manifest.body()
        body["training"]["inner"].update(state_policy="carry", rewarmup_steps=0)
        return fixtures.fixture.Setup(
            RunManifestV2.model_validate(body),
            setup.policy,
            setup.admission_policy,
            setup.rows,
            setup.tree,
        )

    patch.setattr(fixtures.fixture, "setup", carry_setup)
    original_genesis = fixtures.Genesis

    def full_genesis(**fields):
        fields["total_units"] = sum(origin.units for origin in fields["origins"])
        return original_genesis(**fields)

    patch.setattr(fixtures, "Genesis", full_genesis)
    generator = fixtures.network.__wrapped__(directory, type("Request", (), {"param": None})())
    network = next(generator)
    try:
        restored = False
        if (cache / "challenge.db").exists():
            with sqlite3.connect(cache / "challenge.db") as db:
                run_id = db.execute("SELECT run_id FROM runs").fetchone()[0]
                assert run_id == network.manifest.run_id(), (
                    "activation cache requires SAME manifest"
                )
                entries = db.execute(
                    "SELECT hotkey,state,clean_count,admission_id FROM admissions_v2"
                ).fetchall()
                assert {e[0] for e in entries} == {h.ss58 for h in fixtures.HOT}
                assert all(e[1:3] == ("ACTIVE", 12) for e in entries)
                for e in entries:
                    assert (
                        db.execute(
                            "SELECT COUNT(*) FROM admission_trial_results r "
                            "JOIN admission_trials t "
                            "ON t.epoch=r.epoch WHERE t.admission_id=? AND r.outcome='MATCH'",
                            (e[3],),
                        ).fetchone()[0]
                        == 12
                    )
                network.store._services_v2.clear()
                db.backup(network.store._db)
            for name in ("objects", "trials-v2"):
                shutil.copytree(cache / name, network.store.state_dir / name, dirs_exist_ok=True)
            network.now = network.store._now(network.store._db)
            network.push(network.now)
            restored = True
        if not restored:
            for i in range(5):
                if i:
                    network.push(network.now + 101)
                response = network.join(i)
                assert response.status_code == 200, response.text
                identity = response.json()["admission_id"]
                lock = EscrowLock(
                    operation_id=f"{i + 1:064x}",
                    owner=fixtures.COLD[i].ss58,
                    units=1000,
                    origin_ids=[network.origin_ids[i]],
                    admission_id=identity,
                    dispute_id=None,
                    kind="LOCK_ADMISSION",
                )
                response = network.client.post(
                    network.url + "/escrow/lock",
                    json=network.signed(
                        fixtures.COLD[i],
                        "EscrowLock",
                        lock,
                    ),
                )
                assert response.status_code == 200, response.text
                for _ in range(12):
                    network.push(network.now + 1)
                    response = network.client.get(network.url + "/join/" + identity + "/challenge")
                    assert response.status_code == 200, response.text
                    challenge = JoinChallenge.model_validate(response.json()["body"])
                    response = network.client.post(
                        network.url + "/admin/join/" + identity + "/reference",
                        headers=fixtures.admin(),
                    )
                    assert response.status_code == 200, response.text
                    row = network.store._db.execute(
                        "SELECT * FROM admissions_v2 WHERE admission_id=?",
                        (identity,),
                    ).fetchone()
                    proof = WorkProof.model_validate_json(row["reference_json"])
                    job, attempt = network.store.stage_trial_v2(
                        network.manifest.run_id(),
                        challenge,
                        tuple(
                            trial_samples(
                                network.manifest,
                                identity,
                                challenge.nonce,
                                network.store._beacon_v2(challenge.seed_beacon),
                            )
                        ),
                        row["trial_epoch"],
                    )
                    artifacts = validate_artifacts(job, attempt / "published")
                    screen = WorkScreenV2(
                        evidence_kind="work-proof-v1",
                        admission_id=identity,
                        nonce=challenge.nonce,
                        policy_hash=challenge.policy_hash,
                        image_digest=job.manifest.training.reference_spec.image_digest,
                        layout=challenge.layout,
                        challenge_hash=challenge.digest(),
                        rank_results=list(artifacts.ranks),
                        artifact_hashes=[r.sha256 for r in proof.artifact_refs],
                    )
                    response = network.client.post(
                        network.url + "/join/proof",
                        json={
                            "proof": network.signed(fixtures.HOT[i], "WorkProof", proof),
                            "screen": network.signed(fixtures.HOT[i], "WorkScreenV2", screen),
                        },
                    )
                    assert response.status_code == 200, response.text
                    theta, _ = unpack_state(
                        network.store.objects.get(proof.artifact_refs[0].sha256)
                    )
                    final = Finalize(
                        w=row["trial_epoch"],
                        final_theta_hash_w1=state_hash(theta),
                        included=[fixtures.HOT[i].ss58],
                        entitlements_root=proof.digest(),
                    )
                    response = network.client.post(
                        network.url + "/admin/join/" + identity + "/finalize",
                        json=network.signed(fixtures.COORD, "Finalize", final),
                        headers=fixtures.admin(),
                    )
                    assert response.status_code == 200, response.text
            cache.mkdir(exist_ok=True)
            with sqlite3.connect(cache / "challenge.db") as db:
                network.store._db.backup(db)
            for name in ("objects", "trials-v2"):
                shutil.copytree(network.store.state_dir / name, cache / name, dirs_exist_ok=True)
        yield network
    finally:
        generator.close()
        patch.undo()


def execute_round(network, w, *, open_only=False):
    import anyio
    import httpx
    import torch

    from hypertrain.aggregator.core import load_state
    from hypertrain.auditor.replay import AnchorCache, optimizer_hash, pack_state
    from hypertrain.data.store import LocalFSStore
    from hypertrain.data.stream_store import StreamStore
    from hypertrain.miner.island_launch import launch_island
    from hypertrain.protocol import relay_envelope
    from hypertrain.protocol.hashing import MerkleTree, sha256_hex
    from hypertrain.protocol.jcs import canonicalize
    from hypertrain.protocol.messages import f32hex
    from hypertrain.protocol.messages_v2 import (
        ArtifactRef,
        CommitV2,
        IslandJobV1,
        PolicyHashes,
        RoundOpenV2,
        StartStateV2,
        WorkProof,
    )
    from hypertrain.protocol.relay_messages import UploadChunk, UploadChunkManifest
    from hypertrain.relay.app import create_app
    from hypertrain.relay.client import RelayClient
    from hypertrain.relay.core import Relay
    from hypertrain.trainer.compress import state_hash
    from hypertrain.trainer.config import TrainConfig
    from hypertrain.trainer.model import init_params

    manifest = network.manifest
    cache = AnchorCache()
    theta = init_params(TrainConfig.from_manifest_v2(manifest).model)
    if w:
        previous = network.store._record_v2(manifest.run_id(), "applied", str(w - 1))
        theta = {
            n: torch.from_numpy(x.copy())
            for n, x in load_state(network.store.objects, previous["out_state"]).theta.items()
        }
    starts, roster = [], []
    hotkeys = fixtures.HOT[:4] if w == 2 else fixtures.HOT
    for i, hot in enumerate(hotkeys):
        status = network.store._services(manifest.run_id())[1].status(hot.ss58, now=network.now)
        anchor = (
            network.store._restore_anchor_v2(manifest.run_id(), hot.ss58, w - 1, cache)
            if w
            else cache.genesis(manifest, hot.ss58, theta)
        )
        starts.append(
            StartStateV2(
                run_id=manifest.run_id(),
                w=w,
                hotkey=hot.ss58,
                theta_hash=state_hash(theta),
                state_object_sha256=sha256_hex(pack_state(theta, anchor.state)),
                opt_state_hash=optimizer_hash(anchor.state),
                ef_object_sha256=sha256_hex(pack_state(anchor.ef)),
                ef_hash=state_hash(anchor.ef),
                parent_anchor_hash=anchor.anchor_hash,
                global_step0=anchor.state.step,
                anchor_verdict_hash=anchor.proof_hash,
            )
        )
        roster.append(
            {
                "hotkey": hot.ss58,
                "slot": i,
                "q_i": f32hex(1),
                "admission_id": status.record.admission_id,
                "coldkey_group": status.record.coldkey,
                "state": "ACTIVE",
                "eligible_weight": 4194304,
            }
        )
    opening = RoundOpenV2(
        w=w,
        prev_final_hash="0" * 64,
        theta_hash=state_hash(theta),
        outer_state_hash="0" * 64,
        center_hash="0" * 64,
        roster_hash=sha256_hex(canonicalize(roster)),
        honeypot_commit="0" * 64,
        d_open=network.now + 1,
        d_assign=network.now + 2,
        d_commit=network.now + 30,
        d_audit=network.now + 31,
        d_upload=network.now + 150,
        d_final=network.now + 200,
        contract_version=2,
        policy_hashes=PolicyHashes.model_validate(
            {k: getattr(manifest.network, k) for k in PolicyHashes.model_fields}
        ),
        registry_epoch=0,
        start_state_index_hash=sha256_hex(canonicalize([s.body() for s in starts])),
        audit_mode="anchored-full",
        roster=roster,
    )
    response = network.client.post(
        network.url + "/admin/rounds",
        json=network.signed(
            fixtures.COORD,
            "RoundOpenV2",
            opening,
        ),
        headers=fixtures.admin(),
    )
    assert response.status_code == 200, response.text
    if open_only:
        return response
    network.push(opening.d_assign)
    response = network.client.get(network.url + f"/rounds/{w}")
    assert response.status_code == 200, response.text
    payloads = []
    for i, hot in enumerate(fixtures.HOT):
        assignment = next(a for a in response.json()["assignment"] if a["hotkey"] == hot.ss58)
        screen = json.loads(
            network.store._db.execute(
                "SELECT screen_json FROM admissions_v2 WHERE hotkey=?",
                (hot.ss58,),
            ).fetchone()[0]
        )
        accepted = network.client.post(
            network.url + "/accept",
            json=network.signed(
                hot,
                "AcceptV2",
                {
                    "w": w,
                    "hotkey": hot.ss58,
                    "assignment_hash": assignment["assignment_hash"],
                    "image_digest": manifest.training.reference_spec.image_digest,
                    "driver_version": manifest.training.reference_spec.driver_allowlist[0],
                    "n_gpus": 1,
                    "work_screen_hash": sha256_hex(canonicalize(screen)),
                },
            ),
        )
        assert accepted.status_code == 200, accepted.text
        staged = network.store.island_job_v2(manifest.run_id(), w, hot.ss58)
        job = IslandJobV1.model_validate(staged["job"])
        artifacts = launch_island(
            job, network.store.state_dir / "jobs-v2" / str(w) / hot.ss58, backend="cpu", trace=True
        )
        summary = json.loads((artifacts.directory / "rank-0/summary.json").read_bytes())
        leaves = json.loads(artifacts.leaves.read_bytes())
        commitments = summary["commitments"]
        commit = CommitV2.model_validate(
            {
                "w": w,
                "hotkey": hot.ss58,
                "leaf_scheme": "ht-leaf-v1",
                "n_leaves": len(leaves),
                "metrics_root": MerkleTree(
                    [bytes.fromhex(p["loss_f32"] + p["norm_f32"]) for p in leaves]
                ).root.hex(),
                "tokens": len(job.sample_ids) * manifest.training.model.seq_len,
                "ef_in_hash": starts[i].ef_hash,
                **{
                    k: commitments[k]
                    for k in (
                        "leaves_root",
                        "final_theta_hash",
                        "ef_out_hash",
                        "delta_hash",
                    )
                },
                "delta_bytes": artifacts.delta.stat().st_size,
            }
        )
        accepted = network.client.post(
            network.url + "/commit", json=network.signed(hot, "CommitV2", commit)
        )
        assert accepted.status_code == 200, accepted.text
        refs = []
        for path in (artifacts.state, artifacts.ef, artifacts.delta, artifacts.leaves):
            raw = path.read_bytes()
            refs.append(ArtifactRef(sha256=network.store.objects.put(raw), size=len(raw)))
        proof = WorkProof(
            admission_id=roster[i]["admission_id"],
            challenge_hash=sha256_hex(canonicalize(commit.model_dump(mode="json"))),
            leaves_root=commit.leaves_root,
            delta_hash=commit.delta_hash,
            artifact_refs=refs,
        )
        accepted = network.client.post(
            network.url + "/leaves", json=network.signed(hot, "WorkProof", proof)
        )
        assert accepted.status_code == 200, accepted.text
        payloads.append((hot, artifacts.delta))

    async def transport():
        async with httpx.AsyncClient() as backing_http:

            async def now():
                return network.now

            registry = network.store._registry_v2(manifest)
            assigned = network.store.relay_assignment_v2(manifest.run_id(), w, fixtures.HOT[0].ss58)
            relay = Relay(
                run_id=manifest.run_id(),
                master=fixtures.COORD.ss58,
                registry=registry,
                network_manifest_hash=sha256_hex(
                    canonicalize(assigned["network_manifest"]["body"])
                ),
                observers={h.ss58 for h in fixtures.AUDITORS},
                relay_id="local",
                region="local",
                keys={"k1": fixtures.RELAY},
                active_key="k1",
                backing=StreamStore(
                    LocalFSStore(network.store.state_dir / f"relay-{w}"), backing_http
                ),
                now=now,
            )
            async with httpx.AsyncClient(
                transport=httpx.ASGITransport(app=create_app(relay, "x" * 32)),
                base_url="https://relay.test",
            ) as client:
                for hot, path in payloads:
                    assigned = network.store.relay_assignment_v2(manifest.run_id(), w, hot.ss58)
                    raw = path.read_bytes()
                    chunks = UploadChunkManifest(
                        size=len(raw),
                        chunks=[
                            UploadChunk(index=0, off=0, len=len(raw), chunk_sha256=sha256_hex(raw))
                        ],
                    )
                    grant = network.store.upload_grant_v2(
                        manifest.run_id(),
                        canonicalize(
                            relay_envelope.seal(
                                hot,
                                "UploadChunkManifest",
                                manifest.run_id(),
                                chunks,
                                opening.d_upload,
                            )
                        ),
                    )
                    receipt = await RelayClient(registry, client).upload(
                        path,
                        grant_raw=relay_envelope.parse_envelope(grant),
                        assignment=relay_envelope.parse_envelope(assigned["assignment"]),
                        round_open=network.signed(fixtures.COORD, "RoundOpenV2", opening),
                    )
                    acceptance = await network.store.relay_receipt_v2(
                        canonicalize(receipt.model_dump(mode="json")), client
                    )
                    accepted = network.client.post(
                        network.url + "/delta",
                        json=network.signed(
                            hot,
                            "DeltaManifestV2",
                            {
                                "w": w,
                                "hotkey": hot.ss58,
                                "delta_hash": sha256_hex(raw),
                                "size": len(raw),
                                "uri": "sha256:" + sha256_hex(raw),
                                "format": "ht-sparse-v1",
                                "chunks": [{"off": 0, "len": len(raw), "sha256": sha256_hex(raw)}],
                                "grant_hash": sha256_hex(canonicalize(grant["body"])),
                                "master_acceptance_hash": sha256_hex(
                                    canonicalize(acceptance["body"])
                                ),
                            },
                        ),
                    )
                    assert accepted.status_code == 200, accepted.text

    anyio.run(transport)
    network.push(opening.d_audit)
    accepted = network.client.post(
        network.url + f"/admin/rounds/{w}/audits", headers=fixtures.admin()
    )
    assert accepted.status_code == 200, accepted.text
    for i in range(5):
        auditor = fixtures.AUDITORS[i % 2]
        accepted = network.client.post(
            network.url + "/worker/lease",
            json=network.signed(
                auditor,
                "Receipt",
                {
                    "w": w,
                    "commit_hash": f"{w * 10 + i + 1:064x}",
                    "received_round": network.now,
                },
            ),
        )
        assert accepted.status_code == 200, accepted.text
        job = accepted.json()["body"]
        executed = network.client.post(
            network.url + "/worker/jobs/" + job["job_id"] + "/execute",
            json=network.signed(
                auditor,
                "Receipt",
                {
                    "w": w,
                    "commit_hash": job["lease_nonce"],
                    "received_round": network.now,
                },
            ),
        )
        assert executed.status_code == 200, executed.text
        assert executed.json()["verdict"]["result"] == "MATCH"
        accepted = network.client.post(
            network.url + "/worker/jobs/" + job["job_id"] + "/complete",
            json=network.signed(auditor, "ReplayVerdict", executed.json()["verdict"]),
        )
        assert accepted.status_code == 200, accepted.text
    accepted = network.client.post(
        network.url + f"/admin/rounds/{w}/aggregate", headers=fixtures.admin()
    )
    assert accepted.status_code == 200, accepted.text
    return accepted.json()


def test_real_service_two_round_rollback_and_checkpoint(active_network, monkeypatch):
    # Given: five genuine ACTIVE12 same-manifest identities; original signed two-round carry.
    n = active_network
    first = execute_round(n, 0)
    second = execute_round(n, 1)
    assert first["theta_hash"] != second["theta_hash"]
    from hypertrain.aggregator.core import FinalityError
    from hypertrain.challenge.finality_v2 import require_open

    with pytest.raises(FinalityError, match=r"w\+2 blocked"):
        require_open(n.manifest.run_id(), 2, n.store._history_v2(n.manifest.run_id()))
    cause = exclude_miner(n)
    from hypertrain.aggregator.checkpoint import (
        CheckpointError,
        verify_network_checkpoint,
        write_network_checkpoint,
    )
    from hypertrain.protocol.messages import Rollback

    escrow = n.store._services(n.manifest.run_id())[0]
    before = escrow.balances()
    original = [
        tuple(row)
        for row in n.store._db.execute(
            "SELECT kind,id,data FROM records_v2 WHERE kind IN ('commit','delta') ORDER BY kind,id",
        )
    ]
    proposal = Rollback(
        w=0,
        excluded=[fixtures.HOT[4].ss58],
        old_theta_hash_w2=second["theta_hash"],
        new_theta_hash_w2="0" * 64,
        new_outer_state_hash="0" * 64,
        recomputed=["agg_w", "step_w", "agg_w1", "step_w1"],
        cause_hashes=[cause],
    )
    # When: service computes proposal without apply; client signs exact resulting outputs.
    response = n.client.post(
        n.url + "/admin/rollback-preview",
        json=n.signed(
            fixtures.COORD,
            "Rollback",
            proposal,
        ),
        headers=fixtures.admin(),
    )
    assert response.status_code == 200, response.text
    request = proposal.model_copy(update=response.json())
    from hypertrain.auditor.replay import AnchorCache, pack_state, tensor_root

    source_anchor = n.store._restore_anchor_v2(
        n.manifest.run_id(), fixtures.HOT[0].ss58, -1, AnchorCache()
    )
    original_record = n.store._record_v2(
        n.manifest.run_id(), "anchor", "-1:" + fixtures.HOT[0].ss58
    )
    path = Path(original_record["path"])
    original_state = (path / "state").read_bytes()
    original_metadata = (path / "metadata").read_bytes()
    from dataclasses import replace

    bad_m = {name: x.clone() for name, x in source_anchor.state.m.items()}
    next(iter(bad_m.values())).view(-1)[0] = float("nan")
    bad_state = replace(source_anchor.state, m=bad_m)
    blob = pack_state(source_anchor.theta, bad_state)
    (path / "state").write_bytes(blob)
    metadata = json.loads(original_metadata)
    metadata["state_sha256"] = sha256_hex(blob)
    (path / "metadata").write_bytes(canonicalize(metadata))
    n.store._put_record_v2(
        n.manifest.run_id(),
        "anchor",
        "-1:" + fixtures.HOT[0].ss58,
        {
            **original_record,
            "state_root": tensor_root(source_anchor.theta, bad_state),
        },
    )
    rejected = n.client.post(
        n.url + "/admin/rollback",
        json=n.signed(
            fixtures.COORD,
            "Rollback",
            request,
        ),
        headers=fixtures.admin(),
    )
    assert rejected.status_code == 409 and "nonfinite" in rejected.text
    assert (
        n.store._record_v2(n.manifest.run_id(), "applied", "1")["tape_hash"] == second["tape_hash"]
    )
    assert escrow.balances() == before
    (path / "state").write_bytes(original_state)
    (path / "metadata").write_bytes(original_metadata)
    n.store._put_record_v2(
        n.manifest.run_id(), "anchor", "-1:" + fixtures.HOT[0].ss58, original_record
    )
    from hypertrain.aggregator import rollback_v2

    good_receipt_row = n.store._db.execute(
        "SELECT * FROM accepted_v2 WHERE key LIKE ? AND receipt!='{}' LIMIT 1",
        ("%CommitV2%",),
    ).fetchone()
    receipt = json.loads(good_receipt_row["receipt"])
    changed = dict(receipt["body"])
    changed["commit_hash"] = "ff" * 32
    receipt = n.signed(fixtures.COORD, "Receipt", changed)
    n.store._db.execute(
        "UPDATE accepted_v2 SET receipt=? WHERE key=?",
        (json.dumps(receipt), good_receipt_row["key"]),
    )
    rejected = n.client.post(
        n.url + "/admin/rollback",
        json=n.signed(
            fixtures.COORD,
            "Rollback",
            request,
        ),
        headers=fixtures.admin(),
    )
    assert rejected.status_code == 409 and "receipt" in rejected.text
    assert escrow.balances() == before
    n.store._db.execute(
        "UPDATE accepted_v2 SET receipt=? WHERE key=?",
        (good_receipt_row["receipt"], good_receipt_row["key"]),
    )
    real_execute = rollback_v2.execute_repair_context
    calls = 0

    def crash_before_commit(*args, **kwargs):
        nonlocal calls
        result = real_execute(*args, **kwargs)
        calls += 1
        if calls == 4:
            raise RuntimeError("injected crash after independent second replay before CAS")
        return result

    monkeypatch.setattr(rollback_v2, "execute_repair_context", crash_before_commit)
    with pytest.raises(RuntimeError, match="injected crash"):
        n.client.post(
            n.url + "/admin/rollback",
            json=n.signed(
                fixtures.COORD,
                "Rollback",
                request,
            ),
            headers=fixtures.admin(),
        )
    assert (
        n.store._record_v2(n.manifest.run_id(), "applied", "1")["tape_hash"] == second["tape_hash"]
    )
    assert escrow.balances() == before
    monkeypatch.setattr(rollback_v2, "execute_repair_context", real_execute)
    from hypertrain.challenge.store import ChallengeStore

    old_store = n.store
    restarted = ChallengeStore(
        old_store.state_dir,
        old_store.params,
        old_store.coord,
        old_store.owner_hotkey,
        old_store.verify_beacon,
        old_store.objects,
    )
    restarted.clock = old_store.clock
    n.client.app.state.store = restarted
    previous_db = old_store._db
    old_store._db = restarted._db
    previous_db.close()
    old_store._services_v2.clear()
    # Actual fresh SQLite/service authority objects; route closure retains its original instance.
    n.client.app.state.store = old_store
    escrow = old_store._services(n.manifest.run_id())[0]
    response = n.client.post(
        n.url + "/admin/rollback",
        json=n.signed(
            fixtures.COORD,
            "Rollback",
            request,
        ),
        headers=fixtures.admin(),
    )
    assert response.status_code == 200, response.text
    repaired = response.json()
    assert repaired["status"] == "REPAIRED" and repaired["reward_units"] == 0
    assert escrow.balances() == before
    assert original == [
        tuple(row)
        for row in n.store._db.execute(
            "SELECT kind,id,data FROM records_v2 WHERE kind IN ('commit','delta') ORDER BY kind,id",
        )
    ]
    repeated = n.client.post(
        n.url + "/admin/rollback",
        json=n.signed(
            fixtures.COORD,
            "Rollback",
            request,
        ),
        headers=fixtures.admin(),
    )
    assert repeated.status_code == 200 and repeated.json() == repaired
    assert escrow.balances() == before
    assert execute_round(n, 2, open_only=True).status_code == 200
    rounds = []
    for w in (0, 1):
        applied = n.store._record_v2(n.manifest.run_id(), "applied", str(w))
        context = n.store._record_v2(n.manifest.run_id(), "repair-context", str(w))
        rounds.append(
            {
                "round_open": n.store._record_v2(n.manifest.run_id(), "round", str(w)),
                "rollback": n.store._record_v2(n.manifest.run_id(), "rollback", "0")["request"],
                "repair_context": context,
                "inputs": [s["work"] for s in context["body"]["sources"]],
                **applied,
                "reference_reward_units": escrow.policy.R_collectible_units,
            }
        )
    directory = n.store.state_dir / "repair-checkpoint"
    write_network_checkpoint(directory, fixtures.COORD, n.store.objects, n.manifest, rounds)
    assert verify_network_checkpoint(directory, fixtures.COORD.ss58) == []
    from hypertrain.aggregator.rollback_v2 import RepairTape

    repair0 = RepairTape.from_bytes(n.store.objects.get(rounds[0]["tape_hash"]))
    repair1 = RepairTape.from_bytes(n.store.objects.get(rounds[1]["tape_hash"]))
    assert all(e.global_step0 == 0 for e in repair0.body.recomputed)
    assert all(e.global_step0 == 1 for e in repair1.body.recomputed)
    assert len(repair0.body.recomputed) == len(repair1.body.recomputed) == 4
    assert any(
        e.delta_hash != source["work"]["commit"]["delta_hash"]
        for e in repair1.body.recomputed
        for source in rounds[1]["repair_context"]["body"]["sources"]
        if e.hotkey == source["work"]["roster"]["hotkey"]
    )
    assert [
        n.store._record_v2(n.manifest.run_id(), "finality", str(w))["rollback_complete"]
        for w in (0, 1)
    ] == [True, True]
    original_rounds = json.loads(canonicalize(rounds))
    wrong = original_rounds[1]["repair_context"]["body"]
    wrong["sources"][0]["anchor"]["anchor_hash"] = "ff" * 32
    context = original_rounds[1]["repair_context"]
    context["sig"] = fixtures.COORD.sign(
        b"hypertrain/rollback/context/1|" + sha256_hex(canonicalize(wrong)).encode()
    ).hex()
    with pytest.raises(CheckpointError, match="carried anchor"):
        write_network_checkpoint(
            n.store.state_dir / "bad-repair-checkpoint",
            fixtures.COORD,
            n.store.objects,
            n.manifest,
            original_rounds,
        )
    assert not (n.store.state_dir / "bad-repair-checkpoint").exists()


def exclude_miner(network):
    """Real funded signed false-miner claims resolved by actual independent referee work."""
    from hypertrain.auditor.island_bisect import IslandParty
    from hypertrain.protocol.hashing import sha256_hex
    from hypertrain.protocol.jcs import canonicalize
    from hypertrain.protocol.messages_v2 import (
        BisectV2,
        DisputeV2,
        EscrowLock,
        IslandJobV1,
        ResolutionV2,
    )

    n = network
    run = n.manifest.run_id()
    hot, cold = fixtures.HOT[4], fixtures.COLD[4]
    contest = json.loads(
        n.store._db.execute(
            "SELECT data FROM records_v2 WHERE kind='contest' AND json_extract(data,'$.miner')=? "
            "AND json_extract(data,'$.w')=0",
            (hot.ss58,),
        ).fetchone()[0]
    )
    dispute_id = sha256_hex(canonicalize([run, contest["verdict_hash"], hot.ss58]))
    origin = n.store._db.execute(
        "SELECT origin FROM escrow_units WHERE owner=? AND bucket='available' AND units>=100",
        (cold.ss58,),
    ).fetchone()[0]
    lock = EscrowLock(
        operation_id="df" * 32,
        owner=cold.ss58,
        units=100,
        origin_ids=[origin],
        admission_id=None,
        dispute_id=dispute_id,
        kind="LOCK_CONTEST",
    )
    response = n.client.post(n.url + "/escrow/lock", json=n.signed(cold, "EscrowLock", lock))
    assert response.status_code == 200, response.text
    response = n.client.post(
        n.url + "/dispute",
        json=n.signed(
            hot,
            "DisputeV2",
            DisputeV2(
                hotkey=hot.ss58,
                verdict_hash=contest["verdict_hash"],
                action="contest",
                bond_lock=lock.operation_id,
            ),
        ),
    )
    assert response.status_code == 200, response.text
    disputes = n.store._services(run)[2]
    geometry = n.store._record_v2(run, "trace-geometry", contest["verdict_hash"])
    party = IslandParty.published(
        hot.ss58, IslandJobV1.model_validate(geometry["job"]), Path(geometry["directory"])
    )
    for _ in range(64):
        turn = disputes.get(dispute_id)
        if turn.paused:
            break
        key = (
            hot
            if turn.expected_party == hot.ss58
            else next(k for k in fixtures.AUDITORS if k.ss58 == turn.expected_party)
        )
        at = [turn.interval[0], -(-sum(turn.interval) // 2), turn.interval[1]]
        hashes = party.hashes(turn.level, tuple(turn.ctx), at)
        if key == hot:
            hashes = [
                digest if index == 0 else sha256_hex(bytes.fromhex(digest))
                for index, digest in zip(at, hashes, strict=True)
            ]
        answer = BisectV2(
            dispute_id=dispute_id,
            seq=turn.seq,
            level=turn.level,
            ctx=turn.ctx,
            interval=turn.interval,
            N=2,
            hashes=hashes,
            party=key.ss58,
            previous_transcript_hash=turn.transcript_hash,
        )
        response = n.client.post(n.url + "/bisect", json=n.signed(key, "BisectV2", answer))
        assert response.status_code == 200, response.text
    turn = disputes.get(dispute_id)
    assert turn.paused
    response = n.client.post(
        n.url + f"/admin/disputes/{dispute_id}/referee", headers=fixtures.admin()
    )
    assert response.status_code == 200, response.text
    assert response.json()["evidence"]["reason"] == "FRAUD"
    assert response.json()["evidence"]["loser"] == hot.ss58
    resolution = ResolutionV2(
        dispute_id=dispute_id,
        transcript_hash=turn.transcript_hash,
        reason="FRAUD",
        loser=hot.ss58,
        evidence_hash=response.json()["evidence_hash"],
    )
    response = n.client.post(
        n.url + "/resolution", json=n.signed(fixtures.REFEREE, "ResolutionV2", resolution)
    )
    assert response.status_code == 200, response.text
    return resolution.digest()
