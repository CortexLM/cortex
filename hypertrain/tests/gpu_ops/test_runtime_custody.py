"""Internal completion boundary; one real CPU fixture pair, synthetic CUDA metadata."""

from __future__ import annotations

import importlib.util
import io
import json
import shutil
import sys
import tarfile
import threading
from pathlib import Path
from types import SimpleNamespace

import pytest

import hypertrain
from hypertrain.gpu_ops.journal import Journal, fsha, sha256
from hypertrain.protocol.envelope_v2 import seal
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages import Receipt
from hypertrain.protocol.messages_v2 import ArtifactLimits, IslandJobV1, JoinChallenge

STAGE = Path(__file__).resolve().parents[2]
PRODUCT = Path(hypertrain.__file__).resolve().parents[2]
sys.path.insert(0, str(PRODUCT / "scripts"))
SPEC = importlib.util.spec_from_file_location(
    "isolated_runtime_custody", STAGE / "experiments/gpu_network_v2/orchestrate.py"
)
assert SPEC and SPEC.loader
cli = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = cli
SPEC.loader.exec_module(cli)


@pytest.fixture(scope="module")
def retained_custody_work(tmp_path_factory):
    """Original actual_reward miner/reference pair once; publications cloned per case."""
    spec = importlib.util.spec_from_file_location(
        "custody_original_ledger_fixture", PRODUCT / "tests/ledger/test_escrow_v2.py"
    )
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    actual = module.actual_reward.__wrapped__(tmp_path_factory)
    base = tmp_path_factory.getbasetemp()
    miner = next(base.glob("real-reward*/published"))
    reference = next(base.glob("real-reference*/published"))
    return module, actual, miner, reference


@pytest.fixture
def completed(tmp_path, monkeypatch, request, retained_custody_work):
    """Clone genuine module output; CUDA labels/runtime metadata explicitly synthetic."""
    import network_gpu_operation as producer
    from hypertrain.auditor.replay import unpack_state
    from hypertrain.data.trial_assignment import trial_assignment_hash
    from hypertrain.gpu_ops.network_qualification import sources
    from hypertrain.gpu_ops.work_screen import commitments
    from hypertrain.miner.island_launch import validate_artifacts
    from hypertrain.protocol.messages_v2 import ArtifactRef, WorkProof
    from hypertrain.trainer.compress import state_hash

    fixture, _, retained, reference = retained_custody_work
    job = IslandJobV1.model_validate_json((retained / "job.json").read_bytes())
    refjob = IslandJobV1.model_validate_json((reference / "job.json").read_bytes())
    assert job.model_dump(exclude={"deadline"}) == refjob.model_dump(exclude={"deadline"})
    assert retained != reference
    deadline_beacon = (job.deadline - job.manifest.training.beacon.genesis_time) // 3 + 1
    signed_cutoff = job.manifest.training.beacon.genesis_time + (deadline_beacon - 1) * 3
    job = job.model_copy(update={"deadline": signed_cutoff + getattr(request, "param", 0)})
    clock = {"now": job.deadline - 10}
    monkeypatch.setattr(cli.time, "time", lambda: clock["now"])
    coord, miner, owner = fixture.COORD, fixture.HOT, fixture.OTHER
    assert coord.ss58 == job.manifest.training.coord_pubkey
    start, _ = unpack_state((retained / job.object_paths["start_state"]).read_bytes())
    challenge = JoinChallenge(
        admission_id="11" * 32,
        nonce="22" * 32,
        seed_beacon=deadline_beacon - 100,
        deadline_beacon=deadline_beacon,
        manifest_hash=job.run_id,
        theta_hash=state_hash(start),
        assignment_hash=trial_assignment_hash(job.manifest, job.w, tuple(job.sample_ids)),
        layout=job.manifest.training.reference_spec.layout,
        artifact_limits=ArtifactLimits(
            max_object_bytes=268435456, max_body_bytes=65536, max_chunk_manifest_bytes=1048576
        ),
        policy_hash=job.manifest.network.admission_policy_hash,
    )
    wire = seal(coord, "JoinChallenge", job.run_id, challenge, deadline_beacon)
    source_map = sources(PRODUCT)
    source_map["scripts/network_gpu_operation.py"] = fsha(
        PRODUCT / "scripts/network_gpu_operation.py"
    )
    source_map["experiments/gpu_network_v2/orchestrate.py"] = fsha(Path(cli.__file__))
    spec = producer.Operation(
        operation="probe",
        binding=challenge.admission_id,
        hotkey=miner.ss58,
        owner=owner.ss58,
        role="h0",
        instance_id=11,
        machine_id=22,
        job=job,
        binding_kind="trial",
        sources=source_map,
        image_digest=job.manifest.training.reference_spec.image_digest,
        cutoff=job.deadline,
        trace=False,
        challenge="challenge.json",
        context_files={"challenge.json": sha256(canonicalize(wire))},
    )
    name = "op-" + sha256(
        (job.run_id + spec.operation + spec.hotkey + spec.binding + job.digest()).encode()
    )
    original = tmp_path / name
    original.mkdir()
    publication = original / "published"
    shutil.copytree(retained, publication)
    # No training or GPU invocation. Relabel ONLY this copied CPU boundary fixture.
    summary_path = publication / "rank-0/summary.json"
    summary = json.loads(summary_path.read_bytes())
    summary["backend"] = "cuda"
    summary["job_hash"] = sha256(canonicalize(job.model_dump(mode="json")))
    summary_path.write_bytes(canonicalize(summary))
    (publication / "job.json").write_bytes(canonicalize(job.model_dump(mode="json")))
    observed = {"backend": "cuda", "image_digest": spec.image_digest, "test_only": True}
    monkeypatch.setattr(producer, "execution_environment", lambda op: observed)
    artifacts = validate_artifacts(job, publication)
    commit = producer.probe_commit(spec, artifacts, miner, wire)
    root, delta = commitments(artifacts)
    proof = WorkProof(
        admission_id=spec.binding,
        challenge_hash=challenge.digest(),
        leaves_root=root,
        delta_hash=delta,
        artifact_refs=[
            ArtifactRef(sha256=fsha(p), size=p.stat().st_size)
            for p in (artifacts.state, artifacts.ef, artifacts.delta, artifacts.leaves)
        ],
    )
    proof_wire = seal(miner, "WorkProof", job.run_id, proof, deadline_beacon)
    custody = producer.publication_custody(spec, original, artifacts, wire, commit, proof_wire)
    result = {
        "operation": spec.operation,
        "binding": spec.binding,
        "hotkey": spec.hotkey,
        "role": spec.role,
        "job_sha256": job.digest(),
        "publication": "published",
        "status": "CAPTURED_NOT_ACCEPTED",
    }
    producer.publish_custody_result(spec, original, result, custody, threading.Event())
    (tmp_path / "journal").mkdir()
    journal = Journal(tmp_path / "journal")
    journal.append("network_admission_authority", owner=owner.ss58)
    journal.append("supervisor_ready")
    journal.append("ssh_trusted", role="h0")
    cleanup = {"started": False}
    lifecycle = SimpleNamespace(
        dir=tmp_path,
        j=journal,
        cfg={"cleanup_deadline_unix": job.deadline + 300},
        host={"h0": {"machine_id": 22}},
        receipt=lambda role: {"instance_id": 11},
        deadline=lambda: job.deadline,
        cleanup_started=lambda: cleanup["started"],
    )
    runtime = cli.NetworkRuntime(
        lifecycle,
        {
            "runtime": {"qualified": True, "image_digest": spec.image_digest},
            "layout": job.manifest.training.reference_spec.layout.model_dump(),
        },
    )
    subject = {
        "role": spec.role,
        "binding": spec.binding,
        "operation": spec.operation,
        "hotkey": spec.hotkey,
        "spec_sha256": sha256(spec.model_dump_json().encode()),
    }
    receipt = seal(
        owner,
        "Receipt",
        job.run_id,
        Receipt(w=job.w, commit_hash=sha256(canonicalize(subject)), received_round=1),
        101,
    )
    runtime.accept_operation(spec, canonicalize(receipt), owner=owner.ss58, beacon=1)
    journal.append("network_execution_intent", role="h0", name=name, phase="workload", executions=1)
    journal.append(
        "network_operation_started",
        role="h0",
        name=name,
        binding=spec.binding,
        job_sha256=job.digest(),
        operation=spec.operation,
        hotkey=spec.hotkey,
        sources_sha256=sha256(json.dumps(spec.sources, sort_keys=True).encode()),
        cutoff=spec.cutoff,
    )
    archive_path = tmp_path / "rescued.tar"

    def archive():
        with tarfile.open(archive_path, "w") as tar:
            for path in sorted(original.rglob("*")):
                if path.is_file():
                    data = path.read_bytes()
                    member = tarfile.TarInfo("out/" + name + "/" + str(path.relative_to(original)))
                    member.size = len(data)
                    tar.addfile(member, io.BytesIO(data))
        journal.append(
            "network_rescued", role="h0", tar=str(archive_path), tar_sha256=fsha(archive_path)
        )
        journal.append(
            "network_operation_done",
            role="h0",
            name=name,
            result_sha256=fsha(original / "operation-result.json"),
            tar_sha256=fsha(archive_path),
        )

    archive()
    return SimpleNamespace(
        runtime=runtime,
        spec=spec,
        directory=tmp_path,
        original=original,
        custody=custody,
        result=result,
        clock=clock,
        archive=archive,
        archive_path=archive_path,
        owner=owner,
        cleanup=cleanup,
        signed_cutoff=signed_cutoff,
    )


@pytest.mark.parametrize("offset", [0, 1])
def test_completion_original_signed_done_boundary(completed, offset):
    n = completed
    done = n.runtime.journal.last("network_operation_done")
    n.runtime.journal.append(
        "network_operation_done",
        **{k: v for k, v in done.items() if k not in ("kind", "unix", "pid")},
        unix=n.signed_cutoff + offset,
    )
    before = n.runtime.journal.records()
    if offset:
        with pytest.raises(cli.Reject, match="completion_original_done_missing"):
            n.runtime.completion_custody(n.spec, n.directory, tree=PRODUCT)
    else:
        actual = n.runtime.completion_custody(n.spec, n.directory, tree=PRODUCT)
        assert actual["body"]["done"]["unix"] == n.signed_cutoff
        assert actual["body"]["status"] == "NOTSTOREACCEPTED"
    assert n.runtime.journal.records() == before


@pytest.mark.parametrize("completed", [10], indirect=True)
def test_owner_signed_later_job_cannot_extend_original_challenge(completed):
    n = completed
    assert n.spec.job.deadline == n.signed_cutoff + 10
    done = n.runtime.journal.last("network_operation_done")
    n.runtime.journal.append(
        "network_operation_done",
        **{k: v for k, v in done.items() if k not in ("kind", "unix", "pid")},
        unix=n.signed_cutoff + 1,
    )
    with pytest.raises(cli.Reject, match="completion_original_signed_deadline_changed"):
        n.runtime.completion_custody(n.spec, n.directory, tree=PRODUCT)


def test_authenticated_completion_descriptor_is_not_store_acceptance(completed):
    n = completed
    before = n.runtime.journal.records()
    actual = n.runtime.completion_custody(n.spec, n.directory, tree=PRODUCT)
    assert actual["sha256"] == sha256(canonicalize(actual["body"]))
    assert actual["body"]["status"] == "NOTSTOREACCEPTED"
    assert actual["body"]["production_backend_acceptance"] is False
    assert actual["body"]["economic_acceptance"] is False
    assert actual["body"]["publication"] == n.custody["body"]
    assert n.runtime.completion_custody(n.spec, n.directory, tree=PRODUCT) == actual
    assert n.runtime.journal.records() == before


@pytest.mark.parametrize(
    "fault",
    [
        "rescue",
        "host",
        "challenge",
        "owner",
        "source",
        "done",
        "execution",
        "cancel",
        "cutoff",
        "cleanup",
    ],
)
def test_completion_refuses_changed_authority_or_custody(completed, fault):
    n = completed
    cancel = threading.Event()
    if fault == "rescue":
        n.archive_path.write_bytes(n.archive_path.read_bytes() + b"changed rescue")
    elif fault == "host":
        n.runtime.lifecycle.host["h0"]["machine_id"] = 23
    elif fault == "owner":
        authority = n.runtime.journal.last("network_operation_authority")
        receipt = {**authority["receipt"], "sig": "00" * 64}
        n.runtime.journal.append(
            "network_operation_authority",
            **{k: v for k, v in authority.items() if k not in ("kind", "unix", "pid", "receipt")},
            receipt=receipt,
        )
    elif fault == "source":
        n.runtime.journal.append(
            "network_operation_source",
            role="h0",
            run_id=n.spec.job.run_id,
            sources_sha256="00" * 32,
        )
    elif fault == "done":
        n.runtime.journal.append(
            "network_operation_done",
            role="h0",
            name="op-"
            + sha256(
                (
                    n.spec.job.run_id
                    + n.spec.operation
                    + n.spec.hotkey
                    + n.spec.binding
                    + n.spec.job.digest()
                ).encode()
            ),
            result_sha256="00" * 32,
            tar_sha256=fsha(n.archive_path),
        )
    elif fault == "execution":
        path = n.original / "execution-custody.json"
        raw = json.loads(path.read_bytes())
        raw["instance_id"] = 12
        path.write_bytes(canonicalize(raw))
        n.archive()
    elif fault == "challenge":
        # Rehash outer custody/result/archive; wrong challenge signature must still reject.
        n.custody["body"]["trial_authority"]["envelope"]["sig"] = "00" * 64
        trial = n.custody["body"]["trial_authority"]
        trial["sha256"] = sha256(canonicalize(trial["envelope"]))
        n.custody["sha256"] = sha256(canonicalize(n.custody["body"]))
        (n.original / "publication-custody.json").write_bytes(canonicalize(n.custody))
        n.result["publication_custody_sha256"] = fsha(n.original / "publication-custody.json")
        (n.original / "operation-result.json").write_bytes(
            json.dumps(n.result, sort_keys=True).encode()
        )
        n.archive()
    elif fault == "cancel":
        cancel.set()
    elif fault == "cutoff":
        n.clock["now"] = n.spec.cutoff
    else:
        n.cleanup["started"] = True
    from hypertrain.protocol.envelope_v2 import SignatureError

    before = n.runtime.journal.records()
    with pytest.raises((cli.Reject, SignatureError)):
        n.runtime.completion_custody(n.spec, n.directory, tree=PRODUCT, cancel=cancel)
    assert n.runtime.journal.records() == before
