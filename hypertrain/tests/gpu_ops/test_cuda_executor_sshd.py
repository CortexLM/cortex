"""The executor's real ssh transport against a real loopback sshd (docs/adr/0008).

Only the GPU host's tools are fake (docker, nvidia-smi, date on the remote PATH): the OpenSSH
client, the daemon, the login shell that parses the `docker run` command line, the stdin and
stdout streams and the exit codes are real. Skipped where sshd or ssh-keygen is missing."""

# ruff: noqa: F811  (the module-scoped `canned` fixture is imported, then asked for by name)
from __future__ import annotations

import getpass
import json
import os
import shutil
import socket
import subprocess
import time
from collections.abc import Iterator
from dataclasses import dataclass
from pathlib import Path

import pytest

from hypertrain.gpu_ops import cuda_executor
from hypertrain.miner.island_launch import IslandFailure

from . import fake_cuda_host as fake
from .test_cuda_executor_dispatch import CONTEXTS, backend, canned, network  # noqa: F401

SSHD = Path("/usr/sbin/sshd")
pytestmark = pytest.mark.skipif(
    not SSHD.exists() or shutil.which("ssh") is None or shutil.which("ssh-keygen") is None,
    reason="needs ssh, ssh-keygen and /usr/sbin/sshd",
)


@dataclass
class Sshd:
    port: int
    client_key: Path
    host_key_line: str  # `ssh-ed25519 AAAA...` of the daemon


def free_port() -> int:
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return int(probe.getsockname()[1])


@pytest.fixture
def sshd(tmp_path: Path) -> Iterator[tuple[Sshd, Path]]:
    """A daemon of the current user on loopback; returns it with the fake host's bin directory
    still to be put on the remote PATH by the test through `SetEnv`."""
    state = tmp_path / "sshd"
    state.mkdir(mode=0o700)
    for name in ("host", "client"):
        subprocess.run(
            ["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(state / name)], check=True
        )
    (state / "authorized_keys").write_text((state / "client.pub").read_text())
    (state / "authorized_keys").chmod(0o600)
    daemon = Sshd(
        free_port(), state / "client", " ".join((state / "host.pub").read_text().split()[:2])
    )
    yield daemon, state


def start(daemon: Sshd, state: Path, remote_path: str) -> subprocess.Popen[bytes]:
    (state / "sshd_config").write_text(
        f"ListenAddress 127.0.0.1\nPort {daemon.port}\nHostKey {state}/host\nPidFile {state}/pid\n"
        f"AuthorizedKeysFile {state}/authorized_keys\nStrictModes no\nPasswordAuthentication no\n"
        f"KbdInteractiveAuthentication no\nUsePAM no\nAllowUsers {getpass.getuser()}\n"
        f"LogLevel ERROR\nMaxStartups 100\nSetEnv PATH={remote_path} FAKE_HOST_ADDRESS=127.0.0.1\n"
    )
    process = subprocess.Popen(
        [str(SSHD), "-D", "-e", "-f", str(state / "sshd_config")],
        stdout=(state / "sshd.log").open("ab"),
        stderr=subprocess.STDOUT,
        start_new_session=True,
    )
    for _ in range(400):
        try:
            with socket.create_connection(("127.0.0.1", daemon.port), timeout=0.2):
                return process
        except OSError:
            time.sleep(0.025)
    process.kill()
    raise AssertionError("sshd did not start: " + (state / "sshd.log").read_text())


def configure(
    tmp_path, monkeypatch, canned, daemon, state, *, pinned: str, port: int | None = None
):
    """Executor config for the loopback daemon; the real ssh is the one found on PATH."""
    pool = fake.make_pool(tmp_path / "pool", monkeypatch, canned, labels=("a",))
    ref = canned.manifest.training.reference_spec
    pool.toolbox.scenario.install(
        {
            "127.0.0.1": {
                "gpus": [
                    fake.gpu(0, i, cc="10.3", sm=ref.sm_count, driver=ref.driver_allowlist[0])
                    for i in range(2)
                ],
                "images": [canned.image],
            }
        },
        {canned.job.digest(): str(canned.tree)},
    )
    port = daemon.port if port is None else port
    known_hosts = tmp_path / "known_hosts"
    known_hosts.write_text(f"[127.0.0.1]:{port} {pinned}\n")
    settings = dict(pool.config)
    settings["state_dir"] = str(tmp_path / "executor-state")
    settings["sweeps"] = 1
    settings["hosts"] = [
        {
            "label": "a",
            "address": "127.0.0.1",
            "port": port,
            "user": getpass.getuser(),
            "identity_file": str(daemon.client_key),
            "known_hosts_file": str(known_hosts),
        }
    ]
    path = tmp_path / "real.json"
    path.write_text(json.dumps(settings))
    monkeypatch.setenv("PATH", "/usr/bin:/bin")  # the real ssh, not the toolbox's fake one
    assert shutil.which("ssh") != str(pool.toolbox.bin_dir / "ssh")
    return pool, path


def dispatch(path: Path, canned, home: Path):
    job = canned.job
    launch = cuda_executor.load(path)("audit", job, home, CONTEXTS["audit"])
    return launch(job, home, backend="cuda", cancel=None, trace=True)


def journal(path: Path) -> list[dict]:
    lines = (Path(json.loads(path.read_text())["state_dir"]) / "journal.jsonl").read_text()
    return [json.loads(line) for line in lines.splitlines()]


def test_a_job_goes_through_a_real_sshd_and_login_shell(tmp_path, monkeypatch, canned, sshd):
    daemon, state = sshd
    remote = os.pathsep.join([str(tmp_path / "pool" / "fake" / "host-bin"), "/usr/bin", "/bin"])
    process = start(daemon, state, remote)
    try:
        pool, path = configure(
            tmp_path, monkeypatch, canned, daemon, state, pinned=daemon.host_key_line
        )
        artifacts = dispatch(path, canned, canned.stage(tmp_path / "home"))
    finally:
        process.terminate()
        process.wait(timeout=10)
    home = tmp_path / "home"
    assert artifacts.directory == home.resolve() / "published"
    record = json.loads((home / "role-dispatch.json").read_bytes())
    assert record["host"]["label"] == "a" and record["operation"] == "audit"
    assert record["environment"]["gpu_uuids"] == [fake.gpu_uuid(0, 0), fake.gpu_uuid(0, 1)]
    assert pool.toolbox.ssh_log() == []  # the fake ssh was never used
    assert [r["kind"] for r in journal(path)] == ["exec_attempt", "exec_published"]
    assert [e["host"] for e in pool.toolbox.events("run_start")] == ["127.0.0.1"]


def test_a_host_key_that_is_not_the_pinned_one_loses_the_host(tmp_path, monkeypatch, canned, sshd):
    daemon, state = sshd
    process = start(daemon, state, "/usr/bin:/bin")
    wrong = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
    try:
        _, path = configure(tmp_path, monkeypatch, canned, daemon, state, pinned=wrong)
        with pytest.raises(IslandFailure, match="no host able to run the job"):
            dispatch(path, canned, canned.stage(tmp_path / "home"))
    finally:
        process.terminate()
        process.wait(timeout=10)
    [lost] = [r for r in journal(path) if r["kind"] == "exec_host_unavailable"]
    assert "Host key verification failed" in lost["reason"]
    assert not (tmp_path / "home" / "published").exists()


def test_a_closed_port_loses_the_host(tmp_path, monkeypatch, canned, sshd):
    daemon, state = sshd
    _, path = configure(
        tmp_path, monkeypatch, canned, daemon, state, pinned=daemon.host_key_line, port=free_port()
    )
    with pytest.raises(IslandFailure, match="no host able to run the job"):
        dispatch(path, canned, canned.stage(tmp_path / "home"))
    [lost] = [r for r in journal(path) if r["kind"] == "exec_host_unavailable"]
    assert "ssh could not reach or authenticate the host" in lost["reason"]
