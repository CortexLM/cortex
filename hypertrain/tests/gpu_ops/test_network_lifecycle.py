"""Real SSH/separate supervisor, no CREATE or production provider traffic."""

from __future__ import annotations

import getpass
import importlib.util
import json
import os
import select
import signal
import socket
import subprocess
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from types import SimpleNamespace

import pytest

from hypertrain.gpu_ops.journal import Journal, fsha
from hypertrain.gpu_ops.launcher import Orchestrator, Reject
from hypertrain.gpu_ops.provider import Provider
from hypertrain.gpu_ops.remote import ensure_identity

TREE = Path(__file__).parents[2]


def network_module():
    spec = importlib.util.spec_from_file_location(
        "network_lifecycle", TREE / "experiments/gpu_network_v2/orchestrate.py"
    )
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


@pytest.mark.parametrize("fault", [None, "export", "changed-key", "rank", "continuation"])
def test_separate_supervisor_parent_exit_rescues_both_roles_before_delete(
    tmp_path: Path,
    fault: str | None,
    request: pytest.FixtureRequest,
) -> None:
    ssh_servers: list[subprocess.Popen[bytes]] = []
    ports = []

    def stop_servers() -> None:
        # A setup failure before the main try must not leak sshd daemons into the session.
        for proc in ssh_servers:
            if proc.poll() is None:
                os.killpg(proc.pid, signal.SIGTERM)
                proc.wait(timeout=5)

    request.addfinalizer(stop_servers)
    run = tmp_path / "run"
    run.mkdir()
    journal = Journal(run)
    identity = ensure_identity(run)
    for role in ("h0", "h1"):
        host = tmp_path / f"host-{role}"
        subprocess.run(
            ["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(host)],
            check=True,
            capture_output=True,
        )
        authorized = tmp_path / f"authorized-{role}"
        authorized.write_bytes(identity.with_suffix(".pub").read_bytes())
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            ports.append(sock.getsockname()[1])
        cfg_path = tmp_path / f"sshd-{role}"
        cfg_path.write_text(
            f"ListenAddress 127.0.0.1\nPort {ports[-1]}\nHostKey {host}\n"
            f"PidFile {tmp_path}/{role}.pid\nAuthorizedKeysFile {authorized}\nStrictModes no\n"
            "PermitRootLogin prohibit-password\nPasswordAuthentication no\n"
            f"KbdInteractiveAuthentication no\nUsePAM {'yes' if os.getuid() == 0 else 'no'}\n"
            f"AllowUsers {getpass.getuser()}\nLogLevel VERBOSE\n"
        )
        proc = subprocess.Popen(
            ["/usr/sbin/sshd", "-D", "-e", "-f", str(cfg_path)],
            stderr=subprocess.PIPE,
            start_new_session=True,
        )
        ssh_servers.append(proc)
        assert proc.stderr is not None
        assert select.select([proc.stderr], [], [], 15)[0]
        assert b"Server listening" in proc.stderr.readline()

    deleted = set()
    methods = []

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *args):
            return

        def do_GET(self):
            methods.append(("GET", self.path))
            if self.path.startswith("/api/v0/users/current/"):
                body, status = {"credit": 20, "balance": 0}, 200
            elif self.path.startswith("/api/v0/volumes/"):
                body, status = {"volumes": []}, 200
            elif self.path.startswith("/api/v1/instances/"):
                body = {
                    "success": True,
                    "instances": [{"id": i} for i in (1, 2) if i not in deleted],
                }
                status = 200
            else:
                iid = int(self.path.split("?", 1)[0].rstrip("/").split("/")[-1])
                body, status = (
                    ({"success": True, "instances": {"id": iid}}, 200)
                    if iid not in deleted
                    else ({"instances": None}, 200)
                )
            data = json.dumps(body).encode()
            self.send_response(status)
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def do_DELETE(self):
            methods.append(("DELETE", self.path))
            iid = int(self.path.split("?", 1)[0].rstrip("/").split("/")[-1])
            deleted.add(iid)
            data = b'{"success":true}'
            self.send_response(200)
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def do_PUT(self):
            methods.append(("PUT", self.path))
            self.send_error(403)

    api = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    api_thread = threading.Thread(target=api.serve_forever)
    api_thread.start()

    def stop_api() -> None:
        # A non-daemon serve_forever thread left running blocks interpreter exit (CI hang).
        if api_thread.is_alive():
            api.shutdown()
            api.server_close()
            api_thread.join()

    request.addfinalizer(stop_api)
    key = tmp_path / "key"
    key.write_text("a" * 32)
    key.chmod(0o600)
    profile_path = TREE / "experiments/gpu_network_v2/profile.json"
    cfg = {
        "base_url": f"http://127.0.0.1:{api.server_port}",
        "key_file": str(key),
        "hosts": [{"role": r, "machine_id": i} for i, r in enumerate(("h0", "h1"), 1)],
        "cleanup_contract": "network-v2",
        "network_profile_file": str(profile_path),
        "evidence_path": str(tmp_path / "cleanup-evidence.json"),
        "remote_root": str(tmp_path / "remote-{role}"),
        "remote_python": sys.executable,
        "ssh_user": getpass.getuser(),
        "image": "candidate@sha256:" + "11" * 32,
        "provider_min_interval_seconds": 0,
        "poll_seconds": 0,
        "delete_attempts": 1,
        "absence_attempts": 1,
        "supervisor_check_seconds": 30,
    }
    (run / "config.json").write_text(json.dumps(cfg))
    parent = subprocess.Popen(
        [sys.executable, "-c", "import sys; sys.stdin.buffer.read()"], stdin=subprocess.PIPE
    )

    def stop_parent() -> None:
        if parent.poll() is None:
            parent.terminate()
            parent.wait(timeout=5)

    request.addfinalizer(stop_parent)
    journal.append("transaction_open", live=False, pid=parent.pid)
    journal.append("admitted", deadline_unix=__import__("time").time() + 3600)
    orch = Orchestrator(cfg, journal, Provider(cfg["base_url"], "a" * 32, run, journal, False), run)
    runtime = network_module().NetworkRuntime(orch, json.loads(profile_path.read_bytes()))
    for i, role in enumerate(("h0", "h1"), 1):
        journal.append("receipt", role=role, instance_id=i)
        journal.append("instance_ready", role=role, ssh_host="127.0.0.1", ssh_port=ports[i - 1])
        orch.trust(role)
        job = tmp_path / f"job-{role}"
        for rank in (0, 1):
            path = job / f"rank-{rank}" / "checkpoints" / "30.safetensors"
            path.parent.mkdir(parents=True)
            path.write_bytes(f"{role}-{rank}".encode())
        (job / "driver-result.json").write_text(json.dumps({"role": role}))
        # stage() journals the sha256 of each staged job's job.json (current contract).
        (job / "job.json").write_text(json.dumps({"role": role, "name": "driver"}))
        runtime.stage(role, TREE, {"driver": job})
    prior_archives = {}
    if fault == "continuation":
        for role in orch.roles:
            runtime.reserve(role, "qualification", "qualification", 2)
            prior_archives[role] = runtime.rescue(role)
        journal.append("network_workload_promoted")
        for role in orch.roles:
            runtime.reserve(role, "continuation", "workload", 1)
            ssh = orch.ssh(role)
            root = orch.remote_root(role)
            assert (
                ssh.run(
                    f"mkdir -p {root}/out/continuation/checkpoints && "
                    f"printf new-checkpoint > {root}/out/continuation/checkpoints/60.bin",
                    "continuation",
                    run / "logs",
                ).returncode
                == 0
            )
    elif fault == "export":
        root0 = Path(orch.remote_root("h0"))
        (root0 / "out/escape").symlink_to("/etc/hosts")
    elif fault == "changed-key":
        known = orch.ssh("h0").known_hosts
        subprocess.run(
            ["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(tmp_path / "wrong")],
            check=True,
            capture_output=True,
        )
        wrong = (tmp_path / "wrong.pub").read_text().split()[:2]
        known.write_text(f"[127.0.0.1]:{ports[0]} {' '.join(wrong)}\n")
    elif fault == "rank":
        runtime.reserve("h0", "failed-driver", "qualification", 2)
        failed = orch.ssh("h0").run(
            f"{sys.executable} -c 'raise SystemExit(7)'", "failed-rank", run / "logs"
        )
        assert failed.returncode == 7
        journal.append(
            "network_qualification_failed", role="h0", name="failed-driver", rc=failed.returncode
        )
    supervisor = subprocess.Popen(
        [sys.executable, "-m", "hypertrain.gpu_ops.supervisor", str(run)],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        start_new_session=True,
    )
    try:
        assert supervisor.stdout is not None
        assert select.select([supervisor.stdout], [], [], 15)[0]
        assert supervisor.stdout.readline() == b"READY\n"
        parent.terminate()
        parent.wait(timeout=10)
        fd = os.pidfd_open(supervisor.pid)
        try:
            assert select.select([fd], [], [], 45)[0]
        finally:
            os.close(fd)
        custody_fault = fault in ("export", "changed-key")
        assert supervisor.wait(timeout=2) == (4 if custody_fault else 0)
        assert deleted == {1, 2}
        assert len(journal.all("absence_confirmed")) == 2
        assert journal.last("supervisor_parent_exit")
        assert len(journal.all("network_rescued")) == (
            4 if fault == "continuation" else 1 if custody_fault else 2
        )
        if fault == "continuation":
            for role, old in prior_archives.items():
                final = journal.last("network_rescued", role=role)
                assert final and final["tar"] != old["tar"]
                assert "continuation/checkpoints/60.bin" in final["files"]
                assert Path(old["tar"]).is_file()
                assert runtime.rescue(role) == final
        for rec in journal.all("network_rescued"):
            assert any("driver-result.json" in n for n in rec["files"])
            assert len([n for n in rec["files"] if "checkpoints/30.safetensors" in n]) == 2
        records = journal.records()
        last_rescue = max(
            i
            for i, r in enumerate(records)
            if r["kind"] in ("network_rescued", "network_rescue_failed")
        )
        assert all(i > last_rescue for i, r in enumerate(records) if r["kind"] == "delete_ack")
        assert not any(m == "PUT" for m, _ in methods)
        assert bool(journal.last("supervisor_censored")) == custody_fault
        if custody_fault:
            again = orch.cleanup(force=True)
            assert again["all_absent"] and again["censored"]
            assert not again["custody_complete"]
        if fault == "rank":
            assert sum(r["executions"] for r in journal.all("network_execution_intent")) == 2
            assert journal.last("network_workload_promoted") is None
    finally:
        if supervisor.poll() is None:
            os.killpg(supervisor.pid, signal.SIGKILL)
            supervisor.wait(timeout=5)
        stop_parent()
        stop_api()
        stop_servers()


def test_exact_qualification_reservations_and_immutable_deadline(tmp_path: Path) -> None:
    import time

    cfg = {
        "base_url": "http://127.0.0.1:1",
        "hosts": [{"role": "h0"}, {"role": "h1"}],
        "cleanup_contract": "network-v2",
    }
    j = Journal(tmp_path)
    j.append("admitted", deadline_unix=time.time() + 7200)
    first = j.append("create_intent", role="h0")
    orch = Orchestrator(cfg, j, Provider(cfg["base_url"], "a" * 32, tmp_path, j, False), tmp_path)
    expected = first["unix"] + 3600
    j.append("admitted", deadline_unix=time.time() + 10000)
    j.append("create_intent", role="h1")
    assert orch.deadline() == expected
    mod = network_module()
    runtime = mod.NetworkRuntime(
        orch, json.loads((TREE / "experiments/gpu_network_v2/profile.json").read_bytes())
    )
    runtime.reserve("h0", "qual", "qualification", 2)
    with pytest.raises(Reject, match="not_retryable"):
        runtime.reserve("h0", "qual", "qualification", 2)
    runtime.reserve("h1", "qual", "qualification", 2)
    assert sum(r["executions"] for r in j.all("network_execution_intent")) == 4
    with pytest.raises(Reject, match="not_promoted"):
        runtime.reserve("h0", "work", "workload", 1)
    with pytest.raises(Reject, match="not_verified"):
        runtime.record_qualification("h0", "qual", {"passed": False})
    with pytest.raises(Reject, match="exhausted"):
        runtime.reserve("h0", "retryqual", "qualification", 2)
    j.append("network_workload_promoted", remaining_total=122, remaining_per_host=61)
    for role in ("h0", "h1"):
        runtime.reserve(role, "trace-bundle", "workload", 12)
        for i in range(49):
            runtime.reserve(role, f"work-{i}", "workload", 1)
        with pytest.raises(Reject, match="exhausted"):
            runtime.reserve(role, "overflow", "workload", 1)
    assert sum(r["executions"] for r in j.all("network_execution_intent")) == 126


def test_qualification_entrypoint_consumes_frozen_schema(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:

    from hypertrain.gpu_ops.journal import fsha

    j = Journal(tmp_path)
    first = j.append("create_intent", role="h0")
    j.append("receipt", role="h0", instance_id=1)
    j.append("supervisor_ready")
    j.append("staged", role="h0")
    cfg = {
        "base_url": "http://127.0.0.1:1",
        "hosts": [{"role": "h0"}],
        "cleanup_contract": "network-v2",
        "remote_root": str(tmp_path / "remote-{role}"),
    }
    orch = Orchestrator(cfg, j, Provider(cfg["base_url"], "a" * 32, tmp_path, j, False), tmp_path)
    known = tmp_path / "known_hosts-h0"
    known.write_text("strict-key-custody")
    image = "sha256:" + "11" * 32
    root = Path(orch.remote_root("h0"))
    payload = {
        "tree": str(root),
        "profile": str(root / "profile.json"),
        "profile_sha256": "22" * 32,
        "sources": {},
        "registry_manifest": str(root / "registry.json"),
        "image_digest": image,
        "driver_allowlist": ["580"],
        "seed_job": str(root / "seed.json"),
        "hotkey": "h0",
        "output": str(root / "out/qualification"),
        "admitted_unix": int(first["unix"]),
        "instance_id": 1,
        "machine_id": 2,
        "role": "h0",
        "lifecycle_contract_sha256": "33" * 32,
        "known_hosts_sha256": fsha(known),
        "lifecycle_receipts": str(root / "receipts.json"),
        "lifecycle_receipts_sha256": "44" * 32,
    }
    commands = []

    def get(remote, local):
        local.write_text(json.dumps(payload))

    def run(command, tag, logs):
        commands.append(command)
        return SimpleNamespace(returncode=0)

    ssh = SimpleNamespace(get=get, run=run, known_hosts=known, timeout=0)
    monkeypatch.setattr(orch, "ssh", lambda role: ssh)
    mod = network_module()
    monkeypatch.setattr(
        mod,
        "staged_command",
        lambda *a, **kw: "python experiments/gpu_network_v2/run.py out/probe cuda",
    )
    runtime = mod.NetworkRuntime(
        orch, json.loads((TREE / "experiments/gpu_network_v2/profile.json").read_bytes())
    )
    orch.cfg.update(image="candidate@" + image, remote_python="python")
    result = runtime.run_qualification("h0", "probe", "qualification.json")
    assert result["executions"] == 2 and result["workload_allowed"] is False
    assert commands == ["python scripts/network_gpu_qualification.py run qualification.json"]
    driver = sys.modules["network_driver_contract"]
    parsed = driver.Qualification.model_validate(payload)
    assert parsed.tree == root
    with pytest.raises(__import__("pydantic").ValidationError):
        driver.Qualification.model_validate({**payload, "instance_id": True})


@pytest.mark.parametrize("left,gap", [(0.0, 0.0), (0.5, 0.0), (0.5, 1.0), (1.0, 0.25)])
def test_cleanup_recovery_deadline_covers_both_roles_and_pacing(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, left: float, gap: float
) -> None:
    import hypertrain.gpu_ops.launcher as launcher
    from hypertrain.gpu_ops.provider import Response

    clock = [100.0]
    monkeypatch.setattr(launcher.time, "time", lambda: clock[0])
    cfg = {
        "base_url": "http://127.0.0.1:1",
        "hosts": [{"role": r} for r in ("h0", "h1")],
        "cleanup_contract": "network-v2",
        "cleanup_deadline_unix": 100.0 + left,
        "provider_min_interval_seconds": gap,
    }
    j = Journal(tmp_path)
    j.append("admitted", deadline_unix=3700)
    for role in ("h0", "h1"):
        j.append("create_intent", role=role, label=role, baseline_ids=[])
    waits, calls = [], []

    def sleep(seconds):
        waits.append(seconds)
        clock[0] += seconds

    provider = Provider(cfg["base_url"], "a" * 32, tmp_path, j, False, sleep=sleep)
    orch = Orchestrator(cfg, j, provider, tmp_path)
    monkeypatch.setattr(provider, "monotonic", lambda: clock[0])
    endpoint = provider.endpoint("GET", "/api/v1/instances/")
    provider.last[endpoint] = provider.monotonic()

    def call(method, path, tag, body=None, timeout=30):
        calls.append(timeout)
        clock[0] += timeout
        return Response(429, {}, 15.0, "mock", None)

    monkeypatch.setattr(provider, "call", call)
    owned, unresolved = orch.owned()
    assert owned == [] and unresolved == ["h0", "h1"]
    assert calls == ([left - gap] if left > gap else [])
    assert waits == ([gap] if 0 < gap < left else [])
    clock[0] = 100.0 + left
    assert orch.absent_once(1, "expired") == "unknown"
    j.append("receipt", role="h0", instance_id=1)
    assert orch.delete("h0") is False
    assert len(calls) <= 1
    cfg["network_profile_file"] = str(TREE / "experiments/gpu_network_v2/profile.json")
    result = orch.cleanup(force=True)
    assert result["all_absent"] is False and result["censored"] is True
    assert result["custody_complete"] is False
    assert cfg["cleanup_deadline_unix"] == 100.0 + left


def test_cleanup_absence_show_and_delete_share_shrinking_clock(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import hypertrain.gpu_ops.launcher as launcher
    from hypertrain.gpu_ops.provider import Response

    clock = [100.0]
    monkeypatch.setattr(launcher.time, "time", lambda: clock[0])
    cfg = {
        "base_url": "http://127.0.0.1:1",
        "hosts": [{"role": "h0"}, {"role": "h1"}],
        "cleanup_deadline_unix": 101.0,
        "provider_min_interval_seconds": 0,
    }
    j = Journal(tmp_path)
    provider = Provider(cfg["base_url"], "a" * 32, tmp_path, j, False)
    orch = Orchestrator(cfg, j, provider, tmp_path)
    waits, calls = [], []
    monkeypatch.setattr(provider, "sleep", waits.append)

    def call(method, path, tag, body=None, timeout=30):
        calls.append((method, timeout))
        clock[0] += 0.25
        return Response(
            200,
            {"success": True, "instances": [] if "/api/v1/" in path else None},
            None,
            "mock",
            None,
        )

    monkeypatch.setattr(provider, "call", call)
    for role in ("h0", "h1"):
        j.append("receipt", role=role, instance_id=1)
    assert orch.absent_once(1, "h0") == "absent"
    assert orch.delete("h0") is True
    assert orch.delete("h1") is True
    assert orch.confirm_absence("h1") is False
    assert calls == [("GET", 1.0), ("GET", 0.75), ("DELETE", 0.5), ("DELETE", 0.25)]
    assert waits == []


@pytest.mark.parametrize("status,parsed", [(429, {}), (200, {"success": True, "instances": "bad"})])
def test_unknown_inventory_never_confirms_absence(
    tmp_path: Path, status: int, parsed: dict
) -> None:
    from hypertrain.gpu_ops.provider import Response

    cfg = {"base_url": "http://127.0.0.1:1", "hosts": [{"role": "h0"}]}
    j = Journal(tmp_path)
    provider = Provider(cfg["base_url"], "a" * 32, tmp_path, j, False)
    provider.call = lambda *args, **kwargs: Response(status, parsed, None, "mock", None)
    orch = Orchestrator(cfg, j, provider, tmp_path)
    assert orch.absent_once(1, "unknown") == "unknown"


def test_promotion_requires_real_qualification_and_remaining_original_clock(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    import time

    mod = network_module()
    cfg = {
        "base_url": "http://127.0.0.1:1",
        "hosts": [{"role": "h0"}, {"role": "h1"}],
        "cleanup_contract": "network-v2",
    }
    j = Journal(tmp_path)
    j.append("admitted", deadline_unix=time.time() + 3600)
    orch = Orchestrator(cfg, j, Provider(cfg["base_url"], "a" * 32, tmp_path, j, False), tmp_path)
    profile = json.loads((TREE / "experiments/gpu_network_v2/profile.json").read_bytes())
    profile["execution_and_transfer_bound_seconds"] = 40
    runtime = mod.NetworkRuntime(orch, profile)
    for role in orch.roles:
        runtime.reserve(role, "qual", "qualification", 2)
    # Reservation state-machine oracle only; actual runtime_admission stays tested separately.
    monkeypatch.setattr(mod, "runtime_admission", lambda *args: 3300)
    with pytest.raises(Reject, match="promotion_gate"):
        runtime.promote({}, {"admit": True, "long_workload_allowed": True})
    for role in orch.roles:
        verified = {
            "passed": True,
            "root_verified": True,
            "result_sha256": "11" * 32,
            "artifact_manifest_sha256": "22" * 32,
        }
        done = runtime.record_qualification(role, "qual", verified)
        assert runtime.record_qualification(role, "qual", verified) == done
        with pytest.raises(Reject, match="changed_on_resume"):
            runtime.record_qualification(role, "qual", {**verified, "result_sha256": "44" * 32})
    promoted = runtime.promote(
        {"root_reviewed_two_host_full_round_transfer": True},
        {"admit": True, "long_workload_allowed": True},
    )
    assert promoted["remaining_total"] == 122
    assert promoted["remaining_per_host"] == 61
    j.append("admitted", deadline_unix=time.time() + 300)
    with pytest.raises(Reject, match="promotion_gate"):
        runtime.promote(
            {"root_reviewed_two_host_full_round_transfer": True},
            {"admit": True, "long_workload_allowed": True},
        )


def od_large_profile(per_host: int) -> dict:
    """Decoder profile with the od-large/8-GPU geometry of the run manifest (task 6 shape)."""
    profile = json.loads((TREE / "experiments/gpu_network_v2/profile.json").read_bytes())
    profile["model"] |= {
        "arch": "od-encoder",
        "n_layers": 24,
        "d_model": 1024,
        "n_heads": 16,
        "n_kv_heads": 16,
        "d_ff": 4096,
        "n_experts": 1,
        "top_k_experts": 1,
        "vocab": 50432,
        "seq_len": 1024,
        "param_count": 353861632,
        "od": {
            "preset": "od-large",
            "head_layers": 4,
            "objective": "mlm",
            "mask_ratio": "9a99993e",
            "mask_seed": 7,
            "tokenizer_offset": 3,
            "warm_start": False,
            "record": None,
            "decision_rule": "log",
            "rps_weight": "0000803f",
            "distill_temp": "0000803f",
        },
    }
    profile["outer"] = {"opt": "nesterov", "topk_frac": "0000803f", "bits": 8}
    profile["inner"] |= {"H": 4, "J": 2}
    profile["layout"] = {"pp": 1, "n_gpus": 8, "dp_size": 8, "ep_size": 1, "zero1": False}
    profile["schedule"] |= {"executions_per_host": per_host, "executions_total": 2 * per_host}
    return profile


def test_eight_gpu_od_large_budget_is_geometry_derived() -> None:
    mod = network_module()
    decoder = json.loads((TREE / "experiments/gpu_network_v2/profile.json").read_bytes())
    assert mod.execution_quota(decoder) == {
        "total": 126,
        "per_role": 63,
        "qualification": 4,
        "remaining": 122,
    }
    bounds = mod.artifact_budget(od_large_profile(3))
    assert bounds["param_count"] == 353861632 and bounds["tensor_count"] == 339
    # Task 19 measured: state 4246444200, ef 1415481880, delta 359411534,
    # eight-rank execution output 150085361712 bytes; bounds must cover all of them.
    assert bounds["state_bytes"] >= 4246444200 - 65536
    assert bounds["execution_bytes"] >= 150085361712
    assert bounds["retained_bytes_per_host"] == 3 * bounds["execution_bytes"]
    assert mod.execution_quota(od_large_profile(3))["remaining"] == 2
    for change in (
        {"param_count": 353861631},
        {"n_kv_heads": 8},
    ):
        profile = od_large_profile(3)
        profile["model"] |= change
        with pytest.raises(Reject, match="unsupported_artifact_geometry"):
            mod.artifact_budget(profile)
    profile = od_large_profile(3)
    profile["layout"]["dp_size"] = 4
    with pytest.raises(Reject, match="unsupported_artifact_geometry"):
        mod.artifact_budget(profile)
    for schedule in (
        {"executions_total": 7},
        {"executions_per_host": 2, "executions_total": 4},
        {"executions_per_host": True, "executions_total": 2},
    ):
        profile = od_large_profile(3)
        profile["schedule"] |= schedule
        with pytest.raises(Reject, match="execution_reservation_mismatch"):
            mod.execution_quota(profile)


def test_eight_gpu_reservations_follow_profile_quota(tmp_path: Path) -> None:
    import time

    cfg = {
        "base_url": "http://127.0.0.1:1",
        "hosts": [{"role": "h0"}, {"role": "h1"}],
        "cleanup_contract": "network-v2",
    }
    j = Journal(tmp_path)
    j.append("admitted", deadline_unix=time.time() + 7200)
    orch = Orchestrator(cfg, j, Provider(cfg["base_url"], "a" * 32, tmp_path, j, False), tmp_path)
    runtime = network_module().NetworkRuntime(orch, od_large_profile(4))
    for role in ("h0", "h1"):
        runtime.reserve(role, "qual", "qualification", 2)
    with pytest.raises(Reject, match="exhausted"):
        runtime.reserve("h0", "requal", "qualification", 2)
    j.append("network_workload_promoted", remaining_total=4, remaining_per_host=2)
    for role in ("h0", "h1"):
        runtime.reserve(role, "work-0", "workload", 1)
        runtime.reserve(role, "work-1", "workload", 1)
        with pytest.raises(Reject, match="exhausted"):
            runtime.reserve(role, "overflow", "workload", 1)
    assert sum(r["executions"] for r in j.all("network_execution_intent")) == 8


def test_prune_rescued_policy_budget_bounds_and_requires_local_custody() -> None:
    mod = network_module()
    # Legacy default retains every execution remotely plus a remote rescue.tar: > 400GB.
    assert mod.artifact_budget(od_large_profile(4))["disk_artifacts_peak_bytes"] > 400_000_000_000
    pruned = od_large_profile(7)
    pruned["artifacts"] |= {"remote_retention": "prune_rescued"}
    bounds = mod.artifact_budget(pruned)
    # Streamed rescue: only the two resident round directories remain remotely, < 400GB with
    # image and staging; every finished execution is in local custody before it is pruned.
    assert bounds["disk_artifacts_peak_bytes"] == 2 * bounds["execution_bytes"]
    assert bounds["disk_artifacts_peak_bytes"] < 400_000_000_000
    # 7 per host = 2 qualification + 5 workload executions per host (10 total) within 4.7TB.
    assert bounds["local_custody_bytes_required"] < 4_700_000_000_000
    larger = od_large_profile(8)
    larger["artifacts"] |= {"remote_retention": "prune_rescued"}
    assert mod.artifact_budget(larger)["local_custody_bytes_required"] > 4_700_000_000_000
    for change in (
        {"remote_retention": "delete"},
        {"remote_retention": "prune_rescued", "retain_all_checkpoints": False},
        {"remote_retention": "prune_rescued", "path_preserving_rescue": False},
    ):
        bad = od_large_profile(7)
        bad["artifacts"] |= change
        with pytest.raises(Reject, match="unsupported_remote_retention_policy"):
            mod.artifact_budget(bad)
    artifacts = pruned["artifacts"] | prune_inputs() | {"disk_bytes": 400_000_000_000}
    rate = bounds["rescue_min_bytes_per_second"]
    artifacts["rescue_bytes_per_second"] = rate
    evidence = {
        "measured_rescue_bytes_per_second": rate,
        "image_and_dependency_bytes": 1000,
        "staging_bytes_per_host": 1000,
    }
    with pytest.raises(Reject, match="local_custody_disk_budget_exceeded"):
        mod.readiness_budget(pruned | {"artifacts": artifacts}, evidence)
    artifacts["local_custody_bytes"] = 4_700_000_000_000
    ready = mod.readiness_budget(pruned | {"artifacts": artifacts}, evidence)
    assert ready["remote_retention"] == "prune_rescued"
    # The image and staging the host still needs are part of the 400GB check, not waived.
    artifacts |= {"image_and_dependency_bytes": 60_000_000_000}
    evidence |= {"image_and_dependency_bytes": 60_000_000_000}
    with pytest.raises(Reject, match="artifact_disk_budget_exceeded"):
        mod.readiness_budget(pruned | {"artifacts": artifacts}, evidence)


def prune_inputs() -> dict:
    return {"image_and_dependency_bytes": 1000, "staging_bytes_per_host": 1000}


class PruneSsh:
    def __init__(self, calls: list[str], rc: int = 0) -> None:
        self.calls, self.rc = calls, rc

    def run(self, command: str, tag: str, log_dir: Path) -> subprocess.CompletedProcess[str]:
        self.calls.append(command)
        return subprocess.CompletedProcess(command, self.rc, "", "")


def prune_runtime(tmp_path: Path, n_gpus: int = 2):
    import tarfile

    mod = network_module()
    profile = json.loads((TREE / "experiments/gpu_network_v2/profile.json").read_bytes())
    profile["artifacts"]["remote_retention"] = "prune_rescued"
    (tmp_path / "run").mkdir()
    j = Journal(tmp_path / "run")
    calls: list[str] = []
    lifecycle = SimpleNamespace(
        roles=("h0", "h1"),
        dir=tmp_path / "run",
        remote_root=lambda role: "/root/ht",
        ssh=lambda role: PruneSsh(calls),
        cleanup_started=lambda: False,
    )
    runtime = mod.NetworkRuntime.__new__(mod.NetworkRuntime)
    runtime.lifecycle, runtime.journal, runtime.profile = lifecycle, j, profile

    def rescued(names: list[str]) -> dict:
        src = tmp_path / "remote" / str(len(j.records()))
        files = {}
        for name in names:
            for rank in range(n_gpus):
                rel = f"{name}/published/rank-{rank}/summary.json"
                (src / rel).parent.mkdir(parents=True, exist_ok=True)
                (src / rel).write_bytes(b"{}")
        archive = tmp_path / f"rescue-{len(j.records())}.tar"
        mod.export_artifacts(src, archive, profile)
        with tarfile.open(archive) as t:
            files = json.loads(t.extractfile("manifest.json").read())
        return j.append(
            "network_rescued", role="h0", tar=str(archive), tar_sha256=fsha(archive), files=files
        )

    return mod, runtime, j, calls, rescued


def test_prune_rescued_deletes_only_verified_completed_remote_outputs(tmp_path: Path) -> None:
    mod, runtime, j, calls, rescued = prune_runtime(tmp_path)
    custody = rescued(["op-a"])
    # Started but not done (active/interrupted operation): never pruned.
    j.append("network_job_started", role="h0", name="op-b")
    assert runtime.prune_rescued("h0") == []
    j.append("network_operation_done", role="h0", name="op-a", tar_sha256=custody["tar_sha256"])
    j.append("network_qualification_done", role="h0", name="qual", result={})
    # Qualification is retained until promotion; op-a is verified, so only op-a goes.
    assert [r["name"] for r in runtime.prune_rescued("h0")] == ["op-a"]
    assert calls == ["rm -rf -- /root/ht/out/op-a"]
    assert runtime.prune_rescued("h0") == []  # idempotent, never re-deletes
    runtime.profile["artifacts"]["remote_retention"] = "all"
    with pytest.raises(Reject, match="policy_disabled"):
        runtime.prune_rescued("h0")


@pytest.mark.parametrize("fault", ["missing", "tampered", "symlink", "rank", "interrupted", "rm"])
def test_prune_rescued_refuses_without_intact_local_custody(tmp_path: Path, fault: str) -> None:
    # "rank": the rescued tar holds rank-0 only while the profile layout has two ranks.
    mod, runtime, j, calls, rescued = prune_runtime(tmp_path, 1 if fault == "rank" else 2)
    custody = rescued(["op-a"])
    archive = Path(custody["tar"])
    if fault == "missing":
        archive.unlink()
    elif fault == "tampered":
        archive.write_bytes(archive.read_bytes()[:-1] + b"x")
    elif fault == "symlink":
        real = archive.with_suffix(".real")
        archive.rename(real)
        archive.symlink_to(real)
    tar = custody["tar_sha256"] if fault != "interrupted" else "0" * 64
    j.append("network_operation_done", role="h0", name="op-a", tar_sha256=tar)
    if fault == "rm":
        runtime.lifecycle.ssh = lambda role: PruneSsh(calls, rc=1)
    expected = {
        "missing": "local_custody_missing",
        "tampered": "local_custody_changed",
        "symlink": "local_custody_missing",
        "rank": "rank_custody_incomplete",
        "interrupted": "without_rescued_custody",
        "rm": "remote_prune_failed",
    }[fault]
    with pytest.raises(Reject, match=expected):
        runtime.prune_rescued("h0")
    assert calls == (["rm -rf -- /root/ht/out/op-a"] if fault == "rm" else [])
    assert j.last("network_remote_pruned") is None


def test_prune_rescued_qualification_only_after_promotion_and_manifest_binding(
    tmp_path: Path,
) -> None:
    mod, runtime, j, calls, rescued = prune_runtime(tmp_path)
    custody = rescued(["qual"])
    digest = mod.sha256(json.dumps(custody["files"], sort_keys=True).encode())
    j.append(
        "network_qualification_done",
        role="h0",
        name="qual",
        result={"artifact_manifest_sha256": "f" * 64},
    )
    assert runtime.prune_rescued("h0") == []  # not promoted: both rounds stay remote
    j.append("network_workload_promoted", remaining_total=122, remaining_per_host=61)
    with pytest.raises(Reject, match="without_rescued_custody"):
        runtime.prune_rescued("h0")  # recorded manifest differs from local custody
    j.append(
        "network_qualification_done",
        role="h0",
        name="qual",
        result={"artifact_manifest_sha256": digest},
    )
    assert [r["name"] for r in runtime.prune_rescued("h0")] == ["qual"]
    assert calls == ["rm -rf -- /root/ht/out/qual"]


@pytest.mark.parametrize("fault", [None, "rc", "digest", "truncated"])
def test_streamed_rescue_writes_no_remote_archive_and_verifies_locally(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, fault: str | None
) -> None:
    mod = network_module()
    remote = tmp_path / "remote"
    (remote / "out/op-a/published/rank-0").mkdir(parents=True)
    (remote / "out/op-a/published/rank-0/summary.json").write_bytes(b"{}")
    (remote / "experiments/gpu_network_v2").mkdir(parents=True)
    profile = json.loads((TREE / "experiments/gpu_network_v2/profile.json").read_bytes())
    profile["artifacts"]["remote_retention"] = "prune_rescued"
    (remote / "experiments/gpu_network_v2/profile.json").write_text(json.dumps(profile))
    run = tmp_path / "run"
    (run / "logs").mkdir(parents=True)
    script = TREE / "experiments/gpu_network_v2/orchestrate.py"
    tail = {
        None: "",
        "rc": "; exit 3",
        "digest": "; echo deadbeef >&2",
        "truncated": " | head -c 1024",
    }[fault or None]
    monkeypatch.setattr(
        mod,
        "ssh_argv",
        lambda ssh, command: [
            "sh",
            "-c",
            f"cd {remote} && PYTHONPATH={TREE / 'src'} {sys.executable} {script} export out - "
            f"experiments/gpu_network_v2/profile.json{tail}",
        ],
    )
    j = Journal(run)
    runtime = mod.NetworkRuntime.__new__(mod.NetworkRuntime)
    runtime.journal, runtime.profile = j, profile
    runtime.lifecycle = SimpleNamespace(
        dir=run,
        cfg={"remote_python": sys.executable},
        remote_root=lambda role: str(remote),
        ssh=lambda role: SimpleNamespace(timeout=60),
    )
    if fault is None:
        record = runtime.rescue("h0", fresh=True)
        assert set(record["files"]) == {"op-a/published/rank-0/summary.json"}
        assert fsha(Path(record["tar"])) == record["tar_sha256"]
        assert not (remote / "rescue.tar").exists()  # nothing archived remotely
    else:
        with pytest.raises(Reject):
            runtime.rescue("h0", fresh=True)
        assert j.last("network_rescued") is None
        assert not list((run / "rescue/h0").iterdir())  # partial archive removed


def hash_rescue_tree(tmp_path: Path) -> tuple[Path, dict]:
    """A 2-rank qualification output whose result.json claims the exact round-file hashes."""
    from hypertrain.gpu_ops import network_qualification as driver

    src = tmp_path / "remote-out"
    q = src / "qualification"
    rounds = []
    for index in range(2):
        claimed = {}
        for rel in driver.round_files(2, 30, 5):
            path = q / f"round-{index}/published" / rel
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(bytes([index]) * 4096)
            claimed[rel] = fsha(path)
        for rank in range(2):
            (q / f"round-{index}/published/rank-{rank}/summary.json").write_text("{}")
        (q / f"round-{index}/published/job.json").write_text("{}")
        rounds.append(claimed)
    result = {"rounds": rounds}  # only the rounds field is read before full validation fails
    (q / "config.json").write_text("{}")
    profile = json.loads((TREE / "experiments/gpu_network_v2/profile.json").read_bytes())
    return q, {"result": result, "profile": profile, "src": src}


def write_result(q: Path, rounds: list[dict]) -> None:
    from hypertrain.gpu_ops import network_qualification as driver

    env = {
        "backend": "cuda",
        "torch": "2.14.0+cu130",
        "cuda": "13.0",
        "python": "/opt/hypertrain/venv/bin/python",
        "torch_path": "/venv/main/torch/__init__.py",
        "drivers": ["595.84"] * 2,
        "sm_counts": [170] * 2,
        "image_digest": "sha256:" + "44" * 32,
    }
    result = driver.Result(
        status="CUDA_ROUNDS_CAPTURED_NOT_WORKLOAD_PASS",
        config_sha256="11" * 32,
        profile_sha256="22" * 32,
        sources={},
        environment=env,
        instance_id=1,
        machine_id=2,
        role="h0",
        rounds=rounds,
        elapsed_seconds=1,
        workload_allowed=False,
    )
    (q / "result.json").write_text(result.model_dump_json())


def test_hash_only_export_carries_hashes_and_small_json_only(tmp_path: Path) -> None:
    import tarfile

    mod = network_module()
    q, ctx = hash_rescue_tree(tmp_path)
    write_result(q, ctx["result"]["rounds"])
    archive = tmp_path / "hash.tar"
    mod.export_artifacts(ctx["src"], archive, ctx["profile"], True)
    files = mod.verify_export(archive, 1 << 30, True)
    with tarfile.open(archive) as t:
        carried = {m.name for m in t.getmembers()}
    # Every remote file is hashed; only result/config/job/summary bytes travel.
    # 2 rounds x (2 ranks x 12 round files + 2 summaries + job.json) + config + result.
    assert len(files) == 2 * (2 * 12 + 2 + 1) + 2
    assert carried == {"manifest.json"} | {
        "out/" + n for n in files if Path(n).name in mod.HASH_RESCUE_NAMES
    }
    assert not any(n.endswith(".safetensors") for n in carried)
    # The same archive is not a full rescue: the full verifier refuses missing bytes.
    with pytest.raises(Reject, match="coverage_mismatch"):
        mod.verify_export(archive, 1 << 30)


@pytest.mark.parametrize("fault", ["claim", "withheld", "big", "malformed", "unsafe", "budget"])
def test_hash_only_verify_export_rejects(tmp_path: Path, fault: str) -> None:
    import io
    import tarfile

    mod = network_module()
    q, ctx = hash_rescue_tree(tmp_path)
    rounds = ctx["result"]["rounds"]
    if fault == "claim":  # result claims a hash the exported file does not have
        rounds[1]["rank-1/checkpoints/30.safetensors"] = "ab" * 32
    write_result(q, rounds)
    if fault == "big":  # an oversized small-name file is refused at export
        (q / "round-0/published/rank-0/summary.json").write_bytes(b" " * ((1 << 20) + 1))
        with pytest.raises(Reject, match="hash_rescue_file_oversized"):
            mod.export_artifacts(ctx["src"], tmp_path / "x.tar", ctx["profile"], True)
        return
    archive = tmp_path / "hash.tar"
    mod.export_artifacts(ctx["src"], archive, ctx["profile"], True)
    if fault in ("withheld", "malformed", "unsafe"):
        with tarfile.open(archive) as t:
            members = [(m, t.extractfile(m).read()) for m in t.getmembers()]
        with tarfile.open(archive, "w") as t:
            for m, data in members:
                if fault == "withheld" and m.name.endswith("rank-1/summary.json"):
                    continue
                if m.name == "manifest.json" and fault != "withheld":
                    files = json.loads(data)
                    if fault == "malformed":
                        files["qualification/config.json"]["sha256"] = "Z" * 64
                    else:
                        files["../escape/state.safetensors"] = {"bytes": 1, "sha256": "0" * 64}
                    data = json.dumps(files).encode()
                    m.size = len(data)
                t.addfile(m, io.BytesIO(data))
    match = {
        "claim": "result_hash_mismatch",
        "withheld": "coverage_mismatch",
        "malformed": "manifest_malformed",
        "unsafe": "unsafe_path",
        "budget": "budget_exceeded",
    }[fault]
    with pytest.raises(Reject, match=match):
        mod.verify_export(archive, 4096 if fault == "budget" else 1 << 30, True)


@pytest.mark.parametrize("workload", [False, True])
def test_streamed_rescue_is_hash_only_until_a_workload_is_reserved(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, workload: bool
) -> None:
    import tarfile

    mod = network_module()
    remote = tmp_path / "remote"
    rank = remote / "out/qualification/round-0/published/rank-0"
    rank.mkdir(parents=True)
    (rank / "summary.json").write_bytes(b"{}")
    (rank / "state.safetensors").write_bytes(b"s" * 4096)
    (remote / "experiments/gpu_network_v2").mkdir(parents=True)
    profile = json.loads((TREE / "experiments/gpu_network_v2/profile.json").read_bytes())
    profile["artifacts"]["remote_retention"] = "prune_rescued"
    (remote / "experiments/gpu_network_v2/profile.json").write_text(json.dumps(profile))
    run = tmp_path / "run"
    (run / "logs").mkdir(parents=True)
    script = TREE / "experiments/gpu_network_v2/orchestrate.py"
    commands: list[str] = []

    def argv(ssh, command):
        commands.append(command)
        tail = command.split("profile.json", 1)[1]
        return [
            "sh",
            "-c",
            f"cd {remote} && PYTHONPATH={TREE / 'src'} {sys.executable} {script} export out - "
            f"experiments/gpu_network_v2/profile.json{tail}",
        ]

    monkeypatch.setattr(mod, "ssh_argv", argv)
    j = Journal(run)
    if workload:
        j.append("network_execution_intent", role="h0", name="op", phase="workload", executions=1)
    runtime = mod.NetworkRuntime.__new__(mod.NetworkRuntime)
    runtime.journal, runtime.profile = j, profile
    runtime.lifecycle = SimpleNamespace(
        dir=run,
        cfg={"remote_python": sys.executable},
        remote_root=lambda role: str(remote),
        ssh=lambda role: SimpleNamespace(timeout=60),
    )
    record = runtime.rescue("h0", fresh=True)
    with tarfile.open(record["tar"]) as t:
        carried = {m.name for m in t.getmembers()}
    state = "out/qualification/round-0/published/rank-0/state.safetensors"
    assert record["hashes_only"] is (not workload)
    assert commands[0].endswith("profile.json" if workload else "profile.json hashes")
    assert (state in carried) is workload  # hash-only never ships the tensor bytes
    assert state.removeprefix("out/") in record["files"]  # but always its remote hash
    assert runtime.rescue("h0") == record  # resume re-verifies in the recorded mode


def test_verified_dedup_hardlinks_identical_bytes_only(tmp_path: Path) -> None:
    mod = network_module()
    seen: dict[str, Path] = {}
    a, b, c = tmp_path / "rank-0/x", tmp_path / "rank-1/x", tmp_path / "rank-1/y"
    mod.place_verified(a, b"same", seen, True)
    mod.place_verified(b, b"same", seen, True)
    mod.place_verified(c, b"other", seen, True)
    assert a.stat().st_ino == b.stat().st_ino and c.stat().st_ino != a.stat().st_ino
    assert b.read_bytes() == b"same" and c.read_bytes() == b"other"
    # A changed source is never linked: the copy is written fresh from the custody bytes.
    a.unlink()
    a.write_bytes(b"tampered")
    seen[mod.sha256(b"same")] = a
    d = tmp_path / "rank-2/x"
    mod.place_verified(d, b"same", seen, True)
    assert d.stat().st_ino != a.stat().st_ino and d.read_bytes() == b"same"
    # Legacy (dedup off) never links.
    e = tmp_path / "rank-3/x"
    mod.place_verified(e, b"same", {mod.sha256(b"same"): d}, False)
    assert e.stat().st_ino != d.stat().st_ino


def test_streamed_rescue_oversized_stream_aborts_and_leaves_nothing(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    mod = network_module()
    profile = json.loads((TREE / "experiments/gpu_network_v2/profile.json").read_bytes())
    profile["artifacts"]["remote_retention"] = "prune_rescued"
    real = mod.artifact_budget
    monkeypatch.setattr(
        mod, "artifact_budget", lambda p: real(p) | {"rescue_tar_bytes_per_host": 1 << 20}
    )
    # An endless stream: only the byte cap (not a timeout or EOF) can end this test.
    monkeypatch.setattr(mod, "ssh_argv", lambda ssh, command: ["sh", "-c", "yes; echo x >&2"])
    run = tmp_path / "run"
    (run / "logs").mkdir(parents=True)
    j = Journal(run)
    runtime = mod.NetworkRuntime.__new__(mod.NetworkRuntime)
    runtime.journal, runtime.profile = j, profile
    calls: list[str] = []
    runtime.lifecycle = SimpleNamespace(
        dir=run,
        cfg={"remote_python": sys.executable},
        remote_root=lambda role: "/root/ht",
        ssh=lambda role: SimpleNamespace(timeout=600, run=lambda *a: calls.append(a[0])),
    )
    with pytest.raises(Reject, match="network_rescue_archive_budget_exceeded"):
        runtime.rescue("h0", fresh=True)
    assert not list((run / "rescue/h0").iterdir())
    assert j.last("network_rescued") is None and j.last("network_remote_pruned") is None
    assert calls == []  # no remote command, so no prune


def test_stream_bounded_local_write_error_kills_group_before_wait(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import errno

    mod = network_module()
    started: list[subprocess.Popen[bytes]] = []
    real = subprocess.Popen

    def popen(*args, **kwargs):
        started.append(real(*args, **kwargs))
        return started[-1]

    monkeypatch.setattr(mod.subprocess, "Popen", popen)

    class FullDisk:
        def __enter__(self):
            return self

        def __exit__(self, *exc):
            return False

        def write(self, data: bytes) -> int:
            raise OSError(errno.ENOSPC, "No space left on device")

    # One process writes exactly one block and then blocks forever, so it gets no SIGPIPE.
    # Only an explicit group kill ends it; the 600s timer cannot be what releases wait().
    child = "import os, signal; os.write(1, bytes(1 << 20)); signal.pause()"
    argv = [sys.executable, "-c", child]
    with pytest.raises(OSError) as raised:
        mod.stream_bounded(
            argv, SimpleNamespace(open=lambda mode: FullDisk()), tmp_path / "e.log", 1 << 30, 600
        )
    assert raised.value.errno == errno.ENOSPC
    assert started[0].returncode == -signal.SIGKILL
    with pytest.raises(ProcessLookupError):
        os.killpg(started[0].pid, 0)  # no orphan left in the process group


def test_legacy_runtime_schedule_stays_exact() -> None:
    mod = network_module()
    profile = json.loads((TREE / "experiments/gpu_network_v2/profile.json").read_bytes())
    profile["execution_and_transfer_bound_seconds"] = 1
    profile["schedule"]["live_executions"] = 15  # still fits the quota sum, not the legacy shape
    with pytest.raises(Reject, match="execution_reservation_mismatch"):
        schedule_only(mod, profile)
    profile["schedule"]["live_executions"] = 16
    schedule_only(mod, profile)  # the reviewed legacy schedule itself still admits


def schedule_only(mod, profile: dict) -> None:
    """runtime_admission's schedule block, reached with every earlier evidence check satisfied."""
    from hypertrain.protocol.hashing import sha256_hex
    from hypertrain.protocol.jcs import canonicalize

    pin = profile["runtime"] | {"qualified": True, "image_digest": "sha256:" + "a" * 64}
    profile["runtime"] = pin
    profile["artifacts"] |= {"image_and_dependency_bytes": 1000, "staging_bytes_per_host": 1000}
    bounds = mod.artifact_budget(profile)
    profile["artifacts"]["rescue_bytes_per_second"] = bounds["rescue_min_bytes_per_second"]
    evidence = {
        "profile_model": profile["model"],
        "layout": profile["layout"],
        "device": "cuda",
        "torch": "2.14.0+cu130",
        "full_round_transfer_bound_seconds": profile["execution_and_transfer_bound_seconds"],
        "no_slower_hosts": True,
        "image_digest": pin["image_digest"],
        "driver_allowlist": pin["driver_allowlist"],
        "python": pin["remote_python"],
        "cuda": pin["cuda"],
        "full_round_artifacts_sha256": "b" * 64,
        "outer_tape_verified": True,
        "measured_rescue_bytes_per_second": profile["artifacts"]["rescue_bytes_per_second"],
        "image_and_dependency_bytes": 1000,
        "staging_bytes_per_host": 1000,
    }
    profile["runtime_evidence_sha256"] = sha256_hex(canonicalize(evidence))
    mod.runtime_admission(profile, evidence)


def prune_timing_profile() -> dict:
    profile = json.loads((TREE / "experiments/gpu_network_v2/profile.json").read_bytes())
    profile["artifacts"] |= {
        "remote_retention": "prune_rescued",
        "rescue_transfer_seconds": 3300,
        "delete_absence_seconds": 300,
        "local_custody_bytes": 10**15,
    }
    profile["schedule"] |= {
        "hard_deadline_seconds": 43200,
        "staging_seconds": 1800,
        "beacon_service_seconds": 600,
        "rescue_delete_seconds": 3600,
        "hard_margin_seconds": 1800,
    }
    profile["microqualification"] |= {
        "staging_seconds": 1800,
        "probe_and_transfer_seconds": 3600,
        "rescue_delete_seconds": 3600,
        "failure_margin_seconds": 600,
        "qualification_deadline_seconds": 9600,
    }
    return profile


def test_lifecycle_timing_legacy_exact_and_prune_bounded() -> None:
    mod = network_module()
    legacy = json.loads((TREE / "experiments/gpu_network_v2/profile.json").read_bytes())
    assert mod.lifecycle_timing(legacy) == {
        "horizon": 3600,
        "execution_cap": 40,
        "teardown": 300,
        "qualification": 900,
        "qual_stage": 300,
        "qual_probe": 180,
    }
    timing = mod.lifecycle_timing(prune_timing_profile())
    assert timing["horizon"] == 43200 and timing["qualification"] == 9600
    assert timing["execution_cap"] == (43200 - 1800 - 600 - 3600 - 1800) // 63
    for section, change in (
        ("schedule", {"hard_deadline_seconds": 86401}),  # beyond the finite ceiling
        ("schedule", {"hard_deadline_seconds": True}),  # malformed
        ("schedule", {"staging_seconds": 0}),
        ("schedule", {"rescue_delete_seconds": "3600"}),
        ("artifacts", {"rescue_transfer_seconds": 3400}),  # rescue + delete > teardown
        ("microqualification", {"failure_margin_seconds": 601}),  # stages exceed Q
        ("schedule", {"hard_deadline_seconds": 11000}),  # Q + margin > H
        ("schedule", {"hard_deadline_seconds": 7801}),  # no execution time left
    ):
        profile = prune_timing_profile()
        profile[section] |= change
        with pytest.raises(Reject, match="lifecycle_timing_invalid"):
            mod.lifecycle_timing(profile)


def test_prune_runtime_admission_uses_derived_cap_not_legacy_40() -> None:
    mod = network_module()
    profile = prune_timing_profile()
    cap = mod.lifecycle_timing(profile)["execution_cap"]
    profile["execution_and_transfer_bound_seconds"] = cap + 1
    with pytest.raises(Reject, match="missing_or_unqualified_exact_profile_runtime"):
        schedule_only(mod, profile)
    profile = prune_timing_profile()
    profile["execution_and_transfer_bound_seconds"] = cap
    assert cap > 40
    assert schedule_only_total(mod, profile) <= 43200 - 1800


def schedule_only_total(mod, profile: dict) -> int:
    seen: list[int] = []
    real = mod.runtime_admission

    def capture(p, e):
        seen.append(real(p, e))
        return seen[-1]

    mod.runtime_admission = capture
    try:
        schedule_only(mod, profile)
    finally:
        mod.runtime_admission = real
    return seen[0]


@pytest.mark.parametrize(
    ("horizon", "expected"),
    [(None, 3600), (3600, 3600), (43200, 43200), (86400, 86400)],
)
def test_network_lifecycle_deadline_is_reviewed_config_horizon(
    tmp_path: Path, horizon: int | None, expected: int
) -> None:
    cfg = {
        "base_url": "http://127.0.0.1:1",
        "hosts": [{"role": "h0"}],
        "cleanup_contract": "network-v2",
    }
    if horizon is not None:
        cfg["hard_deadline_seconds"] = horizon
    j = Journal(tmp_path)
    first = j.append("create_intent", role="h0")
    orch = Orchestrator(cfg, j, Provider(cfg["base_url"], "a" * 32, tmp_path, j, False), tmp_path)
    assert orch.deadline() == first["unix"] + expected


@pytest.mark.parametrize("horizon", [0, -1, 86401, True, "3600", 3600.0])
def test_network_lifecycle_deadline_rejects_unbounded_or_malformed(
    tmp_path: Path, horizon: object
) -> None:
    cfg = {
        "base_url": "http://127.0.0.1:1",
        "hosts": [{"role": "h0"}],
        "cleanup_contract": "network-v2",
        "hard_deadline_seconds": horizon,
    }
    j = Journal(tmp_path)
    j.append("create_intent", role="h0")
    orch = Orchestrator(cfg, j, Provider(cfg["base_url"], "a" * 32, tmp_path, j, False), tmp_path)
    with pytest.raises(Reject, match="lifecycle_horizon_invalid"):
        orch.deadline()


def test_mission_seven_per_host_execution_cap_boundary() -> None:
    mod = network_module()
    profile = prune_timing_profile()
    seven = {
        "executions_per_host": 7,
        "executions_total": 14,
        "shadow_executions": 8,
        "live_executions": 2,
        "probes": 2,
        "fault_trace_reserve": 2,
    }
    profile["schedule"] |= seven
    cap = mod.lifecycle_timing(profile)["execution_cap"]
    assert cap == (43200 - 1800 - 600 - 3600 - 1800) // 7 == 5057
    profile["execution_and_transfer_bound_seconds"] = cap
    assert schedule_only_total(mod, profile) == 1800 + 7 * cap + 600 + 3600
    profile = prune_timing_profile()
    profile["schedule"] |= seven
    profile["execution_and_transfer_bound_seconds"] = cap + 1
    with pytest.raises(Reject, match="missing_or_unqualified_exact_profile_runtime"):
        schedule_only(mod, profile)


def mission_admission_profile() -> dict:
    profile = prune_timing_profile()
    profile["layout"] = {"pp": 1, "n_gpus": 8, "dp_size": 8, "ep_size": 1, "zero1": False}
    profile["artifacts"]["disk_bytes"] = 429495787520  # measured 400GiB host disk
    profile["admission"] |= {
        "ceiling_usd": "500",
        "cleanup_reserve_usd": "5",
        "max_host_hourly_usd": "5",
        "max_ancillary_usd": "50",
        "gpus_per_host": 8,
        "min_disk_gib": 399,
    }
    return profile


def test_admission_limits_legacy_exact_and_mission_from_profile() -> None:
    from decimal import Decimal

    from hypertrain.gpu_ops import budget

    mod = network_module()
    legacy = json.loads((TREE / "experiments/gpu_network_v2/profile.json").read_bytes())
    limits = mod.admission_limits(legacy)
    assert (limits["ceiling_usd"], limits["reserve"], limits["hardware_cap"]) == (
        "50",
        Decimal("5"),
        Decimal("5"),
    )
    assert (limits["disk_gb"], limits["gpus"], limits["hourly"]) == (80, 2, Decimal("2"))
    mission = mod.admission_limits(mission_admission_profile())
    assert mission["ceiling_usd"] == "500" and mission["reserve"] == Decimal("5")
    assert mission["hardware_cap"] == Decimal("495")
    assert mission["disk_gb"] == 399  # provider disk units are GiB; measured disk is 399.999GiB
    assert mission["gpus"] == 8
    # Two 8-GPU hosts at 4.35/h for the 12h horizon, plus storage and egress, fit the ceiling.
    offer = {
        "dph_total": "4.35",
        "storage_cost": "0.1",
        "inet_up_cost": "0.01",
        "inet_down_cost": "0.01",
    }
    plan = budget.evaluate(
        snapshot0_credit=Decimal("1000"),
        current_credit=Decimal("1000"),
        offers=[offer, offer],
        hard_deadline_seconds=43200,
        disk_gb=mission["disk_gb"],
        egress_gb=400,
        phase_cap=mission["ceiling"],
        reserve=mission["reserve"],
        global_cap=mission["ceiling"],
    )
    assert plan["admit"] and Decimal(plan["hardware_worst_case_usd"]) <= mission["hardware_cap"]
    # The legacy cap still rejects the same lifecycle.
    assert not budget.evaluate(
        snapshot0_credit=Decimal("1000"),
        current_credit=Decimal("1000"),
        offers=[offer, offer],
        hard_deadline_seconds=43200,
        disk_gb=80,
        egress_gb=400,
        phase_cap=Decimal("50"),
    )["admit"]


@pytest.mark.parametrize(
    "change",
    [
        {"ceiling_usd": 500},  # not the signed decimal string
        {"ceiling_usd": "NaN"},
        {"ceiling_usd": "5"},  # no room above the reserve
        {"cleanup_reserve_usd": "50"},  # reserve is fixed, never derived
        {"max_host_hourly_usd": "0"},
        {"gpus_per_host": 2},  # disagrees with the 8-GPU layout
        {"min_disk_gib": 401},  # more than the measured disk
        {"min_disk_gib": True},
    ],
)
def test_admission_limits_reject_malformed_mission(change: dict) -> None:
    mod = network_module()
    profile = mission_admission_profile()
    profile["admission"] |= change
    with pytest.raises(Reject, match="admission_limits_invalid"):
        mod.admission_limits(profile)


def adoption_inputs() -> tuple:
    profile = mission_admission_profile()
    first = 1791838912.29
    rows = [
        {"event": "create_intent", "role": "hostA", "label": "la", "unix": first},
        {"event": "create_response", "instance_id": 11, "success": True, "unix": first + 1},
        {"event": "create_intent", "role": "hostB", "label": "lb", "unix": 1791838912.902},
        {"event": "create_response", "instance_id": 22, "success": True, "unix": first + 1},
    ]
    ownership = {
        "api_lookup": {
            "endpoint": "x",
            "hostA": {"instance_id": 11, "machine_id": 101},
            "hostB": {"instance_id": 22, "machine_id": 202},
        }
    }
    live = {"actual_status": "running", "num_gpus": 8, "gpu_name": "RTX 5090", "disk_space": 400.0}
    snapshot = {
        "instances": [
            {"id": 11, "machine_id": 101, **live},
            {"id": 22, "machine_id": 202, **live},
        ]
    }
    cfg = {"hosts": [{"role": "h0", "machine_id": 101}, {"role": "h1", "machine_id": 202}]}
    now = first + 8000
    record = {"admitted_unix": int(now) - 10, "cutoff_unix": int(first) + 43200}
    return record, cfg, profile, ownership, rows, snapshot, now


def test_adoption_imports_original_create_and_fresh_admission() -> None:
    mod = network_module()
    record, cfg, profile, ownership, rows, snapshot, now = adoption_inputs()
    got = mod.adoption_receipt(record, cfg, profile, ownership, rows, snapshot, now)
    assert got["first_billable_unix"] == 1791838912.29
    assert got["horizon_end_unix"] == 1791838912.29 + 43200
    assert got["cutoff_unix"] == record["cutoff_unix"]
    assert got["admitted_unix"] == record["admitted_unix"]
    assert got["roles"] == {
        "h0": {"instance_id": 11, "machine_id": 101, "label": "la", "create_unix": 1791838912.29},
        "h1": {"instance_id": 22, "machine_id": 202, "label": "lb", "create_unix": 1791838912.902},
    }


def _foreign_machine(i):
    i[1]["hosts"][1]["machine_id"] = 999


def _foreign_live(i):
    i[5]["instances"][1]["machine_id"] = 999


def _extra_live(i):
    i[5]["instances"].append({"id": 33, "machine_id": 101})  # second instance on an owned machine


def _stopped(i):
    i[5]["instances"][0]["actual_status"] = "exited"


def _small_disk(i):
    i[5]["instances"][0]["disk_space"] = 80.0


def _two_gpu(i):
    i[5]["instances"][0]["num_gpus"] = 2


def _missing_intent(i):
    del i[4][2]


def _failed_create(i):
    i[4][3]["success"] = False


@pytest.mark.parametrize(
    "mutate",
    [
        _foreign_machine,
        _foreign_live,
        _extra_live,
        _stopped,
        _small_disk,
        _two_gpu,
        _missing_intent,
        _failed_create,
    ],
)
def test_adoption_rejects_foreign_or_missing_custody(mutate) -> None:
    mod = network_module()
    inputs = list(adoption_inputs())
    mutate(inputs)
    with pytest.raises(Reject, match="adoption_ownership_mismatch"):
        mod.adoption_receipt(*inputs)


@pytest.mark.parametrize(
    "change",
    [
        {"admitted_unix": None},
        {"admitted_unix": 1791838912 + 8000 + 60},  # future
        {"admitted_unix": 1791838912 + 8000 - 400},  # stale
        {"admitted_unix": 1791838900},  # before the original create
        {"cutoff_unix": 1791838912 + 43201},  # beyond the original horizon
        {"cutoff_unix": 1791838912 + 8000 - 10 + 9600 + 1800 - 1},  # qual+margin past cutoff
    ],
)
def test_adoption_rejects_invalid_clock(change: dict) -> None:
    mod = network_module()
    record, *rest = adoption_inputs()
    with pytest.raises(Reject, match="adoption_clock_invalid"):
        mod.adoption_receipt({**record, **change}, *rest)


def test_adoption_rejects_when_qualification_cannot_finish_or_horizon_expired() -> None:
    mod = network_module()
    record, cfg, profile, ownership, rows, snapshot, _ = adoption_inputs()
    late = 1791838912.29 + 43200 - 9600 - 1800 + 5
    with pytest.raises(Reject, match="adoption_clock_invalid"):
        mod.adoption_receipt(
            {**record, "admitted_unix": int(late)}, cfg, profile, ownership, rows, snapshot, late
        )
    legacy = json.loads((TREE / "experiments/gpu_network_v2/profile.json").read_bytes())
    with pytest.raises(Reject, match="adoption_requires_reviewed_finite_profile"):
        mod.adoption_receipt(record, cfg, legacy, ownership, rows, snapshot, 1791838912.29 + 8000)


def test_adoption_preserves_and_ignores_foreign_inventory() -> None:
    mod = network_module()
    record, cfg, profile, ownership, rows, snapshot, now = adoption_inputs()
    snapshot["instances"] += [
        {"id": 55598802, "machine_id": 140475, "actual_status": "exited"},
        {"id": 55620228, "machine_id": 146355, "actual_status": "running"},
    ]
    got = mod.adoption_receipt(record, cfg, profile, ownership, rows, snapshot, now)
    assert {r["instance_id"] for r in got["roles"].values()} == {11, 22}
    assert len(snapshot["instances"]) == 4
    assert [i["id"] for i in mod.owned_instances(snapshot, cfg)] == [11, 22]


def _no_lookup(i):
    del i[3]["api_lookup"]


def _lookup_row_not_dict(i):
    i[3]["api_lookup"]["hostA"] = 55618302


def _journal_line_not_dict(i):
    i[4].append([1, 2])


def _no_instances(i):
    del i[5]["instances"]


def _instance_not_dict(i):
    i[5]["instances"].append("55620228")


def _no_cutoff(i):
    del i[0]["cutoff_unix"]


@pytest.mark.parametrize(
    ("mutate", "reason"),
    [
        (_no_lookup, "adoption_ownership_mismatch"),
        (_lookup_row_not_dict, "adoption_ownership_mismatch"),
        (_journal_line_not_dict, "adoption_ownership_mismatch"),
        (_no_instances, "adoption_ownership_mismatch"),
        (_instance_not_dict, "adoption_ownership_mismatch"),
        (_no_cutoff, "adoption_clock_invalid"),
    ],
)
def test_adoption_rejects_malformed_inputs(mutate, reason: str) -> None:
    mod = network_module()
    inputs = list(adoption_inputs())
    mutate(inputs)
    with pytest.raises(Reject, match=reason):
        mod.adoption_receipt(*inputs)


def test_adopted_lifecycle_deadline_uses_imported_first_billable(tmp_path: Path) -> None:
    from hypertrain.gpu_ops.launcher import network_first_billable

    cfg = {
        "base_url": "http://127.0.0.1:1",
        "hosts": [{"role": "h0"}],
        "cleanup_contract": "network-v2",
        "hard_deadline_seconds": 43200,
    }
    j = Journal(tmp_path)
    assert network_first_billable(j) is None
    cutoff = 1791846900 + 14400
    j.append(
        "adopted", first_billable_unix=1791838912.29, admitted_unix=1791846900, cutoff_unix=cutoff
    )
    orch = Orchestrator(cfg, j, Provider(cfg["base_url"], "a" * 32, tmp_path, j, False), tmp_path)
    # Billing ends at the signed adoption cutoff, never the 12h metadata horizon.
    assert orch.deadline() == cutoff
    assert network_module().qualification_start(j) == 1791846900.0


@pytest.mark.parametrize("cutoff", [None, "1791861300", 1791838912, 1791838912 + 43201, True])
def test_adopted_deadline_rejects_malformed_or_out_of_window_cutoff(tmp_path, cutoff) -> None:
    cfg = {"base_url": "http://127.0.0.1:1", "hosts": [{"role": "h0"}]}
    cfg.update(cleanup_contract="network-v2", hard_deadline_seconds=43200)
    j = Journal(tmp_path)
    j.append("adopted", first_billable_unix=1791838912.29, cutoff_unix=cutoff)
    orch = Orchestrator(cfg, j, Provider(cfg["base_url"], "a" * 32, tmp_path, j, False), tmp_path)
    with pytest.raises(Reject, match="adoption_cutoff_invalid"):
        orch.deadline()


def test_supervisor_alarm_fires_at_adopted_cutoff_not_horizon(tmp_path, monkeypatch) -> None:
    from hypertrain.gpu_ops import supervisor

    now = 1791850000.0
    cutoff = int(now) + 600
    cfg = {"base_url": "http://127.0.0.1:1", "hosts": [{"role": "h0"}], "key_file": "k"}
    cfg.update(cleanup_contract="network-v2", hard_deadline_seconds=43200)
    j = Journal(tmp_path)
    j.append("adopted", first_billable_unix=1791838912.29, cutoff_unix=cutoff)
    j.append("transaction_open", pid=os.getpid(), live=False)
    clock = {"t": now}
    monkeypatch.setattr(supervisor.time, "time", lambda: clock["t"])

    def wait(fds, _w, _x, timeout):
        clock["t"] += timeout  # deterministic: advance the fake clock, no real sleep
        return ([], [], [])

    monkeypatch.setattr(supervisor.select, "select", wait)

    class Stop(Exception):
        pass

    def provider(*args, **kwargs):
        raise Stop

    monkeypatch.setattr(supervisor, "Provider", provider)
    monkeypatch.setattr(supervisor, "read_api_key", lambda _: "a" * 32)
    with pytest.raises(Stop):
        supervisor.supervise(tmp_path, cfg, j, now + 43200, now + 43200)
    reached = j.last("supervisor_deadline_reached")
    assert reached is not None and reached["deadline_unix"] == cutoff - 420
    assert clock["t"] == cutoff - 420


def test_registry_layers_resolves_oci_index_to_exact_linux_amd64(tmp_path: Path) -> None:
    import hashlib

    mod = network_module()
    platform = tmp_path / "amd64.manifest.json"
    platform.write_text(
        json.dumps({"schemaVersion": 2, "config": {"size": 10}, "layers": [{"size": 32}]})
    )
    digest = "sha256:" + hashlib.sha256(platform.read_bytes()).hexdigest()

    def index(rows):
        return {"schemaVersion": 2, "manifests": rows}

    amd64 = {"digest": digest, "platform": {"architecture": "amd64", "os": "linux"}}
    other = {"digest": "sha256:" + "0" * 64, "platform": {"architecture": "arm64", "os": "linux"}}
    assert mod.registry_layers(index([amd64, other]), platform) == 42
    legacy = {"schemaVersion": 2, "config": {"size": 10}, "layers": [{"size": 42}]}
    assert mod.registry_layers(legacy, None) == 52
    for rows, file, reason in (
        ([amd64], None, "reviewed_registry_platform"),  # index without the platform file
        ([other], platform, "reviewed_registry_platform"),  # no linux/amd64 entry
        ([amd64, amd64], platform, "reviewed_registry_platform"),  # ambiguous
        ([{**amd64, "digest": "sha256:" + "1" * 64}], platform, "reviewed_registry_platform"),
    ):
        with pytest.raises(Reject, match=reason):
            mod.registry_layers(index(rows), file)
    platform.write_text(json.dumps({"schemaVersion": 2, "config": {"size": 1}, "layers": []}))
    bad = {**amd64, "digest": "sha256:" + hashlib.sha256(platform.read_bytes()).hexdigest()}
    with pytest.raises(Reject, match="reviewed_complete_image_layers_required"):
        mod.registry_layers(index([bad]), platform)
    for body in (
        {"schemaVersion": 2, "config": {"size": 10**12}, "layers": [{"size": -(10**12)}]},
        {"schemaVersion": 2, "config": {"size": -1}, "layers": [{"size": 2}]},
        {"schemaVersion": True, "config": {"size": 1}, "layers": [{"size": 2}]},
        {"schemaVersion": 2, "config": {"size": 1}, "layers": [{"size": True}]},
    ):
        with pytest.raises(Reject, match="reviewed_complete_image_layers_required"):
            mod.registry_layers(body, None)


def test_staging_cap_legacy_exact_and_prune_bounded_by_measured_disk() -> None:
    mod = network_module()
    legacy = json.loads((TREE / "experiments/gpu_network_v2/profile.json").read_bytes())
    assert mod.staging_cap(legacy) == 1_000_000_000
    assert mod.staging_cap(mission_admission_profile()) == 429495787520


@pytest.mark.parametrize(
    ("parsed", "accepted"),
    [
        ({"success": True}, True),
        ({"success": False, "msg": "SSH key already associated with instance."}, True),
        ({"success": False, "msg": "SSH key already associated with instance"}, False),
        ({"success": False, "msg": "Invalid ssh key."}, False),
        ({"success": False}, False),
        ({"success": False, "msg": "SSH key already associated with instance.", "x": 1}, False),
    ],
)
def test_attach_accepts_only_exact_already_associated_reply(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, parsed: dict, accepted: bool
) -> None:
    from hypertrain.gpu_ops.journal import sha256
    from hypertrain.gpu_ops.provider import Response

    cfg = {"base_url": "http://127.0.0.1:1", "hosts": [{"role": "h0"}, {"role": "h1"}]}
    j = Journal(tmp_path)
    j.append("receipt", role="h0", instance_id=7)
    provider = Provider(cfg["base_url"], "a" * 32, tmp_path, j, False)
    orch = Orchestrator(cfg, j, provider, tmp_path)
    sent = []

    def call(method, path, tag, body=None, timeout=30):
        sent.append((method, path, body))
        return Response(200, parsed, None, "mock", None)

    monkeypatch.setattr(provider, "call", call)
    pub = ensure_identity(tmp_path).with_suffix(".pub").read_text().strip()
    if not accepted:
        with pytest.raises(Reject, match="ssh_attach_failed:h0:HTTP200"):
            orch.attach("h0")
        assert j.last("ssh_attached", role="h0") is None
        return
    orch.attach("h0")
    assert sent == [("POST", "/api/v0/instances/7/ssh/", {"ssh_key": pub})]
    row = j.last("ssh_attached", role="h0")
    assert row["key_sha256"] == sha256(pub.encode()) and row["instance_id"] == 7
    assert row["already_associated"] is (parsed["success"] is False)
