"""Tiny real CPU shadow execution and complete-roster checkpoint boundaries."""

from __future__ import annotations

import importlib.util
import json
import os
import shutil
import sqlite3
import sys
import tempfile
from pathlib import Path

import pytest

spec = importlib.util.spec_from_file_location(
    "service_network_fixture", Path(__file__).parents[1] / "challenge/test_service_network_v2.py"
)
assert spec is not None and spec.loader is not None
fixtures = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = fixtures
spec.loader.exec_module(fixtures)
network = fixtures.network


@pytest.fixture
def od_carry_network(tmp_path, request, monkeypatch):
    """Pin carry before service registration; preserve real dataset/policy setup."""
    from hypertrain.protocol.messages_v2 import RunManifestV2

    setup_type = fixtures.fixture.Setup

    def carry_setup(manifest, policy, admission_policy, rows, tree):
        body = manifest.body()
        body["training"]["inner"].update(state_policy="carry", rewarmup_steps=0)
        return setup_type(RunManifestV2.model_validate(body), policy, admission_policy, rows, tree)

    monkeypatch.setattr(fixtures.fixture, "Setup", carry_setup)
    yield from fixtures.network.__wrapped__(tmp_path, request)


@pytest.mark.parametrize("od_carry_network", ["mlm", "decision", "distill"], indirect=True)
def test_od_carry_admission_reference_requires_real_full_state(od_carry_network):
    """First production prerequisite: genuine corresponding-manifest graduation work."""
    assert od_carry_network.manifest.training.inner.state_policy == "carry"
    assert od_carry_network.manifest.training.reference_spec.layout.n_gpus == 2
    # Positive production prerequisite, not an expected-failure or patched launcher.
    test_actual_shadow_trial_does_not_mutate_production(od_carry_network)


def test_actual_shadow_trial_does_not_mutate_production(network):
    from hypertrain.auditor.replay import unpack_state
    from hypertrain.protocol.messages import Finalize
    from hypertrain.protocol.messages_v2 import EscrowLock, WorkProof, WorkScreenV2
    from hypertrain.trainer.compress import state_hash

    # Given: funded signed public application; no production round.
    identity = network.join().json()["admission_id"]
    lock = EscrowLock(
        operation_id="cb" * 32,
        owner=fixtures.COLD[0].ss58,
        units=1000,
        origin_ids=[network.origin_ids[0]],
        admission_id=identity,
        dispute_id=None,
        kind="LOCK_ADMISSION",
    )
    response = network.client.post(
        network.url + "/escrow/lock", json=network.signed(fixtures.COLD[0], "EscrowLock", lock)
    )
    assert response.status_code == 200, response.text
    network.push(2)
    challenge = network.client.get(network.url + "/join/" + identity + "/challenge")
    assert challenge.status_code == 200, challenge.text
    # When: actual torchrun reference, signed proof, signed exact shadow finality.
    response = network.client.post(
        network.url + "/admin/join/" + identity + "/reference", headers=fixtures.admin()
    )
    assert response.status_code == 200, response.text
    row = network.store._db.execute(
        "SELECT * FROM admissions_v2 WHERE admission_id=?", (identity,)
    ).fetchone()
    proof = WorkProof.model_validate_json(row["reference_json"])
    from hypertrain.data.trial_assignment import trial_samples
    from hypertrain.miner.island_launch import validate_artifacts
    from hypertrain.protocol.messages_v2 import JoinChallenge

    challenge_body = JoinChallenge.model_validate(challenge.json()["body"])
    directory = network.store.state_dir / "trials-v2" / challenge_body.nonce
    job, same_directory = network.store.stage_trial_v2(
        network.manifest.run_id(),
        challenge_body,
        tuple(
            trial_samples(
                network.manifest,
                identity,
                challenge_body.nonce,
                network.store._beacon_v2(challenge_body.seed_beacon),
            )
        ),
        row["trial_epoch"],
    )
    artifacts = validate_artifacts(job, directory / "published")
    screen = WorkScreenV2(
        evidence_kind="work-proof-v1",
        admission_id=identity,
        nonce=challenge_body.nonce,
        policy_hash=challenge_body.policy_hash,
        image_digest=job.manifest.training.reference_spec.image_digest,
        layout=challenge_body.layout,
        challenge_hash=challenge_body.digest(),
        rank_results=list(artifacts.ranks),
        artifact_hashes=[ref.sha256 for ref in proof.artifact_refs],
    )
    response = network.client.post(
        network.url + "/join/proof",
        json={
            "proof": network.signed(fixtures.HOT[0], "WorkProof", proof),
            "screen": network.signed(fixtures.HOT[0], "WorkScreenV2", screen),
        },
    )
    assert response.status_code == 200, response.text
    theta, _ = unpack_state(network.store.objects.get(proof.artifact_refs[0].sha256))
    final = Finalize(
        w=row["trial_epoch"],
        final_theta_hash_w1=state_hash(theta),
        included=[fixtures.HOT[0].ss58],
        entitlements_root=proof.digest(),
    )
    response = network.client.post(
        network.url + "/admin/join/" + identity + "/finalize",
        json=network.signed(fixtures.COORD, "Finalize", final),
        headers=fixtures.admin(),
    )
    assert response.status_code == 200, response.text
    # Then: one genuine clean participation, no production cursor/tape/issuance change.
    assert response.json()["record"]["clean_count"] == 1
    assert (
        network.store._db.execute(
            "SELECT COUNT(*) FROM records_v2 WHERE kind IN ('round','applied','finality')"
        ).fetchone()[0]
        == 0
    )
    assert network.store._services(network.manifest.run_id())[0].balances().issued == 40000


@pytest.mark.parametrize("network", ["mlm", "decision", "distill"], indirect=True)
def test_od_two_rank_signed_service_reference_and_proof(network):
    # All OD objectives use the real signed admission/reference/proof/finality routes,
    # strict record samples, Merkle proof inputs and real N2 torchrun.
    test_actual_shadow_trial_does_not_mutate_production(network)
    row = network.store._db.execute("SELECT reference_json FROM admissions_v2").fetchone()
    assert row[0]
    assert network.manifest.training.reference_spec.layout.n_gpus == 2


@pytest.mark.parametrize(
    "fault", ["round", "included", "run", "type", "opening_round", "predecessor"]
)
def test_resigned_checkpoint_finality_subject_rejects(tmp_path, fault):
    from hypertrain.aggregator.checkpoint import (
        CheckpointError,
        verify_network_checkpoint,
        write_network_checkpoint,
    )
    from hypertrain.data.store import LocalFSStore
    from hypertrain.protocol import envelope_v2
    from hypertrain.protocol.hashing import sha256_hex
    from hypertrain.protocol.jcs import canonicalize
    from hypertrain.protocol.messages_v2 import RunManifestV2

    source = Path(os.environ["HT_NETWORK_CHECKPOINT_SNAPSHOT"])
    stable = Path(tempfile.gettempdir()) / "hypertrain-l0-checkpoint-proof"
    if source != stable:
        shutil.copytree(source, stable, dirs_exist_ok=True)
        source = stable
    directory = tmp_path / "bad-checkpoint"
    shutil.copytree(source, directory)
    lineage = json.loads((directory / "lineage.json").read_bytes())
    item = lineage["rounds"][0]
    target = "finalize"
    final = item[target]
    match fault:
        case "round":
            final["body"]["w"] += 1
        case "included":
            final["body"]["included"] = []
        case "run":
            final["run_id"] = "ff" * 32
        case "type":
            final["type"] = "RunManifestV2"
        case "opening_round":
            target = "round_open"
            item[target]["body"]["w"] += 1
        case "predecessor":
            item["predecessor_tape_hash"] = "ab" * 32
    if fault not in ("type", "predecessor"):
        signed = item[target]
        item[target] = envelope_v2.seal(
            fixtures.COORD, signed["type"], signed["run_id"], signed["body"], signed["exp_drand"]
        )
    raw = canonicalize(lineage)
    (directory / "lineage.json").write_bytes(raw)
    document = json.loads((directory / "MANIFEST.json").read_bytes())
    document["body"]["files"]["lineage.json"] = sha256_hex(raw)
    document["sig"] = fixtures.COORD.sign(
        b"hypertrain/checkpoint/2|" + sha256_hex(canonicalize(document["body"])).encode()
    ).hex()
    (directory / "MANIFEST.json").write_bytes(canonicalize(document))
    assert verify_network_checkpoint(directory, fixtures.COORD.ss58)
    with pytest.raises((CheckpointError, ValueError)):
        write_network_checkpoint(
            tmp_path / "writer-output",
            fixtures.COORD,
            LocalFSStore(directory / "objects"),
            RunManifestV2.model_validate(lineage["manifest"]),
            lineage["rounds"],
        )
    assert not (tmp_path / "writer-output").exists()


def _live_checkpoint(network):
    import anyio
    import httpx

    from hypertrain.aggregator.checkpoint import verify_network_checkpoint, write_network_checkpoint
    from hypertrain.aggregator.cli import main
    from hypertrain.auditor.replay import AnchorCache, optimizer_hash, pack_state
    from hypertrain.data.store import LocalFSStore
    from hypertrain.data.stream_store import StreamStore
    from hypertrain.miner.island_launch import launch_island
    from hypertrain.protocol import relay_envelope
    from hypertrain.protocol.hashing import sha256_hex
    from hypertrain.protocol.jcs import canonicalize
    from hypertrain.protocol.messages import Finalize, f32hex
    from hypertrain.protocol.messages_v2 import (
        ArtifactRef,
        CommitV2,
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
    starts, roster = [], []
    for i, hot in enumerate(fixtures.HOT):
        status = network.store._services(manifest.run_id())[1].status(hot.ss58, now=network.now)
        anchor = cache.genesis(manifest, hot.ss58, theta)
        starts.append(
            StartStateV2(
                run_id=manifest.run_id(),
                w=0,
                hotkey=hot.ss58,
                theta_hash=state_hash(theta),
                state_object_sha256=sha256_hex(pack_state(theta, anchor.state)),
                opt_state_hash=optimizer_hash(anchor.state),
                ef_object_sha256=sha256_hex(pack_state(anchor.ef)),
                ef_hash=state_hash(anchor.ef),
                parent_anchor_hash=anchor.anchor_hash,
                global_step0=0,
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
    n = manifest.network
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
        d_commit=network.now + 30,
        d_audit=network.now + 31,
        d_upload=network.now + 200,
        d_final=network.now + 300,
        contract_version=2,
        policy_hashes=PolicyHashes.model_validate(
            {k: getattr(n, k) for k in PolicyHashes.model_fields}
        ),
        registry_epoch=0,
        start_state_index_hash=sha256_hex(canonicalize([s.body() for s in starts])),
        audit_mode="anchored-full",
        roster=roster,
    )
    response = network.client.post(
        network.url + "/admin/rounds",
        json=network.signed(fixtures.COORD, "RoundOpenV2", opening),
        headers=fixtures.admin(),
    )
    assert response.status_code == 200, response.text
    network.push(opening.d_assign)
    view = network.client.get(network.url + "/rounds/0")
    assert view.status_code == 200, view.text
    payloads = []
    for i, hot in enumerate(fixtures.HOT):
        if i == 0 and os.environ.get("HT_NETWORK_REAL_CLIENT") == "1":
            continue
        assignment = next(a for a in view.json()["assignment"] if a["hotkey"] == hot.ss58)
        row = network.store._db.execute(
            "SELECT screen_json FROM admissions_v2 WHERE hotkey=?", (hot.ss58,)
        ).fetchone()
        screen = json.loads(row[0])
        accept = {
            "w": 0,
            "hotkey": hot.ss58,
            "assignment_hash": assignment["assignment_hash"],
            "image_digest": manifest.training.reference_spec.image_digest,
            "driver_version": "test",
            "n_gpus": 1,
            "work_screen_hash": sha256_hex(canonicalize(screen)),
        }
        accept["driver_version"] = manifest.training.reference_spec.driver_allowlist[0]
        response = network.client.post(
            network.url + "/accept", json=network.signed(hot, "AcceptV2", accept)
        )
        assert response.status_code == 200, response.text
        staged = network.store.island_job_v2(manifest.run_id(), 0, hot.ss58)
        from hypertrain.protocol.messages_v2 import IslandJobV1

        job = IslandJobV1.model_validate(staged["job"])
        directory = network.store.state_dir / "jobs-v2/0" / hot.ss58
        artifacts = launch_island(job, directory, backend="cpu", trace=True)
        summary = json.loads((artifacts.directory / "rank-0/summary.json").read_bytes())
        leaves = json.loads(artifacts.leaves.read_bytes())
        from hypertrain.protocol.hashing import MerkleTree

        commitments = summary["commitments"]
        commit = CommitV2.model_validate(
            {
                "w": 0,
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
                    for k in ("leaves_root", "final_theta_hash", "ef_out_hash", "delta_hash")
                },
                "delta_bytes": artifacts.delta.stat().st_size,
            }
        )
        response = network.client.post(
            network.url + "/commit", json=network.signed(hot, "CommitV2", commit)
        )
        assert response.status_code == 200, response.text
        refs = []
        for path in (artifacts.state, artifacts.ef, artifacts.delta, artifacts.leaves):
            network.store.objects.put(path.read_bytes())
            refs.append(ArtifactRef(sha256=sha256_hex(path.read_bytes()), size=path.stat().st_size))
        descriptor = WorkProof(
            admission_id=roster[i]["admission_id"],
            challenge_hash=sha256_hex(canonicalize(commit.model_dump(mode="json"))),
            leaves_root=commit.leaves_root,
            delta_hash=commit.delta_hash,
            artifact_refs=refs,
        )
        response = network.client.post(
            network.url + "/leaves", json=network.signed(hot, "WorkProof", descriptor)
        )
        assert response.status_code == 200, response.text
        payloads.append((hot, artifacts.delta))

    async def transport_all():
        async with httpx.AsyncClient() as backing_http:

            async def now():
                return network.now

            registry = network.store._registry_v2(manifest)
            relay_assignment = network.store.relay_assignment_v2(
                manifest.run_id(), 0, fixtures.HOT[0].ss58
            )
            relay = Relay(
                run_id=manifest.run_id(),
                master=fixtures.COORD.ss58,
                registry=registry,
                network_manifest_hash=sha256_hex(
                    canonicalize(relay_assignment["network_manifest"]["body"])
                ),
                observers={fixtures.AUDITORS[0].ss58, fixtures.AUDITORS[1].ss58},
                relay_id="local",
                region="local",
                keys={"k1": fixtures.RELAY},
                active_key="k1",
                backing=StreamStore(
                    LocalFSStore(network.store.state_dir / "relay-backing"), backing_http
                ),
                now=now,
            )
            async with httpx.AsyncClient(
                transport=httpx.ASGITransport(app=create_app(relay, "x" * 32)),
                base_url="https://relay.test",
            ) as client:
                if os.environ.get("HT_NETWORK_REAL_CLIENT") == "1":
                    await anyio.to_thread.run_sync(
                        lambda: _exact_network_client(network, relay, create_app(relay, "x" * 32))
                    )
                for hot, path in payloads:
                    assigned = network.store.relay_assignment_v2(manifest.run_id(), 0, hot.ss58)
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
                    delta = {
                        "w": 0,
                        "hotkey": hot.ss58,
                        "delta_hash": sha256_hex(raw),
                        "size": len(raw),
                        "uri": "sha256:" + sha256_hex(raw),
                        "format": "ht-sparse-v1",
                        "chunks": [{"off": 0, "len": len(raw), "sha256": sha256_hex(raw)}],
                        "grant_hash": grant["body"] and sha256_hex(canonicalize(grant["body"])),
                        "master_acceptance_hash": sha256_hex(canonicalize(acceptance["body"])),
                    }
                    response = network.client.post(
                        network.url + "/delta", json=network.signed(hot, "DeltaManifestV2", delta)
                    )
                    assert response.status_code == 200, response.text
                    ack = await network.store.relay_control_v2(
                        manifest.run_id(), delta["grant_hash"], "extend", client
                    )
                    assert (
                        ack["body"]["extension_hash"]
                        == network.store._record_v2(
                            manifest.run_id(), "retention", delta["grant_hash"]
                        )["hash"]
                    )
                ready = await client.get("/readyz")
                assert ready.status_code == 200, ready.text
                drained = await client.post(
                    "/internal/drain", headers={"Authorization": "Bearer " + "x" * 32}
                )
                assert drained.status_code == 200
                assert (await client.get("/readyz")).status_code == 503

    anyio.run(transport_all)
    network.push(opening.d_audit)
    response = network.client.post(network.url + "/admin/rounds/0/audits", headers=fixtures.admin())
    assert response.status_code == 200, response.text
    for index in range(4):
        auditor = fixtures.AUDITORS[index % 2]
        lease_request = network.signed(
            auditor,
            "Receipt",
            {"w": 0, "commit_hash": f"{index + 1:064x}", "received_round": network.now},
        )
        response = network.client.post(network.url + "/worker/lease", json=lease_request)
        assert response.status_code == 200, response.text
        job = response.json()["body"]
        execute = network.signed(
            auditor,
            "Receipt",
            {"w": 0, "commit_hash": job["lease_nonce"], "received_round": network.now},
        )
        response = network.client.post(
            network.url + "/worker/jobs/" + job["job_id"] + "/execute", json=execute
        )
        assert response.status_code == 200, response.text
        assert response.json()["verdict"]["result"] == "MATCH"
        response = network.client.post(
            network.url + "/worker/jobs/" + job["job_id"] + "/complete",
            json=network.signed(auditor, "ReplayVerdict", response.json()["verdict"]),
        )
        assert response.status_code == 200, response.text
    response = network.client.post(
        network.url + "/admin/rounds/0/aggregate", headers=fixtures.admin()
    )
    assert response.status_code == 200, response.text
    applied = response.json()
    final = Finalize(
        w=0,
        final_theta_hash_w1=applied["theta_hash"],
        included=sorted(k.ss58 for k in fixtures.HOT),
        entitlements_root="0" * 64,
    )
    response = network.client.post(
        network.url + "/admin/rounds/0/finalize",
        json=network.signed(fixtures.COORD, "Finalize", final),
        headers=fixtures.admin(),
    )
    assert response.status_code == 200, response.text
    conserved = network.store._services(manifest.run_id())[0].balances()
    repeated = network.client.post(
        network.url + "/admin/rounds/0/finalize",
        json=network.signed(fixtures.COORD, "Finalize", final),
        headers=fixtures.admin(),
    )
    assert repeated.status_code == 200 and repeated.json() == response.json()
    assert network.store._services(manifest.run_id())[0].balances() == conserved
    record = network.store._record_v2(manifest.run_id(), "tape-inputs", "0")
    rounds = [
        {
            "round_open": network.signed(fixtures.COORD, "RoundOpenV2", opening),
            "finalize": network.signed(fixtures.COORD, "Finalize", final),
            "inputs": record["inputs"],
            "tape_hash": applied["tape_hash"],
            "prev_state": applied["prev_state"],
            "predecessor_tape_hash": applied["predecessor_tape_hash"],
            "reference_reward_units": network.store._services(manifest.run_id())[
                0
            ].policy.R_collectible_units,
        }
    ]
    checkpoint = network.store.state_dir / "checkpoint"
    write_network_checkpoint(checkpoint, fixtures.COORD, network.store.objects, manifest, rounds)
    assert verify_network_checkpoint(checkpoint, fixtures.COORD.ss58) == []
    assert main(["verify-checkpoint-v2", str(checkpoint), "--signer", fixtures.COORD.ss58]) == 0
    _funded_dispute_watch(network)


def _exact_network_client(network, relay, relay_app, *, w=0, all_miners=False):
    """Exact product client, real HTTP master plus real TLS relay, no authority mocks."""
    import contextlib
    import io
    import socket
    import ssl
    import subprocess
    import threading

    import httpx
    import uvicorn

    from hypertrain.miner.cli import main

    class ReadyServer(uvicorn.Server):
        def __init__(self, config):
            super().__init__(config)
            self.ready = threading.Event()

        async def startup(self, sockets=None):
            await super().startup(sockets=sockets)
            self.ready.set()

    @contextlib.contextmanager
    def server(app, cert=None, key=None):
        sock = socket.socket()
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
        config = uvicorn.Config(app, log_level="error", ssl_certfile=cert, ssl_keyfile=key)
        process = ReadyServer(config)
        thread = threading.Thread(target=lambda: process.run(sockets=[sock]), daemon=True)
        thread.start()
        assert process.ready.wait(5)
        try:
            yield port
        finally:
            process.should_exit = True
            thread.join(timeout=5)
            assert not thread.is_alive()

    directory = network.store.state_dir / "tls-client"
    directory.mkdir(exist_ok=True)
    cert, key = directory / "cert.pem", directory / "server.key"
    subprocess.run(
        [
            "openssl",
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-days",
            "1",
            "-keyout",
            str(key),
            "-out",
            str(cert),
            "-subj",
            "/CN=localhost",
            "-addext",
            "subjectAltName=DNS:localhost",
        ],
        check=True,
        capture_output=True,
    )
    original_client = httpx.AsyncClient
    with (
        server(relay_app, str(cert), str(key)) as relay_port,
        server(network.client.app) as master_port,
    ):

        async def route(request):
            if request.url.host == "relay.test":
                request.url = request.url.copy_with(host="localhost", port=relay_port)

        def real_async(*args, **kw):
            kw.setdefault("verify", ssl.create_default_context(cafile=str(cert)))
            kw["event_hooks"] = {"request": [route]}
            return original_client(*args, **kw)

        httpx.AsyncClient = real_async
        try:
            for index in range(4 if all_miners else 1):
                file = directory / f"miner-{index}.key"
                file.write_bytes(bytes([80 + index]) * 32)
                file.chmod(0o600)
                config = directory / f"miner-{index}.toml"
                config.write_text(
                    "\n".join(
                        f"{name} = {json.dumps(str(value))}"
                        for name, value in {
                            "api": f"http://127.0.0.1:{master_port}",
                            "keyfile": file,
                            "workdir": directory / "work",
                            "state_source": "cpu",
                            "image_digest": network.manifest.training.reference_spec.image_digest,
                            "run_id": network.manifest.run_id(),
                            "owner_hotkey": fixtures.OWNER.ss58,
                            "device": "cpu",
                        }.items()
                    )
                )
                output = io.StringIO()
                with contextlib.redirect_stdout(output):
                    assert main(["run-v2", "--config", str(config), "--round", str(w)]) == 0
                assert json.loads(output.getvalue()) == {"status": "UPLOADED"}
                assert network.store._record_v2(
                    network.manifest.run_id(), "delta", f"{w}:" + fixtures.HOT[index].ss58
                )
        finally:
            httpx.AsyncClient = original_client


def _funded_dispute_watch(network):
    import httpx

    from hypertrain.auditor.island_bisect import IslandParty
    from hypertrain.data.store import LocalFSStore
    from hypertrain.miner.dispute_watch import DisputeWatch, HttpDisputeTransport, PublishedStates
    from hypertrain.protocol.hashing import sha256_hex
    from hypertrain.protocol.jcs import canonicalize
    from hypertrain.protocol.messages_v2 import DisputeV2, EscrowLock, IslandJobV1, ResolutionV2

    hot = fixtures.HOT[0]
    row = network.store._db.execute(
        "SELECT data FROM records_v2 WHERE kind='contest' AND json_extract(data,'$.miner')=?",
        (hot.ss58,),
    ).fetchone()
    contest = json.loads(row[0])
    verdict_hash = contest["verdict_hash"]
    dispute_id = sha256_hex(canonicalize([network.manifest.run_id(), verdict_hash, hot.ss58]))
    origin = network.store._db.execute(
        "SELECT origin FROM escrow_units WHERE owner=? AND bucket='available' AND units>=100",
        (fixtures.COLD[0].ss58,),
    ).fetchone()[0]
    lock = EscrowLock(
        operation_id="cd" * 32,
        owner=fixtures.COLD[0].ss58,
        units=100,
        origin_ids=[origin],
        admission_id=None,
        dispute_id=dispute_id,
        kind="LOCK_CONTEST",
    )
    response = network.client.post(
        network.url + "/escrow/lock", json=network.signed(fixtures.COLD[0], "EscrowLock", lock)
    )
    assert response.status_code == 200, response.text
    response = network.client.post(
        network.url + "/dispute",
        json=network.signed(
            hot,
            "DisputeV2",
            DisputeV2(
                hotkey=hot.ss58,
                verdict_hash=verdict_hash,
                action="contest",
                bond_lock=lock.operation_id,
            ),
        ),
    )
    assert response.status_code == 200, response.text
    # A signed Resolution still cannot manufacture registered referee evidence.
    turn = response.json()
    forged = ResolutionV2(
        dispute_id=dispute_id,
        transcript_hash=turn["transcript_hash"],
        reason="MATCH",
        loser=None,
        evidence_hash="0" * 64,
    )
    response = network.client.post(
        network.url + "/resolution", json=network.signed(fixtures.REFEREE, "ResolutionV2", forged)
    )
    assert response.status_code == 409 and "UNVERIFIED_REFEREE_EVIDENCE" in response.text
    geometry = network.store._record_v2(network.manifest.run_id(), "trace-geometry", verdict_hash)
    job = IslandJobV1.model_validate(geometry["job"])
    published = Path(geometry["directory"])

    def request(req):
        response = network.client.request(
            req.method,
            str(req.url).replace("https://service.test", ""),
            content=req.content,
            headers=dict(req.headers),
        )
        return httpx.Response(
            response.status_code, content=response.content, headers=dict(response.headers)
        )

    with httpx.Client(
        transport=httpx.MockTransport(request),
        headers={
            "X-Dispute-Signature": hot.sign(
                f"hypertrain/watch/2|{network.manifest.run_id()}|{hot.ss58}".encode()
            ).hex()
        },
    ) as client:
        states = PublishedStates(
            job, published, LocalFSStore(network.store.state_dir / "watch-objects")
        )
        watcher = DisputeWatch(
            network.store.state_dir / "watch-client",
            hot,
            HttpDisputeTransport(client, "https://service.test" + network.url),
            run_id=network.manifest.run_id(),
            coordinator=fixtures.COORD.ss58,
            party=lambda _: IslandParty.published(hot.ss58, job, published),
            state_serve=states.serve,
            beacon=lambda: network.now,
        )
        assert watcher.run_once(timeout=0) == 1
        response = network.client.post(
            network.url + f"/admin/disputes/{dispute_id}/state/0", headers=fixtures.admin()
        )
        assert response.status_code == 200, response.text
        assert watcher.run_once(timeout=0) == 1
        watcher.close()
    assert network.store._db.execute("SELECT COUNT(*) FROM dispute_states_v2").fetchone()[0] == 1
    response = network.client.post(
        network.url + f"/admin/disputes/{dispute_id}/referee", headers=fixtures.admin()
    )
    assert response.status_code == 200, response.text
    assert response.json()["evidence"]["reason"] == "MATCH"
    current = network.store._services(network.manifest.run_id())[2].get(dispute_id)
    resolution = ResolutionV2(
        dispute_id=dispute_id,
        transcript_hash=current.transcript_hash,
        reason="MATCH",
        loser=None,
        evidence_hash=response.json()["evidence_hash"],
    )
    response = network.client.post(
        network.url + "/resolution",
        json=network.signed(fixtures.REFEREE, "ResolutionV2", resolution),
    )
    assert response.status_code == 200, response.text
    response = network.client.post(
        network.url + "/admission/recover",
        json=network.signed(
            hot, "Receipt", {"w": 0, "commit_hash": dispute_id, "received_round": network.now}
        ),
    )
    assert response.status_code == 200, response.text
    assert response.json()["record"]["pending_dispute"] is False
    assert network.store._services(network.manifest.run_id())[0].balances().dispute_locked == 0
    _funded_auditor_fraud(network)
    # Subsequent independently funded miner contest: delivered auditor turn expires,
    # two permitted healthy witnesses attribute AUDITOR_FAULT, never miner fraud.
    from hypertrain.challenge.disputes_v2 import Availability, EventAck, signed_message

    hot2 = fixtures.HOT[1]
    row = network.store._db.execute(
        "SELECT data FROM records_v2 WHERE kind='contest' AND json_extract(data,'$.miner')=?",
        (hot2.ss58,),
    ).fetchone()
    second = json.loads(row[0])
    second_id = sha256_hex(
        canonicalize([network.manifest.run_id(), second["verdict_hash"], hot2.ss58])
    )
    origin = network.store._db.execute(
        "SELECT origin FROM escrow_units WHERE owner=? AND bucket='available' AND units>=100",
        (fixtures.COLD[1].ss58,),
    ).fetchone()[0]
    lock2 = EscrowLock(
        operation_id="ce" * 32,
        owner=fixtures.COLD[1].ss58,
        units=100,
        origin_ids=[origin],
        admission_id=None,
        dispute_id=second_id,
        kind="LOCK_CONTEST",
    )
    response = network.client.post(
        network.url + "/escrow/lock", json=network.signed(fixtures.COLD[1], "EscrowLock", lock2)
    )
    assert response.status_code == 200, response.text
    response = network.client.post(
        network.url + "/dispute",
        json=network.signed(
            hot2,
            "DisputeV2",
            DisputeV2(
                hotkey=hot2.ss58,
                verdict_hash=second["verdict_hash"],
                action="contest",
                bond_lock=lock2.operation_id,
            ),
        ),
    )
    assert response.status_code == 200, response.text
    disputes = network.store._services(network.manifest.run_id())[2]
    turn = disputes.get(second_id)
    party = IslandParty.published(hot2.ss58, job, published)
    from hypertrain.protocol.messages_v2 import BisectV2

    answer = BisectV2(
        dispute_id=second_id,
        seq=turn.seq,
        level=turn.level,
        ctx=turn.ctx,
        interval=turn.interval,
        N=2,
        hashes=party.hashes(
            turn.level,
            tuple(turn.ctx),
            [turn.interval[0], -(-sum(turn.interval) // 2), turn.interval[1]],
        ),
        party=hot2.ss58,
        previous_transcript_hash=turn.transcript_hash,
    )
    # Geometry from this miner's original published path, not another assignment.
    own_geometry = network.store._record_v2(
        network.manifest.run_id(), "trace-geometry", second["verdict_hash"]
    )
    own_party = IslandParty.published(
        hot2.ss58, IslandJobV1.model_validate(own_geometry["job"]), Path(own_geometry["directory"])
    )
    answer = answer.model_copy(
        update={
            "hashes": own_party.hashes(
                turn.level,
                tuple(turn.ctx),
                [turn.interval[0], -(-sum(turn.interval) // 2), turn.interval[1]],
            )
        }
    )
    response = network.client.post(
        network.url + "/bisect", json=network.signed(hot2, "BisectV2", answer)
    )
    assert response.status_code == 200, response.text
    turn = disputes.get(second_id)
    auditor = next(k for k in fixtures.AUDITORS if k.ss58 == turn.expected_party)
    events = disputes.events(auditor.ss58, 0)
    event = next(e for e in events if e.turn.dispute_id == second_id and e.turn.seq == turn.seq)
    ack = EventAck(
        run_id=network.manifest.run_id(),
        cursor=event.cursor,
        event_hash=sha256_hex(canonicalize(event.body())),
        party=auditor.ss58,
        received_beacon=network.now,
        sig="0" * 128,
    )
    ack = ack.model_copy(update={"sig": auditor.sign(signed_message(ack)).hex()})
    response = network.client.post(network.url + "/disputes/ack", json=ack.body())
    assert response.status_code == 200, response.text
    network.push(turn.turn_deadline + 1)
    witnesses = []
    for key in (fixtures.COORD, fixtures.REFEREE):
        witness = Availability(
            run_id=network.manifest.run_id(),
            dispute_id=second_id,
            seq=turn.seq,
            transcript_hash=turn.transcript_hash,
            observed_beacon=network.now,
            service_healthy=True,
            beacon_healthy=True,
            reference_healthy=True,
            signer=key.ss58,
            sig="0" * 128,
        )
        witnesses.append(
            witness.model_copy(update={"sig": key.sign(signed_message(witness)).hex()}).body()
        )
    response = network.client.post(
        network.url + f"/admin/disputes/{second_id}/timeout",
        json={"witnesses": witnesses},
        headers=fixtures.admin(),
    )
    assert response.status_code == 200, response.text
    assert response.json()["resolution"]["reason"] == "AUDITOR_FAULT"
    assert network.store._services(network.manifest.run_id())[0].balances().burned == 0


def test_checkpoint_and_cli_reject_full_seventeen_roster_before_store(tmp_path):
    from hypertrain.aggregator.checkpoint import CheckpointError, write_network_checkpoint
    from hypertrain.aggregator.cli import main
    from hypertrain.protocol.hashing import sha256_hex
    from hypertrain.protocol.jcs import canonicalize
    from hypertrain.protocol.keys import Keypair
    from hypertrain.protocol.messages import f32hex
    from hypertrain.protocol.messages_v2 import PolicyHashes, RoundOpenV2

    s = fixtures.fixture.setup()
    roster = [
        {
            "hotkey": Keypair(bytes([i + 1]) * 32).ss58,
            "slot": i,
            "q_i": f32hex(1),
            "admission_id": sha256_hex(str(i).encode()),
            "coldkey_group": f"o{i}",
            "state": "ACTIVE",
            "eligible_weight": 4194304,
        }
        for i in range(17)
    ]
    opening = RoundOpenV2(
        w=0,
        prev_final_hash="0" * 64,
        theta_hash="0" * 64,
        outer_state_hash="0" * 64,
        center_hash="0" * 64,
        roster_hash=sha256_hex(canonicalize(roster)),
        honeypot_commit="0" * 64,
        d_open=2,
        d_assign=3,
        d_commit=4,
        d_audit=5,
        d_upload=6,
        d_final=20,
        contract_version=2,
        policy_hashes=PolicyHashes.model_validate(
            {k: getattr(s.manifest.network, k) for k in PolicyHashes.model_fields}
        ),
        registry_epoch=0,
        start_state_index_hash="0" * 64,
        audit_mode="anchored-full",
        roster=roster,
    )
    rounds = [{"round_open": {"body": opening.body()}}]

    class ForbiddenStore:
        def get(self, *args):
            raise AssertionError("storage touched before complete roster gate")

    # Given / When / Then
    with pytest.raises(CheckpointError, match="ROSTER_LIMIT_16"):
        write_network_checkpoint(
            tmp_path / "checkpoint", fixtures.COORD, ForbiddenStore(), s.manifest, rounds
        )
    assert not (tmp_path / "checkpoint").exists()
    bundle = tmp_path / "bundle.json"
    bundle.write_text(json.dumps({"manifest": s.manifest.body(), "rounds": rounds}))
    for command in ("aggregate-v2", "checkpoint-v2"):
        args = [command, str(bundle)]
        if command == "checkpoint-v2":
            args.append(str(tmp_path / "out"))
        args += ["--objects", str(tmp_path / "not-created"), "--keyfile", str(tmp_path / "absent")]
        with pytest.raises(CheckpointError, match="ROSTER_LIMIT_16"):
            main(args)
    assert not (tmp_path / "not-created").exists()


def _funded_auditor_fraud(network):
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

    run_id = network.manifest.run_id()
    escrow, _, disputes = network.store._services(run_id)
    hot, cold = fixtures.HOT[2], fixtures.COLD[2]
    row = network.store._db.execute(
        "SELECT data FROM records_v2 WHERE kind='contest' AND json_extract(data,'$.miner')=?",
        (hot.ss58,),
    ).fetchone()
    contest = json.loads(row[0])
    dispute_id = sha256_hex(canonicalize([run_id, contest["verdict_hash"], hot.ss58]))
    origin = network.store._db.execute(
        "SELECT origin FROM escrow_units WHERE owner=? AND bucket='available' AND units>=100",
        (cold.ss58,),
    ).fetchone()[0]
    before = escrow.balances(cold.ss58)
    lock = EscrowLock(
        operation_id="cf" * 32,
        owner=cold.ss58,
        units=100,
        origin_ids=[origin],
        admission_id=None,
        dispute_id=dispute_id,
        kind="LOCK_CONTEST",
    )
    response = network.client.post(
        network.url + "/escrow/lock", json=network.signed(cold, "EscrowLock", lock)
    )
    assert response.status_code == 200, response.text
    response = network.client.post(
        network.url + "/dispute",
        json=network.signed(
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
    geometry = network.store._record_v2(run_id, "trace-geometry", contest["verdict_hash"])
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
        if key != hot:
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
        response = network.client.post(
            network.url + "/bisect", json=network.signed(key, "BisectV2", answer)
        )
        assert response.status_code == 200, response.text
    turn = disputes.get(dispute_id)
    assert turn.paused and turn.level == "op" and tuple(turn.interval) == (0, 1)
    response = network.client.post(
        network.url + f"/admin/disputes/{dispute_id}/referee", headers=fixtures.admin()
    )
    assert response.status_code == 200, response.text
    evidence = response.json()
    assert evidence["evidence"]["reason"] == "FRAUD"
    assert evidence["evidence"]["loser"] == turn.contest.auditor
    resolution = ResolutionV2(
        dispute_id=dispute_id,
        transcript_hash=turn.transcript_hash,
        reason="FRAUD",
        loser=turn.contest.auditor,
        evidence_hash=evidence["evidence_hash"],
    )
    # A re-signed wrong loser cannot consume the independent evidence or collateral.
    response = network.client.post(
        network.url + "/resolution",
        json=network.signed(
            fixtures.REFEREE, "ResolutionV2", resolution.model_copy(update={"loser": hot.ss58})
        ),
    )
    assert response.status_code == 409, response.text
    for _ in range(2):
        response = network.client.post(
            network.url + "/resolution",
            json=network.signed(fixtures.REFEREE, "ResolutionV2", resolution),
        )
        assert response.status_code == 200, response.text
    accepted = disputes.settlement(lock.operation_id)
    assert (
        accepted.loser,
        accepted.lock_id,
        accepted.lock_owner,
        accepted.referee,
        accepted.resolution_hash,
    ) == (
        turn.contest.auditor,
        lock.operation_id,
        cold.ss58,
        fixtures.REFEREE.ss58,
        resolution.digest(),
    )
    assert accepted.outcome == "MATCH" and not accepted.unresolved
    after = escrow.balances(cold.ss58)
    assert after.burned == before.burned and after.available == before.available
    assert after.dispute_locked == before.dispute_locked


def test_four_actual_funded_identities_graduate_shadow_only(network, monkeypatch):
    # Given: four independent coldkeys, conserved funded origin locks.
    snapshot = os.environ.get("HT_NETWORK_VERIFIED_SNAPSHOT")
    if snapshot:
        source = Path(snapshot)
        existing = sqlite3.connect(source / "challenge.db")
        run_id, raw = existing.execute("SELECT run_id,manifest FROM runs").fetchone()
        from hypertrain.protocol.messages_v2 import RunManifestV2

        network.manifest = RunManifestV2.model_validate_json(raw)
        assert run_id == network.manifest.run_id()
        rows = existing.execute(
            "SELECT hotkey,state,clean_count,admission_id FROM admissions_v2 ORDER BY hotkey"
        ).fetchall()
        assert {r[0] for r in rows} == {k.ss58 for k in fixtures.HOT}
        assert all(r[1] == "ACTIVE" and r[2] == 12 for r in rows)
        for _, _, _, admission_id in rows:
            assert (
                existing.execute(
                    "SELECT COUNT(*) FROM admission_trial_results r "
                    "JOIN admission_trials t ON t.epoch=r.epoch "
                    "WHERE t.admission_id=? AND r.outcome='MATCH'",
                    (admission_id,),
                ).fetchone()[0]
                == 12
            )
        network.store.close_v2_notifications()
        network.store._services_v2.clear()
        network.store._db.execute("PRAGMA foreign_keys=OFF")
        existing.backup(network.store._db)
        network.store._db.execute("PRAGMA foreign_keys=ON")
        existing.close()
        # Public training cursor is recomputed from independently verified live objects.
        network.store._db.execute("DELETE FROM records_v2 WHERE kind NOT IN ('dataset')")
        network.store._db.execute("DELETE FROM accepted_v2")
        network.store._db.execute("DELETE FROM audit_leases_v2")
        network.store._db.execute("DELETE FROM escrow_finalized")
        network.store._db.execute(
            "DELETE FROM beacon_pins_v2 WHERE operation_id LIKE 'open:%' "
            "OR operation_id LIKE 'assignment:%' OR operation_id LIKE 'audit:%' "
            "OR operation_id LIKE 'aggregate:%' OR operation_id LIKE 'finalize:%'"
        )
        for directory in ("objects", "trials-v2"):
            shutil.copytree(
                source / directory, network.store.state_dir / directory, dirs_exist_ok=True
            )
        network.now = network.store._now(network.store._db)
        import time

        # Historical signed authority keeps its original beacon/expiry. Translate
        # only the test wall clock; hard timers retain real monotonic durations.
        baseline = time.monotonic()
        snapshot_time = network.manifest.training.beacon.genesis_time + (network.now - 1) * 3
        offset = time.time() - snapshot_time
        clock_dir = network.store.state_dir / "fixture-clock"
        clock_dir.mkdir()
        (clock_dir / "sitecustomize.py").write_text(
            "import os,time\n"
            "_real_time=time.time\n"
            "_offset=float(os.environ['HT_FIXTURE_CLOCK_OFFSET'])\n"
            "time.time=lambda: _real_time()-_offset\n"
        )
        monkeypatch.setenv("HT_FIXTURE_CLOCK_OFFSET", str(offset))
        monkeypatch.setenv(
            "PYTHONPATH", str(clock_dir) + os.pathsep + os.environ.get("PYTHONPATH", "")
        )
        monkeypatch.setattr(time, "time", lambda: snapshot_time + time.monotonic() - baseline)
        network.push(network.now)
        network.store.clock = lambda: (
            network.manifest.training.beacon.genesis_time + (network.now - 1) * 3
        )
        _live_checkpoint(network)
        return
    _graduate_four(network)
    _live_checkpoint(network)


def _graduate_four(network):
    """Execute all required signed references/proofs/finalizations, no authority shortcuts."""
    from hypertrain.auditor.replay import unpack_state
    from hypertrain.data.trial_assignment import trial_samples
    from hypertrain.miner.island_launch import validate_artifacts
    from hypertrain.protocol.messages import Finalize
    from hypertrain.protocol.messages_v2 import EscrowLock, JoinChallenge, WorkProof, WorkScreenV2
    from hypertrain.trainer.compress import state_hash

    identities = []
    for i in range(4):
        current = network.store._db.execute(
            "SELECT * FROM admissions_v2 WHERE hotkey=?", (fixtures.HOT[i].ss58,)
        ).fetchone()
        if current is not None and current["clean_count"] == 12:
            assert current["state"] == "ACTIVE"
            continue
        if i and current is None:
            network.push(network.now + 101)
        identity = current["admission_id"] if current else network.join(i).json()["admission_id"]
        identities.append(identity)
        lock = EscrowLock(
            operation_id=f"{i + 1:064x}",
            owner=fixtures.COLD[i].ss58,
            units=1000,
            origin_ids=[network.origin_ids[i]],
            admission_id=identity,
            dispute_id=None,
            kind="LOCK_ADMISSION",
        )
        if current is None:
            response = network.client.post(
                network.url + "/escrow/lock",
                json=network.signed(fixtures.COLD[i], "EscrowLock", lock),
            )
            assert response.status_code == 200, response.text
        else:
            status = network.store._services(network.manifest.run_id())[1].status(
                fixtures.HOT[i].ss58, now=network.now
            )
            assert status.funding.locked_units == 1000
        # When: twelve actual full reference executions per identity, exact proof/finality.
        for _trial in range(current["clean_count"] if current else 0, 12):
            pending = network.store._db.execute(
                "SELECT r.outcome FROM admissions_v2 a JOIN admission_trial_results r "
                "ON r.epoch=a.trial_epoch WHERE a.admission_id=?",
                (identity,),
            ).fetchone()
            if pending is None or pending["outcome"] != "OPEN":
                network.push(network.now + 1)
            response = network.client.get(network.url + "/join/" + identity + "/challenge")
            assert response.status_code == 200, response.text
            challenge = JoinChallenge.model_validate(response.json()["body"])
            response = network.client.post(
                network.url + "/admin/join/" + identity + "/reference", headers=fixtures.admin()
            )
            assert response.status_code == 200, response.text
            row = network.store._db.execute(
                "SELECT * FROM admissions_v2 WHERE admission_id=?", (identity,)
            ).fetchone()
            proof = WorkProof.model_validate_json(row["reference_json"])
            job, directory = network.store.stage_trial_v2(
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
            artifacts = validate_artifacts(job, directory / "published")
            from hypertrain.protocol.hashing import sha256_hex

            assert [
                sha256_hex(path.read_bytes())
                for path in (artifacts.state, artifacts.ef, artifacts.delta, artifacts.leaves)
            ] == [ref.sha256 for ref in proof.artifact_refs]
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
            theta, _ = unpack_state(network.store.objects.get(proof.artifact_refs[0].sha256))
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
            if _trial == 0:
                assert response.json()["shadow_only"] is True
                assert response.json()["eligible"] is False
                assert (
                    network.store._services(network.manifest.run_id())[0].balances().issued == 40000
                )
        # Then: actual ACTIVE admission, funded locks unchanged, no shadow mint/global advance.
        assert response.json()["record"]["state"] == "ACTIVE"
        assert response.json()["record"]["clean_count"] == 12
    assert (
        network.store._db.execute(
            "SELECT COUNT(*) FROM records_v2 WHERE kind='applied'"
        ).fetchone()[0]
        == 0
    )
    assert network.store._services(network.manifest.run_id())[0].balances().issued == 40000


@pytest.mark.parametrize("od_carry_network", ["mlm", "decision", "distill"], indirect=True)
def test_od_four_active_actual_client_two_round_carry_checkpoint(od_carry_network, monkeypatch):
    """Production service path with same-manifest admission and actual HTTP/TLS miners."""
    network = od_carry_network
    _od_verified_admissions(network, monkeypatch)
    _od_production_rounds(network)


def _od_verified_admissions(network, monkeypatch):
    """Reuse only an authenticated matching manifest's original finalized work."""
    import hashlib
    import time

    from hypertrain.challenge.store import ChallengeError, ChallengeStore
    from hypertrain.miner.island_launch import validate_artifacts
    from hypertrain.protocol import envelope_v2
    from hypertrain.protocol.keys import Keypair
    from hypertrain.protocol.messages_v2 import (
        JoinChallenge,
        RunManifestV2,
        WorkProof,
        WorkScreenV2,
    )

    objective = network.manifest.training.model.od.objective
    snapshot = Path(tempfile.gettempdir()) / f"hypertrain-od-carry-{objective}-active"
    resume = os.environ.get("HT_OD_ADMISSION_RESUME")
    source = snapshot if snapshot.exists() else Path(resume) if resume else None
    if source is not None:
        with (
            sqlite3.connect(f"file:{source / 'challenge.db'}?mode=ro", uri=True) as saved,
            sqlite3.connect(":memory:") as compatible,
        ):
            run_id, raw = saved.execute("SELECT run_id,manifest FROM runs").fetchone()
            manifest = RunManifestV2.model_validate_json(raw)
            expected = network.manifest.body()
            expected["training"]["beacon"]["genesis_time"] = manifest.training.beacon.genesis_time
            assert manifest.body() == expected and manifest.run_id() == run_id
            network.store.close_v2_notifications()
            network.store._services_v2.clear()
            saved.backup(compatible)
            namespace = compatible.execute(
                "SELECT value FROM meta WHERE key='admission-prefix-namespace-v2'"
            ).fetchone()
            if namespace is None:
                # Test-only history import: preserve unknown legacy quotas, never translate them.
                legacy = compatible.execute(
                    "SELECT * FROM admission_quota ORDER BY identity"
                ).fetchall()
                compatible.execute(
                    "ALTER TABLE admission_quota RENAME TO fixture_legacy_admission_quota"
                )
                compatible.commit()
                assert (
                    compatible.execute(
                        "SELECT * FROM fixture_legacy_admission_quota ORDER BY identity"
                    ).fetchall()
                    == legacy
                )
            compatible.backup(network.store._db)
        network.manifest = manifest
        for name in ("objects", "trials-v2"):
            shutil.copytree(source / name, network.store.state_dir / name, dirs_exist_ok=True)
        network.now = network.store._now(network.store._db)
        baseline = time.monotonic()
        historical = manifest.training.beacon.genesis_time + (network.now - 1) * 3
        offset = time.time() - historical
        clock = network.store.state_dir / "fixture-clock"
        clock.mkdir()
        (clock / "sitecustomize.py").write_text(
            "import os,time\n"
            "_real_time=time.time\n"
            "_offset=float(os.environ['HT_FIXTURE_CLOCK_OFFSET'])\n"
            "time.time=lambda: _real_time()-_offset\n"
        )
        monkeypatch.setenv("HT_FIXTURE_CLOCK_OFFSET", str(offset))
        monkeypatch.setenv("PYTHONPATH", str(clock) + os.pathsep + os.environ.get("PYTHONPATH", ""))
        monkeypatch.setattr(time, "time", lambda: historical + time.monotonic() - baseline)
        network.push(network.now)
    if not snapshot.exists():
        _graduate_four(network)
        snapshot.mkdir()
        with sqlite3.connect(snapshot / "challenge.db") as saved:
            network.store._db.backup(saved)
        for name in ("objects", "trials-v2"):
            shutil.copytree(network.store.state_dir / name, snapshot / name)

    admission = network.store._services(network.manifest.run_id())[1]
    namespace = network.store._db.execute(
        "SELECT value FROM meta WHERE key='admission-prefix-namespace-v2'"
    ).fetchone()[0]
    assert namespace == hashlib.sha256(admission.ip_secret).hexdigest()
    assert network.store._prefix_secret_v2() == admission.ip_secret
    reopened = ChallengeStore(
        network.store.state_dir,
        network.store.params,
        Keypair(b"\xcc" * 32),
        owner_hotkey=network.store.owner_hotkey,
        verify_beacon=network.store.verify_beacon,
        objects=network.store.objects,
    )
    try:
        with pytest.raises(ChallengeError, match="admission prefix namespace differs"):
            reopened._prefix_secret_v2()
    finally:
        reopened.close_v2_notifications()
        reopened._db.close()
    rows = network.store._db.execute("SELECT * FROM admissions_v2 ORDER BY hotkey").fetchall()
    assert {r["hotkey"] for r in rows} == {key.ss58 for key in fixtures.HOT}
    for row in rows:
        status = admission.status(row["hotkey"], now=network.now)
        assert status.record.state == "ACTIVE" and status.record.clean_count == 12
        assert status.record.canary_blocks == (1, 2, 3)
        assert status.eligible and not status.shadow_only
        assert status.funding.locked_units == 1000
        trials = network.store._db.execute(
            "SELECT t.*,r.challenge,r.reference,r.proof,r.screen,r.outcome "
            "FROM admission_trials t JOIN admission_trial_results r ON r.epoch=t.epoch "
            "WHERE t.admission_id=? ORDER BY t.epoch",
            (row["admission_id"],),
        ).fetchall()
        assert len(trials) == 12
        for trial in trials:
            challenge = JoinChallenge.model_validate_json(trial["challenge"])
            proof = WorkProof.model_validate_json(trial["reference"])
            signed = envelope_v2.parse_envelope(trial["proof"])
            screen_env = envelope_v2.parse_envelope(trial["screen"])
            assert envelope_v2.verify_envelope(signed.model_dump())
            assert envelope_v2.verify_envelope(screen_env.model_dump())
            assert signed.run_id == screen_env.run_id == network.manifest.run_id()
            assert signed.signer == screen_env.signer == row["hotkey"]
            assert WorkProof.model_validate(signed.body) == proof
            assert proof.digest() == trial["evidence_hash"] and trial["outcome"] == "MATCH"
            from hypertrain.data.trial_assignment import trial_samples

            job, directory = network.store.stage_trial_v2(
                network.manifest.run_id(),
                challenge,
                tuple(
                    trial_samples(
                        network.manifest,
                        row["admission_id"],
                        challenge.nonce,
                        network.store._beacon_v2(challenge.seed_beacon),
                    )
                ),
                trial["epoch"],
            )
            artifacts = validate_artifacts(job, directory / "published")
            assert (
                list(artifacts.ranks) == WorkScreenV2.model_validate(screen_env.body).rank_results
            )
            from hypertrain.protocol.hashing import sha256_hex

            assert [
                sha256_hex(path.read_bytes())
                for path in (artifacts.state, artifacts.ef, artifacts.delta, artifacts.leaves)
            ] == [ref.sha256 for ref in proof.artifact_refs]
            final_raw = network.store._db.execute(
                "SELECT receipt FROM admission_reservations WHERE reservation=?",
                (f"trial-final|{row['admission_id']}|{trial['epoch']}",),
            ).fetchone()[0]
            final = envelope_v2.parse_envelope(final_raw)
            assert envelope_v2.verify_envelope(final.model_dump())
            assert final.signer == fixtures.COORD.ss58 and final.run_id == network.manifest.run_id()
            assert final.body["entitlements_root"] == proof.digest()
            assert final.body["w"] == trial["epoch"]
            assert final.body["included"] == [row["hotkey"]]
            from hypertrain.auditor.replay import unpack_state
            from hypertrain.trainer.compress import state_hash

            assert final.body["final_theta_hash_w1"] == state_hash(
                unpack_state(artifacts.state.read_bytes())[0]
            )
    assert network.store._services(network.manifest.run_id())[0].balances().issued == 40000
    assert (
        network.store._db.execute(
            "SELECT COUNT(*) FROM records_v2 WHERE kind IN ('round','applied','finality')"
        ).fetchone()[0]
        == 0
    )


def _od_production_rounds(network, *, after_audits=None):
    """Two real carried client rounds, independent full audits, signed replay checkpoint."""
    import anyio
    import httpx
    import torch

    from hypertrain.aggregator.checkpoint import (
        network_inputs,
        verify_network_checkpoint,
        write_network_checkpoint,
    )
    from hypertrain.aggregator.core import load_state
    from hypertrain.aggregator.tape_v2 import TapeV2, replay_tape
    from hypertrain.auditor.replay import AnchorCache, optimizer_hash, pack_state, unpack_state
    from hypertrain.data.store import LocalFSStore
    from hypertrain.data.stream_store import StreamStore
    from hypertrain.protocol import envelope_v2
    from hypertrain.protocol.hashing import sha256_hex
    from hypertrain.protocol.jcs import canonicalize
    from hypertrain.protocol.messages import Finalize, f32hex
    from hypertrain.protocol.messages_v2 import (
        AggregationPolicyV2,
        PolicyHashes,
        RoundOpenV2,
        StartStateV2,
    )
    from hypertrain.relay.app import create_app
    from hypertrain.relay.core import Relay
    from hypertrain.trainer.compress import state_hash
    from hypertrain.trainer.config import TrainConfig
    from hypertrain.trainer.model import init_params

    manifest = network.manifest
    run_id = manifest.run_id()
    cfg = TrainConfig.from_manifest_v2(manifest)
    assert cfg.inner.state_policy == "carry"
    rounds, terminal = [], {}
    for w in range(2):
        cache = AnchorCache()
        theta = init_params(cfg.model)
        predecessor, prev_final = "0" * 64, "0" * 64
        if w:
            previous = network.store._record_v2(run_id, "applied", str(w - 1))
            theta = {
                k: torch.from_numpy(v.copy())
                for k, v in load_state(network.store.objects, previous["out_state"]).theta.items()
            }
            predecessor = previous["tape_hash"]
            prev_final = sha256_hex(canonicalize(rounds[-1]["finalize"]["body"]))
        starts, roster = [], []
        for i, hot in enumerate(fixtures.HOT):
            status = network.store._services(run_id)[1].status(hot.ss58, now=network.now)
            anchor = (
                network.store._restore_anchor_v2(run_id, hot.ss58, w - 1, cache)
                if w
                else cache.genesis(manifest, hot.ss58, theta)
            )
            assert anchor.state.step == w * cfg.inner.H
            if w:
                prior_theta, prior_state = unpack_state(terminal[hot.ss58][0])
                prior_ef, _ = unpack_state(terminal[hot.ss58][1])
                assert optimizer_hash(anchor.state) == optimizer_hash(prior_state)
                assert pack_state(anchor.theta, anchor.state) == pack_state(
                    prior_theta, prior_state
                )
                assert pack_state(anchor.ef) == pack_state(prior_ef)
            starts.append(
                StartStateV2(
                    run_id=run_id,
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
            prev_final_hash=prev_final,
            theta_hash=state_hash(theta),
            outer_state_hash="0" * 64,
            center_hash="0" * 64,
            roster_hash=sha256_hex(canonicalize(roster)),
            honeypot_commit="0" * 64,
            d_open=network.now + 1,
            d_assign=network.now + 2,
            d_commit=network.now + 30,
            d_audit=network.now + 31,
            d_upload=network.now + 200,
            d_final=network.now + 300,
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
            json=network.signed(fixtures.COORD, "RoundOpenV2", opening),
            headers=fixtures.admin(),
        )
        assert response.status_code == 200, response.text
        network.push(opening.d_assign)

        view = network.client.get(network.url + f"/rounds/{w}")
        assert view.status_code == 200, view.text

        async def clients(round_index):
            async with httpx.AsyncClient() as backing:

                async def now():
                    return network.now

                assignment = network.store.relay_assignment_v2(
                    run_id, round_index, fixtures.HOT[0].ss58
                )
                relay = Relay(
                    run_id=run_id,
                    master=fixtures.COORD.ss58,
                    registry=network.store._registry_v2(manifest),
                    network_manifest_hash=sha256_hex(
                        canonicalize(assignment["network_manifest"]["body"])
                    ),
                    observers={key.ss58 for key in fixtures.AUDITORS},
                    relay_id="local",
                    region="local",
                    keys={"k1": fixtures.RELAY},
                    active_key="k1",
                    backing=StreamStore(
                        LocalFSStore(network.store.state_dir / f"relay-{round_index}"), backing
                    ),
                    now=now,
                )
                await anyio.to_thread.run_sync(
                    lambda: _exact_network_client(
                        network, relay, create_app(relay, "x" * 32), w=round_index, all_miners=True
                    )
                )

        anyio.run(clients, w)
        for hot in fixtures.HOT:
            identity = f"{w}:{hot.ss58}"
            commit = network.store._record_v2(run_id, "commit", identity)
            assert envelope_v2.verify_envelope(commit)
            assert commit["body"]["tokens"] == manifest.training.batch_samples() * cfg.model.seq_len
            assert manifest.training.batch_samples() == (
                cfg.inner.H * cfg.inner.micro_batch * cfg.inner.grad_accum * 2
            )
            published = (
                network.store.state_dir
                / "tls-client/work"
                / run_id
                / hot.ss58
                / str(w)
                / "published"
            )
            states = [
                (published / f"rank-{rank}/state.safetensors").read_bytes() for rank in range(2)
            ]
            assert states[0] == states[1]
            end_theta, end_state = unpack_state(states[0])
            assert end_state.step == (w + 1) * cfg.inner.H
            assert any(torch.count_nonzero(v) for v in end_state.m.values())
            assert state_hash(end_theta) == commit["body"]["final_theta_hash"]
            terminal[hot.ss58] = (states[0], (published / "rank-0/ef.safetensors").read_bytes())

        network.push(opening.d_audit)
        response = network.client.post(
            network.url + f"/admin/rounds/{w}/audits", headers=fixtures.admin()
        )
        assert response.status_code == 200, response.text
        for index in range(4):
            auditor = fixtures.AUDITORS[index % 2]
            response = network.client.post(
                network.url + "/worker/lease",
                json=network.signed(
                    auditor,
                    "Receipt",
                    {
                        "w": w,
                        "commit_hash": f"{w * 4 + index + 1:064x}",
                        "received_round": network.now,
                    },
                ),
            )
            assert response.status_code == 200, response.text
            job = response.json()["body"]
            assert job["start_state"]["global_step0"] == w * cfg.inner.H
            response = network.client.post(
                network.url + "/worker/jobs/" + job["job_id"] + "/execute",
                json=network.signed(
                    auditor,
                    "Receipt",
                    {"w": w, "commit_hash": job["lease_nonce"], "received_round": network.now},
                ),
            )
            assert response.status_code == 200, response.text
            assert response.json()["verdict"]["result"] == "MATCH"
            response = network.client.post(
                network.url + "/worker/jobs/" + job["job_id"] + "/complete",
                json=network.signed(auditor, "ReplayVerdict", response.json()["verdict"]),
            )
            assert response.status_code == 200, response.text
        if after_audits is not None:
            after_audits(network)
            return
        response = network.client.post(
            network.url + f"/admin/rounds/{w}/aggregate", headers=fixtures.admin()
        )
        assert response.status_code == 200, response.text
        applied = response.json()
        tape = TapeV2.from_bytes(network.store.objects.get(applied["tape_hash"]))
        weights = tape.body.allocation.entries
        assert {e.hotkey for e in weights} == {k.ss58 for k in fixtures.HOT}
        assert sum(e.weight_units for e in weights) == 16777216
        assert all(e.weight_units == 4194304 and not e.probation for e in weights)
        assert all(x.roster.state == "ACTIVE" for x in tape.body.inputs)
        assert tape.body.predecessor_tape_hash == predecessor
        inputs = network.store._record_v2(run_id, "tape-inputs", str(w))["inputs"]
        replayed = replay_tape(
            network.store.objects,
            tape,
            manifest,
            AggregationPolicyV2.model_validate_json(
                network.store.objects.get(manifest.network.aggregation_policy_hash)
            ),
            network.store.objects.get(manifest.network.economics_policy_hash),
            signer=fixtures.COORD.ss58,
            w=w,
            prev_state=applied["prev_state"],
            predecessor_tape_hash=predecessor,
            inputs=network_inputs(inputs),
            reference_reward_units=network.store._services(run_id)[0].policy.R_collectible_units,
        )
        assert replayed.to_bytes() == network.store.objects.get(applied["out_state"])
        final = Finalize(
            w=w,
            final_theta_hash_w1=applied["theta_hash"],
            included=sorted(k.ss58 for k in fixtures.HOT),
            entitlements_root="0" * 64,
        )
        response = network.client.post(
            network.url + f"/admin/rounds/{w}/finalize",
            json=network.signed(fixtures.COORD, "Finalize", final),
            headers=fixtures.admin(),
        )
        assert response.status_code == 200, response.text
        assert response.json()["status"] == "SETTLED"
        balances = network.store._services(run_id)[0].balances()
        assert balances.issued == 40000 + (w + 1) * 1000000
        origins = network.store._db.execute(
            "SELECT hotkey,issued FROM escrow_origins WHERE reward_round=? ORDER BY hotkey", (w,)
        ).fetchall()
        assert [(r[0], r[1]) for r in origins] == [
            (hot, 250000) for hot in sorted(k.ss58 for k in fixtures.HOT)
        ]
        repeated = network.client.post(
            network.url + f"/admin/rounds/{w}/finalize",
            json=network.signed(fixtures.COORD, "Finalize", final),
            headers=fixtures.admin(),
        )
        assert repeated.status_code == 200 and repeated.json() == response.json()
        assert network.store._services(run_id)[0].balances() == balances
        assert network.store._record_v2(run_id, "replayed-finality", str(w))["shadow"] is False
        rounds.append(
            {
                "round_open": network.store._record_v2(run_id, "round", str(w)),
                "finalize": network.store._record_v2(run_id, "finalize", str(w)),
                "inputs": inputs,
                "tape_hash": applied["tape_hash"],
                "prev_state": applied["prev_state"],
                "predecessor_tape_hash": predecessor,
                "reference_reward_units": network.store._services(run_id)[
                    0
                ].policy.R_collectible_units,
            }
        )
    checkpoint = network.store.state_dir / "od-carry-checkpoint"
    body = write_network_checkpoint(
        checkpoint, fixtures.COORD, network.store.objects, manifest, rounds
    )
    assert body["rounds"] == [0, 1]
    assert verify_network_checkpoint(checkpoint, fixtures.COORD.ss58) == []


@pytest.mark.parametrize("od_carry_network", ["mlm"], indirect=True)
def test_public_watch_layer_over_actual_fastapi_tls(od_carry_network, monkeypatch):
    """Funded original audits; real public CLI/TLS routes and committed long-poll wake."""
    network = od_carry_network
    snapshot = Path(tempfile.gettempdir()) / "hypertrain-od-carry-mlm-active"
    assert (snapshot / "challenge.db").is_file(), "original verified48 snapshot required"
    _od_verified_admissions(network, monkeypatch)
    _od_production_rounds(network, after_audits=lambda n: _public_watch_fastapi_tls(n, monkeypatch))


def _public_watch_fastapi_tls(network, monkeypatch):
    import contextlib
    import io
    import socket
    import ssl
    import subprocess
    import threading

    import httpx
    import uvicorn

    from hypertrain.auditor.island_bisect import IslandParty
    from hypertrain.challenge.disputes_v2 import EventAck, Turn, WatchEvent, authentic
    from hypertrain.miner.cli import main
    from hypertrain.protocol import envelope_v2
    from hypertrain.protocol.hashing import sha256_hex
    from hypertrain.protocol.jcs import canonicalize
    from hypertrain.protocol.messages_v2 import BisectV2, DisputeV2, EscrowLock, IslandJobV1

    hot, run_id = fixtures.HOT[0], network.manifest.run_id()
    escrow, _, disputes = network.store._services(run_id)
    contest = json.loads(
        network.store._db.execute(
            "SELECT data FROM records_v2 WHERE kind='contest' AND json_extract(data,'$.miner')=?",
            (hot.ss58,),
        ).fetchone()[0]
    )
    verdict_hash = contest["verdict_hash"]
    dispute_id = sha256_hex(canonicalize([run_id, verdict_hash, hot.ss58]))
    origin = network.store._db.execute(
        "SELECT origin FROM escrow_units WHERE owner=? AND bucket='available' AND units>=100",
        (fixtures.COLD[0].ss58,),
    ).fetchone()[0]
    lock = EscrowLock(
        operation_id="ce" * 32,
        owner=fixtures.COLD[0].ss58,
        units=100,
        origin_ids=[origin],
        admission_id=None,
        dispute_id=dispute_id,
        kind="LOCK_CONTEST",
    )
    response = network.client.post(
        network.url + "/escrow/lock", json=network.signed(fixtures.COLD[0], "EscrowLock", lock)
    )
    assert response.status_code == 200, response.text
    geometry = network.store._record_v2(run_id, "trace-geometry", verdict_hash)
    job = IslandJobV1.model_validate(geometry["job"])
    published = Path(geometry["directory"])
    party = IslandParty.published(hot.ss58, job, published)
    job_path = published.parent / "job.json"
    job_path.write_bytes(canonicalize(job.body()))
    directory = network.store.state_dir / "fastapi-watch"
    directory.mkdir()
    cert, server_key = directory / "cert.pem", directory / "server.key"
    subprocess.run(
        [
            "openssl",
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-days",
            "1",
            "-keyout",
            str(server_key),
            "-out",
            str(cert),
            "-subj",
            "/CN=localhost",
            "-addext",
            "subjectAltName=DNS:localhost",
        ],
        check=True,
        capture_output=True,
    )
    monkeypatch.setenv("SSL_CERT_FILE", str(cert))
    key = directory / "miner.key"
    key.write_bytes(bytes([80]) * 32)
    key.chmod(0o600)
    work = directory / "work"
    watch_path = work / run_id / hot.ss58 / "watch/watch.db"
    waiting, completed = threading.Event(), threading.Event()

    class SubscribedCondition(threading.Condition):
        def wait(self, timeout=None):
            waiting.set()
            return super().wait(timeout)

    class ReadyServer(uvicorn.Server):
        def __init__(self, config):
            super().__init__(config)
            self.ready = threading.Event()

        async def startup(self, sockets=None):
            await super().startup(sockets=sockets)
            self.ready.set()

    sock = socket.socket()
    sock.bind(("127.0.0.1", 0))
    port = sock.getsockname()[1]
    server = ReadyServer(
        uvicorn.Config(
            network.client.app,
            log_level="error",
            ssl_certfile=str(cert),
            ssl_keyfile=str(server_key),
        )
    )
    serving = threading.Thread(target=lambda: server.run(sockets=[sock]), daemon=True)
    config = directory / "miner.toml"
    config.write_text(
        "\n".join(
            f"{name} = {json.dumps(str(value))}"
            for name, value in {
                "api": f"https://localhost:{port}",
                "keyfile": key,
                "workdir": work,
                "state_source": "file",
                "image_digest": job.manifest.training.reference_spec.image_digest,
                "run_id": run_id,
                "owner_hotkey": fixtures.OWNER.ss58,
                "device": "cpu",
            }.items()
        )
    )
    failures, result, output = [], [], io.StringIO()

    def watch():
        try:
            with contextlib.redirect_stdout(output):
                result.append(
                    main(
                        [
                            "watch",
                            "--config",
                            str(config),
                            "--job",
                            str(job_path),
                            "--timeout",
                            "10",
                        ]
                    )
                )
        except BaseException as error:
            failures.append(error)
        finally:
            completed.set()

    worker = threading.Thread(target=watch, daemon=True)
    serving.start()
    try:
        assert server.ready.wait(10)
        with httpx.Client(
            base_url=f"https://localhost:{port}" + network.url,
            verify=ssl.create_default_context(cafile=str(cert)),
        ) as client:
            opened = client.post(
                "/dispute",
                json=network.signed(
                    hot,
                    "DisputeV2",
                    DisputeV2(
                        hotkey=hot.ss58,
                        verdict_hash=verdict_hash,
                        action="contest",
                        bond_lock=lock.operation_id,
                    ),
                ),
            )
            assert opened.status_code == 200, opened.text
            before = escrow.balances()
            params = {"party": hot.ss58, "cursor": 0, "timeout": 0}
            signature = hot.sign(f"hypertrain/watch/2|{run_id}|{hot.ss58}".encode()).hex()
            events = client.get(
                "/disputes", params=params, headers={"X-Dispute-Signature": signature}
            )
            assert events.status_code == 200, events.text
            event = WatchEvent.model_validate(events.json()[0])
            ack = EventAck(
                run_id=run_id,
                cursor=event.cursor,
                event_hash=event.digest(),
                party=hot.ss58,
                received_beacon=network.now,
                sig="00" * 64,
            )
            tables = ("dispute_events_v2", "dispute_acks_v2", "dispute_entries_v2")
            unchanged = {
                t: [tuple(r) for r in network.store._db.execute(f'SELECT * FROM "{t}"')]
                for t in tables
            }
            assert client.get("/disputes", params=params).status_code == 403
            wrong = hot.sign(f"hypertrain/watch/2|{'ff' * 32}|{hot.ss58}".encode()).hex()
            assert (
                client.get(
                    "/disputes", params=params, headers={"X-Dispute-Signature": wrong}
                ).status_code
                == 403
            )
            assert client.post("/disputes/ack", json=ack.body()).status_code == 409
            assert {
                t: [tuple(r) for r in network.store._db.execute(f'SELECT * FROM "{t}"')]
                for t in tables
            } == unchanged
            assert escrow.balances() == before and not watch_path.exists()
            initial = io.StringIO()
            with contextlib.redirect_stdout(initial):
                assert (
                    main(
                        ["watch", "--config", str(config), "--job", str(job_path), "--timeout", "0"]
                    )
                    == 0
                )
            assert json.loads(initial.getvalue()) == {"processed": 1}
            step = BisectV2.model_validate_json(
                network.store._db.execute(
                    "SELECT body FROM dispute_entries_v2 WHERE id=? AND seq=0", (dispute_id,)
                ).fetchone()[0]
            )
            current = disputes.get(dispute_id)
            auditor = next(k for k in fixtures.AUDITORS if k.ss58 == current.expected_party)
            assert current.level == "step" and current.interval == (0, 2)
            counterpart = BisectV2(
                dispute_id=dispute_id,
                seq=current.seq,
                level=current.level,
                ctx=current.ctx,
                interval=current.interval,
                N=2,
                hashes=[step.hashes[0], "aa" * 32, "bb" * 32],
                party=auditor.ss58,
                previous_transcript_hash=current.transcript_hash,
            )
            original_condition = disputes.condition
            disputes.condition = SubscribedCondition()
            try:
                worker.start()
                assert waiting.wait(15), failures
                assert not completed.is_set()
                transition = client.post(
                    "/bisect", json=network.signed(auditor, "BisectV2", counterpart)
                )
                assert transition.status_code == 200, transition.text
                layer = Turn.model_validate(transition.json())
                assert layer.level == "layer" and layer.expected_party == hot.ss58
                assert completed.wait(20), failures
                worker.join(1)
                assert not worker.is_alive() and not failures and result == [0]
                assert json.loads(output.getvalue()) == {"processed": 1}
            finally:
                worker.join(20)
                disputes.condition = original_condition
            row = network.store._db.execute(
                "SELECT body,envelope FROM dispute_entries_v2 WHERE id=? AND seq=?",
                (dispute_id, layer.seq),
            ).fetchone()
            accepted = BisectV2.model_validate_json(row[0])
            signed = envelope_v2.parse_envelope(row[1])
            assert envelope_v2.verify_envelope(signed.model_dump())
            assert signed.signer == hot.ss58 and signed.run_id == run_id
            assert (
                accepted.level,
                accepted.ctx,
                accepted.party,
                accepted.previous_transcript_hash,
            ) == ("layer", layer.ctx, hot.ss58, layer.transcript_hash)
            lo, hi = layer.interval
            assert accepted.hashes == party.hashes(
                "layer", tuple(layer.ctx), [lo, -(-(lo + hi) // 2), hi]
            )
            with sqlite3.connect(watch_path) as saved:
                cursor, raw_event, raw_ack, durable, done = saved.execute(
                    "SELECT cursor,event,ack,envelope,done FROM watch_outbox "
                    "ORDER BY cursor DESC LIMIT 1"
                ).fetchone()
                assert (
                    saved.execute("SELECT MAX(cursor) FROM watch_outbox WHERE done=1").fetchone()[0]
                    == cursor
                )
            received = WatchEvent.model_validate_json(raw_event)
            saved_ack = EventAck.model_validate_json(raw_ack)
            assert (
                cursor > event.cursor and done == 1 and authentic(received) and authentic(saved_ack)
            )
            assert received.turn == layer and saved_ack.event_hash == received.digest()
            assert BisectV2.model_validate(envelope_v2.parse_envelope(durable).body) == accepted
            assert (
                network.store._db.execute(
                    "SELECT ack FROM dispute_acks_v2 WHERE cursor=? AND party=?", (cursor, hot.ss58)
                ).fetchone()[0]
                == saved_ack.model_dump_json()
            )
            assert escrow.balances() == before
    finally:
        server.should_exit = True
        serving.join(10)
        if worker.ident is not None:
            worker.join(20)
        sock.close()
        assert not serving.is_alive() and not worker.is_alive()
