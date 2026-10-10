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

import pytest

from hypertrain.gpu_ops.journal import Journal
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
) -> None:
    ssh_servers = []
    ports = []
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
        if parent.poll() is None:
            parent.terminate()
            parent.wait(timeout=5)
        api.shutdown()
        api.server_close()
        api_thread.join()
        for proc in ssh_servers:
            os.killpg(proc.pid, signal.SIGTERM)
            proc.wait(timeout=5)


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
    from types import SimpleNamespace

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
