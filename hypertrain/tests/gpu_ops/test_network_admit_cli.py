"""Reviewed CLI boundaries and loopback lifecycle; no paid or CUDA evidence."""

from __future__ import annotations

import importlib.util
import json
import os
import select
import subprocess
import sys
import time
from pathlib import Path
from typing import Literal

import pytest

import hypertrain
from hypertrain.gpu_ops.journal import Journal, fsha, sha256
from hypertrain.gpu_ops.launcher import Reject
from hypertrain.protocol import envelope_v2
from hypertrain.protocol.example import example_manifest
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages_v2 import IslandJobV1, RunManifestV2

TREE = Path(hypertrain.__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "network_admit_cli", TREE / "experiments/gpu_network_v2/orchestrate.py"
)
assert SPEC and SPEC.loader
cli = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = cli
SPEC.loader.exec_module(cli)
OWNER = Keypair(bytes(range(32)))


def test_operator_graph_self_reference_excluded_only():
    """Metadata hashing unit, no runtime/bootstrap/economic admission positive."""
    import network_service_proof as driver

    graph = driver.ContinuationGraph.model_construct(
        economic_authority=Path("/private/operator"),
        private_files={"/private/operator": "11" * 32, "/private/owner": "22" * 32},
        run_id="33" * 32,
        owner=OWNER.ss58,
    )
    digest = driver.continuation_graph_hash(graph)
    changed_self = graph.model_copy(
        update={
            "private_files": {**graph.private_files, "/private/operator": "44" * 32},
        }
    )
    assert driver.continuation_graph_hash(changed_self) == digest
    changed_key = graph.model_copy(
        update={
            "private_files": {**graph.private_files, "/private/owner": "55" * 32},
        }
    )
    assert driver.continuation_graph_hash(changed_key) != digest


@pytest.mark.parametrize(
    "fault",
    [
        None,
        "permutation",
        "uuid",
        "worker",
        "source",
        "empty",
        "extra",
        "missing:scripts/network_service_proof.py",
        "missing:src/hypertrain/gpu_ops/network_qualification.py",
        "missing:src/hypertrain/miner/island_worker.py",
        "tampered:scripts/network_service_proof.py",
        "tampered:src/hypertrain/gpu_ops/network_qualification.py",
        "tampered:src/hypertrain/miner/island_worker.py",
        "legacy",
        "signer",
        "run",
        "expiry",
        "future",
        "profile",
        "count",
        "bool-count",
        "duplicate",
        "malformed-uuid",
        "name",
        "driver",
    ],
)
def test_operator_allocation_original_csv_signed_worker(tmp_path, fault):
    """Shipped CSV parser and real source signatures; no CUDA-positive claim."""
    from types import SimpleNamespace

    import network_service_proof as driver
    from hypertrain.gpu_ops.network_qualification import sources

    pins = sources(TREE)
    pins["scripts/network_service_proof.py"] = fsha(TREE / "scripts/network_service_proof.py")
    if fault == "empty":
        pins.clear()
    elif fault == "extra":
        pins["unexpected.py"] = "01" * 32
    elif fault and fault.startswith("missing:"):
        del pins[fault.split(":", 1)[1]]
    elif fault and fault.startswith("tampered:"):
        pins[fault.split(":", 1)[1]] = "02" * 32
    worker = TREE / "src/hypertrain/miner/island_worker.py"
    run_id = "ab" * 32
    profile = {"runtime": {"driver_allowlist": ["580.95.05"]}}
    profile_hash = sha256(canonicalize(profile))
    record = {
        "allocation_contract": "canonical-shipped-source-v1",
        "files": pins,
        "profile_sha256": profile_hash,
        "original_worker_sha256": fsha(worker),
        "physical_allocation": ">=2",
        "training_ranks": 2,
        "run_id": run_id,
    }
    if fault == "worker":
        record["original_worker_sha256"] = "cd" * 32
    if fault == "legacy":
        del record["allocation_contract"]
        record["files"] = dict.fromkeys(
            ("allocation_cli.py", "gpu_allocation.py", "qualify_r9.py"), "01" * 32
        )
    source = {"files": dict(record["files"]), "profile_sha256": profile_hash}
    if fault == "source":
        source["files"]["src/hypertrain/gpu_ops/network_qualification.py"] = "ef" * 32
    if fault == "profile":
        source["profile_sha256"] = "ef" * 32
    files = {}
    for name, body in (
        ("allocation_worker_receipt", record),
        ("source_receipt", source),
    ):
        path = tmp_path / (name + ".json")
        env = envelope_v2.seal(
            Keypair(b"\xcc" * 32) if fault == "signer" else OWNER,
            "Receipt",
            "cd" * 32 if fault == "run" else run_id,
            {
                "w": 0,
                "received_round": 2 if fault == "future" else 1,
                "commit_hash": sha256(canonicalize(body)),
            },
            1 if fault == "expiry" else 100,
        )
        if name == "allocation_worker_receipt" and fault == "worker":
            env["sig"] = "00" * 64
        path.write_bytes(canonicalize({"record": body, "receipt": env}))
        files[name] = path
    uuids = [f"GPU-{i:08x}-1111-2222-3333-444444444444" for i in range(4)]
    rows = [f"{u}, NVIDIA GeForce RTX 5090, 580.95.05" for u in reversed(uuids)]
    if fault == "duplicate":
        rows[-1] = rows[0]
    elif fault == "malformed-uuid":
        rows[0] = rows[0].replace(uuids[-1], "GPU-" + "-" * 36)
    elif fault == "name":
        rows[0] = rows[0].replace("RTX 5090", "RTX 4090")
    elif fault == "driver":
        rows[0] = rows[0].replace("580.95.05", "unqualified")
    inventory = "\n".join(rows)
    selected = uuids[:2]
    if fault == "permutation":
        selected = selected[::-1]
    elif fault == "uuid":
        selected = [uuids[0], "GPU-ffffffff-1111-2222-3333-444444444444"]
    allocation = {
        "allocated_count": 4,
        "inventory": inventory,
        "selected_uuids": selected,
    }
    if fault == "count":
        allocation["allocated_count"] = 3
    elif fault == "bool-count":
        allocation["allocated_count"] = True
    runtime = SimpleNamespace(
        journal=SimpleNamespace(last=lambda *a, **k: allocation),
        profile=profile,
    )
    checked = {"files": files, "record": {"tree": str(TREE)}}
    if fault is None:
        assert (
            len(
                driver.continuation_allocation_authority(
                    runtime,
                    checked,
                    OWNER.ss58,
                    run_id,
                    1,
                    "h0",
                    {"quote": {"num_gpus": 4}},
                )
            )
            == 64
        )
    else:
        with pytest.raises(driver.DriverError):
            driver.continuation_allocation_authority(
                runtime,
                checked,
                OWNER.ss58,
                run_id,
                2 if fault == "expiry" else 1,
                "h0",
                {"quote": {"num_gpus": 4}},
            )


@pytest.fixture(scope="module")
def retained_custody_work(tmp_path_factory):
    """Reuse original genuine miner/reference fixture once, without a third run."""
    path = TREE / "tests/ledger/test_escrow_v2.py"
    spec = importlib.util.spec_from_file_location("custody_original_ledger_fixture", path)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    actual = module.actual_reward.__wrapped__(tmp_path_factory)
    base = tmp_path_factory.getbasetemp()
    miner = next(base.glob("real-reward*/published"))
    reference = next(base.glob("real-reference*/published"))
    return module, actual, miner, reference


def test_probe_retains_original_shadow_reward_authority(
    retained_custody_work, monkeypatch, tmp_path
):
    # Given: original independent executions exist; capture must not launch again.
    import threading

    import hypertrain.miner.island_launch as island
    import network_gpu_operation as adapter
    from hypertrain.auditor.replay import unpack_state
    from hypertrain.data.trial_assignment import trial_assignment_hash
    from hypertrain.protocol.envelope_v2 import seal, verify_envelope
    from hypertrain.protocol.hashing import sha256_hex
    from hypertrain.protocol.messages_v2 import ArtifactLimits, JoinChallenge
    from hypertrain.trainer.compress import state_hash

    fixture, _, miner_path, reference_path = retained_custody_work
    job = IslandJobV1.model_validate_json((miner_path / "job.json").read_bytes())
    refjob = IslandJobV1.model_validate_json((reference_path / "job.json").read_bytes())
    assert job.model_dump(exclude={"deadline"}) == refjob.model_dump(exclude={"deadline"})
    assert miner_path != reference_path

    def prohibited(*args, **kwargs):
        raise AssertionError("custody capture cannot execute another trajectory")

    monkeypatch.setattr(island, "launch_island", prohibited)
    monkeypatch.setattr(island, "_launch_unbounded", prohibited)
    start, _ = unpack_state((miner_path / job.object_paths["start_state"]).read_bytes())
    challenge = JoinChallenge(
        admission_id=fixture.H,
        nonce="22" * 32,
        seed_beacon=1,
        deadline_beacon=101,
        manifest_hash=job.run_id,
        theta_hash=state_hash(start),
        assignment_hash=trial_assignment_hash(job.manifest, job.w, tuple(job.sample_ids)),
        layout=job.manifest.training.reference_spec.layout,
        artifact_limits=ArtifactLimits(
            max_object_bytes=268435456,
            max_body_bytes=65536,
            max_chunk_manifest_bytes=1048576,
        ),
        policy_hash=job.manifest.network.admission_policy_hash,
    )
    signed_challenge = seal(fixture.COORD, "JoinChallenge", job.run_id, challenge, 200)

    class CpuCustodyHelper(adapter.Operation):
        """Test-only helper descriptor; never submitted to production execute/check."""

        backend: Literal["cpu"] = "cpu"

    operation = CpuCustodyHelper(
        operation="probe",
        binding=fixture.H,
        hotkey=fixture.HOT.ss58,
        owner=fixture.COLD.ss58,
        role="h0",
        instance_id=11,
        machine_id=22,
        image_digest=job.manifest.training.reference_spec.image_digest,
        job=job,
        binding_kind="trial",
        backend="cpu",
        trace=False,
        cutoff=job.deadline,
        sources={
            "scripts/network_gpu_operation.py": fsha(TREE / "scripts/network_gpu_operation.py")
        },
        challenge="original-challenge.json",
        context_files={},
    )
    miner = island.validate_artifacts(job, miner_path)
    reference = island.validate_artifacts(refjob, reference_path)
    # When: validate/hash retained objects; actual miner signs its own CommitV2.
    signed_commit = adapter.probe_commit(operation, miner, fixture.HOT, signed_challenge)
    captured = adapter.publication_custody(
        operation,
        miner_path.parent,
        miner,
        signed_challenge,
        signed_commit,
    )
    ref_operation = operation.model_copy(update={"operation": "reference", "job": refjob})
    independent = adapter.publication_custody(
        ref_operation,
        reference_path.parent,
        reference,
        signed_challenge,
    )
    # Then: independent original publications, no launcher-signed miner or MATCH assertion.
    assert verify_envelope(captured["body"]["miner_commit"]["envelope"])
    assert captured["body"]["miner_commit"]["envelope"]["signer"] == fixture.HOT.ss58
    assert independent["body"]["miner_commit"] is None
    assert independent["body"]["operation"] == "reference"
    assert captured["body"]["operation_sha256"] != independent["body"]["operation_sha256"]
    assert captured["body"]["job_sha256"] == job.digest()
    assert independent["body"]["job_sha256"] == refjob.digest()
    assert captured["body"]["sample_ids_sha256"] == independent["body"]["sample_ids_sha256"]
    assert captured["body"]["trial_authority"]["nonce"] == challenge.nonce
    assert captured["body"]["environment"]["backend"] == "cpu"
    assert captured["body"]["layout"] == challenge.layout.model_dump(mode="json")
    assert captured["body"]["sources"] == operation.sources
    assert (
        captured["body"]["rank_summaries"][0]["commitments"]
        == (independent["body"]["rank_summaries"][0]["commitments"])
    )
    assert captured["sha256"] == sha256_hex(canonicalize(captured["body"]))
    assert captured["status"] == independent["status"] == "CAPTURED_NOT_ACCEPTED"
    from pydantic import ValidationError

    with pytest.raises(ValidationError):
        adapter.Operation.model_validate(operation.model_dump())
    # Captured output proves hashing finished; exact cancellation state drives rejection.
    cancel = threading.Event()
    cancel.set()
    assert cancel.is_set()
    with pytest.raises(ValueError, match="deadline/cancellation"):
        adapter.publish_custody_result(operation, tmp_path, {}, captured, cancel)
    assert not (tmp_path / "operation-result.json").exists()
    assert not (tmp_path / "publication-custody.json").exists()
    cancel.clear()
    clock = {"now": operation.cutoff}
    monkeypatch.setattr(adapter.time, "time", lambda: clock["now"])
    with pytest.raises(ValueError, match="deadline/cancellation"):
        adapter.publish_custody_result(operation, tmp_path, {}, captured, cancel)
    assert not (tmp_path / "operation-result.json").exists()
    assert not (tmp_path / "execution-custody.json").exists()
    # Cancellation delivered by the custody write must block subsequent result publication.
    clock["now"] = operation.cutoff - 1
    original_write = adapter.durable_write

    def cancel_at_custody_write(path, payload):
        original_write(path, payload)
        if path.name == "publication-custody.json":
            cancel.set()

    monkeypatch.setattr(adapter, "durable_write", cancel_at_custody_write)
    with pytest.raises(ValueError, match="deadline/cancellation"):
        adapter.publish_custody_result(operation, tmp_path, {}, captured, cancel)
    assert (tmp_path / "publication-custody.json").exists()
    assert not (tmp_path / "operation-result.json").exists()
    assert not (tmp_path / "execution-custody.json").exists()
    with pytest.raises(ValueError, match="signing identity"):
        adapter.probe_commit(operation, miner, fixture.OTHER, signed_challenge)
    wrong_commit = seal(fixture.OTHER, "CommitV2", job.run_id, signed_commit["body"], 200)
    with pytest.raises(ValueError, match="miner commit"):
        adapter.publication_custody(
            operation, miner_path.parent, miner, signed_challenge, wrong_commit
        )
    wrong_challenge = seal(fixture.OTHER, "JoinChallenge", job.run_id, challenge, 200)
    with pytest.raises(ValueError, match="trial authority"):
        adapter.publication_custody(
            operation, miner_path.parent, miner, wrong_challenge, signed_commit
        )
    malformed = {**signed_commit, "sig": "00" * 64}
    with pytest.raises(ValueError, match="miner commit"):
        adapter.publication_custody(
            operation, miner_path.parent, miner, signed_challenge, malformed
        )
    signed_changed = seal(
        fixture.HOT,
        "CommitV2",
        job.run_id,
        {**signed_commit["body"], "delta_hash": "33" * 32},
        200,
    )
    with pytest.raises(ValueError, match="miner commit"):
        adapter.publication_custody(
            operation, miner_path.parent, miner, signed_challenge, signed_changed
        )
    original = miner.delta.read_bytes()
    try:
        miner.delta.write_bytes(bytes([original[0] ^ 1]) + original[1:])
        with pytest.raises(island.IslandFailure, match="terminal compression/EF mismatch"):
            adapter.publication_custody(
                operation, miner_path.parent, miner, signed_challenge, signed_commit
            )
    finally:
        miner.delta.write_bytes(original)


def test_reference_context_preserves_original_signed_challenge_and_trace(
    retained_custody_work,
    tmp_path,
    monkeypatch,
):
    import threading
    from types import SimpleNamespace

    import network_gpu_operation as producer
    import network_service_proof as service
    from hypertrain.auditor.replay import unpack_state
    from hypertrain.data.trial_assignment import trial_assignment_hash
    from hypertrain.gpu_ops.journal import Journal
    from hypertrain.gpu_ops.work_screen import screen_work
    from hypertrain.protocol.envelope_v2 import Intake, seal
    from hypertrain.protocol.hashing import sha256_hex
    from hypertrain.protocol.messages import Receipt
    from hypertrain.protocol.messages_v2 import ArtifactLimits, JoinChallenge
    from hypertrain.trainer.compress import state_hash

    fixture, _, _, reference_path = retained_custody_work
    job = IslandJobV1.model_validate_json((reference_path / "job.json").read_bytes())
    start, _ = unpack_state((reference_path / job.object_paths["start_state"]).read_bytes())
    challenge = JoinChallenge(
        admission_id=fixture.H,
        nonce="44" * 32,
        seed_beacon=1,
        deadline_beacon=101,
        manifest_hash=job.run_id,
        theta_hash=state_hash(start),
        assignment_hash=trial_assignment_hash(job.manifest, job.w, tuple(job.sample_ids)),
        layout=job.manifest.training.reference_spec.layout,
        artifact_limits=ArtifactLimits(
            max_object_bytes=268435456,
            max_body_bytes=65536,
            max_chunk_manifest_bytes=1048576,
        ),
        policy_hash=job.manifest.network.admission_policy_hash,
    )
    signed = seal(fixture.COORD, "JoinChallenge", job.run_id, challenge, 200)
    wire = canonicalize(signed)
    relative = "reference-context-" + job.digest() + "/original-join-challenge.json"
    sources = {"scripts/network_gpu_operation.py": fsha(TREE / "scripts/network_gpu_operation.py")}
    expected = producer.Operation(
        operation="reference",
        binding=fixture.H,
        hotkey=fixture.HOT.ss58,
        owner=OWNER.ss58,
        role="h0",
        instance_id=11,
        machine_id=22,
        job=job,
        binding_kind="trial",
        sources=sources,
        image_digest=job.manifest.training.reference_spec.image_digest,
        cutoff=job.deadline,
        trace=True,
        challenge=relative,
        context_files={relative: sha256_hex(wire)},
    )
    body = {
        "role": "h0",
        "binding": fixture.H,
        "operation": "reference",
        "hotkey": fixture.HOT.ss58,
        "spec_sha256": sha256_hex(expected.model_dump_json().encode()),
    }
    receipt = seal(
        OWNER,
        "Receipt",
        job.run_id,
        Receipt(
            w=job.w,
            commit_hash=sha256_hex(canonicalize(body)),
            received_round=1,
        ),
        200,
    )
    context = {
        "run_id": job.run_id,
        "hotkey": fixture.HOT.ss58,
        "challenge_envelope": signed,
        "challenge": challenge.body(),
        "challenge_hash": challenge.digest(),
        "admission_id": fixture.H,
        "epoch": job.w,
    }
    transferred = []
    launches = []
    cancel = threading.Event()

    class OriginalOperationObserved(Exception):
        pass

    (tmp_path / "runtime-journal").mkdir()

    class RuntimeBoundary:
        journal = Journal(tmp_path / "runtime-journal")
        cancel_during_staging = False
        lifecycle = SimpleNamespace(cleanup_started=lambda: False)

        def owned_host(self, role):
            return {
                "instance_id": 11,
                "machine_id": 22,
                "image_digest": job.manifest.training.reference_spec.image_digest,
            }

        def accept_operation(self, spec, raw, *, owner, beacon):
            original = Intake(job.run_id, {"Receipt": owner.__eq__}).accept(raw, beacon)
            assert original.commit_hash == sha256_hex(canonicalize(body))
            assert spec == expected
            self.journal.append(
                "network_operation_authority",
                role=spec.role,
                binding=spec.binding,
                operation=spec.operation,
                hotkey=spec.hotkey,
                w=job.w,
                receipt=json.loads(raw),
                spec_sha256=sha256_hex(spec.model_dump_json().encode()),
            )

        def stage_context(self, role, paths, remote, name, cutoff, cancel):
            assert cutoff == job.deadline and cancel is original_cancel
            assert paths[relative].read_bytes() == wire
            transferred.append((relative, fsha(paths[relative]), role))
            if self.cancel_during_staging:
                cancel.set()

        def launch_adapter(self, spec):
            return cli.NetworkRuntime.launch_adapter(self, spec)

        def operation(self, spec, directory, cancel=None):
            assert spec.trace is True and cancel is original_cancel
            assert transferred
            launches.append((spec, cancel))
            raise OriginalOperationObserved

    original_cancel = cancel
    runtime = RuntimeBoundary()
    plan = service.decoder_context_plan(
        runtime,
        OWNER.ss58,
        job.run_id,
        sources,
        {fixture.HOT.ss58: {"role": "h0"}},
        {"reference:" + job.digest(): receipt},
        1,
    )
    raw, retained, _ = plan("reference", job, tmp_path, context)
    assert raw == expected.model_dump(mode="json") and retained == receipt
    assert (tmp_path / "original-join-challenge.json").read_bytes() == wire
    factory = service.decoder_service_factory(runtime, plan)
    launch = factory("reference", job, tmp_path, context)
    assert not transferred and not launches
    monkeypatch.setattr(service.time, "time", lambda: job.deadline - 1)
    # Actual screen_work omits trace. The scoped wrapper must supply it to the real adapter.
    with pytest.raises(OriginalOperationObserved):
        screen_work(
            job,
            reference_path,
            challenge,
            now_beacon=1,
            backend="cuda",
            launch=launch,
            cancel=cancel,
        )
    assert transferred == [(relative, sha256_hex(wire), "h0")]
    assert launches == [(expected, cancel)]
    assert challenge.nonce == producer.trial_authority(expected, signed).nonce
    cancel.set()
    with pytest.raises(ValueError, match="CANCELLED"):
        screen_work(
            job,
            reference_path,
            challenge,
            now_beacon=1,
            backend="cuda",
            launch=launch,
            cancel=cancel,
        )
    assert len(transferred) == len(launches) == 1
    # A real Event set while staging prevents the original operation call, without sleeps.
    runtime.cancel_during_staging = True
    cancel.clear()
    with pytest.raises(service.DriverError, match="deadline/cancellation"):
        screen_work(
            job,
            reference_path,
            challenge,
            now_beacon=1,
            backend="cuda",
            launch=launch,
            cancel=cancel,
        )
    assert cancel.is_set() and launches == [(expected, original_cancel)]
    # Original trace-free fixture is not silently upgraded to accepted trace custody.
    artifacts = producer.validate_artifacts(job, reference_path)

    class CpuReferenceCustodyHelper(producer.Operation):
        """Helper-only CPU fixture; actual factory descriptor remains CUDA-only."""

        backend: Literal["cpu"] = "cpu"

    cpu_helper = CpuReferenceCustodyHelper.model_validate(
        {**expected.model_dump(), "backend": "cpu"}
    )
    with pytest.raises(ValueError, match="trace absent"):
        producer.publication_custody(cpu_helper, reference_path.parent, artifacts, signed)
    wrong = {
        **context,
        "challenge_envelope": seal(
            fixture.OTHER,
            "JoinChallenge",
            job.run_id,
            challenge,
            200,
        ),
    }
    with pytest.raises(service.DriverError, match="authority"):
        plan("reference", job, tmp_path, wrong)
    with pytest.raises(service.DriverError, match="accepted challenge"):
        plan("reference", job, tmp_path, {**context, "epoch": job.w + 1})
    (tmp_path / "original-join-challenge.json").write_bytes(b"different original bytes")
    with pytest.raises(service.DriverError, match="file conflicts"):
        plan("reference", job, tmp_path, context)


@pytest.mark.parametrize("required", [False, True])
def test_original_probe_trace_is_explicit_and_preserves_ordinary_default(
    retained_custody_work,
    monkeypatch,
    tmp_path,
    required,
):
    import threading
    from types import SimpleNamespace

    import hypertrain.gpu_ops.work_screen as screen
    from hypertrain.auditor.replay import unpack_state
    from hypertrain.data.trial_assignment import trial_assignment_hash
    from hypertrain.miner.core import NetworkMiner
    from hypertrain.protocol.envelope_v2 import seal
    from hypertrain.protocol.messages_v2 import ArtifactLimits, JoinChallenge
    from hypertrain.trainer.compress import state_hash

    fixture, _, miner_path, _ = retained_custody_work
    job = IslandJobV1.model_validate_json((miner_path / "job.json").read_bytes())
    start, _ = unpack_state((miner_path / job.object_paths["start_state"]).read_bytes())
    challenge = JoinChallenge(
        admission_id=fixture.H,
        nonce="55" * 32,
        seed_beacon=1,
        deadline_beacon=101,
        manifest_hash=job.run_id,
        theta_hash=state_hash(start),
        assignment_hash=trial_assignment_hash(job.manifest, job.w, tuple(job.sample_ids)),
        layout=job.manifest.training.reference_spec.layout,
        artifact_limits=ArtifactLimits(
            max_object_bytes=268435456,
            max_body_bytes=65536,
            max_chunk_manifest_bytes=1048576,
        ),
        policy_hash=job.manifest.network.admission_policy_hash,
    )
    path = tmp_path / "challenge.json"
    path.write_bytes(canonicalize(seal(fixture.COORD, "JoinChallenge", job.run_id, challenge, 200)))
    observed = []
    cancel = threading.Event()

    class LaunchObserved(Exception):
        pass

    def original_launch(job, directory, *, backend, cancel=None, trace=False):
        observed.append((job, directory, backend, cancel, trace))
        raise LaunchObserved

    def original_screen(job, directory, challenge, *, now_beacon, backend, launch, cancel):
        assert now_beacon == 1
        # Original screen_work dispatch does not inject trace; only Miner option may supply it.
        return launch(job, directory, backend=backend, cancel=cancel)

    monkeypatch.setattr(screen, "screen_work", original_screen)
    miner = NetworkMiner.__new__(NetworkMiner)
    miner.run_id = job.run_id
    miner.cfg = SimpleNamespace(device="cpu")
    miner.manifest = job.manifest
    miner.api = SimpleNamespace(call=lambda method, url: {"now_round": 1})
    miner.url = "/v2/runs/" + job.run_id
    # Bound deterministic clock to before original cutoff; test never launches or signs output.
    import hypertrain.miner.core as core

    monkeypatch.setattr(core.time, "time", lambda: job.deadline - 1)
    options = {"trace": True} if required else {}
    with pytest.raises(LaunchObserved):
        miner.probe(
            miner_path / "job.json",
            path,
            launch=original_launch,
            cancel=cancel,
            **options,
        )
    assert observed == [(job, miner_path, "cpu", cancel, required)]
    if required:
        from hypertrain.miner.core import MinerError
        from hypertrain.miner.island_launch import validate_artifacts

        original = validate_artifacts(job, miner_path)
        retained_calls = []

        def retained_without_trace(job, directory, *, backend, cancel=None, trace=False):
            retained_calls.append(trace)
            return original

        # Existing genuine trace-free publication is refused, not retrained or filled in.
        with pytest.raises(MinerError, match="required trace absent"):
            miner.probe(
                miner_path / "job.json",
                path,
                launch=retained_without_trace,
                cancel=cancel,
                trace=True,
            )
        assert retained_calls == [True]


@pytest.fixture
def contract(tmp_path: Path):
    import hypertrain.trainer  # noqa: F401
    from hypertrain.auditor.replay import AnchorCache, pack_state
    from hypertrain.protocol.hashing import MerkleTree
    from hypertrain.trainer.config import TrainConfig
    from hypertrain.trainer.model import init_params

    files = {}

    def put(name, value, binary=False):
        path = tmp_path / (name if binary else name + ".json")
        path.write_bytes(value if binary else json.dumps(value).encode())
        files[name] = {"path": str(path), "sha256": fsha(path)}
        return path

    registry = put(
        "registry",
        {"schemaVersion": 2, "config": {"size": 10}, "layers": [{"size": 42}]},
    )
    image = "sha256:" + fsha(registry)
    profile = json.loads((TREE / "experiments/gpu_network_v2/profile.json").read_bytes())
    profile["runtime"].update(image_digest=image, driver_allowlist=["595.84"])
    profile_path = put("profile", profile)
    body = example_manifest().body()
    for section in ("model", "outer"):
        body[section].update(profile[section])
    body["inner"].update({k: v for k, v in profile["inner"].items() if k != "lr_schedule"})
    body["inner"]["lr_schedule"].update(profile["inner"]["lr_schedule"])
    body["reference_spec"].update(image_digest=image, driver_allowlist=["595.84"], sm_count=170)
    body["reference_spec"]["layout"].update(profile["layout"])
    rows = [bytes([i % 64, 0]) * 17 for i in range(4096)]
    merkle = MerkleTree(rows)
    body["dataset"].update(
        n_samples=4096,
        merkle_root=merkle.root.hex(),
        sample_format="u16[seq_len+1] token ids",
    )
    wrapper = RunManifestV2.model_validate(
        {
            "manifest_version": 2,
            "training": body,
            "network": {
                **{
                    k: "11" * 32
                    for k in (
                        "admission_policy_hash",
                        "economics_policy_hash",
                        "aggregation_policy_hash",
                        "dispute_policy_hash",
                        "audit_policy_hash",
                        "relay_registry_hash",
                    )
                },
                "audit_mode": "anchored-full",
                "full_anchor_version": 1,
                "capabilities": [
                    "island-replay",
                    "all-level-disputes",
                    "transport-receipts",
                ],
            },
        }
    )
    theta = init_params(TrainConfig.from_manifest_v2(wrapper).model)
    anchor = AnchorCache().genesis(wrapper, OWNER.ss58, theta)
    objects = {
        "start_state": put("start_state", pack_state(theta, anchor.state), True),
        "ef_in": put("ef_in", pack_state(anchor.ef), True),
        "v0": put("v0", pack_state({}), True),
        "samples": put("samples", b"".join(rows[:60]), True),
        "sample_proofs": put(
            "sample_proofs", [[p.hex() for p in merkle.proof(i)] for i in range(60)]
        ),
    }
    now = int(time.time())
    job = IslandJobV1(
        job_version=1,
        run_id=wrapper.run_id(),
        w=0,
        manifest=wrapper,
        sample_ids=list(range(60)),
        global_step0=0,
        start_state_sha256=fsha(objects["start_state"]),
        ef_in_sha256=fsha(objects["ef_in"]),
        v0_sha256=fsha(objects["v0"]),
        object_paths={k: p.name for k, p in objects.items()},
        deadline=now + 3600,
    )
    put("job", job.model_dump(mode="json"))
    key = tmp_path / "key"
    key.write_text("a" * 32)
    key.chmod(0o600)
    cfg = {
        "base_url": "http://127.0.0.1:1",
        "key_file": str(key),
        "image": "candidate@" + image,
        "disk_gb": 80,
        "hard_deadline_seconds": 3600,
        "hard_grace_seconds": 0,
        "boot_timeout_seconds": 300,
        "keyscan_attempts": 1,
        "cleanup_deadline_unix": now + 3600,
        "remote_python": "/opt/hypertrain/venv/bin/python",
        "remote_root": str(tmp_path / "remote-{role}"),
        "cleanup_contract": "network-v2",
        "network_profile_file": str(profile_path),
        "evidence_path": str(tmp_path / "evidence.json"),
        "create_margin_seconds": 600,
        "hosts": [
            {"role": r, "machine_id": 5000 + i, "max_dph_total": 2}
            for i, r in enumerate(("h0", "h1"))
        ],
    }
    put("config", cfg)
    put(
        "lifecycle_hashes",
        {
            p: fsha(TREE / p)
            for p in (
                "src/hypertrain/gpu_ops/launcher.py",
                "src/hypertrain/gpu_ops/supervisor.py",
            )
        },
    )
    spec = importlib.util.spec_from_file_location(
        "cli_fixture_driver", TREE / "scripts/network_gpu_qualification.py"
    )
    assert spec and spec.loader
    driver = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = driver
    spec.loader.exec_module(driver)
    sources, source_bundle = cli.effective_bundle(driver, TREE, profile_path)
    put("sources", sources)
    put("bundle", source_bundle, True)
    offers = [
        {
            "id": 900 + i,
            "machine_id": 5000 + i,
            "gpu_name": "RTX 5090",
            "num_gpus": 2,
            "cpu_cores_effective": 8,
            "cpu_ram": 32768,
            "disk_space": 80,
            "dph_total": "0.4",
            "storage_cost": "0.1",
            "inet_up_cost": "0.001",
            "inet_down_cost": "0.001",
            "verified_unix": now,
            "requested_unix": now,
            "received_unix": now,
        }
        for i in range(2)
    ]
    snapshot = {
        "authenticated": True,
        "instances": [],
        "volumes": [],
        "charges": [],
        "uncapped_charges": False,
        "verified_unix": now,
        "requested_unix": now,
        "received_unix": now,
        "prior_attempts_usd": "0",
        "active_liabilities_usd": "0",
        "available_credit_usd": "20",
        "account_id": 4242,
    }
    authorization = {
        "experiment": "hypertrain-network-v2",
        "currency": "USD",
        "ceiling_usd": "50",
        "max_instances": 2,
        "max_seconds_per_instance": 3600,
        "source_message": "TEST_ONLY",
        "date": "2026-10-09",
    }
    evidence = {
        "root_verified_bounded_lifecycle": True,
        "registry_image_verified": True,
        "candidate_image_digest": image,
        "staged_contract_sha256": "22" * 32,
        "deadline_supervisor_contract_sha256": "33" * 32,
    }
    for k, v in {
        "offers": offers,
        "financial": snapshot,
        "authorization": authorization,
        "evidence": evidence,
    }.items():
        put(k, v)
    put(
        "plan",
        cli.admission_plan(profile, evidence, authorization, snapshot, offers, qualification=True),
    )
    put(
        "observations",
        [{"requested_unix": now, "received_unix": now, "verified_unix": now}],
    )
    from hypertrain.miner.admission import sign_join
    from hypertrain.protocol.messages_v2 import HardwareHint

    join = sign_join(
        OWNER,
        OWNER,
        run_id=wrapper.run_id(),
        request_id="aa" * 32,
        expires_beacon=20,
        policy_hash=wrapper.network.admission_policy_hash,
        hardware_hint=HardwareHint(device_name="test", device_count=2, driver="595.84"),
    )
    put("join", join.model_dump(mode="json"))
    put(
        "seed_context",
        {
            "run_id": wrapper.run_id(),
            "owner": OWNER.ss58,
            "hotkey": OWNER.ss58,
            "coldkey": OWNER.ss58,
            "request_id": join.request_id,
            "policy_hash": join.policy_hash,
            "job_sha256": files["job"]["sha256"],
            "join_sha256": files["join"]["sha256"],
            "admission_id": sha256(
                f"join|{wrapper.run_id()}|{join.coldkey}|{join.request_id}".encode()
            ),
            "state": "PROBATION",
        },
    )
    record = {
        "action": "admit",
        "owner": OWNER.ss58,
        "files": files,
        "tree": str(TREE),
        "parent_pid": os.getppid(),
        "cutoff_unix": now + 3600,
        "staging_bytes": 2_000_000,
        "public_dataset_reviewed": True,
        "full_genesis_reviewed": True,
        "quota": {"total": 126, "per_role": 63, "qualification": 4, "remaining": 122},
        "roles": {
            r: {
                "job": "job",
                "name": "qualification",
                "hotkey": OWNER.ss58,
                "join": "join",
                "seed_context": "seed_context",
                "objects": {k: k for k in objects},
            }
            for r in ("h0", "h1")
        },
    }
    path = tmp_path / "review.json"

    def seal():
        receipt = envelope_v2.seal(
            OWNER,
            "Receipt",
            wrapper.run_id(),
            {"w": 0, "commit_hash": sha256(canonicalize(record)), "received_round": 10},
            20,
        )
        path.write_bytes(canonicalize({"record": record, "receipt": receipt}))

    seal()
    return record, path, seal


def test_exact_owner_preflight_and_cli_plan(contract, capsys):
    _, path, _ = contract
    checked = cli.reviewed_launch(path, OWNER.ss58, 10, "admit")
    assert checked["plan"]["reserve_usd"] == "5"
    assert cli.main(["plan", str(TREE / "experiments/gpu_network_v2/profile.json")]) == 0
    assert json.loads(capsys.readouterr().out)["paid_execution"] is False


def test_effective_materialized_tree_matches_frozen_driver_sources(contract, tmp_path):
    import tarfile

    record, path, _ = contract
    checked = cli.reviewed_launch(path, OWNER.ss58, 10, "admit")
    staged = tmp_path / "materialized"
    with tarfile.open(checked["files"]["bundle"]) as archive:
        archive.extractall(staged, filter="data")
    assert checked["driver"].sources(staged) == json.loads(checked["files"]["sources"].read_bytes())
    assert fsha(staged / "experiments/gpu_network_v2/profile.json") == fsha(
        checked["files"]["profile"]
    )


def test_approved_generated_seed_consumed_without_provider(contract, tmp_path, monkeypatch):
    """Original generator validates index/signatures; root Receipt pins exact seed bytes."""
    from hypertrain.miner.admission import sign_join
    from hypertrain.protocol.messages_v2 import HardwareHint

    record, path, _ = contract
    spec = importlib.util.spec_from_file_location(
        "cli_seed_fixture", TREE / "scripts/network_gpu_seed.py"
    )
    assert spec and spec.loader
    seed = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = seed
    spec.loader.exec_module(seed)
    destination, secrets = tmp_path / "generated", tmp_path / "generated-secrets"
    profile = json.loads(Path(record["files"]["profile"]["path"]).read_bytes())
    admitted = seed.generate(
        destination,
        secrets,
        profile["runtime"]["image_digest"],
        profile["runtime"]["driver_allowlist"],
        int(time.time()),
        record["cutoff_unix"],
    )
    job = seed.validate(destination, admitted)
    index = json.loads((destination / "seed.json").read_bytes())
    owner = Keypair((secrets / "owner.seed").read_bytes())
    hot, cold = (Keypair((secrets / f"{r}-0.seed").read_bytes()) for r in ("hot", "cold"))
    join = sign_join(
        hot,
        cold,
        run_id=job.run_id,
        request_id="ab" * 32,
        expires_beacon=20,
        policy_hash=job.manifest.network.admission_policy_hash,
        hardware_hint=HardwareHint(
            device_name="synthetic-functional", device_count=2, driver="595.84"
        ),
    )
    join_path = tmp_path / "generated-join.json"
    join_path.write_bytes(canonicalize(join.body()))
    for key in ("job", "start_state", "ef_in", "v0", "samples", "sample_proofs"):
        p = destination / ("job.json" if key == "job" else key)
        record["files"][key] = {"path": str(p), "sha256": fsha(p)}
    for name in (
        "seed.json",
        "manifest-envelope.json",
        "job-envelope.json",
        "reference-envelope.json",
        "genesis-envelope.json",
    ):
        p = destination / name
        record["files"]["generated-" + name] = {"path": str(p), "sha256": fsha(p)}
    assert record["files"]["generated-seed.json"]["sha256"] == admitted
    record["files"]["join"] = {"path": str(join_path), "sha256": fsha(join_path)}
    context = Path(record["files"]["seed_context"]["path"])
    context.write_bytes(
        canonicalize(
            {
                "run_id": job.run_id,
                "owner": owner.ss58,
                "hotkey": hot.ss58,
                "coldkey": cold.ss58,
                "request_id": join.request_id,
                "policy_hash": join.policy_hash,
                "job_sha256": record["files"]["job"]["sha256"],
                "join_sha256": fsha(join_path),
                "admission_id": sha256(f"join|{job.run_id}|{cold.ss58}|{join.request_id}".encode()),
                "state": "PROBATION",
            }
        )
    )
    record["files"]["seed_context"]["sha256"] = fsha(context)
    record["owner"] = owner.ss58
    for role in record["roles"].values():
        role["hotkey"] = hot.ss58
    # Snapshot shared source after generator work, before signing the admission record.
    driver = sys.modules["cli_fixture_driver"]
    sources, bundle = cli.effective_bundle(driver, TREE, Path(record["files"]["profile"]["path"]))
    for key, value in (("sources", json.dumps(sources).encode()), ("bundle", bundle)):
        p = Path(record["files"][key]["path"])
        p.write_bytes(value)
        record["files"][key]["sha256"] = fsha(p)
    receipt = envelope_v2.seal(
        owner,
        "Receipt",
        job.run_id,
        {"w": 0, "commit_hash": sha256(canonicalize(record)), "received_round": 10},
        20,
    )
    path.write_bytes(canonicalize({"record": record, "receipt": receipt}))
    invoked = []
    monkeypatch.setattr(cli, "Provider", lambda *a, **kw: invoked.append(True))
    checked = cli.reviewed_launch(path, owner.ss58, 10, "admit")
    assert checked["record"]["roles"]["h0"]["hotkey"] == index["hotkey"]
    assert job.manifest.training.dataset.sample_format == "u32[seq_len+1] token ids"
    assert invoked == []


def test_continue_runtime_profile_preserves_original_materialized_sources(
    contract, monkeypatch, tmp_path
):
    record, path, seal = contract
    original = Path(record["files"]["profile"]["path"])
    record["files"]["source_profile"] = {
        "path": str(original),
        "sha256": fsha(original),
    }
    measured = tmp_path / "measured-profile.json"
    profile = json.loads(original.read_bytes())
    profile["runtime"]["qualified"] = True
    profile["execution_and_transfer_bound_seconds"] = 1
    measured.write_text(json.dumps(profile))
    record["files"]["profile"] = {"path": str(measured), "sha256": fsha(measured)}
    config = Path(record["files"]["config"]["path"])
    cfg = json.loads(config.read_bytes())
    cfg["network_profile_file"] = str(measured)
    config.write_text(json.dumps(cfg))
    record["files"]["config"]["sha256"] = fsha(config)
    record["action"] = "continue"
    seal()

    class RuntimeGateReached(Exception):
        pass

    def gate(*args):
        raise RuntimeGateReached

    monkeypatch.setattr(cli, "runtime_admission", gate)
    with pytest.raises(RuntimeGateReached):
        cli.reviewed_launch(path, OWNER.ss58, 10, "continue")
    record["files"]["source_profile"] = record["files"]["profile"]
    seal()
    with pytest.raises(Reject, match="source_changed"):
        cli.reviewed_launch(path, OWNER.ss58, 10, "continue")


def test_total50_budget_preserves_prior_liability(contract):
    """Total authorization is cumulative; neither a fresh50 nor legacy15."""
    record, _, _ = contract
    profile = json.loads(Path(record["files"]["profile"]["path"]).read_bytes())
    evidence = json.loads(Path(record["files"]["evidence"]["path"]).read_bytes())
    authorization = json.loads(Path(record["files"]["authorization"]["path"]).read_bytes())
    snapshot = json.loads(Path(record["files"]["financial"]["path"]).read_bytes())
    offers = json.loads(Path(record["files"]["offers"]["path"]).read_bytes())
    snapshot.update(
        prior_attempts_usd="9.34",
        active_liabilities_usd="8.35",
        available_credit_usd="19.96",
    )
    plan = cli.admission_plan(
        profile, evidence, authorization, snapshot, offers, qualification=True
    )
    assert plan["admit"] is True and plan["debits_so_far_usd"] == "9.34"
    assert plan["phase_cap_usd"] == "50" and plan["reserve_usd"] == "5"
    snapshot["prior_attempts_usd"] = "49"
    with pytest.raises(Reject, match="cost_ceiling"):
        cli.admission_plan(profile, evidence, authorization, snapshot, offers, qualification=True)


@pytest.mark.parametrize(
    "fault",
    [
        "overlay",
        "future-financial",
        "future-offer",
        "future-observation",
        "invalid-hotkey",
        "unbound-hotkey",
        "foreign-context",
        "wrong-genesis",
    ],
)
def test_original_independent_probes_reject_zero_provider(contract, monkeypatch, fault):
    record, path, seal = contract
    invoked = []
    monkeypatch.setattr(cli, "Provider", lambda *a, **kw: invoked.append(True))
    if fault in ("invalid-hotkey", "unbound-hotkey"):
        record["roles"]["h1"]["hotkey"] = (
            "arbitrary-not-an-SS58-key"
            if fault == "invalid-hotkey"
            else Keypair(bytes([9]) * 32).ss58
        )
    else:
        key = {
            "overlay": "sources",
            "future-financial": "financial",
            "future-offer": "offers",
            "future-observation": "observations",
            "foreign-context": "seed_context",
            "wrong-genesis": "start_state",
        }[fault]
        p = Path(record["files"][key]["path"])
        if fault == "wrong-genesis":
            from hypertrain.auditor.replay import pack_state, unpack_state

            theta, state = unpack_state(p.read_bytes())
            assert state is not None
            next(iter(state.m.values())).add_(1)
            p.write_bytes(pack_state(theta, state))
            job_path = Path(record["files"]["job"]["path"])
            job = json.loads(job_path.read_bytes())
            job["start_state_sha256"] = fsha(p)
            job_path.write_text(json.dumps(job))
            record["files"]["job"]["sha256"] = fsha(job_path)
            ctx_path = Path(record["files"]["seed_context"]["path"])
            ctx = json.loads(ctx_path.read_bytes())
            ctx["job_sha256"] = fsha(job_path)
            ctx_path.write_text(json.dumps(ctx))
            record["files"]["seed_context"]["sha256"] = fsha(ctx_path)
        else:
            value = json.loads(p.read_bytes())
            if fault == "overlay":
                value["experiments/gpu_network_v2/profile.json"] = fsha(
                    TREE / "experiments/gpu_network_v2/profile.json"
                )
            elif fault == "foreign-context":
                value["owner"] = Keypair(bytes([9]) * 32).ss58
            else:
                row = value if fault == "future-financial" else value[0]
                row["verified_unix"] = int(time.time()) + 86400
            p.write_text(json.dumps(value))
        record["files"][key]["sha256"] = fsha(p)
    seal()
    with pytest.raises((Reject, ValueError)):
        cli.main(
            [
                "admit",
                str(path),
                "--owner",
                OWNER.ss58,
                "--beacon",
                "10",
                "--run-dir",
                str(path.parent / "run"),
            ]
        )
    assert invoked == []


def test_sequential_remote_calls_share_absolute_cutoff(tmp_path, monkeypatch):
    from types import SimpleNamespace

    import hypertrain.gpu_ops.remote as remote

    clock = [100.0]
    calls = []
    monkeypatch.setattr(cli.time, "time", lambda: clock[0])

    def run(argv, **kw):
        calls.append(kw["timeout"])
        clock[0] += 2
        return subprocess.CompletedProcess(argv, 0, b"ok", b"")

    monkeypatch.setattr(remote.subprocess, "run", run)
    j = Journal(tmp_path)
    orch = SimpleNamespace(cleanup_started=lambda: False)
    ssh = cli.DeadlineSsh(
        host="127.0.0.1",
        port=22,
        user="root",
        identity=tmp_path / "identity",
        known_hosts=tmp_path / "known",
        journal=j,
        role="h0",
        timeout=60,
    )
    ssh.cutoff = 103.0
    ssh.lifecycle = orch
    local = tmp_path / "blob"
    local.write_bytes(b"bytes")
    ssh.put(local, "/root/blob")
    ssh.get("/root/blob", tmp_path / "fetched")
    with pytest.raises(Reject, match="absolute_deadline"):
        ssh.remote_sha256("/root/blob", tmp_path)
    assert calls == [3.0, 1.0]


def test_real_qualification_staging_depletion_censors_cleanup(contract, tmp_path, monkeypatch):
    import hypertrain.gpu_ops.remote as remote

    record, path, _ = contract
    checked = cli.reviewed_launch(path, OWNER.ss58, 10, "admit")
    run = tmp_path / "transport-run"
    run.mkdir()
    (run / "logs").mkdir()
    j = Journal(run)
    clock = [time.time()]
    first = clock[0] - 297
    j.append("create_intent", role="h0", unix=first)
    orch = cli.Orchestrator(
        checked["config"],
        j,
        cli.Provider("http://127.0.0.1:1", "a" * 32, run, j, False),
        run,
    )
    monkeypatch.setattr(cli.time, "time", lambda: clock[0])
    for method in ("boot", "attach", "trust"):
        monkeypatch.setattr(orch, method, lambda role: None)
    monkeypatch.setattr(orch, "deadline", lambda: first + 3600)
    ssh = remote.Ssh(
        host="127.0.0.1",
        port=22,
        user="root",
        identity=run / "identity",
        known_hosts=run / "known",
        journal=j,
        role="h0",
        timeout=60,
    )
    monkeypatch.setattr(orch, "ssh", lambda role: ssh)
    calls = []

    def transport(argv, **kwargs):
        calls.append(kwargs["timeout"])
        clock[0] += 2
        return subprocess.CompletedProcess(
            argv,
            0,
            "" if kwargs.get("text") else b"",
            "" if kwargs.get("text") else b"",
        )

    monkeypatch.setattr(remote.subprocess, "run", transport)
    cleanup = []
    monkeypatch.setattr(
        orch,
        "cleanup",
        lambda force: cleanup.append({"censored": True, "custody_complete": False, "fresh": True}),
    )
    runtime = cli.NetworkRuntime(orch, checked["profile"])
    with pytest.raises(Reject, match="absolute_deadline"):
        try:
            cli.qualify_role(checked, orch, runtime, "h0")
        except Exception:
            orch.cleanup(force=True)
            raise
    assert calls == [3.0, 1.0]
    assert cleanup == [{"censored": True, "custody_complete": False, "fresh": True}]
    assert not j.last("network_qualification_captured")


@pytest.mark.parametrize(
    "fault",
    [
        "owner",
        "signature",
        "stale",
        "future",
        "one-gpu",
        "quota",
        "seed",
        "image",
        "source",
        "theta-only",
        "bool-clock",
        "parent",
    ],
)
def test_preflight_rejects_before_any_provider(contract, monkeypatch, fault):
    record, path, seal = contract
    invoked = []
    monkeypatch.setattr(cli, "Provider", lambda *a, **kw: invoked.append(True))
    owner = OWNER.ss58
    if fault == "owner":
        owner = Keypair(bytes([9]) * 32).ss58
    elif fault == "signature":
        raw = json.loads(path.read_bytes())
        raw["receipt"]["sig"] = "00" * 64
        path.write_text(json.dumps(raw))
    elif fault == "quota":
        record["quota"]["remaining"] = 126
        seal()
    elif fault == "parent":
        record["parent_pid"] = os.getpid()
        seal()
    elif fault == "seed":
        Path(record["files"]["job"]["path"]).unlink()
    elif fault == "source":
        record["files"]["bundle"]["sha256"] = "00" * 32
        seal()
    else:
        key = (
            "financial"
            if fault in ("stale", "future", "bool-clock")
            else "offers"
            if fault == "one-gpu"
            else "profile"
            if fault == "image"
            else "start_state"
        )
        p = Path(record["files"][key]["path"])
        if fault == "theta-only":
            from hypertrain.auditor.replay import pack_state, unpack_state

            theta, _ = unpack_state(p.read_bytes())
            p.write_bytes(pack_state(theta))
        else:
            value = json.loads(p.read_bytes())
            if fault in ("stale", "future"):
                value["verified_unix"] += -31 if fault == "stale" else 100
            elif fault == "bool-clock":
                value["verified_unix"] = True
            elif fault == "one-gpu":
                value[0]["num_gpus"] = 1
            else:
                value["runtime"]["image_digest"] = None
            p.write_text(json.dumps(value))
        record["files"][key]["sha256"] = fsha(p)
        seal()
    with pytest.raises((Reject, TypeError, ValueError, KeyError)):
        cli.main(
            [
                "admit",
                str(path),
                "--owner",
                owner,
                "--beacon",
                "10",
                "--run-dir",
                str(path.parent / "run"),
            ]
        )
    assert invoked == []


def test_loopback_ready_before_two_gpu_create_and_parent_death(contract, mock_factory, tmp_path):
    record, path, seal = contract
    quotes = json.loads(Path(record["files"]["offers"]["path"]).read_bytes())
    for row in quotes:
        row.update(num_gpus=2, cpu_cores_effective=8, cpu_ram=32768, disk_space=80)
    mock = mock_factory(offers=quotes)
    cfg_path = Path(record["files"]["config"]["path"])
    cfg = json.loads(cfg_path.read_bytes())
    cfg["base_url"] = mock.base
    cfg_path.write_text(json.dumps(cfg))
    record["files"]["config"]["sha256"] = fsha(cfg_path)
    parent = subprocess.Popen(
        [sys.executable, "-c", "import sys;print('READY',flush=True);sys.stdin.read()"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
    )
    assert parent.stdout and select.select([parent.stdout], [], [], 10)[0]
    assert parent.stdout.readline() == b"READY\n"
    record["parent_pid"] = parent.pid
    seal()
    run = tmp_path / "run"
    try:
        checked = cli.reviewed_launch(path, OWNER.ss58, 10, "admit")
        result = cli.launch_action("admit", checked, run)
        assert result["status"] == "ADMITTED_NOT_QUALIFIED"
        j = Journal(run)
        ready = j.last("supervisor_ready")
        assert ready
        kinds = [r["kind"] for r in j.records()]
        assert kinds.index("supervisor_ready") < kinds.index("create_intent")
        assert ready["supervisor_pid"] not in (os.getpid(), parent.pid)
        assert len(j.all("create_intent")) == 2 and len(j.all("receipt")) == 2
        state = mock.state()
        assert state["put_count"] == 2
        assert all(
            e["path"] in ("/api/v0/asks/900/", "/api/v0/asks/901/")
            for e in state["events"]
            if e["method"] == "PUT"
        )
        assert not any("charges" in e["path"] for e in state["events"])
        assert (
            cli.Orchestrator(
                checked["config"],
                j,
                cli.Provider(mock.base, "a" * 32, run, j, False),
                run,
            ).deadline()
            == min(r["unix"] for r in j.all("create_intent")) + 3600
        )
        parent.terminate()
        parent.wait(timeout=10)
        fd = os.pidfd_open(ready["supervisor_pid"])
        try:
            assert select.select([fd], [], [], 30)[0]
        finally:
            os.close(fd)
        assert j.last("supervisor_parent_exit")
        assert len(j.all("absence_confirmed")) == 2
        assert j.last("supervisor_censored")
        assert not j.all("network_execution_intent")
    finally:
        if parent.poll() is None:
            parent.terminate()
            parent.wait(timeout=10)


def test_admission_rejects_nonindependent_ready_before_create(
    contract, mock_factory, tmp_path, monkeypatch
):
    record, path, seal = contract
    mock = mock_factory()
    cfg_path = Path(record["files"]["config"]["path"])
    cfg = json.loads(cfg_path.read_bytes())
    cfg["base_url"] = mock.base
    cfg_path.write_text(json.dumps(cfg))
    record["files"]["config"]["sha256"] = fsha(cfg_path)
    seal()
    checked = cli.reviewed_launch(path, OWNER.ss58, 10, "admit")
    monkeypatch.setattr(
        cli.Orchestrator,
        "ensure_supervisor",
        lambda self: {"supervisor_pid": record["parent_pid"]},
    )
    with pytest.raises(Reject, match="independent_supervisor"):
        cli.launch_action("admit", checked, tmp_path / "run")
    assert mock.state()["put_count"] == 0


def test_insufficient_cutoff_rejects_before_provider(contract, monkeypatch, tmp_path):
    _, path, _ = contract
    checked = cli.reviewed_launch(path, OWNER.ss58, 10, "admit")
    checked["record"]["cutoff_unix"] = int(time.time()) + 30
    invoked = []
    monkeypatch.setattr(cli, "Provider", lambda *a, **kw: invoked.append(True))
    with pytest.raises(Reject, match="cutoff_work_margin"):
        cli.launch_action("admit", checked, tmp_path / "run")
    assert invoked == []


def test_qualification_failure_debits_four_and_cleans_both(contract, tmp_path, monkeypatch):
    record, path, seal = contract
    record["action"] = "qualification"
    financial = Path(record["files"]["financial"]["path"])
    snapshot = json.loads(financial.read_bytes())
    snapshot["instances"] = [{"id": 1}, {"id": 2}]
    financial.write_text(json.dumps(snapshot))
    record["files"]["financial"]["sha256"] = fsha(financial)
    seal()
    checked = cli.reviewed_launch(path, OWNER.ss58, 10, "qualification")
    run = tmp_path / "run"
    run.mkdir()
    (run / "config.json").write_bytes(checked["files"]["config"].read_bytes())
    j = Journal(run)
    j.append("transaction_open", pid=record["parent_pid"], live=False)
    j.append(
        "network_admission_authority",
        owner=record["owner"],
        sources_sha256=fsha(checked["files"]["sources"]),
    )
    j.append("admitted", deadline_unix=time.time() + 3600)
    j.append("supervisor_ready", supervisor_pid=record["parent_pid"])
    for i, role in enumerate(("h0", "h1")):
        j.append("create_intent", role=role)
        j.append("receipt", role=role, instance_id=i + 1)
    cleaned = []
    monkeypatch.setattr(
        cli.Orchestrator, "cleanup", lambda self, force: cleaned.append(set(self.roles))
    )

    def failure(checked, orch, runtime, role):
        runtime.reserve(role, "qualification", "qualification", 2)
        raise Reject("injected_rank_failure")

    monkeypatch.setattr(cli, "qualify_role", failure)
    with pytest.raises(Reject, match="injected_rank_failure"):
        cli.launch_action("qualification", checked, run)
    assert sum(r["executions"] for r in j.all("network_execution_intent")) == 4
    assert cleaned == [{"h0", "h1"}]
    assert not j.last("network_workload_promoted")


@pytest.mark.parametrize("fault", [None, "owner", "instance", "cancel"])
def test_continue_dispatches_signed_graph_not_raw_jobs(
    contract, tmp_path, monkeypatch, fault, capsys
):
    """Transport/result seams synthetic; actual reservation/continuation loop stays integrated."""
    from types import SimpleNamespace

    record, path, seal = contract
    record["action"] = "continue"
    seal()
    # This fixture exercises the action boundary; measured CUDA preflight is separately negative.
    record["action"] = "qualification"
    financial = Path(record["files"]["financial"]["path"])
    snapshot = json.loads(financial.read_bytes())
    snapshot["instances"] = [{"id": 1}, {"id": 2}]
    financial.write_text(json.dumps(snapshot))
    record["files"]["financial"]["sha256"] = fsha(financial)
    seal()
    checked = cli.reviewed_launch(path, OWNER.ss58, 10, "qualification")
    record["action"] = "continue"
    checked["record"]["action"] = "continue"
    run = tmp_path / "run"
    run.mkdir()
    (run / "config.json").write_bytes(checked["files"]["config"].read_bytes())
    j = Journal(run)
    j.append("transaction_open", pid=record["parent_pid"])
    j.append(
        "network_admission_authority",
        owner=record["owner"],
        sources_sha256=fsha(checked["files"]["sources"]),
    )
    j.append("admitted", deadline_unix=time.time() + 3600)
    j.append("supervisor_ready", supervisor_pid=record["parent_pid"])
    for i, role in enumerate(("h0", "h1")):
        j.append("create_intent", role=role)
        j.append("receipt", role=role, instance_id=i + 1)
        j.append("staged", role=role)
        j.append(
            "cli_import_attested",
            role=role,
            sources_sha256=fsha(checked["files"]["sources"]),
            bundle_sha256=fsha(checked["files"]["bundle"]),
        )
        j.append(
            "network_execution_intent",
            role=role,
            name="qualification",
            phase="qualification",
            executions=2,
        )
        j.append("network_qualification_captured", role=role, name="qualification")
        result_path = tmp_path / f"result-{role}.json"
        result_path.write_text("{}")
        receipt_path = tmp_path / f"receipt-{role}.json"
        receipt_path.write_text(
            json.dumps(
                {
                    "passed": True,
                    "root_verified": True,
                    "result_sha256": fsha(result_path),
                    "artifact_manifest_sha256": sha256(json.dumps({}, sort_keys=True).encode()),
                }
            )
        )
        checked["files"][f"result-{role}"] = result_path
        checked["files"][f"receipt-{role}"] = receipt_path
        checked["record"]["roles"][role].update(
            qualification_result=f"result-{role}",
            qualification_receipt=f"receipt-{role}",
        )
    checked["financial"]["instances"] = [{"id": 1}, {"id": 2}]

    graph_path = tmp_path / "graph.json"
    checked["run_id"] = json.loads(checked["files"]["job"].read_bytes())["run_id"]
    graph_path.write_text(
        json.dumps(
            {
                "owner": OWNER.ss58,
                "run_id": checked["run_id"],
                "operation_sources": {
                    "experiments/gpu_network_v2/profile.json": fsha(checked["files"]["profile"])
                },
            }
        )
    )
    checked["files"]["graph"] = graph_path
    checked["files"]["graph_driver"] = TREE / "scripts/network_service_proof.py"
    checked["profile"]["execution_and_transfer_bound_seconds"] = 1
    checked["profile"]["runtime"]["qualified"] = True
    evidence_path = tmp_path / "continue-evidence.json"
    evidence_path.write_text(json.dumps({"root_reviewed_two_host_full_round_transfer": True}))
    checked["files"]["evidence"] = evidence_path
    checked["plan"].update(admit=True, long_workload_allowed=True)
    monkeypatch.setattr(cli, "runtime_admission", lambda *a: 100)
    checked["driver"].Result = SimpleNamespace(
        model_validate_json=lambda raw: SimpleNamespace(
            role="h0" if raw == b"h0" else "h1",
            instance_id=1 if raw == b"h0" else 2,
            machine_id=5000 if raw == b"h0" else 5001,
            config_sha256=sha256(b"config"),
            sources=json.loads(checked["files"]["sources"].read_bytes()),
            environment=SimpleNamespace(image_digest=checked["profile"]["runtime"]["image_digest"]),
        )
    )
    for role in ("h0", "h1"):
        (run / f"cli-qualification-{role}.json").write_bytes(b"config")
        checked["files"][f"result-{role}"].write_bytes(role.encode())
        rp = checked["files"][f"receipt-{role}"]
        value = json.loads(rp.read_bytes())
        value["result_sha256"] = fsha(checked["files"][f"result-{role}"])
        rp.write_text(json.dumps(value))
    rescues = []
    monkeypatch.setattr(
        cli.NetworkRuntime,
        "rescue",
        lambda self, role, **kw: rescues.append(role) or {"files": {}},
    )
    monkeypatch.setattr(
        cli.Orchestrator,
        "ssh",
        lambda self, role: SimpleNamespace(run=lambda *a, **kw: SimpleNamespace(returncode=0)),
    )
    monkeypatch.setattr(cli, "deadline_ssh", lambda orch, role, cutoff: orch.ssh(role))
    monkeypatch.setattr(
        cli.Orchestrator,
        "cleanup",
        lambda self, force: {"all_absent": True, "custody_complete": True},
    )
    calls = []

    def intake(path, runtime, owner, run_id, manifest):
        # External production graph input seam; exact owner/owned-instance negative persists.
        if fault == "owner":
            raise Reject("continuation graph owner/run differs")
        if fault == "instance":
            raise Reject("continuation graph owned instance differs")
        assert runtime.owned_host("h0")["instance_id"] == 1
        assert (owner, run_id) == (OWNER.ss58, checked["run_id"])

    def graph(path, runtime, actual, cancel):
        assert j.last("network_workload_promoted") and not cancel.is_set()
        assert actual is checked and path == graph_path
        calls.append(cancel)
        # Physical launch substitution; actual reservation journal records each kernel once.
        for role in ("h0", "h1"):
            for i in range(56):
                runtime.reserve(role, f"signed-{i}", "workload", 1)
        return {
            "status": "SIGNED_GRAPH_SETTLED_PENDING_RELEASE_REVIEW",
            "run_id": checked["run_id"],
            "graph_sha256": fsha(path),
            "fault_proof_complete": False,
            "trial_finalizations": [{}] * 48,
            "lineage": [{}] * 2,
            "normal_total": 116,
            "per_host": 58,
        }

    driver = SimpleNamespace(
        __file__=str(checked["files"]["graph_driver"]),
        continuation_graph=intake,
        decoder_continue=graph,
    )
    checked["continuation_driver"] = driver
    monkeypatch.setattr(cli.NetworkRuntime, "execute_plan", lambda *a: pytest.fail("raw plan"))
    monkeypatch.setattr(cli.NetworkRuntime, "run_staged", lambda *a: pytest.fail("raw staged"))
    monkeypatch.setattr(cli, "reviewed_launch", lambda *a: checked)
    original_launch = cli.launch_action
    if fault == "cancel":

        def cancelled(*args, **kwargs):
            kwargs["cancel"].set()
            return original_launch(*args, **kwargs)

        monkeypatch.setattr(cli, "launch_action", cancelled)
    argv = [
        "continue",
        str(path),
        "--owner",
        OWNER.ss58,
        "--beacon",
        "10",
        "--run-dir",
        str(run),
    ]
    if fault is not None:
        with pytest.raises(Reject, match="owner/run|owned instance|cancelled"):
            cli.main(argv)
        assert not calls and sum(r["executions"] for r in j.all("network_execution_intent")) == 4
    else:
        cli.main(argv)
        result = json.loads(capsys.readouterr().out)
        assert result["status"] == "SIGNED_GRAPH_SETTLED_PENDING_RELEASE_REVIEW"
        assert len(calls) == 1
        assert sum(r["executions"] for r in j.all("network_execution_intent")) == 116
        assert all(
            sum(r["executions"] for r in j.all("network_execution_intent", role=role)) == 58
            for role in ("h0", "h1")
        )
        assert result["results"]["fault_proof_complete"] is False


def test_signed_operation_reserves_once_and_failed_intent_never_retries(
    contract, tmp_path, monkeypatch
):
    import threading
    from types import SimpleNamespace

    import network_gpu_operation as adapter
    from hypertrain.gpu_ops.network_qualification import sources
    from hypertrain.protocol.messages_v2 import IslandJobV1

    record, path, seal = contract
    checked = cli.reviewed_launch(path, OWNER.ss58, 10, "admit")
    job = IslandJobV1.model_validate_json(checked["files"]["job"].read_bytes())
    profile = checked["profile"]
    profile["runtime"]["qualified"] = True
    source_map = sources(TREE)
    source_map["scripts/network_gpu_operation.py"] = fsha(TREE / "scripts/network_gpu_operation.py")
    cutoff = int(time.time()) + 600
    job = job.model_copy(update={"deadline": cutoff})
    spec = adapter.Operation(
        operation="reference",
        binding="ab" * 32,
        binding_kind="trial",
        hotkey=OWNER.ss58,
        owner=OWNER.ss58,
        role="h0",
        instance_id=1,
        machine_id=100,
        sources=source_map,
        image_digest=profile["runtime"]["image_digest"],
        job=job,
        cutoff=cutoff,
        trace=False,
    )
    run = tmp_path / "physical-operation"
    run.mkdir()
    journal = Journal(run)
    known_hosts = tmp_path / "known_hosts"
    known_hosts.write_text("metadata known host")
    lifecycle = SimpleNamespace(
        j=journal,
        roles=("h0", "h1"),
        host={"h0": {"machine_id": 100}},
        cfg={"cleanup_deadline_unix": time.time() + 3600},
        deadline=lambda: time.time() + 3600,
        dir=run,
        cleanup_started=lambda: False,
        receipt=lambda r: {"instance_id": 1},
        remote_root=lambda r: "/private/root",
    )
    runtime = cli.NetworkRuntime(lifecycle, profile)
    runtime.reserve("h0", "qualify", "qualification", 2)
    runtime.reserve("h1", "qualify", "qualification", 2)
    journal.append("network_workload_promoted")
    journal.append("network_admission_authority", owner=OWNER.ss58)
    journal.append("network_stage_intent", role="h0", sources=source_map)
    journal.append("supervisor_ready", supervisor_pid=1)
    journal.append("ssh_trusted", role="h0", known_hosts_sha256=fsha(known_hosts))
    subject = {
        "role": spec.role,
        "binding": spec.binding,
        "operation": spec.operation,
        "hotkey": spec.hotkey,
        "spec_sha256": sha256(spec.model_dump_json().encode()),
    }
    receipt = envelope_v2.seal(
        OWNER,
        "Receipt",
        job.run_id,
        {
            "w": job.w,
            "commit_hash": sha256(canonicalize(subject)),
            "received_round": 10,
        },
        20,
    )
    runtime.accept_operation(spec, canonicalize(receipt), owner=OWNER.ss58, beacon=10)
    ssh = SimpleNamespace(known_hosts=known_hosts, run=lambda *a: SimpleNamespace(returncode=1))
    monkeypatch.setattr(cli, "deadline_ssh", lambda *a: ssh)
    monkeypatch.setattr(cli.NetworkRuntime, "rescue", lambda *a, **kw: {})
    with pytest.raises(Reject, match="operation_stage_failed"):
        runtime.operation(spec, tmp_path, cancel=threading.Event())
    assert sum(r["executions"] for r in journal.all("network_execution_intent")) == 5
    with pytest.raises(Reject, match="not_retryable"):
        runtime.operation(spec, tmp_path, cancel=threading.Event())
    assert sum(r["executions"] for r in journal.all("network_execution_intent")) == 5


@pytest.mark.parametrize(
    "fault",
    [None, "owner", "instance", "absent", "cancel", "hot", "cold", "coord", "auditor"],
)
def test_signed_graph_actual_input_boundary(contract, tmp_path, fault):
    """Real graph validator/authority refusal, no server, provider or CUDA launch."""
    import threading
    from types import SimpleNamespace

    import network_service_proof as driver
    from hypertrain.gpu_ops.network_qualification import sources
    from hypertrain.miner.core import load_keyfile

    key = tmp_path / "owner.key"
    key.write_text("00" * 32)
    key.chmod(0o600)
    owner = load_keyfile(key).ss58
    record, _, _ = contract
    seed = Path(record["files"][record["roles"]["h0"]["job"]]["path"])
    job = IslandJobV1.model_validate_json(seed.read_bytes())
    body = job.manifest.body()
    body["training"]["coord_pubkey"] = owner
    body["training"]["auditors"] = [owner]
    manifest = RunManifestV2.model_validate(body)
    seed.write_text(
        job.model_copy(update={"manifest": manifest, "run_id": manifest.run_id()}).model_dump_json()
    )
    hot_files, cold_files = [], []
    for i in range(4):
        for files, prefix, offset in ((hot_files, "hot", 1), (cold_files, "cold", 5)):
            p = tmp_path / f"{prefix}-{i}.key"
            p.write_text(bytes([i + offset] * 32).hex())
            p.chmod(0o600)
            files.append(p)
    source_map = sources(TREE)
    source_map["scripts/network_gpu_operation.py"] = fsha(TREE / "scripts/network_gpu_operation.py")
    hosts = {
        r: {
            "instance_id": i + 1,
            "machine_id": i + 10,
            "image_digest": "sha256:" + "ab" * 32,
        }
        for i, r in enumerate(("h0", "h1"))
    }
    raw = {
        "run_id": manifest.run_id(),
        "owner": owner,
        "owner_key_file": str(key),
        "coord_key_file": str(key),
        "auditor_key_files": [str(key)] * 4,
        "identities": [
            {
                "hotkey": load_keyfile(hot_files[i]).ss58,
                "coldkey": load_keyfile(cold_files[i]).ss58,
                "role": f"h{i // 2}",
                "hot_key_file": str(hot_files[i]),
                "cold_key_file": str(cold_files[i]),
                "origin_ids": ["region:test"],
                "admission_units": 1,
            }
            for i in range(4)
        ],
        "hosts": hosts,
        "state_dir": str(tmp_path / "production"),
        "evidence": str(tmp_path / "evidence"),
        "master_https": "https://master.test:8443",
        "relay_https": "https://relay.test:8444",
        "metagraph_https": "https://metagraph.test:8445",
        "listen_host": "127.0.0.1",
        "listen_port": 8443,
        "ca": str(key),
        "cert": str(key),
        "tls_key": str(key),
        "admin_token_file": str(key),
        "qualification_receipt": str(key),
        "economic_authority": str(key),
        "operation_sources": source_map,
        "private_files": {str(p): fsha(p) for p in [key, *hot_files, *cold_files]},
    }
    if fault == "owner":
        raw["owner"] = "wrong"
    if fault == "instance":
        raw["hosts"] = {r: dict(v) for r, v in hosts.items()}
        raw["hosts"]["h0"]["instance_id"] = 999
    if fault in ("hot", "cold"):
        raw["identities"][3][fault + "_key_file"] = str(key)
    if fault == "coord":
        raw["coord_key_file"] = str(hot_files[0])
    if fault == "auditor":
        raw["auditor_key_files"][3] = str(hot_files[0])
    path = tmp_path / "graph.json"
    path.write_text(json.dumps(raw))
    runtime = SimpleNamespace(
        lifecycle=SimpleNamespace(roles=("h0", "h1"), deadline=lambda: time.time() + 3600),
        owned_host=hosts.__getitem__,
    )
    if fault in ("owner", "instance", "hot", "cold", "coord", "auditor"):
        with pytest.raises(driver.DriverError, match="owner/run|owned instance|signing identity"):
            driver.continuation_graph(path, runtime, owner, raw["run_id"], manifest)
    elif fault in ("absent", "cancel"):
        cancel = threading.Event()
        if fault == "cancel":
            cancel.set()
        checked = {
            "record": {
                "owner": owner,
                "cutoff_unix": time.time() + 3600,
                "roles": {"h0": {"job": "seed"}},
            },
            "run_id": raw["run_id"],
            "files": {"seed": seed},
        }
        with pytest.raises(driver.DriverError, match="production store absent|cancelled"):
            driver.decoder_continue(path, runtime, checked, cancel)
    else:
        assert (
            driver.continuation_graph(path, runtime, owner, raw["run_id"], manifest).hosts == hosts
        )


def _backend_fixture():
    """Reuse original synthetic rescued-file fixture and real bootstrap writer."""
    spec = importlib.util.spec_from_file_location(
        "continuation_backend_fixture",
        TREE / "tests/challenge/test_backend_authority_v2.py",
    )
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


backend_fixture = _backend_fixture()
network = backend_fixture.network
continuation_authority = backend_fixture.authority


def test_continue_authentic_flat_store_intake(continuation_authority, tmp_path):
    from types import SimpleNamespace

    import network_service_proof as driver

    store, manifest, configs, results, raw = continuation_authority
    store._bootstrap_backend_v2(manifest, configs, results, raw)
    backend_fixture.register(store, manifest)
    accepted = store._record_v2(
        manifest.run_id(),
        "qualification",
        manifest.training.reference_spec.image_digest,
    )
    assert "evidence" not in accepted and accepted["configs"] == [fsha(p) for p in configs]
    receipt = tmp_path / "accepted-receipt.json"
    receipt.write_bytes(canonicalize(accepted["receipt"]))
    for role, config in zip(("h0", "h1"), configs, strict=True):
        (tmp_path / f"cli-qualification-{role}.json").write_bytes(config.read_bytes())
    hosts = {
        f"h{i}": {
            "instance_id": i + 1,
            "machine_id": i + 10,
            "image_digest": manifest.training.reference_spec.image_digest,
        }
        for i in range(2)
    }
    runtime = SimpleNamespace(lifecycle=SimpleNamespace(dir=tmp_path), owned_host=hosts.__getitem__)
    graph = SimpleNamespace(run_id=manifest.run_id(), qualification_receipt=receipt)
    checked = {
        "record": {"roles": {f"h{i}": {"qualification_result": f"r{i}"} for i in range(2)}},
        "files": {f"r{i}": p for i, p in enumerate(results)},
    }
    driver.continuation_qualification(store, manifest, graph, runtime, checked)
    (tmp_path / "cli-qualification-h1.json").write_bytes(configs[0].read_bytes())
    with pytest.raises(driver.DriverError, match="owned instance"):
        driver.continuation_qualification(store, manifest, graph, runtime, checked)


@pytest.mark.parametrize("fault", [None, "empty", "hash", "kind"])
def test_continue_actual_done_result_custody(tmp_path, fault):
    import network_service_proof as driver

    j = Journal(tmp_path)
    for role in ("h0", "h1"):
        for kind, count in (
            ("reference", 24),
            ("probe", 24),
            ("live", 4),
            ("audit", 4),
        ):
            for i in range(count):
                name = kind + str(i)
                j.append(
                    "network_execution_intent",
                    role=role,
                    name=name,
                    phase="workload",
                    executions=1,
                )
                j.append(
                    "network_operation_started",
                    role=role,
                    name=name,
                    operation="live" if fault == "kind" else kind,
                )
                if fault != "empty":
                    j.append(
                        "network_operation_done",
                        role=role,
                        name=name,
                        result_sha256="" if fault == "hash" else "ab" * 32,
                        tar_sha256="cd" * 32,
                    )
    if fault is not None:
        with pytest.raises(driver.DriverError, match="coverage|hash absent"):
            driver.continuation_results(j, ("h0", "h1"))
    else:
        results = driver.continuation_results(j, ("h0", "h1"))
        assert len(results) == 112 and all(r["result_sha256"] and r["tar_sha256"] for r in results)


def test_continue_reviewed_launch_authentic_store_private_identity(
    contract, continuation_authority, tmp_path, monkeypatch
):
    """Real CLI intake joins original store authority and private signers, never launch."""
    from decimal import Decimal
    from types import SimpleNamespace

    import uvicorn

    from hypertrain.gpu_ops import network_qualification as engine
    from hypertrain.miner.admission import sign_join
    from hypertrain.protocol.messages_v2 import HardwareHint

    record, record_path, _ = contract
    store, original, configs, results, _ = continuation_authority
    owner = backend_fixture.service.OWNER
    files = record["files"]

    def put(name, value):
        path = tmp_path / (name + ".json")
        path.write_bytes(canonicalize(value))
        files[name] = {"path": str(path), "sha256": fsha(path)}
        return path

    job = IslandJobV1.model_validate_json(Path(files["job"]["path"]).read_bytes())
    body = job.manifest.body()
    body["network"] = original.network.model_dump(mode="json")
    body["training"]["coord_pubkey"] = original.training.coord_pubkey
    body["training"]["auditors"] = original.training.auditors
    manifest = RunManifestV2.model_validate(body)
    job = job.model_copy(update={"manifest": manifest, "run_id": manifest.run_id()})
    job_path = put("job", job.model_dump(mode="json"))
    record.update(action="continue", owner=owner.ss58)
    profile = json.loads(Path(files["profile"]["path"]).read_bytes())
    profile["runtime"]["qualified"] = True
    profile["execution_and_transfer_bound_seconds"] = 1
    bounds = cli.artifact_budget(profile)
    profile["artifacts"].update(
        image_and_dependency_bytes=1000,
        staging_bytes_per_host=1000,
        rescue_bytes_per_second=bounds["rescue_min_bytes_per_second"],
    )
    evidence = {
        "profile_model": profile["model"],
        "layout": profile["layout"],
        "device": "cuda",
        "torch": "2.14.0+cu130",
        "full_round_transfer_bound_seconds": 1,
        "no_slower_hosts": True,
        "image_digest": profile["runtime"]["image_digest"],
        "driver_allowlist": profile["runtime"]["driver_allowlist"],
        "python": profile["runtime"]["remote_python"],
        "cuda": profile["runtime"]["cuda"],
        "full_round_artifacts_sha256": sha256(
            canonicalize([json.loads(p.read_bytes())["rounds"] for p in results])
        ),
        "outer_tape_verified": True,
        "measured_rescue_bytes_per_second": profile["artifacts"]["rescue_bytes_per_second"],
        "image_and_dependency_bytes": 1000,
        "staging_bytes_per_host": 1000,
    }
    put("evidence", evidence)  # Synthetic test-only runtime metadata, never production authority.
    profile["runtime_evidence_sha256"] = sha256(canonicalize(evidence))
    profile_path = put("profile", profile)
    source_profile = TREE / "experiments/gpu_network_v2/profile.json"
    files["source_profile"] = {
        "path": str(source_profile),
        "sha256": fsha(source_profile),
    }
    qualification_driver = sys.modules["cli_fixture_driver"]
    sources, bundle = cli.effective_bundle(qualification_driver, TREE, source_profile)
    put("sources", sources)
    bundle_path = Path(files["bundle"]["path"])
    bundle_path.write_bytes(bundle)
    files["bundle"]["sha256"] = fsha(bundle_path)
    cfg = json.loads(Path(files["config"]["path"]).read_bytes())
    cfg["network_profile_file"] = str(profile_path)
    for i, host in enumerate(cfg["hosts"]):
        host["machine_id"] = i + 10
    put("config", cfg)
    offers = json.loads(Path(files["offers"]["path"]).read_bytes())
    for i, offer in enumerate(offers):
        offer["machine_id"] = i + 10
    put("offers", offers)
    financial = json.loads(Path(files["financial"]["path"]).read_bytes())
    financial["instances"] = [{"id": 1}, {"id": 2}]
    put("financial", financial)
    cost = cli.budget.evaluate(
        snapshot0_credit=Decimal("20"),
        current_credit=Decimal("20"),
        offers=offers,
        hard_deadline_seconds=3600,
        disk_gb=80,
        egress_gb=30,
        phase_cap=Decimal("50"),
        reserve=Decimal("5"),
    )
    put("plan", {**cost, "long_workload_allowed": True})
    run = tmp_path / "preflight-runtime"
    run.mkdir()
    j = Journal(run)
    for i, (config_path, result_path) in enumerate(zip(configs, results, strict=True)):
        role = f"h{i}"
        hot, cold = (
            backend_fixture.service.HOT[i * 2],
            backend_fixture.service.COLD[i * 2],
        )
        join = sign_join(
            hot,
            cold,
            run_id=manifest.run_id(),
            request_id=f"{i + 1:064x}",
            expires_beacon=20,
            policy_hash=manifest.network.admission_policy_hash,
            hardware_hint=HardwareHint(device_name="test", device_count=2, driver="595.84"),
        )
        join_path = put("join-" + role, join.model_dump(mode="json"))
        context_name = "seed-context-" + role
        put(
            context_name,
            {
                "run_id": manifest.run_id(),
                "owner": owner.ss58,
                "hotkey": hot.ss58,
                "coldkey": cold.ss58,
                "request_id": join.request_id,
                "policy_hash": join.policy_hash,
                "job_sha256": fsha(job_path),
                "join_sha256": fsha(join_path),
                "admission_id": sha256(
                    f"join|{manifest.run_id()}|{cold.ss58}|{join.request_id}".encode()
                ),
                "state": "PROBATION",
            },
        )
        record["roles"][role].update(
            hotkey=hot.ss58,
            join="join-" + role,
            seed_context=context_name,
            qualification_result="result-" + role,
        )
        qualification = engine.Qualification.model_validate_json(config_path.read_bytes())
        lifecycle_receipts = json.loads(qualification.lifecycle_receipts.read_bytes())
        lifecycle_receipts["image_digest"] = profile["runtime"]["image_digest"]
        qualification.lifecycle_receipts.write_bytes(canonicalize(lifecycle_receipts))
        qualification = qualification.model_copy(
            update={
                "profile": profile_path,
                "profile_sha256": fsha(profile_path),
                "seed_job": job_path,
                "hotkey": hot.ss58,
                "registry_manifest": Path(files["registry"]["path"]),
                "image_digest": profile["runtime"]["image_digest"],
                "lifecycle_receipts_sha256": fsha(qualification.lifecycle_receipts),
            }
        )
        config_path.write_text(qualification.model_dump_json())
        (run / f"cli-qualification-{role}.json").write_bytes(config_path.read_bytes())
        result = json.loads(result_path.read_bytes())
        result.update(config_sha256=fsha(config_path), profile_sha256=fsha(profile_path))
        result["environment"]["image_digest"] = profile["runtime"]["image_digest"]
        result_path.write_bytes(canonicalize(result))
        files["result-" + role] = {
            "path": str(result_path),
            "sha256": fsha(result_path),
        }
        j.append("receipt", role=role, instance_id=i + 1)
    accepted_evidence = {
        "run_id": manifest.run_id(),
        "backend": "cuda",
        "reference_hash": sha256(
            canonicalize(manifest.training.reference_spec.model_dump(mode="json"))
        ),
        "layout_hash": manifest.training.reference_spec.layout.model_dump_json(),
        "configs": [fsha(p) for p in configs],
        "results": [fsha(p) for p in results],
        "summaries": [
            fsha(p.parent / f"round-{w}/published/rank-{r}/summary.json")
            for p in results
            for w in range(2)
            for r in range(2)
        ],
    }
    accepted_receipt = envelope_v2.seal(
        owner,
        "Receipt",
        manifest.run_id(),
        {
            "w": 0,
            "commit_hash": sha256(canonicalize(accepted_evidence)),
            "received_round": 1,
        },
        100,
    )
    store._bootstrap_backend_v2(manifest, configs, results, canonicalize(accepted_receipt))
    backend_fixture.register(store, manifest)
    assert "evidence" not in store._record_v2(
        manifest.run_id(),
        "qualification",
        manifest.training.reference_spec.image_digest,
    )

    def key(name, seed):
        path = tmp_path / (name + ".key")
        path.write_bytes(seed)
        path.chmod(0o600)
        return path

    owner_path = key("private-owner", b"\x71" * 32)
    coord_path = key("private-coord", bytes(range(32)))
    auditor_paths = [
        key("private-auditor-0", bytes(31) + b"\x01"),
        key("private-auditor-1", b"\x72" * 32),
    ] * 2
    identities = [
        {
            "hotkey": backend_fixture.service.HOT[i].ss58,
            "coldkey": backend_fixture.service.COLD[i].ss58,
            "role": f"h{i // 2}",
            "hot_key_file": str(key(f"private-hot-{i}", bytes([80 + i]) * 32)),
            "cold_key_file": str(key(f"private-cold-{i}", bytes([90 + i]) * 32)),
            "origin_ids": ["test-only"],
            "admission_units": 1,
        }
        for i in range(4)
    ]
    accepted_path = put("accepted-qualification", accepted_receipt)
    hosts = {
        f"h{i}": {
            "instance_id": i + 1,
            "machine_id": i + 10,
            "image_digest": profile["runtime"]["image_digest"],
        }
        for i in range(2)
    }
    operation_sources = dict(sources)
    operation_sources["scripts/network_gpu_operation.py"] = fsha(
        TREE / "scripts/network_gpu_operation.py"
    )
    operation_sources["experiments/gpu_network_v2/profile.json"] = fsha(profile_path)
    graph_input = {
        "run_id": manifest.run_id(),
        "owner": owner.ss58,
        "owner_key_file": str(owner_path),
        "coord_key_file": str(coord_path),
        "auditor_key_files": [str(p) for p in auditor_paths],
        "identities": identities,
        "hosts": hosts,
        "state_dir": str(store.state_dir),
        "evidence": str(tmp_path / "new-evidence"),
        "master_https": "https://master.test:8443",
        "relay_https": "https://relay.test:8444",
        "metagraph_https": "https://metagraph.test:8445",
        "listen_host": "127.0.0.1",
        "listen_port": 8443,
        "ca": str(owner_path),
        "cert": str(owner_path),
        "tls_key": str(owner_path),
        "admin_token_file": str(owner_path),
        "qualification_receipt": str(accepted_path),
        "economic_authority": str(accepted_path),
        "operation_sources": operation_sources,
        "private_files": {
            str(p): fsha(p)
            for p in [
                owner_path,
                coord_path,
                *auditor_paths,
                accepted_path,
                *(Path(i[k]) for i in identities for k in ("hot_key_file", "cold_key_file")),
            ]
        },
    }
    graph_path = put("graph", graph_input)
    files["graph_driver"] = {
        "path": str(TREE / "scripts/network_service_proof.py"),
        "sha256": fsha(TREE / "scripts/network_service_proof.py"),
    }
    lifecycle = cli.Orchestrator(cfg, j, SimpleNamespace(pace={}, last={}), run)
    runtime = cli.NetworkRuntime(lifecycle, profile)
    prohibited = []

    def forbidden(*args, **kwargs):
        prohibited.append(True)
        pytest.fail("preflight crossed provider/SSH/server boundary")

    monkeypatch.setattr(cli.Provider, "call", forbidden)
    monkeypatch.setattr(cli.Ssh, "run", forbidden)
    monkeypatch.setattr(cli.Ssh, "put", forbidden)
    monkeypatch.setattr(uvicorn.Server, "run", forbidden)

    def intake():
        for name in ("financial", "offers", "observations"):
            value = json.loads(Path(files[name]["path"]).read_bytes())
            for row in value if isinstance(value, list) else [value]:
                row.update(
                    verified_unix=int(time.time()),
                    requested_unix=int(time.time()),
                    received_unix=int(time.time()),
                )
            put(name, value)
        receipt = envelope_v2.seal(
            owner,
            "Receipt",
            manifest.run_id(),
            {"w": 0, "commit_hash": sha256(canonicalize(record)), "received_round": 10},
            20,
        )
        record_path.write_bytes(canonicalize({"record": record, "receipt": receipt}))
        return cli.reviewed_launch(record_path, owner.ss58, 10, "continue")

    checked = intake()
    driver = checked["continuation_driver"]
    assert Path(driver.__file__).resolve() == TREE / "scripts/network_service_proof.py"
    assert "jobs" not in checked["files"] and checked["record"]["action"] == "continue"
    graph = driver.continuation_graph(graph_path, runtime, owner.ss58, checked["run_id"], manifest)
    driver.continuation_qualification(store, manifest, graph, runtime, checked)
    identities[3]["hot_key_file"] = str(owner_path)
    put("graph", graph_input)
    rejected = intake()
    driver = rejected["continuation_driver"]
    with pytest.raises(driver.DriverError, match="miner signing identity"):
        driver.continuation_graph(graph_path, runtime, owner.ss58, rejected["run_id"], manifest)
    identities[3]["hot_key_file"] = str(tmp_path / "private-hot-3.key")
    put("graph", graph_input)
    checked = intake()
    driver = checked["continuation_driver"]
    graph = driver.continuation_graph(graph_path, runtime, owner.ss58, checked["run_id"], manifest)
    (run / "cli-qualification-h1.json").write_bytes(configs[0].read_bytes())
    with pytest.raises(driver.DriverError, match="owned instance"):
        driver.continuation_qualification(store, manifest, graph, runtime, checked)
    assert not prohibited
    assert not j.all("network_execution_intent") and not j.last("network_workload_promoted")


@pytest.mark.parametrize("primary_error", [False, True])
@pytest.mark.parametrize("join_error", [False, True])
def test_continue_shutdown_exception_closes_store(primary_error, join_error):
    import threading
    from types import SimpleNamespace

    import network_service_proof as driver

    closed = []
    store = SimpleNamespace(
        _lease_guards_v2={},
        close_v2_notifications=lambda: closed.append("notifications"),
        _db=SimpleNamespace(close=lambda: closed.append("database")),
    )
    server = SimpleNamespace(should_exit=False, force_exit=False)

    def join(**kwargs):
        if join_error:
            raise RuntimeError("HTTPS join failed")

    thread = SimpleNamespace(join=join, is_alive=lambda: True)
    error = ValueError("primary graph error")
    expected = ValueError if primary_error else RuntimeError if join_error else driver.DriverError
    with pytest.raises(expected) as caught:
        try:
            if primary_error:
                raise error
        finally:
            driver.continuation_close(
                store, server, thread, None, threading.Event(), threading.Event()
            )
    assert closed == ["notifications", "database"] and server.should_exit
    assert server.force_exit is (not join_error)
    if primary_error:
        assert caught.value is error and "HTTPS" in error.__notes__[0]


@pytest.mark.parametrize("cancelled", [False, True])
def test_continue_private_staging_shared_cutoff(tmp_path, monkeypatch, cancelled):
    import threading
    from types import SimpleNamespace

    import hypertrain.gpu_ops.remote as remote

    clock, calls = [100.0], []
    cancel = threading.Event()
    (tmp_path / "logs").mkdir()
    j = Journal(tmp_path)
    ssh = remote.Ssh("127.0.0.1", 22, "root", tmp_path / "key", tmp_path / "known", j, "h0", 60)
    lifecycle = SimpleNamespace(
        dir=tmp_path,
        j=j,
        cfg={"cleanup_deadline_unix": 403},
        deadline=lambda: 1000,
        cleanup_started=lambda: False,
        ssh=lambda role: ssh,
        remote_root=lambda role: "/private",
    )
    runtime = cli.NetworkRuntime(lifecycle, {})
    monkeypatch.setattr(cli.time, "time", lambda: clock[0])

    def transport(argv, **kwargs):
        calls.append(kwargs["timeout"])
        clock[0] += 2
        if cancelled:
            cancel.set()
        return subprocess.CompletedProcess(
            argv,
            0,
            "" if kwargs.get("text") else b"",
            "" if kwargs.get("text") else b"",
        )

    monkeypatch.setattr(remote.subprocess, "run", transport)
    p = tmp_path / "blob"
    p.write_bytes(b"input")
    with pytest.raises(Reject, match="absolute_deadline"):
        runtime.stage_context("h0", {"context/blob": p}, "context", "test", 900, cancel)
    assert calls == ([3] if cancelled else [3, 1])


def test_continue_round_one_late_predecessor(contract, tmp_path, monkeypatch):
    import threading
    from types import SimpleNamespace

    import httpx

    import network_service_proof as driver

    record, _, _ = contract
    job = IslandJobV1.model_validate_json(
        Path(record["files"][record["roles"]["h0"]["job"]]["path"]).read_bytes()
    )
    coord = Keypair(b"z" * 32)
    body = job.manifest.body()
    body["training"]["coord_pubkey"] = coord.ss58
    manifest = RunManifestV2.model_validate(body)
    keys = [Keypair(bytes([i + 1] * 32)) for i in range(4)]
    roster = [
        {
            "hotkey": k.ss58,
            "slot": i,
            "q_i": "3f800000",
            "admission_id": f"{i + 1:064x}",
            "coldkey_group": k.ss58,
            "state": "ACTIVE",
            "eligible_weight": 4194304,
        }
        for i, k in enumerate(keys)
    ]
    settled, replays, built = [], [], []
    finals = {}

    def respond(request):
        if request.url.path.endswith("aggregate"):
            w = int(request.url.path.split("/")[-2])
            return httpx.Response(
                200,
                json={
                    "tape_hash": f"{w + 1:064x}",
                    "prev_state": "22" * 32,
                    "out_state": "23" * 32,
                    "theta_hash": "24" * 32,
                },
            )
        if request.url.path.endswith("finalize"):
            w = int(request.url.path.split("/")[-2])
            finals[w] = json.loads(request.content)
        return httpx.Response(200, json={})

    def build(w):
        if w:
            assert settled == [0] and 0 in finals
        built.append(w)
        opening = driver.RoundOpenV2(
            w=w,
            prev_final_hash=sha256(canonicalize(finals[0]["body"])) if w else "0" * 64,
            theta_hash="11" * 32,
            outer_state_hash="0" * 64,
            center_hash="0" * 64,
            roster_hash=sha256(canonicalize(roster)),
            honeypot_commit="0" * 64,
            d_open=2 + w * 10,
            d_assign=3 + w * 10,
            d_commit=4 + w * 10,
            d_audit=5 + w * 10,
            d_upload=6 + w * 10,
            d_final=10 + w * 10,
            contract_version=2,
            policy_hashes={
                k: getattr(manifest.network, k) for k in driver.PolicyHashes.model_fields
            },
            registry_epoch=0,
            start_state_index_hash="12" * 32,
            audit_mode="anchored-full",
            roster=roster,
        )

        def finality():
            settled.append(w)
            return {
                "disposition": "SETTLED",
                "unresolved_audits": 0,
                "unresolved_disputes": 0,
            }

        return {
            "round_open": envelope_v2.seal(coord, "RoundOpenV2", manifest.run_id(), opening, 30),
            "miners": lambda: [
                {
                    "operation": {},
                    "receipt": {},
                    "key": k,
                    "pins": {},
                    "artifacts": tmp_path,
                }
                for k in keys
            ],
            "auditors": [],
            "accepted_inputs": lambda: [],
            "accepted_finality": finality,
        }

    entries = [SimpleNamespace(hotkey=k.ss58, probation=False, weight_units=4194304) for k in keys]
    monkeypatch.setattr(
        driver.TapeV2,
        "from_bytes",
        lambda raw: SimpleNamespace(
            body=SimpleNamespace(allocation=SimpleNamespace(entries=entries))
        ),
    )
    monkeypatch.setattr(driver.AggregationPolicyV2, "model_validate_json", lambda raw: None)
    monkeypatch.setattr(driver, "network_inputs", lambda raw: raw)

    def replay(*args, **kwargs):
        replays.append(kwargs["predecessor_tape_hash"])
        return SimpleNamespace(to_bytes=lambda: b"outer")

    monkeypatch.setattr(driver, "replay_tape", replay)
    monkeypatch.setattr(driver, "decoder_operation", lambda *a: None)
    monkeypatch.setattr(driver, "write_network_checkpoint", lambda *a: None)
    monkeypatch.setattr(driver, "verify_network_checkpoint", lambda *a: [])
    with httpx.Client(
        base_url="https://metadata.test", transport=httpx.MockTransport(respond)
    ) as c:
        lineage = driver.decoder_live_graph(
            c,
            None,
            manifest,
            [lambda: build(0), lambda: build(1)],
            coord,
            {},
            lambda n: None,
            SimpleNamespace(get=lambda digest: b"outer"),
            tmp_path / "checkpoint",
            threading.Event(),
        )
    assert built == settled == [0, 1]
    assert replays == ["0" * 64, f"{1:064x}"]
    assert lineage[1]["predecessor_tape_hash"] == lineage[0]["tape_hash"]
