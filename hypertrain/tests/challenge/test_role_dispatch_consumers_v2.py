"""Private service launch forwarding; metadata dispatch evidence, no CUDA/training."""

from __future__ import annotations

import importlib.util
import json
import shutil
import sys
import tarfile
import threading
from dataclasses import replace
from pathlib import Path
from types import SimpleNamespace

import pytest

from hypertrain.challenge.store import ChallengeError
from hypertrain.protocol.envelope import body_digest
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages_v2 import EscrowLock

spec = importlib.util.spec_from_file_location(
    "role_backend_fixture", Path(__file__).parent / "test_backend_authority_v2.py"
)
assert spec is not None and spec.loader is not None
backend = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = backend
spec.loader.exec_module(backend)
authority = backend.authority
network = backend.network


def rescued_publication(job, directory, custody_name):
    """Actual validated metadata custody publication, not a trained trajectory."""
    from hypertrain.auditor.replay import pack_state, tensor_root, unpack_state
    from hypertrain.miner.island_launch import validate_artifacts
    from hypertrain.protocol.hashing import MerkleTree, sha256_hex
    from hypertrain.trainer.compress import state_hash
    from hypertrain.trainer.config import TrainConfig
    from hypertrain.trainer.island import IslandAssignment
    from hypertrain.trainer.loop import _make_leaf
    from hypertrain.trainer.optim import init_state

    custody = directory / custody_name
    backend.saved_geometry(job.manifest, backend.service.HOT[0].ss58, custody, published=True)
    published = custody / "published"
    for relative in job.object_paths.values():
        shutil.copyfile(directory / relative, published / relative)
    cfg = TrainConfig.from_manifest_v2(job.manifest)
    theta, carry = unpack_state((directory / job.object_paths["start_state"]).read_bytes())
    state = init_state(cfg.inner, theta, carry if cfg.inner.state_policy == "carry" else None)
    assignment = IslandAssignment(
        job.run_id,
        job.w,
        tuple(job.sample_ids),
        job.global_step0,
        job.manifest.training.reference_spec.layout.n_gpus,
    )
    leaves = [
        _make_leaf(
            cfg,
            assignment,
            t,
            theta,
            replace(state, step=job.global_step0 + t),
            0.0,
            0.0,
        ).preimage
        for t in range(0, cfg.inner.H + 1, cfg.inner.J)
    ]
    for rank in range(job.manifest.training.reference_spec.layout.n_gpus):
        target = published / f"rank-{rank}"
        for p in leaves:
            (target / "checkpoints" / f"{p.t}.safetensors").write_bytes(
                pack_state(theta, replace(state, step=job.global_step0 + p.t))
            )
        final = replace(state, step=job.global_step0 + cfg.inner.H)
        (target / "state.safetensors").write_bytes(pack_state(theta, final))
        (target / "leaves.json").write_bytes(
            canonicalize([p.model_dump(mode="json") for p in leaves])
        )
        summary = json.loads((target / "summary.json").read_bytes())
        ef, _ = unpack_state((target / "ef.safetensors").read_bytes())
        summary["backend"] = "cuda"
        summary["job_hash"] = job.digest()
        summary["commitments"] = {
            "leaves_root": MerkleTree([bytes.fromhex(p.digest()) for p in leaves]).root.hex(),
            "final_theta_hash": state_hash(theta),
            "state_root": tensor_root(theta, final),
            "ef_out_hash": state_hash(ef),
            "delta_hash": sha256_hex((target / "delta.bin").read_bytes()),
            "leaves": [p.digest() for p in leaves],
        }
        (target / "summary.json").write_bytes(canonicalize(summary))
    return validate_artifacts(job, published)


def real_adapter(monkeypatch, operation, directory):
    from hypertrain.gpu_ops.journal import Journal

    spec = importlib.util.spec_from_file_location(
        "consumer_real_runtime",
        Path(__file__).parents[2] / "experiments/gpu_network_v2/orchestrate.py",
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    runtime = object.__new__(module.NetworkRuntime)
    runtime.lifecycle = SimpleNamespace(cleanup_started=lambda: False)
    journal_directory = directory / "runtime-journal"
    journal_directory.mkdir()
    runtime.journal = Journal(journal_directory)
    # Only remote/provider execution is replaced; real adapter owns installation.
    monkeypatch.setattr(runtime, "operation", operation)
    return runtime


def accepted_spec(kind, job, context):
    from hypertrain.protocol.hashing import sha256_hex

    binding = body_digest(context)
    hotkey = context["hotkey"]
    spec = SimpleNamespace(
        job=job,
        trace=kind != "reference",
        operation=kind,
        hotkey=hotkey,
        binding=binding,
        role="h0",
        cutoff=job.deadline,
    )
    name = "op-" + sha256_hex((job.run_id + kind + hotkey + binding + job.digest()).encode())
    return spec, name


def record_rescue(runtime, spec, directory, name, artifacts):
    """Model original rescued custody bytes and journal, not canonical local output."""
    from hypertrain.gpu_ops.journal import fsha

    archive = directory / (name + ".tar")
    with tarfile.open(archive, "w") as saved:
        saved.add(artifacts.directory, arcname="out/" + name + "/published", recursive=True)
    digest = fsha(archive)
    runtime.journal.append("network_rescued", role=spec.role, tar_sha256=digest, tar=str(archive))
    runtime.journal.append("network_operation_done", role=spec.role, name=name, tar_sha256=digest)
    return artifacts


def test_real_adapter_reference_canonical_publication(network, monkeypatch):
    assert network.join().status_code == 200
    _, admission, _ = network.store._services(network.manifest.run_id())
    row = network.store._db.execute("SELECT * FROM admissions_v2").fetchone()
    lock = EscrowLock(
        operation_id="fc" * 32,
        owner=backend.service.COLD[0].ss58,
        units=1000,
        origin_ids=[network.origin_ids[0]],
        admission_id=row["admission_id"],
        dispute_id=None,
        kind="LOCK_ADMISSION",
    )
    admission.lock(
        canonicalize(network.signed(backend.service.COLD[0], "EscrowLock", lock)),
        now=network.now,
    )
    network.push(network.now + 1)
    admission.challenge(row["admission_id"], now=network.now)
    # Test-only remote CUDA-labelled metadata seam, not GPU qualification.
    admission.backend = "cuda"
    calls = []

    def operation(spec, directory, *, cancel):
        assert cancel is not None and not cancel.is_set()
        calls.append(spec.job)
        name = "op-" + backend.sha256_hex(
            (
                spec.job.run_id + spec.operation + spec.hotkey + spec.binding + spec.job.digest()
            ).encode()
        )
        artifacts = rescued_publication(spec.job, directory, name)
        return record_rescue(runtime, spec, directory, name, artifacts)

    runtime = real_adapter(monkeypatch, operation, network.store.state_dir)

    def factory(kind, job, directory, context):
        assert kind == "reference" and context["challenge_hash"] == body_digest(
            context["challenge"]
        )
        spec, _ = accepted_spec(kind, job, context)
        return runtime.launch_adapter(spec)

    network.store._role_launch = factory
    result = network.store.trial_reference_v2(network.manifest.run_id(), row["admission_id"])
    assert result["reference_commitment"] and len(calls) == 1
    assert network.store._db.execute("SELECT reference_json FROM admissions_v2").fetchone()[0]


def test_real_adapter_reference_rejects_wrong_custody_path(network, monkeypatch):
    from hypertrain.gpu_ops.launcher import Reject

    original = rescued_publication

    def wrong_path(job, directory, name):
        return original(job, directory, name + "-wrong")

    monkeypatch.setattr(sys.modules[__name__], "rescued_publication", wrong_path)
    with pytest.raises(Reject, match="^operation_canonical_custody_path$"):
        test_real_adapter_reference_canonical_publication(network, monkeypatch)
    assert (
        network.store._db.execute("SELECT reference_json FROM admissions_v2").fetchone()[0] is None
    )


def test_real_adapter_audit_canonical_publication(authority, monkeypatch):
    from hypertrain.auditor import worker
    from hypertrain.auditor.replay import Outcome
    from hypertrain.protocol.messages import Receipt

    store, manifest, *_ = authority
    backend.test_cuda_dispatch_at_admission_audit_and_geometry(authority, monkeypatch, "live")
    shutil.rmtree(store.state_dir / "audit-v2")
    shutil.rmtree(store.state_dir / "audit-geometry-v2")
    calls = []

    def operation(spec, directory, *, cancel):
        assert cancel is not None and not cancel.is_set()
        calls.append(spec.job)
        name = "op-" + backend.sha256_hex(
            (
                spec.job.run_id + spec.operation + spec.hotkey + spec.binding + spec.job.digest()
            ).encode()
        )
        from hypertrain.gpu_ops.journal import fsha
        from hypertrain.miner.island_launch import validate_artifacts

        case = store._genuine_index.cases["audit_publication"]
        index = json.loads(store._genuine_files[case["publication"]].read_bytes())
        assert index["job_digest"] == spec.job.digest()
        custody = directory / name / "published"
        custody.mkdir(parents=True)
        for relative, member in index["members"].items():
            source = store._genuine_files[member]
            target = (custody / relative).resolve()
            assert target.is_relative_to(custody.resolve())
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, target)
            assert fsha(target) == store._genuine_index.files[member].sha256
        artifacts = validate_artifacts(spec.job, custody)
        return record_rescue(runtime, spec, directory, name, artifacts)

    runtime = real_adapter(monkeypatch, operation, store.state_dir)

    def factory(kind, job, directory, context):
        assert kind == "audit" and context["descriptor"]["job"] == job.body()
        spec, _ = accepted_spec(kind, job, {**context, "hotkey": context["hotkey"]})
        return runtime.launch_adapter(spec)

    store._role_launch = factory

    def audit(*args, **kwargs):
        artifacts = kwargs["launch"](
            kwargs["publication_job"],
            kwargs["publication_directory"],
            backend=kwargs["backend"],
            cancel=args[3].cancelled,
            trace=True,
        )
        return Outcome("MISMATCH", 0, "ab" * 32), artifacts

    monkeypatch.setattr(worker, "execute_island_audit", audit)
    lease = store._db.execute("SELECT * FROM audit_leases_v2").fetchone()
    signer = next(key for key in backend.service.AUDITORS if key.ss58 == lease["auditor"])
    raw = backend.envelope_v2.seal(
        signer,
        "Receipt",
        manifest.run_id(),
        Receipt(w=0, commit_hash=lease["nonce"], received_round=1),
        100,
    )
    result = store.execute_audit_v2(manifest.run_id(), lease["id"], canonicalize(raw))
    assert result["verdict"]["result"] == "MISMATCH" and len(calls) == 1
    descriptor = store._record_v2(
        manifest.run_id(), "audit-geometry", lease["id"] + ":" + lease["nonce"]
    )
    body = backend.envelope_v2.load_json(store.objects.get(descriptor["descriptor_hash"]))
    assert (Path(body["directory"]) / "published/rank-0/trace.json").is_file()
    assert not store._lease_guards_v2


@pytest.mark.parametrize("consumer", ["audit", "referee"])
def test_accepted_role_factory_called_before_kernel(authority, monkeypatch, consumer):
    from hypertrain.auditor import worker
    from hypertrain.protocol.messages import Receipt

    store, manifest, _, _, _ = authority
    if consumer == "audit":
        backend.test_cuda_dispatch_at_admission_audit_and_geometry(authority, monkeypatch, "live")
        # The seeding probe stops after creating owned inputs; a new execution
        # requires an empty attempt directory, not overwrite of durable files.
        shutil.rmtree(store.state_dir / "audit-v2")
        shutil.rmtree(store.state_dir / "audit-geometry-v2")
    else:
        backend.test_referee_uses_frozen_backend_before_evidence(authority, monkeypatch, "cuda")
    calls = []

    def factory(operation, job, directory, context):
        assert operation == consumer and job.manifest == manifest
        assert context["run_id"] == job.run_id == manifest.run_id()
        assert context["descriptor_hash"] == body_digest(context["descriptor"])
        assert context["descriptor"]["job"] == job.body()
        signed = backend.envelope_v2.parse_envelope(context["descriptor_receipt"])
        assert signed.signer == backend.service.COORD.ss58
        assert signed.body["commit_hash"] == context["descriptor_hash"]
        assert backend.envelope_v2.verify_envelope(context["descriptor_receipt"])
        assert str(directory).startswith(str(store.state_dir))
        if operation == "audit":
            assert context["audit_job"]["lease_nonce"] == context["descriptor"]["lease_nonce"]
            assert context["audit_job"]["job_id"] == context["job_id"]
        else:
            assert context["turn"]["transcript_hash"] == context["descriptor"]["transcript_hash"]
            assert context["turn"]["contest"]["referee"] == context["descriptor"]["referee"]
        calls.append(operation)

        def launch(local, target, *, backend, cancel, trace=False):
            assert local == job and target == directory and backend == "cuda"
            assert isinstance(cancel, threading.Event) and not cancel.is_set()
            assert trace is True
            raise ChallengeError(409, "trusted launch dispatch sentinel")

        return launch

    store._role_launch = factory
    if consumer == "audit":

        def preserve_launch(*args, **kwargs):
            assert kwargs["launch"] is not None
            return kwargs["launch"](
                kwargs["publication_job"],
                kwargs["publication_directory"],
                backend=kwargs["backend"],
                cancel=args[3].cancelled,
                trace=True,
            )

        monkeypatch.setattr(worker, "execute_island_audit", preserve_launch)
        lease = store._db.execute("SELECT * FROM audit_leases_v2").fetchone()
        signer = next(key for key in backend.service.AUDITORS if key.ss58 == lease["auditor"])
        receipt = backend.envelope_v2.seal(
            signer,
            "Receipt",
            manifest.run_id(),
            Receipt(w=0, commit_hash=lease["nonce"], received_round=1),
            100,
        )
        with pytest.raises(ChallengeError, match="trusted launch dispatch sentinel"):
            store.execute_audit_v2(manifest.run_id(), lease["id"], canonicalize(receipt))
    else:
        dispute = "bc" * 32
        if hasattr(store, "_genuine_index"):
            case = store._genuine_index.cases["referee"]
            dispute = json.loads(store._genuine_files[case["turn"]].read_bytes())["dispute_id"]
        with pytest.raises(ChallengeError, match="trusted launch dispatch sentinel"):
            store.referee_v2(manifest.run_id(), dispute)
    assert calls == [consumer]
    assert not store._lease_guards_v2


@pytest.mark.parametrize("failure", ["host", "source", "deadline", "cancel", "publication"])
def test_reference_trusted_forwarding_and_refusal(network, failure):
    import json

    from hypertrain.gpu_ops.work_screen import WorkScreenError
    from hypertrain.miner.island_launch import IslandFailure, validate_artifacts

    assert network.join().status_code == 200
    _, admission, _ = network.store._services(network.manifest.run_id())
    row = network.store._db.execute("SELECT * FROM admissions_v2").fetchone()
    lock = EscrowLock(
        operation_id="fa" * 32,
        owner=backend.service.COLD[0].ss58,
        units=1000,
        origin_ids=[network.origin_ids[0]],
        admission_id=row["admission_id"],
        dispute_id=None,
        kind="LOCK_ADMISSION",
    )
    admission.lock(
        canonicalize(network.signed(backend.service.COLD[0], "EscrowLock", lock)),
        now=network.now,
    )
    network.push(network.now + 1)
    admission.challenge(row["admission_id"], now=network.now)
    called = []

    def factory(operation, job, directory, context):
        assert operation == "reference"
        assert context["challenge_hash"] == body_digest(context["challenge"])
        accepted = backend.envelope_v2.parse_envelope(context["challenge_envelope"])
        assert accepted.body == context["challenge"]
        assert backend.envelope_v2.verify_envelope(context["challenge_envelope"])
        assert context["admission_id"] == row["admission_id"]
        assert job.sample_ids and (directory / "start_state").is_file()
        assert (
            job.deadline
            == network.manifest.training.beacon.genesis_time
            + (context["challenge"]["deadline_beacon"] - 1) * 3
        )
        if failure in ("host", "source", "deadline"):
            raise ChallengeError(409, "reviewed " + failure + " binding rejected")
        if failure == "cancel":
            key = f"reference:{row['admission_id']}:{context['challenge']['nonce']}"
            network.store._lease_guards_v2[key].revoke()

        fixture_module = backend

        def launch(local, target, *, backend, cancel, trace=False):
            assert local == job and target == directory and backend == "cpu"
            assert isinstance(cancel, threading.Event) and not cancel.is_set()
            called.append(operation)
            # Return originally valid artifact objects after corrupting original bytes.
            saved = fixture_module.saved_geometry(
                job.manifest, context["hotkey"], directory / "forged", published=True
            )
            artifacts = validate_artifacts(saved, directory / "forged/published")
            path = artifacts.directory / "rank-0/summary.json"
            body = json.loads(path.read_bytes())
            body["job_hash"] = "00" * 32
            path.write_bytes(canonicalize(body))
            return artifacts

        return launch

    network.store._role_launch = factory
    with pytest.raises((ChallengeError, IslandFailure, WorkScreenError, FileNotFoundError)):
        network.store.trial_reference_v2(network.manifest.run_id(), row["admission_id"])
    assert called == (["reference"] if failure == "publication" else [])
    assert not network.store._lease_guards_v2
    assert (
        network.store._db.execute("SELECT reference_commitment FROM admissions_v2").fetchone()[0]
        is None
    )
