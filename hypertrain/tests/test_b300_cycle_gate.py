"""scripts/b300/cycle_gate.sh and gpu_watch.sh against fake nvidia-smi and docker on PATH.

The fakes stand at the scripts' real boundaries: `nvidia-smi` (GPU uuids and compute apps) and
`docker logs` of the Lium executor container (the SSH-key markers that bracket a validator
cycle). No GPU, no network.
"""

from __future__ import annotations

import json
import os
import subprocess
import time
from datetime import UTC, datetime, timedelta
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
GATE = ROOT / "scripts/b300/cycle_gate.sh"
WATCH = ROOT / "scripts/b300/gpu_watch.sh"
UUIDS = [f"GPU-00000000-0000-0000-0000-00000000000{i}" for i in range(4)]
FOREIGN = "/root/app/.venv/bin/python"
OURS = "/opt/hypertrain/venv/bin/python"

FAKE_SMI = r"""#!/usr/bin/env bash
state=$FAKE_STATE
case "$*" in
  *--query-gpu=uuid*)
    ids=
    for a in "$@"; do case $a in --id=*) ids=${a#--id=} ;; esac; done
    i=0
    while read -r uuid; do
      if [ -z "$ids" ] || [[ ",$ids," == *",$i,"* ]]; then echo "$uuid"; fi
      i=$((i + 1))
    done <"$state/uuids" ;;
  *--query-compute-apps*) [ -e "$state/smi-fail" ] && exit 9; cat "$state/apps" ;;
  *--query-gpu=index,memory.used*) [ -e "$state/smi-fail" ] && exit 9; echo "0, 4 MiB" ;;
esac
"""

FAKE_DOCKER = r"""#!/usr/bin/env bash
case $1 in
  info) exit 0 ;;
  logs) cat "$FAKE_STATE/executor-log" ;;
esac
"""


@pytest.fixture
def host(tmp_path: Path) -> dict[str, str]:
    """A fake host: PATH shims plus FAKE_STATE files the tests rewrite between runs."""
    bin_dir, state = tmp_path / "bin", tmp_path / "state"
    bin_dir.mkdir()
    state.mkdir()
    for name, body in (("nvidia-smi", FAKE_SMI), ("docker", FAKE_DOCKER)):
        shim = bin_dir / name
        shim.write_text(body)
        shim.chmod(0o755)
    (state / "uuids").write_text("\n".join(UUIDS) + "\n")
    (state / "apps").write_text("")
    (state / "executor-log").write_text("")
    return {"PATH": f"{bin_dir}:{os.environ['PATH']}", "FAKE_STATE": str(state)}


def marker(kind: str, age_s: int) -> str:
    """One `docker logs -t` line of the executor for an SSH-key call `age_s` seconds ago."""
    at = datetime.now(UTC) - timedelta(seconds=age_s)
    stamp = at.strftime("%Y-%m-%dT%H:%M:%S.%f") + "000Z"
    return f'{stamp} INFO:     13.223.237.157:4384 - "POST /{kind}_ssh_key HTTP/1.1" 200 OK\n'


def app(gpu: int, name: str) -> str:
    return f"{UUIDS[gpu]}, 4242, {name}, 2666 MiB\n"


def gate(host: dict[str, str], *args: str) -> tuple[int, dict[str, object]]:
    run = subprocess.run(
        [str(GATE), "--timeout", "0", "--poll", "1", *args],
        env={**os.environ, **host},
        capture_output=True,
        text=True,
        timeout=30,
    )
    return run.returncode, json.loads(run.stdout.splitlines()[-1])


def set_state(host: dict[str, str], **files: str) -> None:
    for name, text in files.items():
        (Path(host["FAKE_STATE"]) / name.replace("_", "-")).write_text(text)


def test_gate_is_clear_on_an_idle_host_after_a_finished_cycle(host):
    set_state(host, executor_log=marker("upload", 400) + marker("remove", 300))
    rc, out = gate(host)
    assert rc == 0
    assert out["clear"] is True and out["last_marker"] == "remove"


def test_gate_blocks_while_an_upload_marker_has_no_remove(host):
    set_state(host, executor_log=marker("remove", 900) + marker("upload", 60))
    rc, out = gate(host)
    assert rc == 1
    assert out["in_cycle"] is True and out["foreign_compute_apps"] == 0


def test_gate_does_not_wait_on_an_upload_older_than_a_cycle(host):
    set_state(host, executor_log=marker("upload", 1500))
    rc, out = gate(host, "--cycle-max", "720")
    assert rc == 0
    assert out["stale_upload"] is True and out["in_cycle"] is False


def test_gate_blocks_on_a_foreign_compute_app(host):
    set_state(host, apps=app(2, FOREIGN), executor_log=marker("remove", 300))
    rc, out = gate(host)
    assert rc == 1
    assert out["foreign_compute_apps"] == 1 and out["in_cycle"] is False


def test_gate_lets_our_own_job_share_the_host(host):
    set_state(host, apps=app(1, OURS) + app(2, OURS), executor_log=marker("remove", 300))
    rc, out = gate(host)
    assert rc == 0 and out["foreign_compute_apps"] == 0


def test_gate_only_looks_at_the_selected_gpus(host):
    set_state(host, apps=app(0, FOREIGN), executor_log=marker("remove", 300))
    assert gate(host, "--gpus", "1,2")[0] == 0
    assert gate(host, "--gpus", "0,3")[0] == 1


def test_watch_writes_a_row_for_a_failed_sample_instead_of_a_gap(host, tmp_path):
    set_state(host, apps=app(0, FOREIGN))
    prefix = tmp_path / "w"
    proc = subprocess.Popen(
        [str(WATCH), str(prefix), "1"], env={**os.environ, **host}, stderr=subprocess.PIPE
    )
    try:
        time.sleep(1.5)
        (Path(host["FAKE_STATE"]) / "smi-fail").write_text("")
        time.sleep(2.0)
    finally:
        proc.terminate()
        proc.wait(timeout=10)
    apps = prefix.with_suffix(".apps.csv").read_text().splitlines()
    assert apps[0] == "time,gpu_uuid,pid,process_name,used_memory"
    rows = [row.split(",") for row in apps[1:]]
    sampled = [row for row in rows if len(row) == 5]
    failed = [row for row in rows if len(row) == 2]
    assert sampled and all(row[3].strip() == FOREIGN for row in sampled)
    assert failed and all(row[1] == "nvidia-smi-failed" for row in failed)
    mem = prefix.with_suffix(".mem.csv").read_text().splitlines()
    assert any(row.endswith(",0, 4 MiB") for row in mem[1:])
    assert any(row.endswith(",nvidia-smi-failed") for row in mem[1:])
