"""Real kernel-backed small stdlib children. No Torch or exact1GiB workload."""

from __future__ import annotations

import json
import os
import select
import signal
import socket
import sys
import threading
import time

import pytest

from hypertrain.miner import island_launch as launch


@pytest.fixture
def small(monkeypatch, tmp_path):
    monkeypatch.setattr(launch, "_CAPACITY_MEMORY", 64 << 20)
    monkeypatch.setattr(launch, "_CAPACITY_CPU_PERCENT", 50)
    return launch.CapacityAttempt("11" * 32, "22" * 32, tmp_path / "runtime.lock", lambda: True)


@pytest.mark.parametrize("mode", ["success", "oom", "cancel", "deadline"])
def test_actual_small_kernel_attempt_and_cleanup(tmp_path, small, mode):
    """Subscribe readiness before triggering real bounded command and await pidfd exit."""
    cancel = threading.Event()
    events = []
    path = str(tmp_path / "ready.sock")
    deadline = int(time.time()) + (4 if mode == "deadline" else 20)
    script = f"""import os,socket,signal
d=os.fork()
if d==0:
    signal.pause();os._exit(0)
s=socket.socket(socket.AF_UNIX);s.connect({path!r})
s.sendall((str(os.getpid())+','+str(d)).encode()+b'\\n')
assert s.recv(1)==b'G'
if {mode!r}=='oom':
    x=bytearray(96*1024*1024)
elif {mode!r}=='success':
    os.kill(d,signal.SIGKILL);os.waitpid(d,0)
else:
    signal.pause()
"""
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as listener:
        listener.bind(path)
        listener.listen(1)
        listener.settimeout(15)

        def ready():
            connection, _ = listener.accept()
            with connection:
                connection.settimeout(10)
                raw = connection.recv(128).decode().strip()
                descriptors = [os.pidfd_open(int(pid)) for pid in raw.split(",")]
                try:
                    connection.sendall(b"G")
                    if mode == "cancel":
                        cancel.set()
                    for fd in descriptors:
                        assert select.select([fd], [], [], 15)[0]
                    events.append("both-exited")
                finally:
                    for fd in descriptors:
                        os.close(fd)

        watcher = threading.Thread(target=ready)
        watcher.start()
        runtime = tmp_path / "attempt"
        try:
            if mode == "success":
                launch.run_capacity_argv(
                    [sys.executable, "-c", script],
                    runtime,
                    deadline,
                    small,
                    env=dict(os.environ),
                    cancel=cancel,
                )
            else:
                with pytest.raises(launch.IslandFailure, match="execution failed"):
                    launch.run_capacity_argv(
                        [sys.executable, "-c", script],
                        runtime,
                        deadline,
                        small,
                        env=dict(os.environ),
                        cancel=cancel,
                    )
        finally:
            watcher.join(timeout=20)
        assert not watcher.is_alive() and events == ["both-exited"]
    observed = json.loads((runtime / "capacity-observed.json").read_text())
    assert observed["memory.max"] == str(64 << 20)
    assert observed["cpu.max"] == "50000 100000"
    assert observed["memory.oom.group"] == "1" and observed["memory.swap.max"] == "0"
    assert observed["affinity"] == [0, 1, 2, 3] and observed["nice"] == 19
    result = json.loads((runtime / "capacity-result.json").read_text())
    if mode == "oom":
        assert "Result=oom-kill" in result["status"]
        counters = dict(
            line.split()
            for line in result["status"].splitlines()
            if line.startswith(("oom_kill ", "oom_group_kill "))
        )
        assert int(counters["oom_kill"]) >= 1
        assert int(counters["oom_group_kill"]) == 1
    assert (runtime / "capacity-cleaned.json").is_file()
    with pytest.raises(launch.IslandFailure, match="already recorded"):
        launch.run_capacity_argv(
            [sys.executable, "-c", "raise SystemExit(99)"],
            runtime,
            int(time.time()) + 20,
            small,
            env=dict(os.environ),
        )


def test_charge_denial_launches_nothing(tmp_path):
    attempt = launch.CapacityAttempt("33" * 32, "44" * 32, tmp_path / "lock", lambda: False)
    with pytest.raises(launch.IslandFailure, match="already charged"):
        launch.run_capacity_argv(
            [sys.executable, "-c", "raise SystemExit(99)"],
            tmp_path / "attempt",
            int(time.time()) + 10,
            attempt,
            env=dict(os.environ),
        )
    assert not (tmp_path / "attempt/capacity-attempt.json").exists()


def test_missing_manager_permission_has_no_unbounded_fallback(tmp_path, small, monkeypatch):
    monkeypatch.setenv("PATH", "")
    with pytest.raises(launch.IslandFailure, match="manager unavailable|cleanup incomplete"):
        launch.run_capacity_argv(
            [sys.executable, "-c", "raise SystemExit(99)"],
            tmp_path / "attempt",
            int(time.time()) + 10,
            small,
            env=dict(os.environ),
        )
    assert (tmp_path / "attempt/capacity-attempt.json").exists()
    assert not (tmp_path / "attempt/capacity-observed.json").exists()


def test_real_runner_parent_death_retains_absolute_manager_timer(tmp_path, small):
    """Fork only a stdlib waiting controller; manager timer survives its SIGKILL."""
    path = str(tmp_path / "dead.sock")
    runtime = tmp_path / "attempt"
    deadline = int(time.time()) + 6
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as listener:
        listener.bind(path)
        listener.listen(1)
        listener.settimeout(15)
        parent = os.fork()
        if parent == 0:
            try:
                code = (
                    "import os,socket,signal;s=socket.socket(socket.AF_UNIX);"
                    f"s.connect({path!r});s.sendall(str(os.getpid()).encode());"
                    "s.recv(1);signal.pause()"
                )
                launch.run_capacity_argv(
                    [sys.executable, "-c", code], runtime, deadline, small, env=dict(os.environ)
                )
            finally:
                os._exit(1)
        try:
            connection, _ = listener.accept()
            with connection:
                connection.settimeout(15)
                child = int(connection.recv(128))
                fd = os.pidfd_open(child)
                try:
                    connection.sendall(b"G")
                    os.kill(parent, signal.SIGKILL)
                    os.waitpid(parent, 0)
                    assert not select.select([fd], [], [], 0)[0]
                    assert connection.recv(1) == b""
                    assert select.select([fd], [], [], 5)[0]
                    assert time.time() >= deadline
                finally:
                    os.close(fd)
            record = json.loads((runtime / "capacity-attempt.json").read_text())
            status = launch._manager(
                "systemctl", "show", record["timer_service"], "-p", "Result", "-p", "ExecMainStatus"
            )
            assert "Result=success" in status and "ExecMainStatus=0" in status
            with pytest.raises(launch.IslandFailure, match="already recorded"):
                launch.run_capacity_argv(
                    [sys.executable, "-c", "pass"],
                    runtime,
                    int(time.time()) + 20,
                    small,
                    env=dict(os.environ),
                )
        finally:
            if (runtime / "capacity-attempt.json").exists():
                record = json.loads((runtime / "capacity-attempt.json").read_text())
                for key in ("unit", "timer", "timer_service"):
                    launch._manager("systemctl", "stop", record[key], check=False)
                launch._manager(
                    "systemctl",
                    "reset-failed",
                    record["unit"],
                    record["timer"],
                    record["timer_service"],
                    check=False,
                )
