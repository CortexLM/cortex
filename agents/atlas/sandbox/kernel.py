#!/usr/bin/python3 -I
"""Controller-side supervisor. No provider credential enters the container."""
from __future__ import annotations

import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import threading
import time

def remaining_seconds(config: dict) -> float:
    deadline_ms = config.get("deadline_ms")
    if deadline_ms is None:
        return config["seconds"]
    if type(deadline_ms) is not int or not 0 < deadline_ms <= 9007199254740991:
        raise ValueError("Invalid kernel deadline")
    remaining = min(config["seconds"], deadline_ms / 1000 - time.time())
    if remaining <= 0:
        raise ValueError("Kernel capability expired")
    return remaining


def container_args(config: dict) -> list[str]:
    name, image = config["name"], config["image"]
    if not re.fullmatch(r"cortex-kernel-[a-f0-9-]{36}", name):
        raise ValueError("Invalid kernel identity")
    if not re.fullmatch(r"(?:[a-zA-Z0-9./_:-]+@)?sha256:[a-f0-9]{64}", image):
        raise ValueError("Kernel image must be digest pinned")
    workspace = Path(config["workspace"])
    if not workspace.is_absolute() or workspace.resolve() != workspace or not workspace.is_dir():
        raise ValueError("Invalid kernel workspace")
    if "," in str(workspace):
        raise ValueError("Invalid mount path")
    for key, ceiling in (("memory_mb", 65536), ("workspace_mb", 4096), ("cpus", 64), ("pids", 1024), ("seconds", 86400)):
        value = config[key]
        if type(value) is not int or not 0 < value <= ceiling:
            raise ValueError("Unbounded kernel resource")
    return [
        "/usr/bin/docker", "run", "--pull=never", "--rm", "--interactive",
        "--name", name, "--label", f"cortex.kernel={name}",
        "--network=none", "--ipc=private", "--read-only",
        "--cap-drop=ALL", "--security-opt=no-new-privileges",
        "--user=65532:65532", "--pids-limit", str(config["pids"]),
        "--memory", f"{config['memory_mb']}m", "--memory-swap", f"{config['memory_mb']}m",
        "--cpus", str(config["cpus"]), "--ulimit", "nofile=256:256",
        "--ulimit", "fsize=268435456:268435456", "--log-driver=none",
        "--tmpfs", "/tmp:rw,nosuid,nodev,noexec,size=64m,mode=1777",
        "--tmpfs", f"/work:rw,nosuid,nodev,size={config['workspace_mb']}m,uid=65532,gid=65532,mode=700",
        "--workdir=/work", "--env=HOME=/work", "--env=PRIME_AGENT_BASH_SHELL=/bin/bash",
        "--env=NO_COLOR=1", "--env=PRIME_AGENT_KERNEL_OWNER_PID=1",
        "--entrypoint=/usr/bin/timeout", image,
        "--signal=KILL", str(remaining_seconds(config)), "/usr/local/bin/python", "-m", "rlm.repl",
    ]


def checkpoint(config: dict, env: dict[str, str], restore: bool) -> None:
    """Keep an opaque, bounded archive; never extract untrusted bytes on the host."""
    archive = Path(config["workspace"]) / "kernel.tar"
    cmd = ["/usr/bin/docker", "exec"]
    if restore:
        if not archive.exists():
            return
        if archive.is_symlink() or not archive.is_file() or archive.stat().st_size > config["workspace_mb"] * 1024 * 1024 + 1024 * 1024:
            raise ValueError("Invalid kernel checkpoint")
        with archive.open("rb") as data:
            subprocess.run(
                cmd + ["-i", config["name"], "/bin/tar", "-xf", "-", "-C", "/work",
                       "--no-same-owner", "--no-same-permissions"],
                stdin=data, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                env=env, timeout=15, check=True,
            )
        return
    partial = archive.with_suffix(".part")
    limit = config["workspace_mb"] * 1024 * 1024 + 1024 * 1024
    exporter = subprocess.Popen(
        cmd + [config["name"], "/bin/tar", "-cf", "-", "-C", "/work", "."],
        stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, env=env,
    )
    timer = threading.Timer(15, exporter.kill)
    timer.start()
    try:
        with partial.open("wb") as data:
            os.chmod(partial, 0o600)
            size = 0
            assert exporter.stdout is not None
            while chunk := exporter.stdout.read(65536):
                size += len(chunk)
                if size > limit:
                    raise ValueError("Kernel checkpoint exceeds quota")
                data.write(chunk)
            data.flush()
            os.fsync(data.fileno())
        if exporter.wait(timeout=5) != 0:
            raise ValueError("Kernel checkpoint failed")
        partial.replace(archive)
    finally:
        timer.cancel()
        if exporter.poll() is None:
            exporter.kill()
        exporter.wait(timeout=5)
        if exporter.stdout is not None:
            exporter.stdout.close()



def main() -> int:
    if len(sys.argv) != 4 or sys.argv[2:] != ["-m", "rlm.repl"]:
        raise ValueError("Kernel launcher accepts only the RLM protocol")
    config = json.loads(Path(sys.argv[1]).read_text())
    args = container_args(config)
    env = {"PATH": "/usr/bin:/bin", "HOME": "/nonexistent", "LANG": "C.UTF-8"}
    stopped = False

    def stop(_signal: int, _frame: object) -> None:
        nonlocal stopped
        stopped = True

    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    deadline = time.monotonic() + remaining_seconds(config)
    parent = os.getppid()
    child = subprocess.Popen(args, env=env, stdout=subprocess.PIPE)
    failed = threading.Event()

    def output() -> None:
        try:
            assert child.stdout is not None
            while line := child.stdout.readline(16 * 1024 * 1024 + 1):
                if len(line) > 16 * 1024 * 1024:
                    raise ValueError("Kernel protocol quota")
                frame = json.loads(line)
                if frame.get("event") == "ready":
                    checkpoint(config, env, restore=True)
                elif frame.get("event") == "done" and "saved" in frame:
                    checkpoint(config, env, restore=False)
                sys.stdout.buffer.write(line)
                sys.stdout.buffer.flush()
        except Exception:
            failed.set()

    reader = threading.Thread(target=output, daemon=True)
    reader.start()
    try:
        while child.poll() is None:
            if (stopped or failed.is_set() or time.monotonic() >= deadline or os.getppid() != parent
                    or ("deadline_ms" in config and time.time() * 1000 >= config["deadline_ms"])):
                return 124
            time.sleep(0.1)
        reader.join(timeout=20)
        if failed.is_set() or reader.is_alive():
            return 1
        return child.returncode
    finally:
        resource = subprocess.run(
            ["/usr/bin/docker", "inspect", "--format={{.Id}} {{index .Config.Labels \"cortex.kernel\"}}", config["name"]],
            env=env, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, timeout=10, check=False,
        )
        identity = resource.stdout.decode().strip().split()
        if len(identity) == 2 and re.fullmatch(r"[a-f0-9]{64}", identity[0]) and identity[1] == config["name"]:
            subprocess.run(
                ["/usr/bin/docker", "rm", "--force", identity[0]],
                env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=10, check=False,
            )
        try:
            child.wait(timeout=5)
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait(timeout=5)


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception:
        print("Cortex kernel supervisor failed", file=sys.stderr)
        sys.exit(1)
