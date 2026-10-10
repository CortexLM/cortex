"""Actual strict loopback SSH; hermetic publication seam only, no kernel/GPU claim."""

from __future__ import annotations

import ast
import getpass
import importlib.util
import json
import os
import select
import shlex
import socket
import subprocess
import sys
import threading
import time
from pathlib import Path
from types import SimpleNamespace

import pytest

from hypertrain.gpu_ops.journal import Journal, fsha, sha256
from hypertrain.gpu_ops.launcher import Orchestrator, Reject
from hypertrain.gpu_ops.provider import Provider
from hypertrain.gpu_ops.remote import ensure_identity
from hypertrain.protocol import envelope_v2
from hypertrain.protocol.example import example_manifest
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages_v2 import IslandJobV1, RunManifestV2

TREE = Path(__file__).parents[2]


def module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec and spec.loader
    value = importlib.util.module_from_spec(spec)
    sys.modules[name] = value
    spec.loader.exec_module(value)
    return value


RUNTIME = module("role_runtime", TREE / "experiments/gpu_network_v2/orchestrate.py")
OP = module("role_operation", TREE / "scripts/network_gpu_operation.py")
OWNER = Keypair(bytes(range(32)))


@pytest.fixture
def context(tmp_path, mock_factory):
    run = tmp_path / "run"
    run.mkdir()
    (run / "logs").mkdir()
    journal = Journal(run)
    identity = ensure_identity(run)
    host = tmp_path / "host"
    subprocess.run(
        ["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(host)],
        check=True,
        capture_output=True,
    )
    authorized = tmp_path / "authorized"
    authorized.write_bytes(identity.with_suffix(".pub").read_bytes())
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    config = tmp_path / "sshd"
    config.write_text(
        f"ListenAddress 127.0.0.1\nPort {port}\nHostKey {host}\nPidFile {tmp_path}/sshd.pid\n"
        f"AuthorizedKeysFile {authorized}\nStrictModes no\nPermitRootLogin prohibit-password\n"
        "PasswordAuthentication no\nKbdInteractiveAuthentication no\n"
        f"UsePAM {'yes' if os.getuid() == 0 else 'no'}\nAllowUsers {getpass.getuser()}\n"
        "LogLevel VERBOSE\n"
    )
    server = subprocess.Popen(
        ["/usr/sbin/sshd", "-D", "-e", "-f", str(config)],
        stderr=subprocess.PIPE,
        start_new_session=True,
    )
    assert server.stderr and select.select([server.stderr], [], [], 15)[0]
    assert b"Server listening" in server.stderr.readline()
    provider = mock_factory()
    cutoff = int(time.time()) + 3500
    profile = json.loads((TREE / "experiments/gpu_network_v2/profile.json").read_bytes())
    profile["runtime"].update(
        image_digest="sha256:" + "11" * 32,
        driver_allowlist=["595.84"],
        qualified=True,
        remote_python=sys.executable,
    )
    cfg = {
        "base_url": provider.base,
        "hosts": [{"role": "h0", "machine_id": 5000}, {"role": "h1", "machine_id": 5001}],
        "cleanup_contract": "network-v2",
        "remote_root": str(tmp_path / "remote-{role}"),
        "remote_python": sys.executable,
        "ssh_user": getpass.getuser(),
        "image": "candidate@" + profile["runtime"]["image_digest"],
        "cleanup_deadline_unix": cutoff,
    }
    orch = Orchestrator(cfg, journal, Provider(provider.base, "a" * 32, run, journal, False), run)
    journal.append("admitted", deadline_unix=cutoff)
    journal.append("network_admission_authority", owner=OWNER.ss58)
    journal.append("supervisor_ready", supervisor_pid=99999999)
    for role, i in (("h0", 1), ("h1", 2)):
        journal.append("receipt", role=role, instance_id=i)
        journal.append("instance_ready", role=role, ssh_host="127.0.0.1", ssh_port=port)
        orch.trust(role)
        journal.append("staged", role=role, tar_sha256="00" * 32)
        journal.append(
            "network_execution_intent",
            role=role,
            name="qualification",
            phase="qualification",
            executions=2,
        )
    journal.append("network_workload_promoted")
    body = example_manifest().body()
    for section in ("model", "outer"):
        body[section].update(profile[section])
    body["inner"].update({k: v for k, v in profile["inner"].items() if k != "lr_schedule"})
    body["inner"]["lr_schedule"].update(profile["inner"]["lr_schedule"])
    body["reference_spec"].update(
        image_digest=profile["runtime"]["image_digest"],
        driver_allowlist=["595.84"],
        layout=profile["layout"],
    )
    manifest = RunManifestV2.model_validate(
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
                "capabilities": ["island-replay", "all-level-disputes", "transport-receipts"],
            },
        }
    )
    inputs = tmp_path / "inputs"
    inputs.mkdir()
    for key in ("start_state", "ef_in", "v0", "samples", "sample_proofs"):
        (inputs / key).write_bytes(key.encode())
    job = IslandJobV1(
        job_version=1,
        run_id=manifest.run_id(),
        w=0,
        manifest=manifest,
        sample_ids=list(range(60)),
        global_step0=0,
        start_state_sha256=fsha(inputs / "start_state"),
        ef_in_sha256=fsha(inputs / "ef_in"),
        v0_sha256=fsha(inputs / "v0"),
        object_paths={k: k for k in ("start_state", "ef_in", "v0", "samples", "sample_proofs")},
        deadline=cutoff,
    )
    engine = __import__("hypertrain.gpu_ops.network_qualification", fromlist=["sources"])
    sources = engine.sources(TREE)
    sources["scripts/network_gpu_operation.py"] = fsha(TREE / "scripts/network_gpu_operation.py")
    spec = OP.Operation(
        operation="reference",
        binding="ab" * 32,
        binding_kind="trial",
        hotkey=OWNER.ss58,
        owner=OWNER.ss58,
        role="h0",
        instance_id=1,
        machine_id=5000,
        job=job,
        sources=sources,
        image_digest=profile["runtime"]["image_digest"],
        cutoff=cutoff - 310,
        trace=False,
    )
    runtime = RUNTIME.NetworkRuntime(orch, profile)
    context = {
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
        {"w": 0, "commit_hash": sha256(canonicalize(context)), "received_round": 10},
        20,
    )
    runtime.accept_operation(spec, canonicalize(receipt), owner=OWNER.ss58, beacon=10)
    try:
        yield runtime, spec, inputs, provider
    finally:
        server.terminate()
        server.wait(timeout=15)


@pytest.mark.parametrize(
    "fault",
    [
        None,
        "remote-failure",
        "CPU-publication",
        "canonical-reference",
        "canonical-audit",
        "canonical-collision",
        "canonical-symlink",
        "canonical-cancel",
        "canonical-expired",
    ],
)
def test_loopback_debit_transport_custody_and_nonretryable(context, monkeypatch, fault):
    runtime, spec, inputs, provider = context
    canonical_case = fault is not None and fault.startswith("canonical-")
    if fault == "canonical-audit":
        spec = spec.model_copy(
            update={"operation": "audit", "binding_kind": "commit-lease", "trace": True}
        )
        receipt = envelope_v2.seal(
            OWNER,
            "Receipt",
            spec.job.run_id,
            {
                "w": 0,
                "commit_hash": sha256(
                    canonicalize(
                        {
                            "role": spec.role,
                            "binding": spec.binding,
                            "operation": spec.operation,
                            "hotkey": spec.hotkey,
                            "spec_sha256": sha256(spec.model_dump_json().encode()),
                        }
                    )
                ),
                "received_round": 10,
            },
            20,
        )
        runtime.accept_operation(spec, canonicalize(receipt), owner=OWNER.ss58, beacon=10)
    ssh_run = RUNTIME.DeadlineSsh.run
    calls = []

    def dispatch(ssh, command, tag, logs):
        if " scripts/network_gpu_operation.py " in command:
            assert runtime.journal.last("network_execution_intent", role="h0")["executions"] == 1
            remote = (
                Path(runtime.lifecycle.remote_root("h0"))
                / "out"
                / runtime.journal.last("network_operation_started")["name"]
            )
            result = {
                "status": "CAPTURED_NOT_ACCEPTED",
                "binding": spec.binding,
                "job_sha256": spec.job.digest(),
                "hotkey": spec.hotkey,
                "role": spec.role,
                "operation": spec.operation,
                "publication": "published",
            }
            code = (
                "import pathlib,json; p=pathlib.Path("
                + repr(str(remote))
                + "); (p/'published/rank-0').mkdir(parents=True); (p/'published/rank-1').mkdir(); "
                + "[(p/f'published/rank-{r}/summary.json').write_text("
                + '\'{"backend":"cuda"}\') for r in (0,1)]; '
                + "(p/'published/state').write_bytes(b'original'); "
                + "[(p/f'published/rank-{r}'/f).write_bytes(f.encode()) for r in (0,1) "
                + "for f in ('state.safetensors','ef.safetensors','delta.bin',"
                + "'leaves.json','trace.json')]; "
                + "[(p/f'published/rank-{r}/checkpoints').mkdir() for r in (0,1)]; "
                + "[(p/f'published/rank-{r}/checkpoints/0.safetensors').write_bytes(b'checkpoint') "
                + "for r in (0,1)]; "
                + "(p/'operation-result.json').write_text("
                + repr(json.dumps(result))
                + ")"
            )
            calls.append("hermetic-publication-not-CUDA")
            if fault == "CPU-publication":
                code = code.replace("cuda", "cpu")
            if fault == "remote-failure":
                code += ";raise SystemExit(7)"
            command = shlex.quote(sys.executable) + " -c " + shlex.quote(code)
        if "experiments/gpu_network_v2/orchestrate.py export" in command:
            command = command.replace(
                "cd " + shlex.quote(runtime.lifecycle.remote_root("h0")) + " && env PYTHONPATH=src",
                "cd " + shlex.quote(str(TREE)) + " && env PYTHONPATH=src",
            )
            command = command.replace(
                "export out rescue.tar",
                "export "
                + shlex.quote(runtime.lifecycle.remote_root("h0") + "/out")
                + " "
                + shlex.quote(runtime.lifecycle.remote_root("h0") + "/rescue.tar"),
            )
        return ssh_run(ssh, command, tag, logs)

    monkeypatch.setattr(RUNTIME.DeadlineSsh, "run", dispatch)
    from hypertrain.gpu_ops.remote import Ssh

    original_run = Ssh.run

    def rescue_run(ssh, command, tag, logs):
        if tag == "network-export":
            command = (
                "cd "
                + shlex.quote(str(TREE))
                + " && env PYTHONPATH=src "
                + shlex.quote(sys.executable)
                + " experiments/gpu_network_v2/orchestrate.py export "
                + shlex.quote(runtime.lifecycle.remote_root("h0") + "/out")
                + " "
                + shlex.quote(runtime.lifecycle.remote_root("h0") + "/rescue.tar")
                + " experiments/gpu_network_v2/profile.json"
            )
        return original_run(ssh, command, tag, logs)

    monkeypatch.setattr(Ssh, "run", rescue_run)
    import hypertrain.miner.island_launch as island

    def validate(job, directory):
        assert job == spec.job and (directory / "state").read_bytes() == b"original"
        return SimpleNamespace(
            directory=directory,
            state=directory / "rank-0/state.safetensors",
            ef=directory / "rank-0/ef.safetensors",
            delta=directory / "rank-0/delta.bin",
            leaves=directory / "rank-0/leaves.json",
        )

    monkeypatch.setattr(island, "validate_artifacts", validate)
    if canonical_case:
        if fault == "canonical-collision":
            (inputs / "published").mkdir()
            (inputs / "published/state").write_bytes(b"stale")
        if fault == "canonical-symlink":
            (inputs / "published").symlink_to(inputs)
        event = threading.Event()
        initial_validate = validate

        def final_validate(job, directory):
            result = initial_validate(job, directory)
            if directory.parent.name.startswith("canonical-"):
                if fault == "canonical-cancel":
                    event.set()
                if fault == "canonical-expired":
                    monkeypatch.setattr(RUNTIME.time, "time", lambda: spec.cutoff + 1)
            return result

        monkeypatch.setattr(island, "validate_artifacts", final_validate)
        launch = runtime.launch_adapter(spec)
        if fault in (
            "canonical-collision",
            "canonical-symlink",
            "canonical-cancel",
            "canonical-expired",
        ):
            with pytest.raises(Reject, match="collision|expired"):
                launch(spec.job, inputs, backend="cuda", trace=spec.trace, cancel=event)
            if fault == "canonical-collision":
                assert (inputs / "published/state").read_bytes() == b"stale"
            if fault in ("canonical-cancel", "canonical-expired"):
                assert not (inputs / "published").exists()
                assert not [p for p in inputs.glob("canonical-*") if p.is_dir()]
            if fault in ("canonical-collision", "canonical-symlink"):
                with pytest.raises(Reject, match="failed_not_retryable"):
                    launch(spec.job, inputs, backend="cuda", trace=spec.trace, cancel=event)
        else:
            artifacts = launch(spec.job, inputs, backend="cuda", trace=spec.trace, cancel=event)
            assert artifacts.directory == inputs / "published"
            rescued = (
                inputs / runtime.journal.last("network_operation_started")["name"] / "published"
            )
            original = {
                str(p.relative_to(rescued)): p.read_bytes()
                for p in rescued.rglob("*")
                if p.is_file()
            }
            assert {
                str(p.relative_to(artifacts.directory)): p.read_bytes()
                for p in artifacts.directory.rglob("*")
                if p.is_file()
            } == original
            assert len(calls) == 1
            duplicate = launch(spec.job, inputs, backend="cuda", trace=spec.trace, cancel=event)
            assert duplicate.directory == artifacts.directory and len(calls) == 1
            # Execute actual reference loader and audit equality, not a mocked path.
            from hypertrain.challenge.store import ChallengeError

            admission = ast.parse((TREE / "src/hypertrain/challenge/admission.py").read_text())
            statement = next(
                node
                for node in ast.walk(admission)
                if isinstance(node, ast.Assign)
                and any(
                    isinstance(target, ast.Name) and target.id == "data" for target in node.targets
                )
                and "published" in ast.unparse(node.value)
            )
            for name in ("state.safetensors", "ef.safetensors", "delta.bin", "leaves.json"):
                env = {"directory": inputs, "name": name}
                exec(
                    compile(
                        ast.Module(body=[statement], type_ignores=[]),
                        "actual-reference-loader",
                        "exec",
                    ),
                    env,
                )
                assert (
                    env["data"]
                    == getattr(
                        artifacts,
                        {
                            "state.safetensors": "state",
                            "ef.safetensors": "ef",
                            "delta.bin": "delta",
                            "leaves.json": "leaves",
                        }[name],
                    ).read_bytes()
                )
            store = ast.parse((TREE / "src/hypertrain/challenge/store.py").read_text())
            check = next(
                node
                for node in ast.walk(store)
                if isinstance(node, ast.If)
                and ast.unparse(node.test)
                == "artifacts.directory != geometry_directory / 'published'"
            )
            exec(
                compile(
                    ast.Module(body=[check], type_ignores=[]), "actual-audit-path-check", "exec"
                ),
                {
                    "artifacts": artifacts,
                    "geometry_directory": inputs,
                    "ChallengeError": ChallengeError,
                },
            )
            (artifacts.directory / "rank-0/trace.json").write_bytes(b"different")
            with pytest.raises(Reject, match="collision"):
                launch(spec.job, inputs, backend="cuda", trace=spec.trace, cancel=event)
            assert len(calls) == 1
    elif fault is None:
        artifacts = runtime.operation(spec, inputs)
        assert artifacts.directory.name == "published"
        assert runtime.journal.last("network_operation_done")
    else:
        with pytest.raises(Reject):
            runtime.operation(spec, inputs)
        assert runtime.journal.last("network_operation_done") is None
    assert calls == ["hermetic-publication-not-CUDA"]
    assert runtime.journal.last("network_rescued")["files"]
    with pytest.raises(Reject, match="not_retryable|not_qualified"):
        runtime.operation(spec, inputs)
    assert provider.state()["put_count"] == 0


@pytest.mark.parametrize("change", ["binding", "hotkey", "cutoff", "sources", "instance_id"])
def test_unaccepted_context_rejects_before_debit(context, change):
    runtime, spec, inputs, _ = context
    value = {
        "binding": "cc" * 32,
        "hotkey": Keypair(bytes([8]) * 32).ss58,
        "cutoff": 1,
        "sources": {},
        "instance_id": 99,
    }[change]
    before = len(runtime.journal.all("network_execution_intent"))
    with pytest.raises(Reject):
        runtime.operation(spec.model_copy(update={change: value}), inputs)
    assert len(runtime.journal.all("network_execution_intent")) == before


def test_cancelled_rejects_before_launch(context):
    runtime, spec, inputs, _ = context
    event = threading.Event()
    event.set()
    with pytest.raises(Reject, match="not_qualified"):
        runtime.operation(spec, inputs, cancel=event)


def test_changed_dedicated_known_hosts_rejects_before_debit(context):
    runtime, spec, inputs, _ = context
    runtime.lifecycle.ssh("h0").known_hosts.write_text("changed")
    before = len(runtime.journal.all("network_execution_intent"))
    with pytest.raises(Reject, match="known_hosts"):
        runtime.operation(spec, inputs)
    assert len(runtime.journal.all("network_execution_intent")) == before


def test_canonical_seams_default_and_remote_hook_signatures():
    import inspect

    from hypertrain.auditor.worker import execute_island_audit
    from hypertrain.gpu_ops.work_screen import screen_work

    assert inspect.signature(screen_work).parameters["launch"].default is None
    assert inspect.signature(execute_island_audit).parameters["launch"].default is None


def test_work_screen_trusted_launch_requires_original_validation(context, monkeypatch):
    runtime, spec, inputs, _ = context
    import hypertrain.gpu_ops.work_screen as screen
    from hypertrain.data.trial_assignment import trial_assignment_hash
    from hypertrain.protocol.messages_v2 import ArtifactLimits, JoinChallenge

    challenge = JoinChallenge(
        admission_id=spec.binding,
        nonce="11" * 32,
        seed_beacon=1,
        deadline_beacon=101,
        manifest_hash=spec.job.run_id,
        theta_hash="22" * 32,
        assignment_hash=trial_assignment_hash(
            spec.job.manifest, spec.job.w, tuple(spec.job.sample_ids)
        ),
        layout=spec.job.manifest.training.reference_spec.layout,
        artifact_limits=ArtifactLimits(
            max_object_bytes=100, max_body_bytes=65536, max_chunk_manifest_bytes=1048576
        ),
        policy_hash="11" * 32,
    )
    import hypertrain.auditor.replay as replay
    import hypertrain.trainer.compress as compress

    monkeypatch.setattr(replay, "unpack_state", lambda blob: ({}, None))
    monkeypatch.setattr(compress, "state_hash", lambda theta: "22" * 32)
    called = []

    def launch(job, directory, **kwargs):
        called.append((job, kwargs))
        return SimpleNamespace(directory=inputs)

    def refused(job, directory):
        raise ValueError("independent artifact validator rejects hermetic metadata")

    monkeypatch.setattr(screen, "validate_artifacts", refused)
    with pytest.raises(ValueError, match="validator rejects"):
        screen.screen_work(spec.job, inputs, challenge, now_beacon=2, backend="cuda", launch=launch)
    assert len(called) == 1 and called[0][0] == spec.job


def test_operation_owner_receipt_context_cannot_change(context):
    runtime, spec, inputs, _ = context
    altered = spec.model_copy(update={"trace": True})
    body = {
        "role": altered.role,
        "binding": altered.binding,
        "operation": altered.operation,
        "hotkey": altered.hotkey,
        "spec_sha256": sha256(altered.model_dump_json().encode()),
    }
    raw = envelope_v2.seal(
        OWNER,
        "Receipt",
        spec.job.run_id,
        {"w": 0, "commit_hash": sha256(canonicalize(body)), "received_round": 10},
        20,
    )
    with pytest.raises(Reject, match="immutable"):
        runtime.accept_operation(altered, canonicalize(raw), owner=OWNER.ss58, beacon=10)


@pytest.fixture
def probe_metadata(tmp_path, monkeypatch):
    from hypertrain.data.trial_assignment import trial_assignment_hash
    from hypertrain.miner.core import NetworkMiner
    from hypertrain.protocol.messages_v2 import ArtifactLimits, JoinChallenge

    body = example_manifest().body()
    body["coord_pubkey"] = OWNER.ss58
    profile = json.loads((TREE / "experiments/gpu_network_v2/profile.json").read_bytes())
    for section in ("model", "outer"):
        body[section].update(profile[section])
    body["inner"].update({k: v for k, v in profile["inner"].items() if k != "lr_schedule"})
    body["inner"]["lr_schedule"].update(profile["inner"]["lr_schedule"])
    body["reference_spec"].update(layout=profile["layout"])
    manifest = RunManifestV2.model_validate(
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
                "capabilities": ["island-replay", "all-level-disputes", "transport-receipts"],
            },
        }
    )
    inputs = tmp_path / "probe"
    inputs.mkdir()
    paths = {k: k for k in ("start_state", "ef_in", "v0", "samples", "sample_proofs")}
    for p in paths.values():
        (inputs / p).write_bytes(p.encode())
    job = IslandJobV1(
        job_version=1,
        run_id=manifest.run_id(),
        w=0,
        manifest=manifest,
        sample_ids=list(range(manifest.training.batch_samples())),
        global_step0=0,
        start_state_sha256=fsha(inputs / "start_state"),
        ef_in_sha256=fsha(inputs / "ef_in"),
        v0_sha256=fsha(inputs / "v0"),
        object_paths=paths,
        deadline=int(time.time()) + 30,
    )
    challenge = JoinChallenge(
        admission_id="ab" * 32,
        nonce="11" * 32,
        seed_beacon=1,
        deadline_beacon=101,
        manifest_hash=job.run_id,
        theta_hash="22" * 32,
        assignment_hash=trial_assignment_hash(manifest, 0, tuple(job.sample_ids)),
        layout=manifest.training.reference_spec.layout,
        artifact_limits=ArtifactLimits(
            max_object_bytes=100, max_body_bytes=65536, max_chunk_manifest_bytes=1048576
        ),
        policy_hash="11" * 32,
    )
    job_path = inputs / "job.json"
    job_path.write_text(job.model_dump_json())
    challenge_path = inputs / "challenge.json"
    challenge_path.write_bytes(
        canonicalize(envelope_v2.seal(OWNER, "JoinChallenge", job.run_id, challenge, 101))
    )
    miner = object.__new__(NetworkMiner)
    miner.cfg = SimpleNamespace(device="cuda")
    miner.run_id = job.run_id
    miner.manifest = manifest
    miner.kp = OWNER
    miner.url = "/unused"
    uploads = []
    miner.api = SimpleNamespace(
        call=lambda *args: {"now_round": 2},
        base="",
        c=SimpleNamespace(put=lambda *a, **k: uploads.append(a)),
    )
    import hypertrain.auditor.replay as replay
    import hypertrain.trainer.compress as compress

    monkeypatch.setattr(replay, "unpack_state", lambda blob: ({}, None))
    monkeypatch.setattr(compress, "state_hash", lambda theta: "22" * 32)
    return miner, job, job_path, challenge_path, uploads


@pytest.mark.parametrize("fault", ["cancel", "deadline", "authority"])
def test_actual_probe_rejects_before_cached_kernel_or_publication(
    probe_metadata, monkeypatch, fault
):
    import hypertrain.gpu_ops.work_screen as screen
    from hypertrain.miner.core import MinerError

    miner, job, path, challenge, uploads = probe_metadata
    event = threading.Event()
    if fault == "cancel":
        event.set()
    if fault == "deadline":
        path.write_text(job.model_copy(update={"deadline": 1}).model_dump_json())
    if fault == "authority":
        miner.run_id = "cc" * 32

    def forbidden(*args, **kwargs):
        raise AssertionError("cached kernel must not run")

    monkeypatch.setattr(screen, "launch_island", forbidden)
    with pytest.raises(MinerError):
        miner.probe(path, challenge, launch=forbidden, cancel=event)
    assert not uploads


def test_actual_probe_forwards_guard_and_event_despite_cached_launcher(probe_metadata, monkeypatch):
    import hypertrain.gpu_ops.work_screen as screen
    import hypertrain.miner.island_launch as island
    from hypertrain.gpu_ops.work_screen import WorkScreenError

    miner, job, path, challenge, uploads = probe_metadata
    event = threading.Event()
    calls = []

    def forbidden(*args, **kwargs):
        raise AssertionError("cached/global launcher bypass")

    monkeypatch.setattr(screen, "launch_island", forbidden)
    monkeypatch.setattr(island, "launch_island", forbidden)

    def guarded(actual, directory, *, backend, cancel, trace=False):
        assert actual == job and backend == "cuda" and cancel is event and not trace
        calls.append(actual)
        event.set()
        return SimpleNamespace(directory=directory)

    monkeypatch.setattr(
        screen, "validate_artifacts", lambda actual, directory: SimpleNamespace(directory=directory)
    )
    with pytest.raises(WorkScreenError, match="CANCELLED"):
        miner.probe(path, challenge, launch=guarded, cancel=event)
    assert calls == [job] and not uploads


def test_actual_probe_inflight_cancel_reaps_process_retains_custody(
    probe_metadata, monkeypatch, tmp_path
):
    import hypertrain.miner.island_launch as island

    miner, job, path, challenge, uploads = probe_metadata
    cancel = threading.Event()
    ready = threading.Event()
    processes = []
    errors = []
    listener = socket.socket(socket.AF_UNIX)
    address = str(tmp_path / "ready.sock")
    listener.bind(address)
    listener.listen()
    listener.settimeout(10)
    child = (
        "import socket,signal,sys;s=socket.socket(socket.AF_UNIX);"
        "s.connect(sys.argv[1]);s.sendall(b'R');signal.pause()"
    )
    monkeypatch.setattr(island, "launch_argv", lambda *args: [sys.executable, "-c", child, address])
    original = subprocess.Popen

    def spawn(*args, **kwargs):
        process = original(*args, **kwargs)
        processes.append(process)
        return process

    monkeypatch.setattr(island.subprocess, "Popen", spawn)

    def guarded(actual, directory, *, backend, cancel, trace=False):
        assert actual == job and backend == "cuda" and cancel is event
        (directory / "charged-intent").write_bytes(b"nonretryable")
        island._launch_unbounded(actual, directory, backend, dict(os.environ), cancel)
        raise AssertionError("cancelled process cannot publish")

    event = cancel

    def consume():
        try:
            miner.probe(path, challenge, launch=guarded, cancel=cancel)
        except BaseException as error:
            errors.append(error)
        finally:
            ready.set()

    worker = threading.Thread(target=consume)
    # Completion/cancel signals exist before triggering the real consumer.
    worker.start()
    try:
        connection, _ = listener.accept()
        with connection:
            assert connection.recv(1) == b"R"
        cancel.set()
        assert ready.wait(10), "consumer did not reap on subscribed cancellation"
    finally:
        cancel.set()
        worker.join(timeout=10)
        listener.close()
        for process in processes:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=10)
    assert len(processes) == 1 and processes[0].returncode is not None
    assert len(errors) == 1 and isinstance(errors[0], island.IslandFailure)
    assert (path.parent / "charged-intent").read_bytes() == b"nonretryable"
    assert (path.parent / "stdout.log").exists() and not uploads


def test_remote_probe_wrapper_explicit_consumer_guard_preserves_failed_outputs(
    probe_metadata, monkeypatch
):
    import signal

    import httpx

    import hypertrain.gpu_ops.work_screen as screen
    import hypertrain.miner.core as core
    import hypertrain.miner.island_launch as island

    miner, job, path, challenge, uploads = probe_metadata
    cfg = SimpleNamespace(
        run_id=job.run_id,
        owner_hotkey=OWNER.ss58,
        keyfile=path,
        api="https://metadata.invalid",
        device="cuda",
        image_digest=job.manifest.training.reference_spec.image_digest,
        workdir=path.parent / "miner",
    )
    spec = OP.Operation(
        operation="probe",
        binding="ab" * 32,
        binding_kind="trial",
        hotkey=OWNER.ss58,
        owner=OWNER.ss58,
        role="h0",
        instance_id=1,
        machine_id=5000,
        job=job,
        sources={},
        image_digest=cfg.image_digest,
        cutoff=job.deadline,
        trace=False,
        miner_config="config",
        ca="ca",
        master=cfg.api,
        challenge=challenge.name,
    )
    (path.parent / "config").write_bytes(b"metadata config")
    (path.parent / "ca").write_bytes(b"metadata CA")
    monkeypatch.setattr(OP, "check", lambda *args: None)
    monkeypatch.setattr(core.MinerConfig, "load", lambda *args, **kwargs: cfg)
    monkeypatch.setattr(core, "load_keyfile", lambda *args: OWNER)
    callbacks = {}
    monkeypatch.setattr(
        OP.signal, "signal", lambda sig, handler: callbacks.__setitem__(sig, handler)
    )
    manifest = envelope_v2.seal(OWNER, "RunManifestV2", job.run_id, job.manifest, 101)
    requests = []

    def respond(request):
        requests.append(request)
        assert request.method == "GET"
        return httpx.Response(200, json={"manifest_envelope": manifest, "now_round": 2})

    init = httpx.Client.__init__

    def client_init(self, *args, **kwargs):
        kwargs["transport"] = httpx.MockTransport(respond)
        init(self, *args, **kwargs)

    monkeypatch.setattr(httpx.Client, "__init__", client_init)
    original = island.launch_island

    def forbidden(*args, **kwargs):
        raise AssertionError("cached probe launcher bypass")

    monkeypatch.setattr(screen, "launch_island", forbidden)
    seen = []

    def kernel(actual, directory, backend, env, cancel):
        assert actual == job and backend == "cuda" and not cancel.is_set()
        seen.append(cancel)
        (directory / "retained.log").write_bytes(b"failed charged publication")
        callbacks[signal.SIGTERM](signal.SIGTERM, None)
        assert cancel.is_set()
        raise island.IslandFailure("subscribed cancellation")

    monkeypatch.setattr(island, "_launch_unbounded", kernel)
    path.unlink()  # Wrapper owns its durable job write; accepted descriptor remains in spec.
    with pytest.raises(island.IslandFailure, match="subscribed"):
        OP.execute(spec, path.parent, path.parent)
    assert len(seen) == 1 and island.launch_island is original
    assert (path.parent / "published/retained.log").read_bytes() == b"failed charged publication"
    assert requests and all(r.method == "GET" for r in requests) and not uploads


@pytest.mark.parametrize(
    "fault",
    [
        "two",
        "duplicate",
        "failed-response",
        "cancel",
        "replace-interrupt",
        "symlink",
        "count-bound",
        "response-bound",
    ],
)
def test_real_cli_capture_multiple_original_requests(probe_metadata, monkeypatch, fault):
    import signal

    import httpx

    import hypertrain.miner.core as core

    miner, job, path, challenge, _ = probe_metadata
    cfg = SimpleNamespace(
        run_id=job.run_id,
        owner_hotkey=OWNER.ss58,
        keyfile=path,
        api="https://capture.invalid",
        device="cuda",
        image_digest=job.manifest.training.reference_spec.image_digest,
        workdir=path.parent / "miner",
    )
    spec = OP.Operation(
        operation="live",
        binding="ab" * 32,
        binding_kind="commit-lease",
        hotkey=OWNER.ss58,
        owner=OWNER.ss58,
        role="h0",
        instance_id=1,
        machine_id=5000,
        job=job,
        sources={},
        image_digest=cfg.image_digest,
        cutoff=job.deadline,
        trace=True,
        miner_config="config",
        ca="ca",
        master=cfg.api,
    )
    (path.parent / "config").write_bytes(b"metadata config")
    (path.parent / "ca").write_bytes(b"metadata CA")
    monkeypatch.setattr(OP, "check", lambda *args: None)
    monkeypatch.setattr(core.MinerConfig, "load", lambda *args, **kwargs: cfg)
    monkeypatch.setattr(core, "load_keyfile", lambda *args: OWNER)
    callbacks = {}
    monkeypatch.setattr(
        OP.signal, "signal", lambda sig, handler: callbacks.__setitem__(sig, handler)
    )
    manifest = envelope_v2.seal(OWNER, "RunManifestV2", job.run_id, job.manifest, 101)
    sent = []
    replies = []

    def respond(request):
        if request.method == "GET":
            return httpx.Response(200, json={"manifest_envelope": manifest, "now_round": 2})
        sent.append(request.content)
        content = b' { "original": ' + str(len(sent)).encode() + b" }\n"
        status = 409 if fault == "failed-response" and len(sent) == 2 else 200
        if fault == "response-bound" and len(sent) == 2:
            content = b"x" * ((1 << 20) + 1025)
        replies.append((status, content))
        if fault == "cancel" and len(sent) == 2:
            callbacks[signal.SIGTERM](signal.SIGTERM, None)
        return httpx.Response(status, content=content)

    init = httpx.Client.__init__

    def client_init(self, *args, **kwargs):
        kwargs["transport"] = httpx.MockTransport(respond)
        init(self, *args, **kwargs)

    monkeypatch.setattr(httpx.Client, "__init__", client_init)
    signed = []
    for index in range(5 if fault == "count-bound" else 2):
        body = {
            "w": 0,
            "commit_hash": ("11" if fault == "duplicate" else f"{index + 1:02x}") * 32,
            "received_round": 10,
        }
        raw = envelope_v2.seal(OWNER, "Receipt", job.run_id, body, 101)
        signed.append(json.dumps(raw, indent=2).encode() + b"\n")

    def round_metadata(self, w):
        assert w == job.w
        for data in signed:
            response = self.api.c.post(self.api.base + self.url + "/capture-metadata", content=data)
            response.raise_for_status()
        raise core.MinerError("metadata stop after actual CLI capture")

    monkeypatch.setattr(core.NetworkMiner, "run_round", round_metadata)
    replace = OP.os.replace

    def interrupt(source, destination, **kwargs):
        if destination == "signed-api.json" and len(sent) == 1:
            raise OSError("interrupt second capture replacement")
        return replace(source, destination, **kwargs)

    if fault == "replace-interrupt":
        monkeypatch.setattr(OP.os, "replace", interrupt)
    outside = path.parent / "outside"
    outside.write_bytes(b"preserve")
    if fault == "symlink":
        (path.parent / "signed-api.json").symlink_to(outside)
    path.unlink()
    with pytest.raises(
        (ValueError, OSError, httpx.HTTPStatusError), match="capture|product miner|409|interrupt"
    ):
        OP.execute(spec, path.parent, path.parent)
    assert not (path.parent / "operation-result.json").exists()
    assert outside.read_bytes() == b"preserve" and not list(path.parent.glob(".*.next"))
    if fault == "symlink":
        assert not sent and (path.parent / "signed-api.json").is_symlink()
        return
    records = json.loads((path.parent / "signed-api.json").read_bytes())
    expected = 1 if fault == "replace-interrupt" else 4 if fault == "count-bound" else 2
    assert records == [json.loads(data) for data in signed[:expected]]
    # Exact whitespace/signature bytes survive; duplicate requests are not deduplicated.
    for index in range(expected):
        assert (path.parent / f"signed-api-{index}.bin").read_bytes() == signed[index]
    assert sent == signed[: len(sent)]
    accepted = json.loads((path.parent / "accepted-api.json").read_bytes())
    accepted_count = (
        1 if fault in ("failed-response", "replace-interrupt", "response-bound") else expected
    )
    assert len(accepted) == accepted_count
    for index, row in enumerate(accepted):
        assert row["index"] == index and row["request_hex"] == sent[index].hex()
        assert row["response_hex"] == replies[index][1].hex() and row["status"] == 200
        assert row["response"] == json.loads(replies[index][1])
        provenance = json.loads((path.parent / f"response-api-{index}.json").read_bytes())
        assert provenance["url"].endswith("/capture-metadata") and provenance["method"] == "POST"
    if fault == "failed-response":
        failed = json.loads((path.parent / "response-api-1.json").read_bytes())
        assert failed["status"] == 409 and failed["response_hex"] == replies[1][1].hex()
