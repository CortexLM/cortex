"""Small D3 dispatch/authority/cancellation contracts; no graduation or model training."""

from __future__ import annotations

import contextlib
import ctypes
import importlib.util
import json
import os
import shutil
import socket
import sqlite3
import ssl
import subprocess
import sys
import threading
from pathlib import Path

import certifi
import pytest
from pydantic import Field, ValidationError, create_model

SCRIPTS = Path(__file__).parents[2] / "scripts"
sys.path.insert(0, str(SCRIPTS))
spec = importlib.util.spec_from_file_location(
    "service_proof_driver", SCRIPTS / "network_service_proof.py"
)
assert spec is not None and spec.loader is not None
driver = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = driver
spec.loader.exec_module(driver)


@pytest.mark.parametrize("present", [False, True])
@pytest.mark.parametrize("raises", [False, True])
def test_cli_ca_environment_restores_native_presence_and_value(monkeypatch, present, raises):
    """Tiny native environment restoration check; no signed package or CLI graph."""
    original = os.environ
    if present:
        monkeypatch.setenv("SSL_CERT_FILE", certifi.where())
    else:
        monkeypatch.delenv("SSL_CERT_FILE", raising=False)
    before = os.environ.get("SSL_CERT_FILE")
    getenv = ctypes.CDLL(None).getenv
    getenv.argtypes = [ctypes.c_char_p]
    getenv.restype = ctypes.c_char_p
    expected = (
        pytest.raises(RuntimeError, match="CA exception control")
        if raises
        else contextlib.nullcontext()
    )
    with expected:
        with monkeypatch.context() as patch:
            patch.setenv("SSL_CERT_FILE", certifi.where())
            os.environ["SSL_CERT_FILE"] = "temporary-operation-ca.pem"
            assert getenv(b"SSL_CERT_FILE") == b"temporary-operation-ca.pem"
            if raises:
                raise RuntimeError("CA exception control")
    assert os.environ is original and os.environ.get("SSL_CERT_FILE") == before
    assert getenv(b"SSL_CERT_FILE") == (None if before is None else os.fsencode(before))


@pytest.fixture(scope="module")
def cli_ca(tmp_path_factory):
    """Real temporary CLI trust anchor; independent of metadata HTTP transport."""
    directory = tmp_path_factory.mktemp("driver-cli-ca")
    cert, key = directory / "ca.pem", directory / "ca.key"
    subprocess.run(
        [
            "openssl",
            "req",
            "-x509",
            "-newkey",
            "ec",
            "-pkeyopt",
            "ec_paramgen_curve:P-256",
            "-nodes",
            "-days",
            "1",
            "-keyout",
            str(key),
            "-out",
            str(cert),
            "-subj",
            "/CN=master.test",
            "-addext",
            "subjectAltName=DNS:master.test",
        ],
        check=True,
        capture_output=True,
    )
    return cert, key


def verify_cli_ca(context, cert, key):
    """Bounded real TLS handshake in memory; wrong CA must fail verification."""
    assert context.verify_mode == ssl.CERT_REQUIRED and context.check_hostname
    server = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    server.load_cert_chain(cert, key)
    incoming = [ssl.MemoryBIO(), ssl.MemoryBIO()]
    outgoing = [ssl.MemoryBIO(), ssl.MemoryBIO()]
    peers = [
        context.wrap_bio(incoming[0], outgoing[0], server_hostname="master.test"),
        server.wrap_bio(incoming[1], outgoing[1], server_side=True),
    ]
    completed = [False, False]
    for _ in range(10):
        for i, peer in enumerate(peers):
            if not completed[i]:
                try:
                    peer.do_handshake()
                    completed[i] = True
                except ssl.SSLWantReadError:
                    pass
            incoming[1 - i].write(outgoing[i].read())
        if all(completed):
            return
    pytest.fail("TLS handshake did not complete within bounded exchanges")


fixture_spec = importlib.util.spec_from_file_location(
    "driver_hermetic_authority", SCRIPTS.parent / "tests/challenge/authority_fixture_v2.py"
)
assert fixture_spec is not None and fixture_spec.loader is not None
authority_fixture = importlib.util.module_from_spec(fixture_spec)
sys.modules[fixture_spec.name] = authority_fixture
fixture_spec.loader.exec_module(authority_fixture)


@pytest.fixture(scope="module")
def signed_package(tmp_path_factory):
    """Real signed intake/escrow predicates, explicit metadata-only execution oracle."""
    root = tmp_path_factory.mktemp("driver-signed-metadata")
    source, directory = root / "source", root / "bundle"
    run_id, originals = authority_fixture.build_authority(source)
    with pytest.MonkeyPatch.context() as patch:
        patch.setattr(driver.authority, "RUN", run_id)
        patch.setattr(
            driver.authority,
            "Bundle",
            create_model(
                "DriverHermeticBundle",
                __base__=driver.authority.Bundle,
                run_id=(str, Field(default=run_id, pattern="^" + run_id + "$")),
            ),
        )
        patch.setattr(driver.authority, "validate_artifacts", authority_fixture.metadata_artifacts)
        digest = driver.authority.export(source, directory)
        bundle = driver.authority.validate(directory, digest)
        secrets = root / "roles"
        secrets.mkdir(mode=0o700)
        for role, public in bundle.public_roles.items():
            path = secrets / f"{role}.seed"
            path.write_bytes(originals[public])
            path.chmod(0o600)
        token = secrets / "admin.token"
        token.write_text("hermetic-driver-token")
        token.chmod(0o600)
        template = root / "service"
        with driver.authority.bootstrap(
            directory, digest, template, secrets, "https://master.test"
        ):
            offset = float(os.environ["HT_FIXTURE_CLOCK_OFFSET"])
        from hypertrain.beacon.core import FixtureBeacon

        beacon = FixtureBeacon(current=bundle.beacon_round).get(bundle.beacon_round)
        payload = {
            "round": beacon.round,
            "signature": beacon.signature,
            "randomness": beacon.randomness,
        }
        (template / "runtime.json").write_text(
            json.dumps(
                {
                    "fixture_clock_offset": offset,
                    "beacon": payload,
                }
            )
        )
        yield directory, digest, secrets, template, offset, payload


def settings(tmp_path):
    return driver.Settings(
        master_https="https://master.test",
        relay_https="https://relay.test",
        ca=tmp_path / "ca.pem",
        snapshot=tmp_path / "snapshot",
        snapshot_digest="a" * 64,
        secrets=tmp_path / "keys",
        service_state=tmp_path / "service",
        evidence=tmp_path / "proof",
        verified_beacon=tmp_path / "beacon.json",
        fixture_clock_directory=tmp_path / "clock",
        fixture_clock_offset=0,
    )


def test_decoder_count_charges_full_live_geometry_not_extra_qualification():
    profile = json.loads((SCRIPTS.parent / "experiments/gpu_network_v2/profile.json").read_bytes())
    count = driver.decoder_execution_count(profile)
    assert count["shadow"] == 96 and count["live"] == 16
    assert count["qualification"] == 4 and count["faulttrace_remaining"] == 10
    assert count["required_total"] == 126 and count["required_per_host"] == 63
    assert count["excess"] == 0 and count["kernels_executed"] == 0


def test_decoder_count_cli_blocks_before_authority_or_provider(tmp_path, monkeypatch, capsys):
    profile = SCRIPTS.parent / "experiments/gpu_network_v2/profile.json"

    def forbidden(*args, **kwargs):
        raise AssertionError("workload launch before budget reconciliation")

    monkeypatch.setattr(driver, "proof", forbidden)
    monkeypatch.setattr(driver.subprocess, "Popen", forbidden)
    assert driver.main(["--decoder-count-profile", str(profile)]) == 0
    result = json.loads(capsys.readouterr().out)
    assert result["reason"] == "D2_EXECUTION_COUNT_ONLY" and result["backend_authority"] == "NONE"
    assert not list(tmp_path.iterdir())


@pytest.mark.parametrize(
    "section, field, value",
    [("inner", "H", 2), ("schedule", "shadow_participations_per_identity", 11)],
)
def test_decoder_count_rejects_reduced_work(section, field, value):
    profile = json.loads((SCRIPTS.parent / "experiments/gpu_network_v2/profile.json").read_bytes())
    profile[section][field] = value
    with pytest.raises(driver.DriverError, match="original execution contract"):
        driver.decoder_execution_count(profile)


def test_exact_decoder_four_identity_legacy_bootstrap_without_capacity_receipt(tmp_path):
    """Real signed registration/genesis/joins/locks, no screen/training/CUDA claim."""
    from fastapi.testclient import TestClient

    from hypertrain.challenge.app import Config, create_app
    from hypertrain.challenge.store import ChallengeError
    from hypertrain.data.store import LocalFSStore
    from hypertrain.ledger import Params
    from hypertrain.protocol import envelope_v2
    from hypertrain.protocol.jcs import canonicalize
    from hypertrain.protocol.keys import Keypair

    seed_spec = importlib.util.spec_from_file_location(
        "driver_decoder_seed", SCRIPTS / "network_gpu_seed.py"
    )
    assert seed_spec is not None and seed_spec.loader is not None
    seed = importlib.util.module_from_spec(seed_spec)
    sys.modules[seed_spec.name] = seed
    seed_spec.loader.exec_module(seed)
    public, private = tmp_path / "seed", tmp_path / "secrets"
    digest = seed.generate(
        public, private, "sha256:" + "ab" * 32, ["candidate-not-observed"], 1000, 2000
    )
    job = seed.validate(public, digest)
    index = json.loads((public / "seed.json").read_bytes())
    token = private / "admin.token"
    token.write_text("decoder-contract-test")
    token.chmod(0o600)
    # Existing loader accepts hex; raw generated seed begins with whitespace byte0x0b.
    coordinator = private / "service-coord.hex"
    coordinator.write_text((private / "coord.seed").read_bytes().hex())
    coordinator.chmod(0o600)
    config = Config(
        "hypertrain",
        tmp_path / "service",
        "https://master.test",
        None,
        token,
        None,
        coordinator,
        index["roles"]["owner"],
        Params("hypertrain", 1000, 4320, 1, job.manifest.training.verify.E_vest_rounds),
    )
    import httpx

    # Subnet gate (netuid 100): the fake master registers the four decoder hotkeys.
    registered = {Keypair((private / f"hot-{i}.seed").read_bytes()).ss58: i for i in range(4)}
    app = create_app(
        config,
        objects=LocalFSStore(public / "objects"),
        verify_beacon=driver.authority.fixture_beacon,
        transport=httpx.MockTransport(
            lambda _: httpx.Response(200, json={"netuid": 100, "hotkeys": registered})
        ),
    )
    store = app.state.store
    store.clock = lambda: 1000.0
    try:
        with TestClient(app) as client:
            headers = {"Authorization": "Bearer decoder-contract-test"}
            from hypertrain.beacon.core import FixtureBeacon

            b = FixtureBeacon(current=1).get(1)
            response = client.post(
                "/v1/admin/beacon",
                headers=headers,
                json={"round": 1, "signature": b.signature, "randomness": b.randomness},
            )
            assert response.status_code == 200
            response = client.post(
                "/v2/admin/runs",
                headers=headers,
                content=(public / "manifest-envelope.json").read_bytes(),
            )
            assert response.status_code == 201 and response.json()["run_id"] == job.run_id
            root = f"/v2/runs/{job.run_id}"
            response = client.post(
                root + "/admin/genesis",
                headers=headers,
                content=(public / "genesis-envelope.json").read_bytes(),
            )
            assert response.status_code == 200
            genesis = driver.authority.TestGenesis.model_validate(
                envelope_v2.parse_envelope((public / "genesis-envelope.json").read_bytes()).body
            )
            roster = []
            for i in range(4):
                hot = Keypair((private / f"hot-{i}.seed").read_bytes())
                cold = Keypair((private / f"cold-{i}.seed").read_bytes())
                admission_id = driver.decoder_join_lock(
                    client,
                    job.manifest,
                    hot,
                    cold,
                    [o.origin_id for o in genesis.origins if o.owner == cold.ss58],
                    1000,
                    f"{i + 1:064x}",
                    f"{i + 10:064x}",
                    100,
                )
                status = client.get(root + "/admission/" + hot.ss58)
                assert status.status_code == 200
                assert status.json()["record"]["clean_count"] == 0
                assert not status.json()["eligible"]
                assert status.json()["funding"]["locked_units"] == 1000
                roster.append({"hotkey": hot.ss58, "admission_id": admission_id})
            assert store.require_roster_v2(job.run_id, roster=roster) is None
            owner = Keypair((private / "owner.seed").read_bytes())
            body = canonicalize(
                {
                    "version": 1,
                    "profile_id": "tiny-cpu-service-32-v1",
                    "run_id": job.run_id,
                    "backend_binding_hash": "0" * 64,
                    "implementation_hash": "0" * 64,
                    "max_complete_roster": 32,
                    "memory_reservation_bytes": 1 << 30,
                    "max_outer_work_units": 50_000_000,
                    "max_audit_step_units": 512,
                    "max_repair_step_units": 1024,
                    "max_object_bytes": 65536,
                    "max_tape_bytes": 1 << 20,
                }
            )
            receipt = canonicalize(
                envelope_v2.seal(
                    owner,
                    "Receipt",
                    job.run_id,
                    {"w": 0, "commit_hash": driver.sha256_hex(body), "received_round": 1},
                    10000,
                )
            )
            with pytest.raises(ChallengeError) as error:
                store.bootstrap_service_capacity_v2(job.run_id, body, receipt)
            assert error.value.status == 409 and error.value.detail == "SERVICE_PROFILE_GEOMETRY"
            assert store._backend_v2(job.manifest) == "cpu"
            assert store._db.execute("SELECT COUNT(*) FROM admissions_v2").fetchone()[0] == 4
            assert store._db.execute("SELECT COUNT(*) FROM admission_trials").fetchone()[0] == 0
            escrow, _, _ = store._services(job.run_id)
            escrow.verify()
            assert not store._db.execute(
                "SELECT 1 FROM records_v2 WHERE kind='service-admission'"
            ).fetchone()
    finally:
        store.close_v2_notifications()
        store._db.close()


def test_decoder_operation_cancel_rejects_before_http(tmp_path):
    from types import SimpleNamespace

    seed_spec = importlib.util.spec_from_file_location(
        "driver_cancel_seed", SCRIPTS / "network_gpu_seed.py"
    )
    assert seed_spec is not None and seed_spec.loader is not None
    seed = importlib.util.module_from_spec(seed_spec)
    seed_spec.loader.exec_module(seed)
    from hypertrain.protocol.keys import Keypair

    public, private = tmp_path / "seed", tmp_path / "roles"
    digest = seed.generate(public, private, "sha256:" + "ab" * 32, ["candidate"], 1000, 2000)
    job = seed.validate(public, digest)
    index = json.loads((public / "seed.json").read_bytes())
    hot = Keypair((private / "hot-0.seed").read_bytes())
    operation = {
        "operation": "probe",
        "binding": "11" * 32,
        "hotkey": hot.ss58,
        "owner": index["roles"]["owner"],
        "role": "h0",
        "instance_id": 1,
        "machine_id": 2,
        "job": job.model_dump(mode="json"),
        "binding_kind": "trial",
        "sources": {},
        "image_digest": job.manifest.training.reference_spec.image_digest,
        "cutoff": job.deadline,
    }
    cancel = threading.Event()
    cancel.set()
    with pytest.raises(driver.DriverError, match="identity/kind/cancellation"):
        driver.decoder_operation(
            SimpleNamespace(), SimpleNamespace(), operation, {}, hot, {}, tmp_path, cancel
        )


@pytest.mark.parametrize("fault", ["trial", "signer", "identity"])
def test_decoder_runtime_consumer_rejects_wrong_trial_without_dispatch(tmp_path, fault):
    from types import SimpleNamespace

    import httpx

    from hypertrain.protocol import envelope_v2
    from hypertrain.protocol.keys import Keypair
    from hypertrain.protocol.messages_v2 import ArtifactLimits, JoinChallenge

    seed_spec = importlib.util.spec_from_file_location(
        "driver_bound_seed", SCRIPTS / "network_gpu_seed.py"
    )
    assert seed_spec is not None and seed_spec.loader is not None
    seed = importlib.util.module_from_spec(seed_spec)
    seed_spec.loader.exec_module(seed)
    public, private = tmp_path / "seed", tmp_path / "roles"
    digest = seed.generate(public, private, "sha256:" + "ab" * 32, ["candidate"], 1000, 2000)
    job = seed.validate(public, digest)
    index = json.loads((public / "seed.json").read_bytes())
    hot = Keypair((private / "hot-0.seed").read_bytes())
    coord = Keypair((private / "coord.seed").read_bytes())
    challenge = JoinChallenge(
        admission_id="22" * 32,
        nonce="33" * 32,
        seed_beacon=1,
        deadline_beacon=101,
        manifest_hash=job.run_id,
        theta_hash="44" * 32,
        assignment_hash="55" * 32,
        layout=job.manifest.training.reference_spec.layout,
        artifact_limits=ArtifactLimits(
            max_object_bytes=65536, max_body_bytes=65536, max_chunk_manifest_bytes=1048576
        ),
        policy_hash=job.manifest.network.admission_policy_hash,
    )
    operation = {
        "operation": "probe",
        "binding": "11" * 32,
        "hotkey": hot.ss58,
        "owner": index["roles"]["owner"],
        "role": "h0",
        "instance_id": 1,
        "machine_id": 2,
        "job": job.model_dump(mode="json"),
        "binding_kind": "trial",
        "sources": {},
        "image_digest": job.manifest.training.reference_spec.image_digest,
        "cutoff": job.deadline,
    }
    if fault == "signer":
        challenge = challenge.model_copy(update={"admission_id": "11" * 32})
        coord = hot
    if fault == "identity":
        operation["hotkey"] = coord.ss58
    calls = []

    def respond(request):
        calls.append(request.url.path)
        return httpx.Response(
            200, json=envelope_v2.seal(coord, "JoinChallenge", job.run_id, challenge, 101)
        )

    with httpx.Client(
        base_url="https://master.test", transport=httpx.MockTransport(respond)
    ) as client:
        with pytest.raises(driver.DriverError):
            driver.decoder_operation(
                client,
                SimpleNamespace(),
                operation,
                {},
                hot,
                {"owner": index["roles"]["owner"], "run_id": job.run_id},
                tmp_path,
                threading.Event(),
            )
    assert calls == (
        [] if fault == "identity" else [f"/v2/runs/{job.run_id}/join/" + "11" * 32 + "/challenge"]
    )


def test_decoder_probe_consumer_real_routes_receipt_and_debit_metadata(tmp_path, monkeypatch):
    """One real reference/proof transition; metadata oracle, not independent gradients."""
    from types import SimpleNamespace

    import hypertrain.challenge.admission as admission_module
    import network_gpu_operation as operation_module
    from hypertrain.data.trial_assignment import trial_samples
    from hypertrain.gpu_ops.journal import Journal, sha256
    from hypertrain.protocol import envelope_v2
    from hypertrain.protocol.jcs import canonicalize

    runtime_spec = importlib.util.spec_from_file_location(
        "driver_real_runtime", SCRIPTS.parent / "experiments/gpu_network_v2/orchestrate.py"
    )
    assert runtime_spec is not None and runtime_spec.loader is not None
    runtime_module = importlib.util.module_from_spec(runtime_spec)
    runtime_spec.loader.exec_module(runtime_module)
    network_module = authority_fixture.service
    monkeypatch.setattr(network_module.time, "time", lambda: 1700000000.0)
    original_setup = network_module.fixture.Setup

    def carry(manifest, policy, admission_policy, rows, tree):
        body = manifest.body()
        body["training"]["inner"].update(state_policy="carry", rewarmup_steps=0)
        return original_setup(
            driver.RunManifestV2.model_validate(body), policy, admission_policy, rows, tree
        )

    monkeypatch.setattr(network_module.fixture, "Setup", carry)
    (tmp_path / "network").mkdir()
    generator = network_module.network.__wrapped__(
        tmp_path / "network", SimpleNamespace(param="mlm")
    )
    n = next(generator)
    try:
        hot, cold = network_module.HOT[0], network_module.COLD[0]
        origins = [n.origin_ids[0]]
        admission_id = driver.decoder_join_lock(
            n.client, n.manifest, hot, cold, origins, 1000, "61" * 32, "62" * 32, 10000
        )
        n.push(2)
        signed = n.client.get(n.url + "/join/" + admission_id + "/challenge").json()
        challenge = driver.JoinChallenge.model_validate(envelope_v2.parse_envelope(signed).body)
        with n.store._lock:
            epoch = n.store._db.execute(
                "SELECT trial_epoch FROM admissions_v2 WHERE admission_id=?", (admission_id,)
            ).fetchone()[0]
            job, inputs = n.store.stage_trial_v2(
                n.manifest.run_id(),
                challenge,
                trial_samples(
                    n.manifest,
                    admission_id,
                    challenge.nonce,
                    n.store._beacon_v2(challenge.seed_beacon),
                ),
                epoch,
            )
        profile = json.loads(
            (SCRIPTS.parent / "experiments/gpu_network_v2/profile.json").read_bytes()
        )
        profile["layout"] = job.manifest.training.reference_spec.layout.model_dump()
        profile["runtime"].update(
            qualified=True, image_digest=job.manifest.training.reference_spec.image_digest
        )
        journal_dir = tmp_path / "journal"
        journal_dir.mkdir()
        journal = Journal(journal_dir)
        accepted_manifest = {
            "role": "h0",
            "instance_id": 1,
            "machine_id": 2,
            "image_digest": job.manifest.training.reference_spec.image_digest,
        }
        lifecycle = SimpleNamespace(
            j=journal,
            dir=journal_dir,
            roles=["h0", "h1"],
            cleanup_started=lambda: False,
            deadline=lambda: job.deadline + 100,
            frozen=lambda role: accepted_manifest,
            cfg={"cleanup_deadline_unix": job.deadline + 1000},
            receipt=lambda role: {"instance_id": 1},
            host={"h0": {"machine_id": 2}},
        )
        runtime = runtime_module.NetworkRuntime(lifecycle, profile)
        runtime.reserve("h0", "metadata-qual-h0", "qualification", 2)
        runtime.reserve("h1", "metadata-qual-h1", "qualification", 2)
        journal.append("network_workload_promoted", metadata_only=True)
        journal.append(
            "network_admission_authority", owner=network_module.OWNER.ss58, metadata_only=True
        )
        journal.append("supervisor_ready", metadata_only=True)
        journal.append("ssh_trusted", role="h0", metadata_only=True)
        op = operation_module.Operation(
            operation="probe",
            binding=admission_id,
            hotkey=hot.ss58,
            owner=network_module.OWNER.ss58,
            role="h0",
            instance_id=1,
            machine_id=2,
            job=job,
            binding_kind="trial",
            sources={},
            image_digest=job.manifest.training.reference_spec.image_digest,
            cutoff=job.deadline,
            trace=False,
        )
        context = {
            "role": op.role,
            "binding": op.binding,
            "operation": op.operation,
            "hotkey": op.hotkey,
            "spec_sha256": sha256(op.model_dump_json().encode()),
        }
        receipt = envelope_v2.seal(
            network_module.OWNER,
            "Receipt",
            job.run_id,
            {
                "w": job.w,
                "commit_hash": sha256(canonicalize(context)),
                "received_round": n.now,
            },
            10000,
        )
        runtime.accept_operation(
            op, canonicalize(receipt), owner=network_module.OWNER.ss58, beacon=n.now
        )
        from hypertrain.gpu_ops.launcher import Reject

        with pytest.raises(Reject, match="source_or_run_changed"):
            runtime.operation(op, tmp_path / "refused", cancel=threading.Event())
        assert not journal.all("network_execution_intent", phase="workload")
        cancelled = threading.Event()
        cancelled.set()
        with pytest.raises(Reject, match="context_not_qualified"):
            runtime.operation(op, tmp_path / "refused", cancel=cancelled)
        screen_original = authority_fixture._screen

        def reference(job, directory, challenge, **kwargs):
            runtime.reserve("h0", "reference-" + job.digest(), "workload", 1)
            return screen_original(job, directory, challenge, **kwargs)

        monkeypatch.setattr(admission_module, "screen_work", reference)

        def dispatch(operation, destination, *, cancel):
            runtime.reserve(op.role, "probe-" + job.digest(), "workload", 1)
            import shutil

            miner_inputs = tmp_path / "miner-inputs"
            shutil.copytree(inputs, miner_inputs, ignore=shutil.ignore_patterns("published"))
            screen, proof = screen_original(
                job, miner_inputs, challenge, now_beacon=n.now, backend="cpu"
            )
            return {
                "status": "CAPTURED_NOT_ACCEPTED",
                "cli_outcome": {
                    "proof": envelope_v2.seal(
                        hot, "WorkProof", job.run_id, proof, challenge.deadline_beacon
                    ),
                    "screen": envelope_v2.seal(
                        hot, "WorkScreenV2", job.run_id, screen, challenge.deadline_beacon
                    ),
                },
            }

        # Exact constructor-only factory contract; launch result remains independently validated.
        factory = driver.decoder_service_factory(
            runtime,
            lambda operation, j, d, c: (
                op.model_copy(update={"operation": operation}).model_dump(mode="json"),
                receipt,
                {"owner": network_module.OWNER.ss58, "beacon": n.now},
            ),
        )
        from hypertrain.gpu_ops.launcher import Reject

        with pytest.raises(Reject, match="receipt_changed"):
            factory("reference", job, inputs, {"challenge": signed})
        assembled = op.model_copy(update={"operation": "reference"})
        context_body = {
            "role": assembled.role,
            "binding": assembled.binding,
            "operation": assembled.operation,
            "hotkey": assembled.hotkey,
            "spec_sha256": sha256(assembled.model_dump_json().encode()),
        }
        original_receipt = envelope_v2.seal(
            network_module.OWNER,
            "Receipt",
            job.run_id,
            {
                "w": job.w,
                "commit_hash": sha256(canonicalize(context_body)),
                "received_round": n.now,
            },
            10000,
        )
        plan = driver.decoder_context_plan(
            runtime,
            network_module.OWNER.ss58,
            job.run_id,
            {},
            {hot.ss58: {"role": "h0"}},
            {"reference:" + job.digest(): original_receipt},
            n.now,
        )
        context = {
            "run_id": job.run_id,
            "hotkey": hot.ss58,
            "admission_id": admission_id,
            "epoch": job.w,
            "challenge": challenge.body(),
            "challenge_hash": challenge.digest(),
            "challenge_envelope": signed,
        }
        raw, kept, pins = plan("reference", job, inputs, context)
        assert raw == assembled.model_dump(mode="json") and kept == original_receipt
        driver.decoder_service_factory(runtime, plan)("reference", job, inputs, context)
        with pytest.raises(driver.DriverError, match="run differs"):
            plan("reference", job, inputs, {**context, "run_id": "00" * 32})
        monkeypatch.setattr(runtime, "operation", dispatch)
        result = driver.decoder_operation(
            n.client,
            runtime,
            op.model_dump(mode="json"),
            receipt,
            hot,
            {
                "beacon": n.now,
                "owner": network_module.OWNER.ss58,
                "run_id": job.run_id,
                "admin_headers": network_module.admin(),
            },
            tmp_path / "artifacts",
            threading.Event(),
        )
        assert result["state"] == "PROBATION"
        assert n.store._db.execute(
            "SELECT proof_digest FROM admissions_v2 WHERE admission_id=?", (admission_id,)
        ).fetchone()[0]
        assert (
            sum(r["executions"] for r in journal.all("network_execution_intent", phase="workload"))
            == 2
        )
        assert not n.store._db.execute(
            "SELECT 1 FROM admission_trial_results WHERE outcome='MATCH'"
        ).fetchone()
        escrow, _, _ = n.store._services(job.run_id)
        escrow.verify()
    finally:
        generator.close()
        n.store._db.close()


@pytest.mark.parametrize(
    "fault",
    [
        None,
        "missing-status",
        "forged-status",
        "wrong-round",
        "missing-acceptance",
        "wrong-signer",
        "wrong-challenge",
        "wrong-commit",
        "wrong-run",
    ],
)
def test_decoder_live_actual_cli_wrapper_shape_and_consumer(tmp_path, monkeypatch, fault, cli_ca):
    """Actual cli.main plus remote wrapper; training/network producer replaced by metadata."""
    from types import SimpleNamespace

    import httpx

    import network_gpu_operation as operation
    from hypertrain.miner import cli
    from hypertrain.protocol import envelope_v2
    from hypertrain.protocol.keys import Keypair

    seed_spec = importlib.util.spec_from_file_location(
        "driver_live_seed", SCRIPTS / "network_gpu_seed.py"
    )
    assert seed_spec is not None and seed_spec.loader is not None
    seed = importlib.util.module_from_spec(seed_spec)
    seed_spec.loader.exec_module(seed)
    public, private = tmp_path / "seed", tmp_path / "roles"
    digest = seed.generate(public, private, "sha256:" + "ab" * 32, ["candidate"], 1000, 2000)
    job = seed.validate(public, digest)
    index = json.loads((public / "seed.json").read_bytes())
    hot = Keypair((private / "hot-0.seed").read_bytes())
    directory = tmp_path / "operation"
    directory.mkdir()
    for relative in job.object_paths.values():
        shutil.copyfile(public / relative, directory / relative)
    spec = operation.Operation(
        operation="live",
        binding="33" * 32,
        hotkey=hot.ss58,
        owner=index["roles"]["owner"],
        role="h0",
        instance_id=1,
        machine_id=2,
        job=job,
        binding_kind="commit-lease",
        sources={},
        image_digest=job.manifest.training.reference_spec.image_digest,
        cutoff=job.deadline,
        miner_config="miner.toml",
        ca="ca.pem",
        master="https://master.test",
    )
    common = {"w": job.w, "hotkey": hot.ss58}
    bodies = {
        "AcceptV2": {
            **common,
            "assignment_hash": "11" * 32,
            "image_digest": spec.image_digest,
            "driver_version": "candidate",
            "n_gpus": 2,
            "work_screen_hash": "12" * 32,
        },
        "CommitV2": {
            **common,
            "leaf_scheme": "ht-leaf-v1",
            "n_leaves": 7,
            **{
                k: "13" * 32
                for k in (
                    "leaves_root",
                    "metrics_root",
                    "final_theta_hash",
                    "ef_in_hash",
                    "ef_out_hash",
                    "delta_hash",
                )
            },
            "delta_bytes": 1,
            "tokens": job.manifest.training.batch_samples() * job.manifest.training.model.seq_len,
        },
        "DeltaManifestV2": {
            **common,
            "delta_hash": "13" * 32,
            "uri": "sha256:" + "13" * 32,
            "size": 1,
            "format": "ht-sparse-v1",
            "chunks": [{"off": 0, "len": 1, "sha256": "14" * 32}],
            "grant_hash": "15" * 32,
            "master_acceptance_hash": "16" * 32,
        },
    }
    bodies["WorkProof"] = {
        "admission_id": "17" * 32,
        "challenge_hash": driver.sha256_hex(driver.canonicalize(bodies["CommitV2"])),
        "leaves_root": bodies["CommitV2"]["leaves_root"],
        "delta_hash": bodies["CommitV2"]["delta_hash"],
        "artifact_refs": [{"sha256": "18" * 32, "size": 1}],
    }
    kinds = ["AcceptV2", "CommitV2", "WorkProof", "DeltaManifestV2"]
    signed = [envelope_v2.seal(hot, kind, job.run_id, bodies[kind], 10000) for kind in kinds]
    assert "w" not in signed[2]["body"] and "hotkey" not in signed[2]["body"]
    monkeypatch.setattr(operation, "check", lambda *args: None)
    monkeypatch.setattr(operation, "confined", lambda tree, relative: tree / relative)
    monkeypatch.setattr(operation.time, "time", lambda: 1001)
    monkeypatch.setattr(operation.signal, "signal", lambda *args: None)
    cfg = SimpleNamespace(
        workdir=directory / "miner",
        keyfile=private / "hot-0.seed",
        run_id=job.run_id,
        owner_hotkey=spec.owner,
        api=spec.master,
        device="cuda",
        image_digest=spec.image_digest,
    )
    monkeypatch.setattr(cli.MinerConfig, "load", lambda *args, **kwargs: cfg)
    import hypertrain.miner.core as core

    transport_original = httpx.Client
    responses = []

    def respond(request):
        responses.append(request.url.path)
        return httpx.Response(200, json={"w": job.w, "hotkey": hot.ss58, "accepted": True})

    class MetadataTransport(transport_original):
        def __init__(self, *args, **kwargs):
            super().__init__(
                *args,
                base_url="https://master.test",
                transport=httpx.MockTransport(respond),
                **kwargs,
            )

    monkeypatch.setattr(httpx, "Client", MetadataTransport)

    class MetadataMiner:
        def __init__(self, config, client):
            self.manifest = job.manifest
            self.kp = hot
            self.api = SimpleNamespace(c=client)

        def __enter__(self):
            return self

        def __exit__(self, *args):
            self.api.c.close()

        def run_round(self, w):
            assert w == job.w
            for value in signed:
                self.api.c.post(
                    "/v2/runs/" + job.run_id + "/train", content=driver.canonicalize(value)
                ).raise_for_status()
            return "UPLOADED"

    monkeypatch.setattr(core, "NetworkMiner", MetadataMiner)
    monkeypatch.setattr(cli, "NetworkMiner", MetadataMiner)
    cert, key = cli_ca
    shutil.copyfile(cert, tmp_path / spec.ca)
    original_env = os.environ
    before_env = dict(original_env)
    if fault is None:
        # A real cli.main exception after CA assignment must also restore its parent.
        exception_directory = tmp_path / "exception-operation"
        exception_directory.mkdir()
        with pytest.raises(ssl.SSLCertVerificationError):
            with monkeypatch.context() as patch:
                for name in ("SSL_CERT_FILE", "REQUESTS_CA_BUNDLE", "CURL_CA_BUNDLE"):
                    patch.setenv(name, before_env.get(name, certifi.where()))
                    if name not in before_env:
                        patch.delenv(name)
                for name, value in before_env.items():
                    if name.startswith("HYPERTRAIN_MINER_"):
                        patch.setenv(name, value)
                patch.setattr(cfg, "workdir", exception_directory / "miner")

                def reject_wrong_ca(*args, **kwargs):
                    verify_cli_ca(ssl.create_default_context(cafile=certifi.where()), cert, key)

                patch.setattr(MetadataMiner, "run_round", reject_wrong_ca)
                operation._capture_miner_cli(spec, tmp_path, exception_directory, threading.Event())
        assert os.environ is original_env and dict(os.environ) == before_env
    # Capture real cli.main outcome using wrapper's capture path; explicit metadata HTTP only.
    with monkeypatch.context() as patch:
        for name in ("SSL_CERT_FILE", "REQUESTS_CA_BUNDLE", "CURL_CA_BUNDLE"):
            patch.setenv(name, before_env.get(name, certifi.where()))
            if name not in before_env:
                patch.delenv(name)
        for name, value in before_env.items():
            if name.startswith("HYPERTRAIN_MINER_"):
                patch.setenv(name, value)
        captured = operation._capture_miner_cli(spec, tmp_path, directory, threading.Event())
        assert os.environ["SSL_CERT_FILE"] == str(tmp_path / spec.ca)
        verify_cli_ca(ssl.create_default_context(), cert, key)
    assert os.environ is original_env and dict(os.environ) == before_env
    # Same HTTPX construction later used by clock intake, immediately after the CLI.
    with transport_original():
        pass
    phase, published, challenge, commit, proof = captured
    assert challenge is None and commit is None and proof is None
    assert not {"status", "publication", "publication_custody_sha256", "environment"} & phase.keys()
    assert not published.exists()
    for name in ("operation-result.json", "publication-custody.json", "execution-custody.json"):
        assert not (directory / name).exists()
    assert phase["signed_api"] == json.loads((directory / "signed-api.json").read_bytes())
    assert phase["accepted_api"] == json.loads((directory / "accepted-api.json").read_bytes())
    # Explicit metadata-only Runtime result double, never completed work/Store acceptance.
    result = {
        **phase,
        "status": "CAPTURED_NOT_ACCEPTED",
        "operation": spec.operation,
        "binding": spec.binding,
        "hotkey": spec.hotkey,
        "role": spec.role,
        "job_sha256": job.digest(),
    }
    if fault is None:
        # Reuse actual capture; real production completion still rejects missing publication.
        with monkeypatch.context() as patch:
            patch.setattr(operation, "_capture_miner_cli", lambda *args: captured)
            with pytest.raises(FileNotFoundError):
                operation.execute(spec, tmp_path, directory)
        assert not (directory / "publication-custody.json").exists()
        assert not (directory / "operation-result.json").exists()
        import torch

        with monkeypatch.context() as patch:
            patch.setattr(torch.cuda, "is_available", lambda: False)
            with pytest.raises(ValueError, match="custody CUDA environment unavailable"):
                operation.execution_environment(spec)
    assert result["cli_outcome"] == {"status": "UPLOADED"}
    assert [x["type"] for x in result["signed_api"]] == kinds
    assert [x["request"] for x in result["accepted_api"]] == result["signed_api"]
    if fault == "missing-status":
        result["cli_outcome"] = {}
    if fault == "forged-status":
        result["cli_outcome"] = {"status": "PASS"}
    if fault == "wrong-round":
        result["signed_api"][0] = envelope_v2.seal(
            hot, "AcceptV2", job.run_id, {**bodies["AcceptV2"], "w": job.w + 1}, 10000
        )
    if fault == "missing-acceptance":
        result["accepted_api"] = []
    if fault in ("wrong-signer", "wrong-challenge", "wrong-commit", "wrong-run"):
        signer = Keypair(bytes([99]) * 32) if fault == "wrong-signer" else hot
        body = (
            {**bodies["WorkProof"], "challenge_hash": "19" * 32}
            if fault == "wrong-challenge"
            else bodies["WorkProof"]
        )
        run = "20" * 32 if fault == "wrong-run" else job.run_id
        position = 1 if fault == "wrong-commit" else 2
        if fault == "wrong-commit":
            body = {**bodies["CommitV2"], "delta_hash": "21" * 32}
        forged = envelope_v2.seal(signer, kinds[position], run, body, 10000)
        result["signed_api"][position] = forged
        result["accepted_api"][position]["request"] = forged
    runtime = SimpleNamespace(
        accept_operation=lambda *args, **kwargs: None, operation=lambda *args, **kwargs: result
    )

    def assignment(request):
        return httpx.Response(200, json={"assignment": [{"hotkey": hot.ss58}]})

    with transport_original(
        base_url="https://master.test", transport=httpx.MockTransport(assignment)
    ) as client:
        args = (
            client,
            runtime,
            spec.model_dump(mode="json"),
            {},
            hot,
            {"owner": spec.owner, "run_id": job.run_id, "beacon": 1},
            directory,
            threading.Event(),
        )
        if fault is None:
            assert driver.decoder_operation(*args) is result
        else:
            with pytest.raises(driver.DriverError):
                driver.decoder_operation(*args)
    assert len(responses) == 4


def test_decoder_live_graph_full_metadata_sequence(tmp_path, monkeypatch):
    """Full driver call ordering; HTTP/tape oracles explicit, no production ACTIVE claim."""
    from types import SimpleNamespace

    import httpx

    from hypertrain.protocol import envelope_v2
    from hypertrain.protocol.keys import Keypair

    seed_spec = importlib.util.spec_from_file_location(
        "driver_graph_seed", SCRIPTS / "network_gpu_seed.py"
    )
    assert seed_spec is not None and seed_spec.loader is not None
    seed = importlib.util.module_from_spec(seed_spec)
    seed_spec.loader.exec_module(seed)
    public, private = tmp_path / "seed", tmp_path / "roles"
    digest = seed.generate(public, private, "sha256:" + "ab" * 32, ["candidate"], 1000, 2000)
    job = seed.validate(public, digest)
    coordinator = Keypair((private / "coord.seed").read_bytes())
    original_seal = envelope_v2.seal
    hot = [Keypair((private / f"hot-{i}.seed").read_bytes()) for i in range(4)]
    cold = [Keypair((private / f"cold-{i}.seed").read_bytes()) for i in range(4)]
    auditors = [Keypair((private / f"auditor-{i}.seed").read_bytes()) for i in range(2)] * 2
    roster = [
        {
            "hotkey": k.ss58,
            "slot": i,
            "q_i": "3f800000",
            "admission_id": f"{i + 1:064x}",
            "coldkey_group": cold[i].ss58,
            "state": "ACTIVE",
            "eligible_weight": 4194304,
        }
        for i, k in enumerate(hot)
    ]
    opening = driver.RoundOpenV2(
        w=0,
        prev_final_hash="0" * 64,
        theta_hash="11" * 32,
        outer_state_hash="0" * 64,
        center_hash="0" * 64,
        roster_hash=driver.sha256_hex(driver.canonicalize(roster)),
        honeypot_commit="0" * 64,
        d_open=2,
        d_assign=3,
        d_commit=4,
        d_audit=5,
        d_upload=6,
        d_final=10,
        contract_version=2,
        policy_hashes=driver.PolicyHashes.model_validate(
            {k: getattr(job.manifest.network, k) for k in driver.PolicyHashes.model_fields}
        ),
        registry_epoch=0,
        start_state_index_hash="12" * 32,
        audit_mode="anchored-full",
        roster=roster,
    )
    actions = []
    sequence = iter(range(4))

    def respond(request):
        path = request.url.path
        actions.append(path)
        if path.endswith("/worker/lease"):
            i = next(sequence)
            start = driver.StartStateV2(
                run_id=job.run_id,
                w=0,
                hotkey=hot[i].ss58,
                **{
                    k: "31" * 32
                    for k in (
                        "theta_hash",
                        "state_object_sha256",
                        "opt_state_hash",
                        "ef_object_sha256",
                        "ef_hash",
                        "parent_anchor_hash",
                        "anchor_verdict_hash",
                    )
                },
                global_step0=0,
            )
            lease = {
                "run_id": job.run_id,
                "job_id": f"{i + 1:064x}",
                "auditor_id": auditors[i].ss58,
                "attempt": 1,
                "lease_nonce": f"{i + 10:064x}",
                "lease_expires": 6,
                "absolute_deadline": 10,
                "reservation_id": "32" * 32,
                "replay_step_budget": 30,
                "anchor_age": 0,
                "manifest": job.manifest.body(),
                "challenge_envelope": original_seal(
                    coordinator,
                    "AuditChallengeV2",
                    job.run_id,
                    {
                        "w": 0,
                        "target": hot[i].ss58,
                        "beacon_round": 5,
                        "beacon_sig_sha256": "38" * 32,
                        "mode": "full",
                        "segments": [],
                        "reasons": ["random"],
                        "serve_deadline": 10,
                        "anchor_hash": start.digest(),
                        "audit_mode": "anchored-full",
                    },
                    10,
                ),
                "commit_envelope": original_seal(
                    hot[i],
                    "CommitV2",
                    job.run_id,
                    {
                        "w": 0,
                        "hotkey": hot[i].ss58,
                        "leaf_scheme": "ht-leaf-v1",
                        "n_leaves": 7,
                        **{
                            k: "39" * 32
                            for k in (
                                "leaves_root",
                                "metrics_root",
                                "final_theta_hash",
                                "ef_in_hash",
                                "ef_out_hash",
                                "delta_hash",
                            )
                        },
                        "delta_bytes": 1,
                        "tokens": job.manifest.training.batch_samples() * 16,
                    },
                    10,
                ),
                "sample_ids": job.sample_ids,
                "start_state": start.body(),
                "preimages": [
                    {
                        "run_id": job.run_id,
                        "w": 0,
                        "t": 0,
                        "stages": [{"theta": "41" * 32, "m": "42" * 32, "v": "43" * 32}],
                        "batch_ids_sha256": "44" * 32,
                        "rng_ctr": 0,
                        "loss_f32": "00000000",
                        "norm_f32": "00000000",
                    }
                ],
                "ef_in": {"sha256": "33" * 32, "size": 1},
                "v0": {"sha256": "34" * 32, "size": 1},
                "created_beacon": 5,
            }
            return httpx.Response(
                200, json=original_seal(coordinator, "AuditJobV2", job.run_id, lease, 10)
            )
        if path.endswith("/execute"):
            return httpx.Response(
                200,
                json={
                    "verdict": {
                        "result": "MATCH",
                        "first_bad_leaf": None,
                        "challenge_hash": "35" * 32,
                        "recomputed_leaves_root": "36" * 32,
                        "replay_env": {
                            "image_digest": job.manifest.training.reference_spec.image_digest,
                            "driver": "metadata",
                            "gpu_uuid_sha256": "37" * 32,
                            "sm_count": 1,
                        },
                    }
                },
            )
        if path.endswith("/aggregate"):
            return httpx.Response(
                200,
                json={
                    "tape_hash": "21" * 32,
                    "prev_state": "22" * 32,
                    "out_state": "23" * 32,
                    "theta_hash": "24" * 32,
                },
            )
        return httpx.Response(200, json={})

    # Bounded metadata downstream arithmetic/objects; no recomputation claim from these sentinels.
    entries = [SimpleNamespace(hotkey=k.ss58, probation=False, weight_units=4194304) for k in hot]
    monkeypatch.setattr(
        driver.TapeV2,
        "from_bytes",
        lambda raw: SimpleNamespace(
            body=SimpleNamespace(allocation=SimpleNamespace(entries=entries))
        ),
    )
    policy = driver.AggregationPolicyV2.model_validate_json(
        driver.LocalFSStore(public / "objects").get(job.manifest.network.aggregation_policy_hash)
    )
    monkeypatch.setattr(driver.AggregationPolicyV2, "model_validate_json", lambda raw: policy)
    monkeypatch.setattr(driver, "network_inputs", lambda values: values)
    monkeypatch.setattr(
        driver, "replay_tape", lambda *args, **kwargs: SimpleNamespace(to_bytes=lambda: b"outer")
    )
    writes = []
    monkeypatch.setattr(driver, "write_network_checkpoint", lambda *args: writes.append(args))
    monkeypatch.setattr(driver, "verify_network_checkpoint", lambda *args: [])
    monkeypatch.setattr(driver, "decoder_operation", lambda *args: actions.append("miner-API"))
    context = {
        "round_open": original_seal(coordinator, "RoundOpenV2", job.run_id, opening, 10),
        "miners": [
            {"operation": {}, "receipt": {}, "key": k, "pins": {}, "artifacts": tmp_path}
            for k in hot
        ],
        "auditors": auditors,
        "accepted_inputs": lambda: [],
        "accepted_finality": lambda: {
            "disposition": "SETTLED",
            "unresolved_audits": 0,
            "unresolved_disputes": 0,
        },
    }
    with httpx.Client(
        base_url="https://master.test", transport=httpx.MockTransport(respond)
    ) as client:
        lineage = driver.decoder_live_graph(
            client,
            SimpleNamespace(),
            job.manifest,
            [context],
            coordinator,
            {"Authorization": "Bearer metadata"},
            lambda n: actions.append(f"beacon-{n}"),
            SimpleNamespace(get=lambda digest: b"outer"),
            tmp_path / "checkpoint",
            threading.Event(),
        )
    assert len(lineage) == 1 and len(writes) == 1
    assert actions.count("miner-API") == 4
    assert sum(p.endswith("/execute") for p in actions) == 4
    assert sum(p.endswith("/complete") for p in actions) == 4
    assert actions[-2].endswith("/aggregate") and actions[-1].endswith("/finalize")


def test_parser_help_exits_before_work(capsys):
    with pytest.raises(SystemExit) as error:
        driver.main(["--help"])
    assert error.value.code == 0


@pytest.mark.parametrize(
    "endpoint", ["http://master.test", "https://user:secret@master.test", "https://master.test?x=1"]
)
def test_endpoint_requires_explicit_https_without_ambient_credentials(tmp_path, endpoint):
    body = settings(tmp_path).model_dump()
    body["master_https"] = endpoint
    with pytest.raises(ValidationError):
        driver.Settings.model_validate(body)


def test_digest_authority_rejects_before_service_write(tmp_path, signed_package):
    directory, _, _, _, _, _ = signed_package
    cfg = settings(tmp_path).model_copy(update={"snapshot": directory})
    with pytest.raises(ValueError, match="admitted bundle hash"):
        driver.prepare(cfg)
    assert not cfg.evidence.exists()


def test_wrong_role_rejects_before_live_write(tmp_path, monkeypatch, signed_package):
    directory, digest, roles, template, _, _ = signed_package
    cfg = settings(tmp_path).model_copy(update={"snapshot": directory, "snapshot_digest": digest})
    shutil.copytree(template, cfg.service_state)
    shutil.copytree(roles, cfg.secrets)
    (cfg.secrets / "coord.seed").write_bytes(bytes(32))
    monkeypatch.setattr(driver, "durable_destination", lambda path: True)
    with pytest.raises(driver.DriverError, match="wrong role key"):
        driver.prepare(cfg)
    assert not cfg.evidence.exists()


def test_exact_product_cli_command():
    assert driver.miner_command(1) == [
        sys.executable,
        "-m",
        "hypertrain.miner.cli",
        "run-v2",
        "--round",
        "1",
    ]


def test_cancel_kills_real_ready_child_without_delay(tmp_path):
    cancel = threading.Event()
    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    listener.listen()
    listener.settimeout(10)
    errors = []
    code = (
        "import socket; s=socket.create_connection(('127.0.0.1',"
        + str(listener.getsockname()[1])
        + "));"
        "s.sendall(b'READY'); s.recv(1)"
    )

    def child():
        try:
            driver.run_child(
                [sys.executable, "-c", code], dict(os.environ), tmp_path / "child.log", cancel, 10
            )
        except driver.DriverError as error:
            errors.append(str(error))

    worker = threading.Thread(target=child)
    worker.start()
    connection, _ = listener.accept()
    connection.settimeout(10)
    assert connection.recv(5) == b"READY"
    cancel.set()
    assert connection.recv(1) == b""
    worker.join(timeout=10)
    connection.close()
    listener.close()
    assert not worker.is_alive() and len(errors) == 1


def test_precancel_never_launches_child(tmp_path, monkeypatch):
    cancel = threading.Event()
    cancel.set()

    def forbidden(*args, **kwargs):
        raise AssertionError("child launch")

    monkeypatch.setattr(driver.subprocess, "Popen", forbidden)
    with pytest.raises(driver.DriverError, match="cancelled before miner"):
        driver.run_child(
            [sys.executable, "-c", "pass"], dict(os.environ), tmp_path / "none.log", cancel, 10
        )
    assert not (tmp_path / "none.log").exists()


def test_missing_service_adapter_rejects_before_evidence(tmp_path, monkeypatch, signed_package):
    directory, digest, _, _, _, _ = signed_package
    monkeypatch.setattr(driver, "durable_destination", lambda path: True)
    cfg = settings(tmp_path).model_copy(update={"snapshot": directory, "snapshot_digest": digest})
    with pytest.raises(driver.DriverError, match="MISSING_ADAPTER"):
        driver.prepare(cfg)
    assert not cfg.evidence.exists()


def test_pytest_retention_destination_rejects(tmp_path):
    assert not driver.durable_destination(tmp_path / "output")


def test_failed_live_work_exports_journal_and_indexes_before_return(tmp_path, monkeypatch):
    cfg = settings(tmp_path)
    cfg.service_state.mkdir()
    with sqlite3.connect(cfg.service_state / "challenge.db") as db:
        db.execute("CREATE TABLE accepted(id TEXT)")
        db.execute("INSERT INTO accepted VALUES('original')")
    objects = cfg.service_state / "objects"
    objects.mkdir()
    (objects / "original").write_bytes(b"signed-evidence")
    config = tmp_path / "driver.json"
    config.write_text(cfg.model_dump_json())

    def failed_work(settings, cancel):
        settings.evidence.mkdir()
        raise driver.DriverError("actual service failed")

    monkeypatch.setattr(driver, "proof", failed_work)
    monkeypatch.setattr(driver.signal, "signal", lambda *args: None)
    with pytest.raises(driver.DriverError, match="actual service failed"):
        driver.main(["--config", str(config)])
    result = json.loads((cfg.evidence / "result.json").read_bytes())
    index = json.loads((cfg.evidence / "index.json").read_bytes())
    assert result["status"] == "FAIL"
    assert "service-export/challenge.db" in index["files"]
    assert result["index_sha256"] == driver.sha256_hex((cfg.evidence / "index.json").read_bytes())
    assert (cfg.evidence / "service-export/objects/original").read_bytes() == b"signed-evidence"
    for relative, entry in index["files"].items():
        data = (cfg.evidence / relative).read_bytes()
        assert driver.sha256_hex(data) == entry["sha256"] and len(data) == entry["size"]


@pytest.mark.parametrize("offset", [float("nan"), float("inf"), -float("inf")])
def test_nonfinite_clock_rejects_at_configuration(tmp_path, offset):
    body = settings(tmp_path).model_dump()
    body["fixture_clock_offset"] = offset
    with pytest.raises(ValidationError):
        driver.Settings.model_validate(body)
    assert not settings(tmp_path).evidence.exists()


@pytest.mark.parametrize("mutation", ["offset", "beacon", "nonfinite", "valid"])
def test_exact_bootstrap_clock_binding(tmp_path, monkeypatch, mutation):
    from types import SimpleNamespace

    from hypertrain.beacon.core import FixtureBeacon

    cfg = settings(tmp_path)
    cfg.service_state.mkdir()
    beacon = FixtureBeacon(current=52).get(52)
    payload = {
        "round": beacon.round,
        "signature": beacon.signature,
        "randomness": beacon.randomness,
    }
    cfg.verified_beacon.write_text(json.dumps(payload))
    offset = 123.5
    runtime = {"fixture_clock_offset": offset, "beacon": payload}
    if mutation == "beacon":
        other = FixtureBeacon(current=53).get(53)
        runtime["beacon"] = {
            "round": other.round,
            "signature": other.signature,
            "randomness": other.randomness,
        }
    if mutation == "nonfinite":
        runtime["fixture_clock_offset"] = float("nan")
    (cfg.service_state / "runtime.json").write_text(json.dumps(runtime))
    cfg = cfg.model_copy(
        update={"fixture_clock_offset": 999999 if mutation == "offset" else offset}
    )
    manifest = SimpleNamespace(training=SimpleNamespace(beacon=SimpleNamespace(genesis_time=1000)))
    bundle = SimpleNamespace(beacon_round=52)
    monkeypatch.setattr(driver.time, "time", lambda: 1000 + 51 * 3 + offset + 10)
    monkeypatch.setattr(driver.time, "monotonic", lambda: 20)
    if mutation == "valid":
        assert driver.bound_clock(cfg, bundle, manifest) == 1000 + 51 * 3 - 10
    else:
        with pytest.raises(driver.DriverError):
            driver.bound_clock(cfg, bundle, manifest)
    assert not cfg.evidence.exists()


@pytest.mark.parametrize("failure", ["copy", "hash", "index_write", "none"])
def test_export_errors_never_publish_unindexed_pass(tmp_path, monkeypatch, failure):
    cfg = settings(tmp_path)
    cfg.service_state.mkdir()
    with sqlite3.connect(cfg.service_state / "challenge.db") as db:
        db.execute("CREATE TABLE original(id TEXT)")
    objects = cfg.service_state / "objects"
    objects.mkdir()
    (objects / "artifact").write_bytes(b"original")
    cfg.evidence.mkdir()
    if failure == "copy":
        monkeypatch.setattr(
            driver.shutil,
            "copytree",
            lambda *args, **kwargs: (_ for _ in ()).throw(OSError("injected copy failure")),
        )
    if failure == "hash":
        original_read = Path.read_bytes

        def fail_read(path):
            if path.name == "artifact":
                raise OSError("injected hash read failure")
            return original_read(path)

        monkeypatch.setattr(Path, "read_bytes", fail_read)
    if failure == "index_write":
        original_publish = driver.durable_json
        failed = []

        def fail_index(path, value):
            if path.name == "index.json" and not failed:
                failed.append(True)
                raise OSError("injected index write failure")
            return original_publish(path, value)

        monkeypatch.setattr(driver, "durable_json", fail_index)
    if failure == "none":
        driver.finalize_evidence(cfg, True, None)
    else:
        with pytest.raises(driver.DriverError, match="incomplete evidence export"):
            driver.finalize_evidence(cfg, True, None)
    result = json.loads((cfg.evidence / "result.json").read_bytes())
    index = json.loads((cfg.evidence / "index.json").read_bytes())
    assert result["status"] == ("PASS" if failure == "none" else "FAIL")
    assert result["export_complete"] == (failure == "none")
    assert index["export_complete"] == (failure == "none")
    assert result["index_sha256"] == driver.sha256_hex((cfg.evidence / "index.json").read_bytes())
    assert (objects / "artifact").exists()


def test_rescue_copy_error_preserves_original_workload_exception(tmp_path, monkeypatch):
    cfg = settings(tmp_path)
    cfg.service_state.mkdir()
    with sqlite3.connect(cfg.service_state / "challenge.db") as db:
        db.execute("CREATE TABLE original(id TEXT)")
    (cfg.service_state / "objects").mkdir()
    config = tmp_path / "config.json"
    config.write_text(cfg.model_dump_json())
    original_error = driver.DriverError("original live failure")

    def fail_work(settings, cancel):
        settings.evidence.mkdir()
        raise original_error

    def fail_copy(*args, **kwargs):
        raise OSError("rescue copy failure")

    monkeypatch.setattr(driver, "proof", fail_work)
    monkeypatch.setattr(driver.shutil, "copytree", fail_copy)
    monkeypatch.setattr(driver.signal, "signal", lambda *args: None)
    with pytest.raises(driver.DriverError) as observed:
        driver.main(["--config", str(config)])
    assert observed.value is original_error
    result = json.loads((cfg.evidence / "result.json").read_bytes())
    index = json.loads((cfg.evidence / "index.json").read_bytes())
    assert result["status"] == "FAIL" and result["workload_error"] == str(original_error)
    assert result["errors"] and not index["export_complete"]


@pytest.mark.parametrize("fault", ["offset", "beacon", "valid"])
def test_prepare_clock_probe_rejects_before_any_output(
    tmp_path, monkeypatch, fault, signed_package
):
    from hypertrain.beacon.core import FixtureBeacon

    directory, digest, roles, template, offset, payload = signed_package
    cfg = settings(tmp_path)
    cfg = cfg.model_copy(
        update={
            "snapshot": directory,
            "snapshot_digest": digest,
            "fixture_clock_offset": offset + 999999 if fault == "offset" else offset,
            "fixture_clock_directory": cfg.service_state / "fixture-clock",
            "ca": Path(certifi.where()),
        }
    )
    shutil.copytree(template, cfg.service_state)
    shutil.copytree(roles, cfg.secrets)
    monkeypatch.setattr(driver, "durable_destination", lambda *args: True)
    cfg.verified_beacon.write_text(json.dumps(payload))
    if fault == "beacon":
        b = FixtureBeacon(current=payload["round"] + 1).get(payload["round"] + 1)
        (cfg.service_state / "runtime.json").write_text(
            json.dumps(
                {
                    "fixture_clock_offset": offset,
                    "beacon": {
                        "round": b.round,
                        "signature": b.signature,
                        "randomness": b.randomness,
                    },
                }
            )
        )
    if fault == "valid":
        bundle, manifest = driver.prepare(cfg)
        assert bundle.run_id == manifest.run_id() == driver.authority.RUN
        with sqlite3.connect(cfg.evidence / "initial-service.db") as db:
            assert (
                db.execute("SELECT COUNT(*) FROM admissions_v2 WHERE state='ACTIVE'").fetchone()[0]
                == 4
            )
            assert (
                db.execute(
                    "SELECT COUNT(*) FROM admission_trial_results WHERE outcome='MATCH'"
                ).fetchone()[0]
                == 48
            )
    else:
        with pytest.raises(driver.DriverError):
            driver.prepare(cfg)
        assert not cfg.evidence.exists()
