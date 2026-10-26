from __future__ import annotations

import getpass
import hashlib
import json
import os
import select
import subprocess
import sys
from collections.abc import Iterator
from pathlib import Path
from typing import Any

import numpy as np
import pytest

HERE = Path(__file__).resolve().parent
TREE = HERE.parents[1]
KEY = "a" * 32
ACCOUNT = 4242
DIGEST = "sha256:" + "ab" * 32


def offer(i: int) -> dict[str, Any]:
    return {
        "id": 900 + i,
        "machine_id": 5000 + i,
        "host_id": 70 + i,
        "gpu_name": "RTX 5090",
        "num_gpus": 1,
        "dph_total": 0.40,
        "storage_cost": 0.15,
        "inet_up_cost": 0.01,
        "inet_down_cost": 0.01,
        "driver_version": ["580.95.05", "575.64.03", "580.95.05"][i],
        "geolocation": "US",
    }


class Mock:
    def __init__(self, proc: subprocess.Popen[str], port: int, cfg: dict[str, Any]) -> None:
        self.proc, self.port, self.cfg = proc, port, cfg
        self.base = f"http://127.0.0.1:{port}"

    def state(self) -> dict[str, Any]:
        import urllib.request

        with urllib.request.urlopen(self.base + "/__state", timeout=10) as r:  # noqa: S310
            return dict(json.loads(r.read()))


@pytest.fixture
def mock_factory(tmp_path: Path) -> Iterator[Any]:
    mocks: list[Mock] = []

    def start(**faults: Any) -> Mock:
        cfg = {
            "key": KEY,
            "account_id": ACCOUNT,
            "credit": 28.0,
            "offers": [offer(i) for i in range(3)],
            **faults,
        }
        cfg_path = tmp_path / f"mock-{len(mocks)}.json"
        cfg_path.write_text(json.dumps(cfg))
        proc = subprocess.Popen(
            [
                sys.executable,
                str(HERE / "mock_vast.py"),
                str(cfg_path),
                str(tmp_path / f"mockstate-{len(mocks)}"),
            ],
            stdout=subprocess.PIPE,
            text=True,
            start_new_session=True,
        )
        assert proc.stdout is not None
        assert select.select([proc.stdout], [], [], 30)[0], "mock did not start"
        port = int(proc.stdout.readline().split()[1])
        m = Mock(proc, port, cfg)
        mocks.append(m)
        return m

    yield start
    for jf in tmp_path.rglob("journal.jsonl"):
        for rec in journal(jf.parent):
            if rec["kind"] == "supervisor_ready":
                try:
                    os.kill(rec["supervisor_pid"], 15)
                except ProcessLookupError:
                    pass
                wait_pid_exit(rec["supervisor_pid"], 30)
    for m in mocks:
        m.proc.terminate()
        m.proc.wait(timeout=30)


@pytest.fixture
def run_cfg(tmp_path: Path) -> Any:
    def make(mock: Mock, **over: Any) -> Path:
        key = tmp_path / "vast-key"
        if not key.exists():
            fd = os.open(key, os.O_WRONLY | os.O_CREAT, 0o600)
            os.write(fd, KEY.encode())
            os.close(fd)
        snap = tmp_path / "budget-snapshot-0.json"
        snap.write_text(json.dumps({"account_id": ACCOUNT, "credit_usd": "28.00"}))
        shard = tmp_path / "shard.u32"
        if not shard.exists():
            rng = np.random.default_rng(0)
            shard.write_bytes(
                rng.integers(0, 259, size=(64, 1025), dtype=np.uint32).astype("<u4").tobytes()
            )
        cfg = {
            "base_url": mock.base,
            "key_file": str(key),
            "budget_snapshot": str(snap),
            "image": "docker.io/vastai/pytorch@" + DIGEST,
            "disk_gb": 20,
            "hard_deadline_seconds": 3600,
            "create_margin_seconds": 600,
            "cleanup_margin_seconds": 60,
            "hosts": [
                {
                    "role": f"h{i}",
                    "machine_id": 5000 + i,
                    "max_dph_total": 0.60,
                }
                for i in range(3)
            ],
            "tree": str(TREE),
            "shard": str(shard),
            "remote_root": str(tmp_path / "remote-{role}"),
            "remote_python": sys.executable,
            "ssh_user": getpass.getuser(),
            "profile": "tiny",
            "device": "cpu",
            "runs_per_host": 2,
            "evidence_path": str(tmp_path / "evidence.json"),
            "poll_seconds": 15,
            "supervisor_check_seconds": 0.2,
            "keyscan_attempts": 20,
            **over,
        }
        p = (
            tmp_path
            / f"cfg-{hashlib.sha256(json.dumps(cfg, sort_keys=True).encode()).hexdigest()[:8]}.json"
        )
        p.write_text(json.dumps(cfg))
        return p

    return make


def launch(
    cfg: Path, run_dir: Path, *extra: str, env: dict[str, str] | None = None
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [
            sys.executable,
            "-m",
            "hypertrain.gpu_ops.launcher",
            "run",
            "--config",
            str(cfg),
            "--run-dir",
            str(run_dir),
            *extra,
        ],
        capture_output=True,
        text=True,
        timeout=900,
        env={**os.environ, "HT_GPU_VIRTUAL_TIME": "1", **(env or {})},
    )


def journal(run_dir: Path) -> list[dict[str, Any]]:
    return [json.loads(x) for x in (run_dir / "journal.jsonl").read_text().splitlines() if x]


def wait_pid_exit(pid: int, timeout: float) -> bool:
    try:
        fd = os.pidfd_open(pid)
    except ProcessLookupError:
        return True
    try:
        return bool(select.select([fd], [], [], timeout)[0])
    finally:
        os.close(fd)
