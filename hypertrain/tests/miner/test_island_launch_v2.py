"""Real tiny torchrun parent integration; no CUDA inference from CPU checks."""

from __future__ import annotations

import hashlib
import json
import subprocess
import threading
import time
from pathlib import Path

import numpy as np
import pytest
import torch

import hypertrain.trainer  # noqa: F401
from hypertrain.auditor.replay import pack_state, unpack_state
from hypertrain.miner.island_launch import IslandFailure, launch_argv, launch_island
from hypertrain.protocol.example import example_manifest
from hypertrain.protocol.hashing import MerkleTree
from hypertrain.protocol.messages import f32hex
from hypertrain.protocol.messages_v2 import IslandJobV1, RunManifestV2
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.island import emulate, train_island
from hypertrain.trainer.loop import Assignment
from hypertrain.trainer.model import init_params, param_shapes


def tiny_manifest(n: int = 2, ep: int = 1, zero1: bool = True) -> RunManifestV2:
    b = example_manifest().body()
    b["model"].update(
        n_layers=1,
        d_model=8,
        n_heads=2,
        n_kv_heads=2,
        d_ff=12,
        vocab=16,
        seq_len=4,
        n_experts=2 if ep > 1 else 1,
        top_k_experts=1,
        compute_dtype="fp32",
    )
    b["inner"].update(H=2, J=1, micro_batch=1, grad_accum=1, state_policy="reset", rewarmup_steps=0)
    b["inner"]["lr_schedule"].update(warmup=0, stable=100, peak_lr=f32hex(0.001))
    b["reference_spec"]["layout"].update(pp=1, n_gpus=n, dp_size=n // ep, ep_size=ep, zero1=zero1)
    b["model"]["param_count"] = sum(
        int(np.prod(s)) for s in param_shapes(TrainConfig.from_manifest(b).model).values()
    )
    return RunManifestV2.model_validate(
        {
            "manifest_version": 2,
            "training": b,
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


def staged_job(directory: Path, n: int = 2, ep: int = 1) -> IslandJobV1:
    w = tiny_manifest(n, ep)
    cfg = TrainConfig.from_manifest_v2(w)
    theta = init_params(cfg.model)
    rows = np.arange(16 * 5, dtype=np.uint32).reshape(16, 5) % 16
    tree = MerkleTree([r.astype("<u4").tobytes() for r in rows])
    b = w.body()
    b["training"]["dataset"].update(merkle_root=tree.root.hex(), n_samples=16, depth=4)
    w = RunManifestV2.model_validate(b)
    count = w.training.batch_samples()
    inputs = {
        "start_state": pack_state(theta),
        "ef_in": pack_state({k: torch.zeros_like(x) for k, x in theta.items()}),
        "v0": pack_state({}),
        "samples": rows[:count].astype("<u4").tobytes(),
        "sample_proofs": json.dumps(
            [[p.hex() for p in tree.proof(i)] for i in range(count)]
        ).encode(),
    }
    directory.mkdir(parents=True, exist_ok=True)
    for k, data in inputs.items():
        (directory / k).write_bytes(data)
    return IslandJobV1(
        job_version=1,
        run_id=w.run_id(),
        w=1,
        manifest=w,
        sample_ids=list(range(count)),
        global_step0=0,
        start_state_sha256=hashlib.sha256(inputs["start_state"]).hexdigest(),
        ef_in_sha256=hashlib.sha256(inputs["ef_in"]).hexdigest(),
        v0_sha256=hashlib.sha256(inputs["v0"]).hexdigest(),
        object_paths={k: k for k in inputs},
        deadline=int(time.time()) + 120,
    )


@pytest.mark.parametrize("n,ep", [(2, 1), (4, 1), (2, 2), (4, 2)])
def test_real_two_rank_artifacts_match_emulation(tmp_path: Path, n: int, ep: int) -> None:
    job = staged_job(tmp_path, n, ep)
    output = launch_island(job, tmp_path, backend="cpu")
    cfg = TrainConfig.from_manifest_v2(job.manifest)
    rows = np.frombuffer((tmp_path / "samples").read_bytes(), "<u4").reshape(-1, 5)
    theta, _ = unpack_state((tmp_path / "start_state").read_bytes())
    refs = emulate(
        n,
        lambda c: train_island(
            cfg,
            job.manifest.training.reference_spec.layout,
            c,
            theta,
            Assignment(job.run_id, 1, tuple(job.sample_ids)),
            lambda i: rows[i],
        ),
    )
    assert output.delta.read_bytes() == refs[0].delta_payload
    assert output.state.read_bytes() == pack_state(refs[0].final_theta, refs[0].final_state)
    assert launch_island(job, tmp_path, backend="cpu") == output
    with pytest.raises(IslandFailure, match="actual-path trace"):
        launch_island(job, tmp_path, backend="cpu", trace=True)
    cancelled = threading.Event()
    cancelled.set()
    with pytest.raises(IslandFailure, match="cancellation"):
        launch_island(job, tmp_path, backend="cpu", cancel=cancelled)
    with pytest.raises(IslandFailure, match="CUDA oracle"):
        launch_island(job, tmp_path, backend="cuda")
    assert not list(tmp_path.glob("attempt-*"))


def test_n8_launch_uses_manifest_count(tmp_path: Path) -> None:
    job = staged_job(tmp_path, 8)
    assert "--nproc-per-node=8" in launch_argv(job, tmp_path, "cuda")
    assert len(job.sample_ids) == 16
    assert "--max-restarts=0" in launch_argv(job, tmp_path, "cuda")
    with pytest.raises(IslandFailure, match="deadline"):
        launch_island(
            job.model_copy(update={"deadline": int(time.time()) - 1}),
            tmp_path,
            backend="cpu",
        )


def test_wrong_input_and_symlink_leave_no_publication(tmp_path: Path) -> None:
    job = staged_job(tmp_path)
    (tmp_path / "start_state").write_bytes(b"corrupt")
    with pytest.raises(IslandFailure, match="input hash mismatch"):
        launch_island(job, tmp_path, backend="cpu")
    assert not (tmp_path / "published").exists()
    (tmp_path / "start_state").unlink()
    (tmp_path / "start_state").symlink_to("/etc/hosts")
    with pytest.raises(IslandFailure, match="escapes"):
        launch_island(job, tmp_path, backend="cpu")


def test_rank_death_and_live_cancel_teardown(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from hypertrain.miner import island_launch

    job = staged_job(tmp_path)
    original = subprocess.Popen
    cancel = threading.Event()
    spawned = []

    def start(*args, **kwargs):
        proc = original(*args, **kwargs)
        spawned.append(proc)
        cancel.set()
        return proc

    monkeypatch.setattr(island_launch.subprocess, "Popen", start)
    with pytest.raises(IslandFailure, match="rank failure"):
        launch_island(job, tmp_path, backend="cpu", cancel=cancel)
    assert spawned[0].returncode is not None
    assert not (tmp_path / "published").exists()


def test_blocked_child_cancels_on_registered_event(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import socket
    import sys

    from hypertrain.miner import island_launch

    # Given: register readiness before a child blocks indefinitely in signal.pause.
    job = staged_job(tmp_path)
    original = subprocess.Popen
    cancelled = threading.Event()
    reader, writer = socket.socketpair()
    with reader, writer:
        reader.settimeout(10)
        script = f"import os,signal; os.write({writer.fileno()},b'R'); signal.pause()"
        spawned = []

        def start(*args, **kwargs):
            proc = original(*args, **kwargs, pass_fds=(writer.fileno(),))
            spawned.append(proc)
            assert reader.recv(1) == b"R"
            cancelled.set()
            return proc

        monkeypatch.setattr(
            island_launch, "launch_argv", lambda *args: [sys.executable, "-c", script]
        )
        monkeypatch.setattr(island_launch.subprocess, "Popen", start)
        # When/Then: cancellation wakes teardown, reaps the group, publishes nothing.
        with pytest.raises(IslandFailure, match="rank failure"):
            launch_island(job, tmp_path, backend="cpu", cancel=cancelled)
    assert spawned[0].returncode is not None
    assert not (tmp_path / "published").exists()


def test_noisy_child_streams_logs_and_joins_success_watcher(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    import sys
    import tracemalloc

    from hypertrain.miner import island_launch

    job = staged_job(tmp_path)
    original_popen = subprocess.Popen
    original_thread = threading.Thread
    watchers = []

    def spawn(*args, **kwargs):
        assert kwargs["stdout"] != subprocess.PIPE
        assert kwargs["stderr"] != subprocess.PIPE
        return original_popen(*args, **kwargs)

    def thread(*args, **kwargs):
        result = original_thread(*args, **kwargs)
        watchers.append(result)
        return result

    def artifacts(job, directory):
        return island_launch.IslandArtifacts(
            directory,
            directory / "state",
            directory / "ef",
            directory / "delta",
            directory / "leaves",
            (),
        )

    def forbidden_communicate(*args, **kwargs):
        pytest.fail("launcher must not buffer subprocess output with communicate")

    script = "import os; b=b'x'*65536; " + "; ".join(
        ["[os.write(1,b) for _ in range(64)]", "[os.write(2,b) for _ in range(64)]"]
    )
    monkeypatch.setattr(island_launch, "launch_argv", lambda *args: [sys.executable, "-c", script])
    monkeypatch.setattr(island_launch, "validate_artifacts", artifacts)
    monkeypatch.setattr(island_launch.subprocess, "Popen", spawn)
    monkeypatch.setattr(original_popen, "communicate", forbidden_communicate)
    monkeypatch.setattr(island_launch.threading, "Thread", thread)
    cancel = threading.Event()
    tracemalloc.start()
    try:
        result = launch_island(job, tmp_path, backend="cpu", cancel=cancel)
        _, peak = tracemalloc.get_traced_memory()
    finally:
        tracemalloc.stop()
    assert peak < 1024 * 1024  # eight MiB logs never become parent byte buffers
    assert (result.directory / "stdout.log").stat().st_size == 4 * 1024 * 1024
    assert (result.directory / "stderr.log").stat().st_size == 4 * 1024 * 1024
    assert len(watchers) == 1 and not watchers[0].is_alive()
    assert not cancel.is_set()  # completion wakes watcher without faking caller cancellation


def test_noisy_failure_retains_only_bounded_error_tail(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    import sys

    from hypertrain.miner import island_launch

    job = staged_job(tmp_path)
    script = "import os; os.write(2,b'x'*100000+b'END'); raise SystemExit(7)"
    monkeypatch.setattr(island_launch, "launch_argv", lambda *args: [sys.executable, "-c", script])
    with pytest.raises(IslandFailure, match="rank failure 7") as error:
        launch_island(job, tmp_path, backend="cpu")
    assert str(error.value).endswith("END")
    assert len(str(error.value)) <= 4020
    assert not (tmp_path / "published").exists()


def test_capacity_branch_uses_real_bounded_failure_before_publication(tmp_path, monkeypatch):
    import os
    import sys

    from hypertrain.miner import island_launch

    job = staged_job(tmp_path)
    monkeypatch.setattr(island_launch, "_CAPACITY_MEMORY", 64 << 20)
    monkeypatch.setattr(island_launch, "_CAPACITY_CPU_PERCENT", 50)
    calls = []

    def charge():
        calls.append("charged")
        return True

    def argv(original, attempt, backend):
        assert original == job and backend == "cpu"
        assert json.loads((attempt / "job.json").read_text()) == job.model_dump(mode="json")
        for relative in job.object_paths.values():
            assert (attempt / relative).read_bytes() == (tmp_path / relative).read_bytes()
        return [sys.executable, "-c", "raise SystemExit(7)"]

    monkeypatch.setattr(island_launch, "launch_argv", argv)
    capacity = island_launch.CapacityAttempt(
        "77" * 32, "66" * 32, tmp_path / "runtime.lock", charge
    )
    with pytest.raises(IslandFailure, match="execution failed"):
        launch_island(job, tmp_path, backend="cpu", capacity=capacity)
    assert calls == ["charged"]
    assert (tmp_path / "capacity-runtime/capacity-cleaned.json").exists()
    assert not (tmp_path / "published").exists()
    assert os.path.exists(tmp_path / "capacity-runtime/capacity-result.json")


def test_disagreement_corruption_and_publication_resume(tmp_path: Path) -> None:
    from hypertrain.miner.island_launch import validate_artifacts

    job = staged_job(tmp_path)
    result = launch_island(job, tmp_path, backend="cpu", trace=True)
    summary = result.directory / "rank-1" / "summary.json"
    raw = json.loads(summary.read_bytes())
    raw["commitments"]["delta_hash"] = "11" * 32
    summary.write_text(json.dumps(raw))
    with pytest.raises(IslandFailure, match="disagreement"):
        validate_artifacts(job, result.directory)
    raw["commitments"]["delta_hash"] = hashlib.sha256(
        (result.directory / "rank-1" / "delta.bin").read_bytes()
    ).hexdigest()
    summary.write_text(json.dumps(raw))
    result.delta.write_bytes(b"corruption")
    with pytest.raises(IslandFailure, match="compression"):
        launch_island(job, tmp_path, backend="cpu")


def test_published_v0_corruption_rejected(tmp_path: Path) -> None:
    from hypertrain.miner.island_launch import validate_artifacts

    # Given: a complete independently validated N2 publication.
    job = staged_job(tmp_path)
    result = launch_island(job, tmp_path, backend="cpu")
    (result.directory / "v0").write_bytes(b"corrupt derived-state object")
    # When/Then: validation never accepts a changed authenticated input.
    with pytest.raises(IslandFailure, match="published input hash"):
        validate_artifacts(job, result.directory)


def test_checkpoint_optimizer_counter_is_validated(tmp_path: Path) -> None:
    from hypertrain.miner.island_launch import validate_artifacts

    # Given: stage hashes do not include the optimizer step scalar.
    job = staged_job(tmp_path)
    result = launch_island(job, tmp_path, backend="cpu")
    checkpoint = result.directory / "rank-0/checkpoints/1.safetensors"
    theta, state = unpack_state(checkpoint.read_bytes())
    assert state is not None
    state.step += 1
    checkpoint.write_bytes(pack_state(theta, state))
    # When/Then: an otherwise byte-identical checkpoint cannot serve a false counter.
    with pytest.raises(IslandFailure, match="checkpoint leaf state"):
        validate_artifacts(job, result.directory)


def test_experiment_runtime_and_cost_admission_fail_closed() -> None:
    import importlib.util

    path = Path(__file__).parents[2] / "experiments/gpu_network_v2/orchestrate.py"
    spec = importlib.util.spec_from_file_location("network_experiment", path)
    assert spec and spec.loader
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    profile = json.loads((path.parent / "profile.json").read_bytes())
    with pytest.raises(mod.Reject, match="unqualified"):
        mod.runtime_admission(profile, {})
    evidence = {
        "profile_model": profile["model"],
        "layout": profile["layout"],
        "device": "cuda",
        "torch": "2.14.0+cu130",
        "full_round_transfer_bound_seconds": 40,
        "no_slower_hosts": True,
        "measured_rescue_bytes_per_second": 100000000,
        "image_and_dependency_bytes": 10000000000,
        "staging_bytes_per_host": 1000000,
        "image_digest": "sha256:" + "11" * 32,
        "driver_allowlist": ["test-driver"],
        "python": "/opt/hypertrain/venv/bin/python",
        "cuda": "13.0",
        "full_round_artifacts_sha256": "11" * 32,
        "outer_tape_verified": True,
    }
    profile["artifacts"].update(
        rescue_bytes_per_second=100000000,
        image_and_dependency_bytes=10000000000,
        staging_bytes_per_host=1000000,
    )
    profile["runtime"].update(
        image_digest=evidence["image_digest"],
        driver_allowlist=evidence["driver_allowlist"],
        qualified=True,
    )
    from hypertrain.protocol.jcs import canonicalize

    profile.update(
        execution_and_transfer_bound_seconds=40,
        runtime_evidence_sha256=hashlib.sha256(canonicalize(evidence)).hexdigest(),
    )
    assert mod.runtime_admission(profile, evidence) == 3300
    auth = {
        "experiment": "hypertrain-network-v2",
        "currency": "USD",
        "ceiling_usd": "15",
        "max_instances": 2,
        "max_seconds_per_instance": 3600,
        "source_message": "test explicit authorization",
        "date": "2026-10-08",
    }
    snap = {
        "authenticated": True,
        "instances": [],
        "volumes": [],
        "uncapped_charges": False,
        "verified_unix": int(time.time()),
        "prior_attempts_usd": "0",
        "active_liabilities_usd": "0",
        "available_credit_usd": "15",
    }
    offers = [
        {
            "machine_id": i,
            "gpu_name": "RTX 5090",
            "num_gpus": 2,
            "dph_total": "2",
            "storage_cost": "0",
            "inet_up_cost": "0",
            "inet_down_cost": "0",
            "cpu_cores_effective": 8,
            "cpu_ram": 32768,
            "disk_space": 80,
            "verified_unix": int(time.time()),
        }
        for i in [1, 2]
    ]
    assert mod.admission_plan(profile, evidence, auth, snap, offers)["admit"]
    with pytest.raises(mod.Reject, match="authorization"):
        mod.admission_plan(profile, evidence, {}, snap, offers)
    with pytest.raises(mod.Reject, match="unknown"):
        mod.admission_plan(profile, evidence, auth, {**snap, "instances": None}, offers)
    with pytest.raises(mod.Reject, match="ceiling"):
        mod.admission_plan(profile, evidence, auth, {**snap, "prior_attempts_usd": "8"}, offers)


def test_exact_staged_contract_over_real_ssh(tmp_path: Path) -> None:
    import getpass
    import importlib.util
    import os
    import select
    import signal
    import socket
    import sys

    from hypertrain.gpu_ops.journal import Journal
    from hypertrain.gpu_ops.launcher import Orchestrator
    from hypertrain.gpu_ops.provider import Provider
    from hypertrain.gpu_ops.remote import ensure_identity

    tree = Path(__file__).parents[2]
    spec = importlib.util.spec_from_file_location(
        "network_ssh_runtime", tree / "experiments/gpu_network_v2/orchestrate.py"
    )
    assert spec and spec.loader
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    run = tmp_path / "lifecycle"
    run.mkdir()
    journal = Journal(run)
    identity = ensure_identity(run)
    host = tmp_path / "host-key"
    subprocess.run(
        ["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(host)],
        check=True,
        capture_output=True,
    )
    authorized = tmp_path / "authorized_keys"
    authorized.write_bytes(identity.with_suffix(".pub").read_bytes())
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    config = tmp_path / "sshd_config"
    config.write_text(
        f"ListenAddress 127.0.0.1\nPort {port}\nHostKey {host}\n"
        f"PidFile {tmp_path}/sshd.pid\nAuthorizedKeysFile {authorized}\nStrictModes no\n"
        "PermitRootLogin prohibit-password\nPasswordAuthentication no\n"
        f"KbdInteractiveAuthentication no\nUsePAM {'yes' if os.getuid() == 0 else 'no'}\n"
        f"AllowUsers {getpass.getuser()}\nLogLevel VERBOSE\n"
    )
    server = subprocess.Popen(
        ["/usr/sbin/sshd", "-D", "-e", "-f", str(config)],
        stderr=subprocess.PIPE,
        start_new_session=True,
    )
    try:
        assert server.stderr is not None
        assert select.select([server.stderr], [], [], 15)[0]
        assert b"Server listening" in server.stderr.readline()
        cfg = {
            "base_url": "http://127.0.0.1:1",
            "hosts": [{"role": "h0", "machine_id": 1}],
            "remote_root": str(tmp_path / "remote"),
            "remote_python": sys.executable,
            "ssh_user": getpass.getuser(),
            "image": "test@sha256:" + "11" * 32,
        }
        provider = Provider(cfg["base_url"], "a" * 32, run, journal, live=False)
        lifecycle = Orchestrator(cfg, journal, provider, run)
        journal.append("admitted", deadline_unix=time.time() + 3600)
        journal.append("receipt", role="h0", instance_id=1)
        journal.append("instance_ready", role="h0", ssh_host="127.0.0.1", ssh_port=port)
        journal.append("supervisor_ready", supervisor_pid=os.getpid())
        lifecycle.trust("h0")
        job_dir = tmp_path / "job"
        job = staged_job(job_dir)
        (job_dir / "job.json").write_text(job.model_dump_json())
        profile = json.loads((tree / "experiments/gpu_network_v2/profile.json").read_bytes())
        profile["execution_and_transfer_bound_seconds"] = 40
        runtime = mod.NetworkRuntime(lifecycle, profile)
        runtime.stage("h0", tree, {"tiny": job_dir})
        done = runtime.run_staged("h0", "tiny", backend="cpu")
        assert done["ssh_rc"] == 0
        assert runtime.run_staged("h0", "tiny", backend="cpu") == done
        assert len(journal.all("network_job_started")) == 1
        rescued = runtime.rescue("h0")
        assert any("rank-0/checkpoints/" in name for name in rescued["files"])
        assert any("rank-1/checkpoints/" in name for name in rescued["files"])
        assert lifecycle.rescue("h0") == "verified"
        assert not journal.all("provider_call")
    finally:
        os.killpg(server.pid, signal.SIGTERM)
        server.wait(timeout=15)


def test_tiny_full_h_artifact_readiness_and_path_preserving_rescue(
    tmp_path: Path,
) -> None:
    import importlib.util

    from hypertrain.trainer.config import ModelConfig
    from hypertrain.trainer.optim import OptState

    spec = importlib.util.spec_from_file_location(
        "network_budget",
        Path(__file__).parents[2] / "experiments/gpu_network_v2/orchestrate.py",
    )
    assert spec and spec.loader
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    profile = json.loads((Path(spec.origin).parent / "profile.json").read_bytes())
    bounds = mod.artifact_budget(profile)
    assert profile["inner"]["H"] == 30 and profile["inner"]["J"] == 5
    assert profile["inner"]["state_policy"] == "carry"
    assert profile["schedule"]["executions_total"] == 126
    cfg = ModelConfig(
        **{
            k: v
            for k, v in profile["model"].items()
            if k
            not in (
                "param_count",
                "capacity_factor",
                "aux_loss_coef",
                "init_std",
                "master_dtype",
            )
        }
    )
    shapes = param_shapes(cfg)
    assert sum(int(np.prod(x)) for x in shapes.values()) == bounds["param_count"] == 61856
    assert len(shapes) == bounds["tensor_count"]
    theta = init_params(cfg)
    st = OptState(
        {k: torch.zeros_like(x) for k, x in theta.items()},
        {k: torch.zeros_like(x) for k, x in theta.items()},
        30,
    )
    assert len(pack_state(theta, st)) <= bounds["state_bytes"]
    assert bounds["disk_artifacts_peak_bytes"] < 80000000000
    with pytest.raises(mod.Reject, match="measured"):
        mod.readiness_budget(profile, {})
    profile["artifacts"].update(
        image_and_dependency_bytes=10000000000,
        staging_bytes_per_host=1000000,
        rescue_bytes_per_second=bounds["rescue_min_bytes_per_second"],
    )
    ev = {
        "image_and_dependency_bytes": 10000000000,
        "staging_bytes_per_host": 1000000,
        "measured_rescue_bytes_per_second": bounds["rescue_min_bytes_per_second"],
    }
    assert mod.readiness_budget(profile, ev)["priced_transfer_gb_per_host"] > 2
    with pytest.raises(mod.Reject, match="unqualified"):
        mod.staged_command(profile, "/work", "tiny", "/venv/main/bin/python", "wrong", "cuda")
    profile["runtime"].update(
        image_digest="sha256:" + "11" * 32, driver_allowlist=["test"], qualified=True
    )
    command = mod.staged_command(
        profile,
        "/work",
        "tiny",
        "/opt/hypertrain/venv/bin/python",
        profile["runtime"]["image_digest"],
        "cuda",
    )
    assert command.endswith("run.py out/tiny cuda")
    assert "CUBLAS_WORKSPACE_CONFIG=:4096:8" in command
    assert "CUDA_DISABLE_PTX_JIT=1" in command
    assert "MKL_NUM_THREADS=1" in command
    out = tmp_path / "out"
    for rank in range(2):
        p = out / "job" / "published" / f"rank-{rank}" / "checkpoints" / "30.safetensors"
        p.parent.mkdir(parents=True)
        p.write_bytes(bytes([rank]) * 128)
    archive = tmp_path / "rescue.tar"
    mod.export_artifacts(out, archive, profile)
    files = mod.verify_export(archive, bounds["rescue_tar_bytes_per_host"])
    assert len(files) == 2
    assert len({x["sha256"] for x in files.values()}) == 2
    assert all("checkpoints/30.safetensors" in name for name in files)


def test_fresh_account_snapshot_uses_get_only() -> None:
    import importlib.util

    from hypertrain.gpu_ops.provider import Response

    path = Path(__file__).parents[2] / "experiments/gpu_network_v2/orchestrate.py"
    spec = importlib.util.spec_from_file_location("network_get_refresh", path)
    assert spec and spec.loader
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    methods = []

    class ReadOnly:
        def call(self, method, path, tag):
            methods.append((method, path))
            if path == "/api/v0/users/current/":
                data = {"credit": "20.953246133119993"}
            elif path == "/api/v1/instances/":
                data = {"success": True, "instances": []}
            elif path == "/api/v0/volumes/":
                data = {"success": True, "volumes": []}
            else:
                data = {"success": True, "results": [], "next_token": None}
            return Response(200, data, None, "mock", None)

    snapshot = mod.refresh_readonly(ReadOnly())
    assert snapshot["instances"] == snapshot["volumes"] == []
    assert [m for m, _ in methods] == ["GET"] * 4


def test_qualification_cost_admits_without_measured_runtime_but_not_long_workload() -> None:
    import importlib.util

    path = Path(__file__).parents[2] / "experiments/gpu_network_v2/orchestrate.py"
    spec = importlib.util.spec_from_file_location("network_qualification", path)
    assert spec and spec.loader
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    profile = json.loads((path.parent / "profile.json").read_bytes())
    profile["runtime"]["image_digest"] = "sha256:" + "11" * 32
    evidence = {
        "root_verified_bounded_lifecycle": True,
        "registry_image_verified": True,
        "candidate_image_digest": profile["runtime"]["image_digest"],
        "staged_contract_sha256": "22" * 32,
        "deadline_supervisor_contract_sha256": "33" * 32,
    }
    auth = {
        "experiment": "hypertrain-network-v2",
        "currency": "USD",
        "ceiling_usd": "15",
        "max_instances": 2,
        "max_seconds_per_instance": 3600,
        "source_message": "bounded qualification allowed",
        "date": "2026-10-08",
    }
    snapshot = {
        "authenticated": True,
        "instances": [],
        "volumes": [],
        "uncapped_charges": False,
        "verified_unix": int(time.time()),
        "prior_attempts_usd": "0",
        "active_liabilities_usd": "0",
        "available_credit_usd": "20.953246133119993",
    }
    offers = [
        {
            "machine_id": n,
            "gpu_name": "RTX 5090",
            "num_gpus": 2,
            "dph_total": "1",
            "storage_cost": "0.1",
            "inet_up_cost": "0.001",
            "inet_down_cost": "0.001",
            "cpu_cores_effective": 8,
            "cpu_ram": 32768,
            "disk_space": 80,
            "verified_unix": int(time.time()),
        }
        for n in (1, 2)
    ]
    plan = mod.admission_plan(profile, evidence, auth, snapshot, offers, qualification=True)
    assert plan["admit"] and not plan["long_workload_allowed"]
    assert float(plan["hard_deadline_hours"]) == 1  # failure still priced for whole liability
    assert float(plan["worst_case_total_usd"]) <= 15
    with pytest.raises(mod.Reject, match="unqualified"):
        mod.admission_plan(profile, evidence, auth, snapshot, offers)
    with pytest.raises(mod.Reject, match="contract_missing"):
        mod.admission_plan(profile, {}, auth, snapshot, offers, qualification=True)
    with pytest.raises(mod.Reject, match="ceiling"):
        mod.admission_plan(
            profile,
            evidence,
            auth,
            {**snapshot, "prior_attempts_usd": "10"},
            offers,
            qualification=True,
        )
    profile["runtime"]["driver_allowlist"] = ["candidate-driver"]
    cmd = mod.staged_command(
        profile,
        "/work",
        "probe",
        profile["runtime"]["remote_python"],
        profile["runtime"]["image_digest"],
        "cuda",
        qualification=True,
    )
    assert cmd.endswith("probe cuda")
    with pytest.raises(mod.Reject, match="unqualified"):
        mod.staged_command(
            profile,
            "/work",
            "work",
            profile["runtime"]["remote_python"],
            profile["runtime"]["image_digest"],
            "cuda",
        )


def test_d2_execution_plan_invokes_launcher_and_rescues_every_host_on_fault(
    tmp_path: Path, monkeypatch
) -> None:
    import importlib.util
    import os
    import sys

    from hypertrain.gpu_ops.journal import Journal
    from hypertrain.gpu_ops.launcher import Orchestrator
    from hypertrain.gpu_ops.provider import Provider

    path = Path(__file__).parents[2] / "experiments/gpu_network_v2/orchestrate.py"
    spec = importlib.util.spec_from_file_location("network_plan", path)
    assert spec and spec.loader
    mod = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = mod
    spec.loader.exec_module(mod)
    fixture_spec = importlib.util.spec_from_file_location(
        "d2_genuine_service",
        path.parents[2] / "tests/challenge/test_service_network_v2.py",
    )
    assert fixture_spec and fixture_spec.loader
    service = importlib.util.module_from_spec(fixture_spec)
    sys.modules[fixture_spec.name] = service
    fixture_spec.loader.exec_module(service)
    index_path = os.environ.get("HT_GENUINE_CUDA_INDEX")
    if not index_path:
        raise ValueError("genuine CUDA frozen index required")
    context = service.genuine_network(Path(index_path), tmp_path, accepted=False)
    current, *_ = next(context)
    index, files = current.store._genuine_index, current.store._genuine_files
    reservation = files[index.d2_reservation]
    record = json.loads(reservation.read_bytes())["record"]
    cfg = json.loads(files["lifecycle_config"].read_bytes())
    profile = json.loads(files["runtime_profile"].read_bytes())
    evidence = json.loads(files["runtime_evidence"].read_bytes())
    lifecycle_path = Path(index.lifecycle)
    assert lifecycle_path.is_absolute() and not lifecycle_path.is_symlink()
    journal = Journal(lifecycle_path)
    provider = Provider(
        cfg["base_url"],
        "read-only-fixture-no-provider-write",
        lifecycle_path,
        journal,
        live=False,
    )
    lifecycle = Orchestrator(cfg, journal, provider, lifecycle_path)
    runtime = mod.NetworkRuntime(lifecycle, profile)
    sibling_started = threading.Event()
    failed = threading.Event()
    original_append = journal.append

    def append(kind, **fields):
        result = original_append(kind, **fields)
        if kind == "network_plan_fault":
            failed.set()
        return result

    monkeypatch.setattr(journal, "append", append)
    original_ssh = lifecycle.ssh
    first_jobs = {
        role: next(j["name"] for j in record["jobs"] if j["role"] == role) for role in ("h0", "h1")
    }

    def ssh(role):
        current_ssh = original_ssh(role)
        original_run = current_ssh.run

        def run(command, tag, logs):
            executing = tag == "network-" + first_jobs[role]
            if executing and role == "h1":
                sibling_started.set()
            if executing and role == "h0":
                assert sibling_started.wait(timeout=10), "sibling not started"
                result = original_run("/bin/sh -c 'exit 17'", tag, logs)
                assert result.returncode == 17
                raise mod.Reject("actual rank failure")
            result = original_run(command, tag, logs)
            if executing and role == "h1":
                assert failed.wait(timeout=10), "sibling fault not observed"
            return result

        monkeypatch.setattr(current_ssh, "run", run)
        return current_ssh

    monkeypatch.setattr(lifecycle, "ssh", ssh)
    before = len(journal.all("network_execution_intent"))
    try:
        with pytest.raises(mod.Reject, match="actual rank failure"):
            runtime.execute_plan(
                evidence,
                record["jobs"],
                reservation,
                store=current.store,
                beacon=index.beacon,
            )
        assert failed.is_set() and sibling_started.is_set()
        started = journal.all("network_job_started")
        assert {row["role"] for row in started} == {"h0", "h1"}
        assert len(journal.all("network_execution_intent")) > before
        assert journal.last("network_plan_terminal")["failed"] is True
        assert {row["role"] for row in journal.all("network_rescued")} == {"h0", "h1"}
        assert not journal.all("provider_call", write=True)
        assert lifecycle.cfg["cleanup_deadline_unix"] == record["cutoff_unix"]
    finally:
        context.close()


@pytest.mark.parametrize(
    "fault",
    [
        "owner",
        "expired",
        "run",
        "jobs",
        "quota",
        "bool_clock",
        "cutoff",
        "budget",
        "source",
        "missing",
        "replay",
    ],
)
def test_d2_signed_reservation_rejects_before_dispatch(tmp_path, fault):
    """CPU-only signed intake negative; deliberately no qualified runtime claim."""
    import importlib.util
    import os
    import sys
    from types import SimpleNamespace

    from pydantic import ValidationError

    from hypertrain.gpu_ops.journal import Journal, sha256
    from hypertrain.protocol import envelope_v2
    from hypertrain.protocol.jcs import canonicalize
    from hypertrain.protocol.keys import Keypair
    from hypertrain.protocol.messages import Receipt

    spec = importlib.util.spec_from_file_location(
        "d2_negative",
        Path(__file__).parents[2] / "experiments/gpu_network_v2/orchestrate.py",
    )
    assert spec and spec.loader
    mod = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = mod
    spec.loader.exec_module(mod)
    owner = Keypair(b"\x32" * 32)
    run = "ab" * 32
    cutoff = int(time.time()) + 100
    job = {
        "role": "h0",
        "name": "job-0",
        "equivalents": 1,
        "backend": "cuda",
        "directory": str(tmp_path),
        "job_sha256": "11" * 32,
        "job_digest": "11" * 32,
        "inputs": {},
    }
    jobs = [
        {**job, "role": role, "name": "job-" + str(i)} for role in ("h0", "h1") for i in range(61)
    ]
    record = {
        "schema": "ht-d2-execution-reservation/1",
        "action": "execute_plan",
        "owner": owner.ss58,
        "run_id": run,
        "reference_hash": "11" * 32,
        "qualification_authority_hash": "11" * 32,
        "profile_sha256": "11" * 32,
        "runtime_evidence_sha256": "11" * 32,
        "source_map_sha256": "11" * 32,
        "bundle_sha256": "11" * 32,
        "image_digest": "sha256:" + "11" * 32,
        "cutoff_unix": cutoff,
        "budget_authority_sha256": "11" * 32,
        "budget_plan_sha256": "11" * 32,
        "quota": {"total": 126, "per_role": 63, "qualification": 4, "remaining": 122},
        "roles": {"h0": {}, "h1": {}},
        "jobs_sha256": sha256(canonicalize(jobs)),
        "jobs": jobs,
        "files": {},
    }
    if fault == "quota":
        record["quota"]["remaining"] = 126
    if fault == "bool_clock":
        record["cutoff_unix"] = True
    if fault == "cutoff":
        record["cutoff_unix"] = cutoff + 1
    signer = Keypair(b"\x33" * 32) if fault == "owner" else owner
    signed = envelope_v2.seal(
        signer,
        "Receipt",
        "cd" * 32 if fault == "run" else run,
        Receipt(w=0, commit_hash=sha256(canonicalize(record)), received_round=1),
        1 if fault == "expired" else 10,
    )
    path = tmp_path / "reservation.json"
    path.write_bytes(canonicalize({"record": record, "receipt": signed}))
    lifecycle = SimpleNamespace(
        j=Journal(tmp_path),
        cfg={"cleanup_deadline_unix": cutoff},
        deadline=lambda: cutoff,
        cleanup_started=lambda: False,
        admitted=lambda: {"plan": {}},
        roles=["h0", "h1"],
    )
    lifecycle.j.append("network_admission_authority", owner=owner.ss58)
    runtime = mod.NetworkRuntime(lifecycle, {})
    store = SimpleNamespace(owner_hotkey=owner.ss58, _now=lambda _: 2, _db=None)
    if fault == "jobs":
        jobs = list(reversed(jobs))
    if fault == "missing":
        path.unlink()
    if fault in ("budget", "source"):
        lifecycle.j.append("supervisor_ready", supervisor_pid=os.getpid())
        file = tmp_path / fault
        file.write_bytes(b"{}")
        record["files"][fault] = {"path": str(file), "sha256": "00" * 32}
        signed = envelope_v2.seal(
            owner,
            "Receipt",
            run,
            Receipt(w=0, commit_hash=sha256(canonicalize(record)), received_round=1),
            10,
        )
        path.write_bytes(canonicalize({"record": record, "receipt": signed}))
    if fault == "replay":
        lifecycle.j.append("network_plan_intake", digest="ff" * 32)
    with pytest.raises((mod.Reject, ValidationError, FileNotFoundError)):
        runtime.execute_plan({}, jobs, path, store=store, beacon=2)
    assert not lifecycle.j.all("network_execution_intent")
    assert len(lifecycle.j.all("network_plan_intake")) == (1 if fault == "replay" else 0)


@pytest.mark.parametrize("objective", ["mlm", "decision", "distill"])
def test_real_od_two_rank_worker_paths(tmp_path: Path, objective: str) -> None:
    import importlib.util

    spec = importlib.util.spec_from_file_location(
        "worker_od_fixture", Path(__file__).parents[1] / "layout/test_od_island_v2.py"
    )
    assert spec and spec.loader
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    wrapper = mod.od_manifest(objective, 2, True)
    cfg = TrainConfig.from_manifest_v2(wrapper)
    theta = init_params(cfg.model)
    get = mod.samples(cfg)
    count = wrapper.training.batch_samples()
    raw_rows = [get(i).astype("<u2").tobytes() for i in range(count)]
    tree = MerkleTree(raw_rows)
    body = wrapper.body()
    body["training"]["dataset"].update(merkle_root=tree.root.hex(), n_samples=count, depth=2)
    wrapper = RunManifestV2.model_validate(body)
    blobs = {
        "start_state": pack_state(theta),
        "ef_in": pack_state({k: torch.zeros_like(x) for k, x in theta.items()}),
        "v0": pack_state({}),
        "samples": b"".join(raw_rows),
        "sample_proofs": json.dumps(
            [[p.hex() for p in tree.proof(i)] for i in range(count)]
        ).encode(),
    }
    for name, data in blobs.items():
        (tmp_path / name).write_bytes(data)
    job = IslandJobV1(
        job_version=1,
        run_id=wrapper.run_id(),
        w=0,
        manifest=wrapper,
        sample_ids=list(range(count)),
        global_step0=0,
        start_state_sha256=hashlib.sha256(blobs["start_state"]).hexdigest(),
        ef_in_sha256=hashlib.sha256(blobs["ef_in"]).hexdigest(),
        v0_sha256=hashlib.sha256(blobs["v0"]).hexdigest(),
        object_paths={k: k for k in blobs},
        deadline=int(time.time()) + 120,
    )
    result = launch_island(job, tmp_path, backend="cpu", trace=True)
    refs = emulate(
        2,
        lambda c: train_island(
            cfg,
            wrapper.training.reference_spec.layout,
            c,
            theta,
            Assignment(wrapper.run_id(), 0, tuple(job.sample_ids)),
            get,
        ),
    )
    assert result.delta.read_bytes() == refs[0].delta_payload
    assert result.state.read_bytes() == pack_state(refs[0].final_theta, refs[0].final_state)
    assert (result.directory / "rank-0/trace.json").exists()
