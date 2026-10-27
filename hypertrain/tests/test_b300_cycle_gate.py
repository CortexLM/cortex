"""cycle_gate.sh, gpu_watch.sh and d20/run_container.sh against fake nvidia-smi and docker on PATH.

The fakes stand at the scripts' real boundaries: `nvidia-smi` (GPU uuids and compute apps) and
`docker` (`logs` of the Lium executor container, whose SSH-key markers bracket a validator cycle,
and `run` / `rm` of the step container). The fake `docker logs` accepts only the exact arguments the
gate relies on. No GPU, no network.
"""

from __future__ import annotations

import json
import os
import shutil
import signal
import subprocess
import sys
import time
from collections.abc import Iterator
from datetime import UTC, datetime, timedelta
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
GATE = ROOT / "scripts/b300/cycle_gate.sh"
WATCH = ROOT / "scripts/b300/gpu_watch.sh"
RUNNER = ROOT / "scripts/b300/d20/run_container.sh"
UUIDS = [f"GPU-00000000-0000-0000-0000-00000000000{i}" for i in range(4)]
FOREIGN = "/root/app/.venv/bin/python"
OURS = "/opt/hypertrain/venv/bin/python"
DIGEST = "sha256:" + "ab" * 32
IMAGE = f"ghcr.io/cortexlm/cortex/hypertrain-gpu@{DIGEST}"

# A shell ignores SIGINT in jobs it starts with `&` without job control, and `nohup` ignores SIGHUP;
# a signal ignored at entry cannot be trapped, and pytest may be such a job (a CI script, nohup).
# Reset both before exec.
DEFAULT_SIGNALS = (
    "import os, signal, sys; "
    "signal.signal(signal.SIGINT, signal.SIG_DFL); signal.signal(signal.SIGHUP, signal.SIG_DFL); "
    "os.execv(sys.argv[1], sys.argv[1:])"
)

FAKE_SMI = r"""#!/usr/bin/env bash
state=$FAKE_STATE
case "$*" in
  *--query-gpu=uuid*)
    [ -e "$state/uuid-fail" ] && exit 9
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

# `logs` is only answered for exactly `logs -t --since 30m <executor>` (the gate) or `logs -t
# --since <RFC3339> <executor>` (the runner): any other shape is an error, so a changed flag, a
# dropped -t or a wrong container name fails the tests instead of passing against a lenient fake.
FAKE_DOCKER = r"""#!/usr/bin/env bash
state=$FAKE_STATE
echo "$*" >>"$state/docker.calls"
case $1 in
  info) exit 0 ;;
  logs)
    if [ -e "$state/docker-fail" ]; then
      echo "Error response from daemon: No such container" >&2; exit 1
    fi
    [ "$2" = -t ] && [ "$3" = --since ] && [ "$5" = "${FAKE_EXECUTOR:-executor-executor-1}" ] ||
      { echo "unexpected docker logs arguments: $*" >&2; exit 125; }
    if [ "$4" != 30m ] && ! [[ $4 =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9:]{8}Z$ ]]; then
      echo "unexpected --since $4" >&2; exit 125
    fi
    n=$(cat "$state/log.calls" 2>/dev/null || echo 0); echo $((n + 1)) >"$state/log.calls"
    f=$state/executor-log
    if [ "$n" -ge 1 ] && [ -e "$state/executor-log-later" ]; then f=$state/executor-log-later; fi
    # Honour --since like docker (RFC3339 stamps compare as strings): lines at or after it.
    if [ "$4" = 30m ]; then cat "$f"; else awk -v since="$4" '$1 >= since' "$f"; fi ;;
  rm) printf '%s\n' "$*" >>"$state/docker.rm"
      [ -e "$state/run.pid" ] && kill "$(cat "$state/run.pid")" 2>/dev/null; exit 0 ;;
  run)
    printf '%s\n' "$@" >"$state/docker-run.argv"
    if [ -n "${FAKE_RUN_SLEEP:-}" ]; then
      echo $$ >"$state/run.pid"; exec sleep "$FAKE_RUN_SLEEP"
    fi
    if [ -n "${FAKE_MARK_DURING:-}" ]; then
      printf '%s INFO: x - "POST /upload_ssh_key HTTP/1.1" 200 OK\n' "$(date -u +%FT%T.%NZ)" \
        >>"$state/executor-log"
    fi
    exit "${FAKE_RUN_RC:-0}" ;;
  *) echo "unexpected docker call: $*" >&2; echo "docker $*" >>"$state/tripwire"; exit 127 ;;
esac
"""

# `date +%s` and `date -u +%s` answer $FAKE_NOW when it is set, so an age is exact; every other call
# (the marker stamps, the log lines) is the real date.
FAKE_DATE = r"""#!/usr/bin/env bash
if [ -n "${FAKE_NOW:-}" ]; then
  case "$*" in "+%s" | "-u +%s") echo "$FAKE_NOW"; exit 0 ;; esac
fi
exec "$FAKE_REAL_DATE" "$@"
"""

# Anything that would leave the host or escalate. The scripts under test never call these.
TRIPWIRE = r"""#!/usr/bin/env bash
echo "$(basename "$0") $*" >>"$FAKE_STATE/tripwire"
echo "unexpected call: $(basename "$0") $*" >&2
exit 127
"""


@pytest.fixture
def host(tmp_path: Path) -> Iterator[dict[str, str]]:
    """A fake host: PATH shims plus FAKE_STATE files the tests rewrite between runs.

    No test may call anything outside the shims: the tripwire log must be empty at teardown.
    """
    bin_dir, state = tmp_path / "bin", tmp_path / "state"
    bin_dir.mkdir()
    state.mkdir()
    shims = {"nvidia-smi": FAKE_SMI, "docker": FAKE_DOCKER, "date": FAKE_DATE}
    shims.update(
        dict.fromkeys(("sudo", "ssh", "scp", "rsync", "curl", "wget", "systemctl"), TRIPWIRE)
    )
    for name, body in shims.items():
        shim = bin_dir / name
        shim.write_text(body)
        shim.chmod(0o755)
    (state / "uuids").write_text("\n".join(UUIDS) + "\n")
    (state / "apps").write_text("")
    (state / "executor-log").write_text("")
    yield {
        "PATH": f"{bin_dir}:{os.environ['PATH']}",
        "FAKE_STATE": str(state),
        "FAKE_REAL_DATE": shutil.which("date") or "/usr/bin/date",
    }
    tripwire = state / "tripwire"
    assert not tripwire.exists(), tripwire.read_text()


def marker(kind: str, age_s: float, status: str = "200 OK", now: datetime | None = None) -> str:
    """One `docker logs -t` line of the executor for an SSH-key call `age_s` s before `now`."""
    at = (now or datetime.now(UTC)) - timedelta(seconds=age_s)
    stamp = at.strftime("%Y-%m-%dT%H:%M:%S.%f") + "000Z"
    return f'{stamp} INFO:     13.223.237.157:4384 - "POST /{kind}_ssh_key HTTP/1.1" {status}\n'


def app(gpu: int, name: str) -> str:
    return f"{UUIDS[gpu]}, 4242, {name}, 2666 MiB\n"


def set_state(host: dict[str, str], **files: str) -> None:
    for name, text in files.items():
        (Path(host["FAKE_STATE"]) / name.replace("_", "-")).write_text(text)


def gate(host: dict[str, str], *args: str) -> tuple[int, dict[str, object]]:
    run = subprocess.run(
        [str(GATE), "--timeout", "0", "--poll", "1", *args],
        env={**os.environ, **host},
        capture_output=True,
        text=True,
        timeout=30,
    )
    return run.returncode, json.loads(run.stdout.splitlines()[-1])


def test_gate_is_clear_on_an_idle_host_after_a_finished_cycle(host):
    set_state(host, executor_log=marker("upload", 400) + marker("remove", 300))
    rc, out = gate(host)
    assert rc == 0
    assert out["clear"] is True and out["last_marker"] == "remove" and out["open_cycles"] == 0


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


def test_default_cycle_max_covers_the_longest_cycle_observed(host):
    # Hyderabad, 2026-10-26: cycles stayed open 749 s (20:02:39) and 783 s (19:47:22).
    set_state(host, executor_log=marker("upload", 780))
    assert gate(host)[0] == 1
    set_state(host, executor_log=marker("upload", 1000))
    rc, out = gate(host)
    assert rc == 0 and out["stale_upload"] is True


def test_a_marker_exactly_cycle_max_old_has_ended_and_one_second_younger_has_not(host):
    # The clock is frozen, so the ages are exact: the cut is "younger than --cycle-max seconds".
    now = datetime(2026, 10, 27, 12, 0, 0, tzinfo=UTC)
    host["FAKE_NOW"] = str(int(now.timestamp()))
    set_state(host, executor_log=marker("upload", 720, now=now))
    rc, out = gate(host, "--cycle-max", "720")
    assert (rc, out["marker_age_s"], out["stale_upload"], out["open_cycles"]) == (0, 720, True, 0)
    set_state(host, executor_log=marker("upload", 719, now=now))
    rc, out = gate(host, "--cycle-max", "720")
    assert (rc, out["marker_age_s"], out["stale_upload"], out["open_cycles"]) == (1, 719, False, 1)


def test_nested_uploads_need_as_many_removes(host):
    # Two uploads (a second one about 7 minutes after the first), then the removes 1 s apart.
    one_remove = marker("upload", 500) + marker("upload", 120) + marker("remove", 60)
    set_state(host, executor_log=one_remove)
    rc, out = gate(host)
    assert rc == 1 and out["open_cycles"] == 1
    set_state(host, executor_log=one_remove + marker("remove", 59))
    assert gate(host)[0] == 0


def test_only_successful_marker_lines_count(host):
    set_state(host, executor_log=marker("remove", 300) + marker("upload", 30, "401 Unauthorized"))
    rc, out = gate(host)
    assert rc == 0 and out["last_marker"] == "remove"


def test_gate_blocks_on_a_foreign_compute_app(host):
    set_state(host, apps=app(2, FOREIGN), executor_log=marker("remove", 300))
    rc, out = gate(host)
    assert rc == 1
    assert out["foreign_compute_apps"] == 1 and out["in_cycle"] is False


def test_gate_lets_our_own_named_processes_pass_and_counts_unnamed_ones_as_foreign(host):
    set_state(host, apps=app(1, OURS) + app(2, OURS), executor_log=marker("remove", 300))
    rc, out = gate(host)
    assert rc == 0 and out["foreign_compute_apps"] == 0
    # Our own short-lived processes also show as `[No data]` or a bare `python`: a name match cannot
    # tell them from another tenant's, so the gate fails closed.
    set_state(host, apps=app(1, OURS) + app(2, "[No data]"))
    assert gate(host)[1]["foreign_compute_apps"] == 1


def test_gate_only_looks_at_the_selected_gpus(host):
    set_state(host, apps=app(0, FOREIGN), executor_log=marker("remove", 300))
    assert gate(host, "--gpus", "1,2")[0] == 0
    assert gate(host, "--gpus", "0,3")[0] == 1


def test_an_unreadable_executor_log_is_not_clear(host):
    set_state(host, executor_log=marker("remove", 300), docker_fail="")
    rc, out = gate(host)
    assert (rc, out["clear"], out["executor_log_ok"]) == (4, False, False)


def test_a_log_without_any_marker_is_not_clear_unless_allowed(host):
    # A recreated executor container has an empty log while the cycles go on.
    set_state(host, executor_log="2026-10-26T20:00:00.000000000Z INFO: unrelated\n")
    rc, out = gate(host)
    assert (rc, out["clear"], out["last_marker"]) == (1, False, "none")
    assert gate(host, "--allow-no-marker")[0] == 0


def test_a_marker_time_that_cannot_be_parsed_blocks(host):
    set_state(host, executor_log='not-a-time INFO: x - "POST /remove_ssh_key HTTP/1.1" 200 OK\n')
    rc, out = gate(host)
    assert (rc, out["clear"], out["parse_error"]) == (1, False, True)


@pytest.mark.parametrize("fault", ["smi-fail", "uuid-fail"])
def test_a_failing_nvidia_smi_is_not_clear(host, fault):
    set_state(host, executor_log=marker("remove", 300), **{fault: ""})
    rc, out = gate(host)
    assert (rc, out["clear"], out["nvidia_smi_ok"]) == (4, False, False)


def test_an_empty_gpu_list_is_not_clear(host):
    set_state(host, uuids="", apps=app(1, FOREIGN), executor_log=marker("remove", 300))
    rc, out = gate(host)
    assert (rc, out["clear"], out["nvidia_smi_ok"]) == (4, False, False)


def test_gate_waits_through_a_cycle_and_clears_on_a_later_poll(host):
    set_state(host, executor_log=marker("upload", 30))
    set_state(host, executor_log_later=marker("upload", 31) + marker("remove", 1))
    run = subprocess.run(
        [str(GATE), "--timeout", "20", "--poll", "1"],
        env={**os.environ, **host},
        capture_output=True,
        text=True,
        timeout=60,
    )
    polls = [json.loads(line) for line in run.stdout.splitlines()]
    assert run.returncode == 0
    assert [p["clear"] for p in polls] == [False, True]


def test_numbers_with_a_leading_zero_are_decimal(host):
    set_state(host, executor_log=marker("remove", 300))
    run = subprocess.run(
        [str(GATE), "--timeout", "08", "--poll", "09", "--cycle-max", "0900"],
        env={**os.environ, **host},
        capture_output=True,
        text=True,
        timeout=30,
    )
    assert (run.returncode, run.stderr) == (0, "")


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


def runner(
    host: dict[str, str], out: Path, *args: str, env: dict[str, str] | None = None
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [str(RUNNER), "--image", IMAGE, "--gpu", "5", "--out", str(out), *args],
        env={**os.environ, **host, **(env or {})},
        capture_output=True,
        text=True,
        timeout=60,
    )


@pytest.mark.parametrize(
    "args",
    [
        ["--image", "ghcr.io/x/y:latest"],
        ["--mode", "dp8"],
        ["--gpu", "five"],
        ["--sm-count", "148; rm -rf /"],
    ],
    ids=["tag_not_digest", "unknown_mode", "gpu_not_a_number", "injected_sm_count"],
)
def test_runner_refuses_bad_arguments_before_touching_docker(host, tmp_path, args):
    out = tmp_path / "out"
    run = subprocess.run(
        [str(RUNNER), "--image", IMAGE, "--gpu", "5", "--out", str(out), *args],
        env={**os.environ, **host},
        capture_output=True,
        text=True,
        timeout=60,
    )
    assert run.returncode == 2 and not out.exists()
    assert not (Path(host["FAKE_STATE"]) / "docker.calls").exists()


def test_runner_needs_a_new_absolute_output_directory(host, tmp_path):
    (tmp_path / "taken").mkdir()
    assert runner(host, tmp_path / "taken").returncode == 2
    relative = subprocess.run(
        [str(RUNNER), "--image", IMAGE, "--gpu", "5", "--out", "relative"],
        env={**os.environ, **host},
        capture_output=True,
        text=True,
        timeout=60,
    )
    assert relative.returncode == 2


def test_runner_exits_3_when_the_gate_does_not_clear_and_never_starts_the_container(host, tmp_path):
    set_state(host, executor_log=marker("upload", 30))
    out = tmp_path / "out"
    run = runner(host, out, "--gate-timeout", "0")
    assert run.returncode == 3
    assert json.loads((out / "gate.jsonl").read_text().splitlines()[-1])["clear"] is False
    assert "run" not in (Path(host["FAKE_STATE"]) / "docker.calls").read_text().split()


def test_runner_passes_the_container_status_on_and_records_the_overlap(host, tmp_path):
    # A validator cycle closed 20 min before the window, another opens during it.
    set_state(host, executor_log=marker("upload", 1500) + marker("remove", 1400))
    out = tmp_path / "out"
    run = runner(host, out, "--mode", "job-kimi", env={"FAKE_RUN_RC": "7", "FAKE_MARK_DURING": "1"})
    assert run.returncode == 7
    record = json.loads((out / "run.json").read_text())
    assert (record["rc"], record["interrupted"], record["gated"]) == (7, False, True)
    assert record["gate_last"]["clear"] is True and record["executor_log_ok"] is True
    assert record["last_marker_before_window"]["kind"] == "remove"
    assert [m["kind"] for m in record["executor_markers_in_window"]] == ["upload"]
    argv = (Path(host["FAKE_STATE"]) / "docker-run.argv").read_text().split("\n")
    assert "ht-s9-job-kimi-gpu5" in argv and "device=5" in argv and IMAGE in argv
    assert f"HT_IMAGE_DIGEST={DIGEST}" in argv and f"{out}:/out" in argv
    removed = (Path(host["FAKE_STATE"]) / "docker.rm").read_text().split()
    assert set(removed) <= {"rm", "-f", "ht-s9-job-kimi-gpu5"}


def test_runner_container_is_removed_at_exit_and_only_the_out_dir_is_writable(host, tmp_path):
    out = tmp_path / "out"
    assert runner(host, out, "--no-gate", "--mode", "smoke").returncode == 0
    argv = (Path(host["FAKE_STATE"]) / "docker-run.argv").read_text().splitlines()
    assert "--rm" in argv
    assert argv[argv.index("--name") + 1] == "ht-s9-smoke-gpu5"
    # The checkout (which the container runs code from) is read-only; the out dir is not.
    mounts = [argv[i + 1] for i, arg in enumerate(argv) if arg == "-v"]
    assert mounts == [f"{ROOT}:/opt/ht:ro", f"{out}:/out"]
    joined = " ".join(argv)
    for host_access in ("--privileged", "--pid=host", "--pid host", "docker.sock"):
        assert host_access not in joined
    for host_network in ("--net=host", "--net host", "--network=host", "--network host"):
        assert host_network not in joined


def test_runner_says_so_when_the_executor_log_cannot_be_read(host, tmp_path):
    set_state(host, docker_fail="")
    out = tmp_path / "out"
    run = runner(host, out, "--no-gate")
    assert run.returncode == 0
    record = json.loads((out / "run.json").read_text())
    assert (record["gated"], record["gate_last"], record["executor_log_ok"]) == (False, None, False)


@pytest.mark.parametrize(
    "interrupt",
    [signal.SIGTERM, signal.SIGINT, signal.SIGHUP],
    ids=["sigterm", "sigint", "sighup"],
)
def test_runner_removes_only_its_container_and_reports_143_when_interrupted(
    host, tmp_path, interrupt
):
    set_state(host, executor_log=marker("remove", 300))
    out = tmp_path / "out"
    proc = subprocess.Popen(
        [
            sys.executable,
            "-c",
            DEFAULT_SIGNALS,
            str(RUNNER),
            "--image",
            IMAGE,
            "--gpu",
            "5",
            "--out",
            str(out),
        ],
        env={**os.environ, **host, "FAKE_RUN_SLEEP": "30"},
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    pid_file = Path(host["FAKE_STATE"]) / "run.pid"
    deadline = time.monotonic() + 20
    while not pid_file.exists() and time.monotonic() < deadline:
        time.sleep(0.1)
    assert pid_file.exists()
    proc.send_signal(interrupt)
    proc.communicate(timeout=30)
    assert proc.returncode == 143
    record = json.loads((out / "run.json").read_text())
    assert (record["rc"], record["interrupted"]) == (143, True)
    removed = (Path(host["FAKE_STATE"]) / "docker.rm").read_text().split()
    assert set(removed) <= {"rm", "-f", "ht-s9-multi-gpu5"} and "ht-s9-multi-gpu5" in removed


def wait_until(condition, seconds: float) -> bool:
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if condition():
            return True
        time.sleep(0.1)
    return condition()


def watcher_pids(prefix: Path) -> list[int]:
    """Processes that have `prefix` as an argument: the gpu_watch.sh of one run (and its forks)."""
    pids = []
    for entry in Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        try:
            argv = (entry / "cmdline").read_bytes().split(b"\0")
        except OSError:  # the process ended between the listing and the read
            continue
        if str(prefix).encode() in argv:
            pids.append(int(entry.name))
    return pids


def test_a_sigkilled_runner_leaves_no_watcher_but_a_live_runner_keeps_it_sampling(host, tmp_path):
    set_state(host, executor_log=marker("remove", 300))
    out = tmp_path / "out"
    prefix = out / "gpu-watch"
    run_pid = Path(host["FAKE_STATE"]) / "run.pid"
    proc = subprocess.Popen(
        [str(RUNNER), "--image", IMAGE, "--gpu", "5", "--out", str(out)],
        env={**os.environ, **host, "FAKE_RUN_SLEEP": "60"},
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        assert wait_until(run_pid.exists, 30)
        # Alive: the watcher takes sample after sample (header plus at least two rows).
        mem = prefix.with_suffix(".mem.csv")
        assert wait_until(lambda: mem.exists() and len(mem.read_text().splitlines()) >= 3, 30)
        assert watcher_pids(prefix)
        proc.kill()
        proc.wait(timeout=10)
        assert wait_until(lambda: not watcher_pids(prefix), 30), "the watcher outlived its launcher"
    finally:
        # SIGKILL skips every trap: end the fake container (a sleep) and any stuck watcher here.
        leftovers = watcher_pids(prefix)
        if run_pid.exists():
            leftovers.append(int(run_pid.read_text()))
        for pid in leftovers:
            try:
                os.kill(pid, signal.SIGKILL)
            except ProcessLookupError:  # already gone
                pass
        proc.kill()
        proc.wait(timeout=10)


@pytest.mark.parametrize("parent", ["12x", "0"], ids=["not_a_number", "zero"])
def test_watch_refuses_a_parent_pid_that_is_not_positive_before_writing_anything(
    host, tmp_path, parent
):
    run = subprocess.run(
        [str(WATCH), str(tmp_path / "w"), "1", parent],
        env={**os.environ, **host},
        capture_output=True,
        text=True,
        timeout=30,
    )
    assert run.returncode == 2 and "PARENT_PID" in run.stderr
    assert list(tmp_path.glob("w.*")) == []
