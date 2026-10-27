"""scripts/b300/run_host.sh against fake docker / nvidia-smi / aws and a fake IMDSv2 server.

The fakes stand at the script's real boundaries: docker and nvidia-smi on PATH (argv recorded),
EC2 instance metadata over HTTP (session PUT, then spot/instance-action). No GPU, no network.
"""

from __future__ import annotations

import contextlib
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import threading
import time
from collections.abc import Callable, Iterator
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any

import pytest

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/b300/run_host.sh"
DIGEST = "sha256:f4b2885f2dc89478da645b9f5e6f02891cd3309ef0a1f2aa0de04328e11cba0f"
IMAGE = f"ghcr.io/cortexlm/cortex/hypertrain-gpu@{DIGEST}"
SESSION = "fake-imds-session"
NOTICE = {"action": "terminate", "time": "2026-10-26T14:00:00Z"}

FAKE_DOCKER = r"""
import json, os, pathlib, re, sys, time
argv = sys.argv[1:]
state = pathlib.Path(os.environ["FAKE_STATE"])
if os.environ.get("FAKE_NEEDS_SUDO") == "1" and os.environ.get("FAKE_SUDO") != "1":
    sys.exit("permission denied while trying to connect to the docker API")
with (state / "docker.jsonl").open("a") as f:
    f.write(json.dumps(argv) + "\n")
cmd = argv[0]
if cmd in ("run", "pull"):
    try:  # fd 9 is the flock of the results directory: a long-lived child must not hold it
        os.fstat(9)
        (state / "fd9-leaked").touch()
    except OSError:
        pass


# Containers are state/ctr-<id> files holding {"name", "labels"}; <file>.stopped and <file>.removed
# mark a container that exited or was removed. Looked up, filtered and destroyed like docker does.
def containers():
    for p in sorted(state.glob("ctr-*")):
        if p.suffix not in (".stopped", ".removed"):
            yield p, json.loads(p.read_text())


def marker(p, kind):
    return pathlib.Path(f"{p}.{kind}")


def running(p):
    return not marker(p, "stopped").exists() and not marker(p, "removed").exists()


def find(ref):
    hits = [(p, d) for p, d in containers() if ref in (p.name, d["name"]) and running(p)]
    return hits[0] if hits else None


def matches(doc, flt):
    key, _, val = flt.partition("=")
    if key == "label":
        k, eq, v = val.partition("=")
        return k in doc["labels"] and (not eq or doc["labels"][k] == v)
    if key == "name":
        return bool(re.search(val, doc["name"]) or re.search(val, "/" + doc["name"]))
    sys.exit(f"Error response from daemon: invalid filter '{key}'")


def destroy(ref, kind, rm=False):
    hit = find(ref)
    if hit is None:
        print(f"Error response from daemon: No such container: {ref}", file=sys.stderr)
        return 1
    marker(hit[0], kind).touch()
    if rm:
        marker(hit[0], "removed").touch()
    with (state / "destroyed.jsonl").open("a") as f:
        f.write(json.dumps([kind, hit[0].name, hit[1]["name"]]) + "\n")
    return 0


if cmd == "info":
    print(state)
elif cmd == "pull":
    if os.environ.get("FAKE_PULL_SLEEP"):
        (state / "pull.pid").write_text(str(os.getpid()))
        time.sleep(float(os.environ["FAKE_PULL_SLEEP"]))
    sys.exit(int(os.environ.get("FAKE_PULL_RC", "0")))
elif cmd == "image":
    print(json.dumps([os.environ.get("FAKE_REPO_DIGEST", argv[-1])]))
elif cmd == "ps":
    every = any(a in ("-a", "--all", "-aq", "-qa") for a in argv[1:])
    flts = [argv[i + 1] for i, a in enumerate(argv) if a == "--filter"]
    for p, doc in containers():
        if (every or running(p)) and not marker(p, "removed").exists():
            if all(matches(doc, f) for f in flts):
                print(p.name)
elif cmd == "stop":
    args, skip = [], False
    for a in argv[1:]:
        if skip:
            skip = False
        elif a in ("-t", "--time"):
            skip = True
        else:
            args.append(a)
    sys.exit(max([destroy(ref, "stopped") for ref in args], default=0))
elif cmd == "rm":
    force = any(a in ("-f", "--force") for a in argv[1:])
    refs = [a for a in argv[1:] if not a.startswith("-")]
    sys.exit(max([destroy(ref, "stopped", rm=True) if force else 1 for ref in refs], default=0))
elif cmd == "run":
    full = argv[argv.index("--name") + 1]
    name = full.removeprefix("ht-b300-")
    labels = dict(argv[i + 1].split("=", 1) for i, a in enumerate(argv) if a == "--label")
    if find(full) is not None:
        print(f'docker: Error response from daemon: Conflict. The container name "/{full}" is '
              "already in use. You have to remove (or rename) that container to be able to "
              "reuse that name.", file=sys.stderr)
        sys.exit(125)
    mounts = dict(a.split(":")[:2] for a in argv if a.count(":") >= 1 and a.startswith("/"))
    out = pathlib.Path(next(h for h, c in mounts.items() if c == "/out"))
    # Resolve the program like python would: relative to -w, through the bind mounts.
    workdir = argv[len(argv) - 1 - argv[::-1].index("-w") + 1]
    if "-c" in argv:
        prog = next(w for w in argv[argv.index("-c") + 1].split() if w.endswith(".py"))
    else:
        prog = next((a for a in argv if a.endswith(".py")), None)
    if prog is not None:
        path = prog if prog.startswith("/") else f"{workdir}/{prog}"
        host = next((h + path[len(c):] for h, c in mounts.items() if path.startswith(c + "/")), "")
        if not pathlib.Path(host).is_file():
            sys.exit(f"python: can't open file {path!r}")
    crash = os.environ.get("FAKE_CRASH", "")
    if crash and (name == crash or (crash == "miner_prod" and name.startswith("miner-prod-"))):
        sys.exit(1)  # dies before it writes any evidence
    if name == os.environ.get("FAKE_BLOCK"):
        ctr = state / f"ctr-{name}"
        for kind in ("stopped", "removed"):
            marker(ctr, kind).unlink(missing_ok=True)
        ctr.write_text(json.dumps({"name": full, "labels": labels}))
        while not marker(ctr, "stopped").exists():
            time.sleep(0.05)
        sys.exit(int(os.environ.get("FAKE_BLOCK_RC", "143")))
    if name == "gpu_probe":
        ok = os.environ.get("FAKE_GPU_OK", "1") == "1"
        (out / "gpu_probe.json").write_text(json.dumps({"ok": ok}))
        sys.exit(0 if ok else 1)
    if name == "mfu":
        steps, w, tps, fpt, n, peak = [1.0, 1.0, 1.0], 1, 65536, 6 * 10**9, 8, 209.5e12
        mfu = tps * 2 / 2.0 * fpt / (n * peak)
        root = "ab" * 32
        replay = root if os.environ.get("FAKE_REPLAY", "1") == "1" else "cd" * 32
        ev = {"kind": "mfu", "step_seconds": steps, "warmup_steps": w, "tokens_per_step": tps,
              "flops_per_token": fpt, "n_gpus": n, "dp_size": n, "peak_vendor_flops": peak,
              "vendor": {"mfu": mfu, "tokens_per_s": tps}, "tokens_per_s": tps,
              "leaves_root": root, "replay_leaves_root": replay, "rank_leaves_roots": [root] * n,
              "replay_match": replay == root, "attention_backend": "sdpa-flash",
              "attention_paths": ["flash"], "gpu_name": "fake B300"}
        (out / argv[-1].removeprefix("/out/")).write_text(json.dumps(ev))
    elif name == "miner_test":
        result = os.environ.get("FAKE_SMOKE", "PASS")
        (out / "mine-smoke-v2.json").write_text(json.dumps({"result": result, "stages": []}))
        print(json.dumps({"result": result}))
    elif name.startswith("miner-prod-"):
        w = argv[argv.index("--round") + 1]
        cfg_dir = pathlib.Path(next(h for h, c in mounts.items() if h == c))
        pub = cfg_dir / f"work/run/hotkey/{w}/published/rank-0"
        pub.mkdir(parents=True)
        commitments = {"leaves_root": "11" * 32, "delta_hash": "22" * 32,
                       "state_root": "33" * 32, "leaves": ["44" * 32]}
        (pub / "summary.json").write_text(json.dumps({"commitments": commitments}))
        print(json.dumps({"status": "UPLOADED"}))
"""

FAKE_SMI = r"""
import os, sys
if any(a.startswith("--query-gpu") for a in sys.argv):
    if os.environ.get("FAKE_SMI_FAIL"):
        sys.exit("NVIDIA-SMI has failed because it couldn't communicate with the NVIDIA driver")
    n, drv, cc = (os.environ.get(k, d) for k, d in
                  (("FAKE_GPUS", "8"), ("FAKE_DRIVER", "595.91.07"), ("FAKE_CC", "10.3")))
    for i in range(int(n)):
        last = i == int(n) - 1  # FAKE_DRIVER_LAST / FAKE_CC_LAST: the last GPU differs
        d = os.environ.get("FAKE_DRIVER_LAST", drv) if last else drv
        c = os.environ.get("FAKE_CC_LAST", cc) if last else cc
        print(f"{i}, NVIDIA B300 SXM6 AC, {d}, {c}, 275040 MiB, 00000000:{i:02X}:00.0")
else:
    print("fake nvidia-smi table")
"""

# df -P -B1G PATH...: one row per path, Used from FAKE_DF_USED, Available from FAKE_DF_AVAIL
# (comma separated, in path order; 500 when absent). Used is large on purpose.
FAKE_DF = r"""
import os, sys
paths = [a for a in sys.argv[1:] if not a.startswith("-")]
avail = os.environ.get("FAKE_DF_AVAIL", "").split(",")
print("Filesystem 1G-blocks Used Available Capacity Mounted on")
for i, _ in enumerate(paths):
    free = int(avail[i]) if i < len(avail) and avail[i] else 500
    print(f"/dev/fake{i} 1000G {os.environ.get('FAKE_DF_USED', '900')}G {free}G 50% /mnt/fake{i}")
"""

FAKE_SUDO = r"""
import json, os, pathlib, sys
with (pathlib.Path(os.environ["FAKE_STATE"]) / "sudo.jsonl").open("a") as f:
    f.write(json.dumps(sys.argv[1:]) + "\n")
args = sys.argv[1:]
if args[:1] == ["-n"]:
    args = args[1:]
os.execvpe(args[0], args, {**os.environ, "FAKE_SUDO": "1"})
"""

# aws s3 cp --recursive SRC s3://DST with the CLI's filters: --exclude/--include are tried in order
# on SRC/<relative path>, the last match wins, everything is included by default. The files that
# pass are copied under state/s3/<bucket and key>.
FAKE_AWS = r"""
import fnmatch, json, os, pathlib, shutil, sys
state = pathlib.Path(os.environ["FAKE_STATE"])
args = sys.argv[1:]
with (state / "aws.jsonl").open("a") as f:
    f.write(json.dumps(args) + "\n")
filters, paths, i = [], [], 2
while i < len(args):
    if args[i] in ("--exclude", "--include"):
        filters.append((args[i][2:], args[i + 1]))
        i += 2
    else:
        if not args[i].startswith("--"):
            paths.append(args[i])
        i += 1
src, dst = pathlib.Path(paths[0]), paths[1].removeprefix("s3://")
for f in sorted(src.rglob("*")):
    if f.is_file():
        keep = True
        for kind, pattern in filters:
            if fnmatch.fnmatchcase(str(f), f"{src}/{pattern}"):
                keep = kind == "include"
        if keep:
            target = state / "s3" / dst / f.relative_to(src)
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(f, target)
"""

FAKE_FI_INFO = "print('fake fi_info')\n"


class Imds(BaseHTTPRequestHandler):
    notice = False

    def do_PUT(self) -> None:  # noqa: N802 - http.server API
        ok = (
            self.path == "/latest/api/token"
            and "X-aws-ec2-metadata-token-ttl-seconds" in self.headers
        )
        self.send_response(200 if ok else 400)
        self.end_headers()
        if ok:
            self.wfile.write(SESSION.encode())

    def do_GET(self) -> None:  # noqa: N802 - http.server API
        authed = self.headers.get("X-aws-ec2-metadata-token") == SESSION
        hit = self.path == "/latest/meta-data/spot/instance-action" and authed and Imds.notice
        self.send_response(200 if hit else (404 if authed else 401))
        self.end_headers()
        if hit:
            self.wfile.write(json.dumps(NOTICE).encode())

    def log_message(self, *_: Any) -> None:
        pass


@pytest.fixture
def imds() -> Iterator[str]:
    Imds.notice = False
    server = ThreadingHTTPServer(("127.0.0.1", 0), Imds)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    yield f"http://127.0.0.1:{server.server_address[1]}"
    server.shutdown()


@pytest.fixture
def env(tmp_path: Path, imds: str) -> dict[str, str]:
    bindir, state, efa = tmp_path / "bin", tmp_path / "state", tmp_path / "infiniband"
    for d in (bindir, state, efa, tmp_path / "results"):
        d.mkdir()
    (efa / "rdmap0s0").mkdir()
    fakes = {
        "docker": FAKE_DOCKER,
        "nvidia-smi": FAKE_SMI,
        "aws": FAKE_AWS,
        "sudo": FAKE_SUDO,
        "fi_info": FAKE_FI_INFO,
        "df": FAKE_DF,
    }
    for name, body in fakes.items():
        (bindir / name).write_text(f"#!{sys.executable}\n{body}")
        (bindir / name).chmod(0o755)
    return {
        **os.environ,
        "PATH": f"{bindir}:{os.environ['PATH']}",
        "FAKE_STATE": str(state),
        "HT_IMDS_URL": imds,
        "HT_B300_POLL_SECONDS": "0.2",
        "HT_B300_EFA_SYSFS": str(efa),
        "HT_B300_MIN_DISK_GB": "0",
        "GIT_CEILING_DIRECTORIES": str(tmp_path),
    }


def run(
    env: dict[str, str], tmp_path: Path, *args: str, script: Path = SCRIPT
) -> subprocess.CompletedProcess[str]:
    argv = ["bash", str(script), "--results-root", str(tmp_path / "results"), "--host", "h1"]
    return subprocess.run(
        [*argv, *args], env=env, capture_output=True, text=True, timeout=60, check=False
    )


def docker_calls(tmp_path: Path) -> list[list[str]]:
    trace = tmp_path / "state/docker.jsonl"
    return [json.loads(x) for x in trace.read_text().splitlines()] if trace.exists() else []


def runs_of(tmp_path: Path, name: str) -> list[list[str]]:
    return [c for c in docker_calls(tmp_path) if c[0] == "run" and f"ht-b300-{name}" in c]


def git(ht: Path, *args: str) -> str:
    cfg = ["-c", "user.name=t", "-c", "user.email=t@example.com", "-c", "commit.gpgsign=false"]
    done = subprocess.run(  # noqa: S603 - fixed argv
        ["git", "-C", str(ht), *cfg, *args], check=True, capture_output=True, text=True
    )
    return done.stdout.strip()


@pytest.fixture
def checkout(tmp_path: Path) -> Path:
    """A clean git checkout holding only scripts/: the HT that a copy of run_host.sh mounts."""
    ht = tmp_path / "ht"
    shutil.copytree(ROOT / "scripts", ht / "scripts", ignore=shutil.ignore_patterns("__pycache__"))
    git(ht, "init", "-q")
    git(ht, "add", ".")
    git(ht, "commit", "-q", "-m", "scripts")
    return ht


def uploaded(tmp_path: Path, dest: str = "bucket/b300/h1") -> set[str]:
    """Relative paths the fake aws copied to s3://<dest>."""
    root = tmp_path / "state/s3" / dest
    return {str(f.relative_to(root)) for f in root.rglob("*") if f.is_file()}


def stop_json(tmp_path: Path) -> dict[str, Any]:
    return json.loads((tmp_path / "results/h1/stop.json").read_text())


def alive(pid: int) -> bool:
    try:
        return Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[0] != "Z"
    except FileNotFoundError:
        return False


def watchdog_pid(tmp_path: Path) -> int:
    match = re.search(r"watchdog pid (\d+)", (tmp_path / "results/h1/run.log").read_text())
    assert match
    return int(match.group(1))


def wait_until(cond: Callable[[], object], what: str, timeout: float = 30) -> None:
    end = time.monotonic() + timeout
    while not cond():
        assert time.monotonic() < end, f"timed out waiting for {what}"
        time.sleep(0.05)


@pytest.fixture
def launch(env: dict[str, str], tmp_path: Path) -> Iterator[Callable[..., subprocess.Popen[bytes]]]:
    """Start run_host.sh in its own process group; teardown frees fake containers and kills it."""
    procs: list[subprocess.Popen[bytes]] = []

    def start(*args: str, script: Path = SCRIPT, **extra_env: str) -> subprocess.Popen[bytes]:
        argv = ["bash", str(script), "--results-root", str(tmp_path / "results"), "--host", "h1"]
        proc = subprocess.Popen(  # noqa: S603 - fixed argv
            [*argv, *args],
            env={**env, **extra_env},
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            start_new_session=True,
        )
        procs.append(proc)
        return proc

    yield start
    for ctr in (tmp_path / "state").glob("ctr-*"):
        ctr.with_suffix(".stopped").touch()
    for proc in procs:
        with contextlib.suppress(ProcessLookupError):
            os.killpg(proc.pid, signal.SIGKILL)
        proc.wait()


def test_full_run_pins_the_digest_and_runs_the_8gpu_flash_bench(env, tmp_path):
    p = run(env, tmp_path)
    assert p.returncode == 0, p.stdout + p.stderr
    stop = stop_json(tmp_path)
    assert stop["reason"] == "completed"
    assert stop["steps"] == {"host_probe": 0, "pull": 0, "gpu_probe": 0, "mfu": 0, "miner_test": 0}
    assert ["pull", IMAGE] in docker_calls(tmp_path)
    (mfu,) = runs_of(tmp_path, "mfu")
    assert mfu[mfu.index("--entrypoint") + 2] == IMAGE
    assert "HT_ATTN_BACKEND=sdpa-flash" in mfu
    assert "--network" not in mfu  # torchrun --standalone must resolve the container hostname
    assert mfu[mfu.index("--nproc-per-node") + 1] == "8"
    assert mfu[mfu.index("--profile") + 1] == "mfu-dense-1b-dp8"
    assert "--nccl-pcie" not in mfu  # B300 GPUs talk over NVLink
    (smoke,) = runs_of(tmp_path, "miner_test")
    assert smoke[smoke.index("--user") + 1] == f"{os.getuid()}:{os.getgid()}"  # 0600 evidence


def test_docker_without_group_membership_goes_through_sudo(env, tmp_path):
    p = run({**env, "FAKE_NEEDS_SUDO": "1"}, tmp_path)
    assert p.returncode == 0, p.stdout + p.stderr
    sudo = [json.loads(x) for x in (tmp_path / "state/sudo.jsonl").read_text().splitlines()]
    assert ["-n", "docker", "pull", IMAGE] in sudo
    assert all(c[:2] == ["-n", "docker"] for c in sudo)


@pytest.mark.parametrize(
    "fake,check",
    [
        ({"FAKE_DRIVER": "580.95.05"}, "driver"),
        ({"FAKE_GPUS": "7"}, "gpu_count"),
        ({"FAKE_CC": "12.0"}, "compute_cap"),
        ({"FAKE_GPUS": "9"}, "gpu_count"),  # more than expected is a different host too
        ({"FAKE_DRIVER_LAST": "580.95.05"}, "driver"),  # one GPU on another driver
        ({"FAKE_CC_LAST": "10.0"}, "compute_cap"),  # one GPU of another capability
    ],
)
def test_wrong_host_stops_before_any_pull(env, tmp_path, fake, check):
    p = run({**env, **fake}, tmp_path)
    assert p.returncode == 3, p.stdout + p.stderr
    probe = json.loads((tmp_path / "results/h1/host_probe.json").read_text())
    assert probe["checks"][check] is False and not probe["ok"]
    assert not [c for c in docker_calls(tmp_path) if c[0] in ("pull", "run")]


def test_missing_efa_fails_the_host_probe(env, tmp_path):
    (tmp_path / "infiniband/rdmap0s0").rmdir()
    assert run(env, tmp_path).returncode == 3


def test_tag_only_image_is_refused_before_docker(env, tmp_path):
    p = run(env, tmp_path, "--image", "ghcr.io/cortexlm/cortex/hypertrain-gpu:latest")
    assert p.returncode == 2 and "REPO@sha256" in p.stderr
    assert docker_calls(tmp_path) == []


def test_pulled_image_must_carry_the_pinned_digest(env, tmp_path):
    other = f"ghcr.io/cortexlm/cortex/hypertrain-gpu@sha256:{'0' * 64}"
    assert run({**env, "FAKE_REPO_DIGEST": other}, tmp_path).returncode == 4
    assert runs_of(tmp_path, "gpu_probe") == []


def test_rerun_skips_passed_steps_but_reprobes_the_host(env, tmp_path, checkout):
    script = checkout / "scripts/b300/run_host.sh"
    assert run(env, tmp_path, script=script).returncode == 0
    first = len(docker_calls(tmp_path))
    assert run(env, tmp_path, script=script).returncode == 0
    again = docker_calls(tmp_path)[first:]
    assert [c for c in again if c[0] in ("pull", "run")] == []
    assert ["info", "--format", "{{.DockerRootDir}}"] in again  # host probe ran again
    for kept in ("gpu_probe.json", "mfu-dp8-flash.json", "mine-smoke-v2.json"):
        assert (tmp_path / "results/h1" / kept).exists()  # a skipped step keeps its evidence
    assert run(env, tmp_path, "--force", "--steps", "mfu", script=script).returncode == 0
    assert len(runs_of(tmp_path, "mfu")) == 2


def test_new_image_reruns_steps_that_passed_with_the_old_one(env, tmp_path):
    assert run(env, tmp_path).returncode == 0
    other = f"ghcr.io/cortexlm/cortex/hypertrain-gpu@sha256:{'1' * 64}"
    assert run({**env, "FAKE_PULL_RC": "1"}, tmp_path, "--image", other).returncode == 4
    assert ["pull", other] in docker_calls(tmp_path)


def test_a_second_run_on_the_same_results_is_refused(env, tmp_path):
    import fcntl

    out = tmp_path / "results/h1"
    out.mkdir(parents=True)
    with (out / ".lock").open("w") as held:
        fcntl.flock(held, fcntl.LOCK_EX | fcntl.LOCK_NB)
        p = run(env, tmp_path)
    assert p.returncode == 2 and ".lock" in p.stdout + p.stderr
    assert docker_calls(tmp_path) == []


def test_mfu_replay_mismatch_fails_and_stops_before_mining(env, tmp_path):
    p = run({**env, "FAKE_REPLAY": "0"}, tmp_path)
    assert p.returncode == 6, p.stdout + p.stderr
    assert stop_json(tmp_path)["steps"]["mfu"] != 0
    assert runs_of(tmp_path, "miner_test") == []


def test_failed_smoke_is_a_failed_miner_step(env, tmp_path):
    assert run({**env, "FAKE_SMOKE": "FAIL"}, tmp_path).returncode == 7


def arm_notice_when_running(tmp_path: Path, name: str) -> None:
    def arm() -> None:
        while not (tmp_path / f"state/ctr-{name}").exists():
            time.sleep(0.05)
        Imds.notice = True

    threading.Thread(target=arm, daemon=True).start()


def test_spot_notice_stops_containers_and_uploads(env, tmp_path):
    arm_notice_when_running(tmp_path, "miner_test")
    p = run({**env, "FAKE_BLOCK": "miner_test"}, tmp_path, "--upload", "s3://bucket/b300")
    assert p.returncode == 143, p.stdout + p.stderr
    stop = stop_json(tmp_path)
    assert stop["reason"] == "spot-interruption" and stop["spot_notice"] == NOTICE
    assert ["stop", "--time", "60", "ctr-miner_test"] in docker_calls(tmp_path)
    assert len((tmp_path / "state/aws.jsonl").read_text().splitlines()) == 1
    assert {"stop.json", "spot-instance-action.json", "run.log"} <= uploaded(tmp_path)


def test_duration_deadline_stops_the_run(env, tmp_path):
    t0 = time.monotonic()
    p = run({**env, "FAKE_BLOCK": "mfu"}, tmp_path, "--duration", "2")
    assert p.returncode == 143 and stop_json(tmp_path)["reason"] == "duration"
    assert time.monotonic() - t0 < 30


def test_sigterm_drains_the_running_container(env, tmp_path):
    argv = ["bash", str(SCRIPT), "--results-root", str(tmp_path / "results"), "--host", "h1"]
    proc = subprocess.Popen(  # noqa: S603 - fixed argv
        argv,
        env={**env, "FAKE_BLOCK": "gpu_probe"},
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    deadline = time.monotonic() + 30
    while not (tmp_path / "state/ctr-gpu_probe").exists():
        assert time.monotonic() < deadline and proc.poll() is None
        time.sleep(0.05)
    proc.send_signal(signal.SIGTERM)
    assert proc.wait(timeout=30) == 143
    stop = stop_json(tmp_path)
    assert stop["reason"] == "signal" and "gpu_probe" not in stop["steps"]
    assert ["stop", "--time", "60", "ctr-gpu_probe"] in docker_calls(tmp_path)


def test_prod_needs_an_endpoint(env, tmp_path):
    p = run(env, tmp_path, "--prod")
    assert p.returncode == 2 and "HT_B300_PROD_ENDPOINT" in p.stderr
    assert docker_calls(tmp_path) == []


def test_prod_rounds_upload_and_never_print_the_key(env, tmp_path):
    cfg_dir = tmp_path / "miner"
    cfg_dir.mkdir()
    (cfg_dir / "miner.toml").write_text('keyfile = "hk.bin"\nworkdir = "work"\n')
    sentinel = "do-not-print-this-sentinel"
    (cfg_dir / "hk.bin").write_text(sentinel)
    prod = {
        "HT_B300_PROD_ENDPOINT": "https://master.example/challenge/hypertrain",
        "HT_B300_MINER_CONFIG": str(cfg_dir / "miner.toml"),
        "HT_B300_PROD_ROUNDS": "3 4",
    }
    p = run({**env, **prod}, tmp_path, "--prod")
    assert p.returncode == 0, p.stdout + p.stderr
    rounds = [c for c in docker_calls(tmp_path) if c[0] == "run" and "run-v2" in c]
    assert [c[c.index("--round") + 1] for c in rounds] == ["3", "4"]
    assert all(f"HYPERTRAIN_MINER_API={prod['HT_B300_PROD_ENDPOINT']}" in c for c in rounds)
    assert all(f"HT_IMAGE_DIGEST={DIGEST}" in c for c in rounds)
    commitments = json.loads(
        (tmp_path / "results/h1/miner-prod/round-4-commitments.json").read_text()
    )
    assert commitments == {
        "leaves_root": "11" * 32,
        "delta_hash": "22" * 32,
        "state_root": "33" * 32,
    }
    results = tmp_path / "results"
    assert sentinel not in p.stdout + p.stderr
    assert not [f for f in results.rglob("*") if f.is_file() and sentinel in f.read_text()]


def test_process_group_sigterm_still_drains(env, tmp_path):
    # systemd and SSM cancel signal the whole group: tee dies with the script, the drain must not.
    argv = ["bash", str(SCRIPT), "--results-root", str(tmp_path / "results"), "--host", "h1"]
    proc = subprocess.Popen(  # noqa: S603 - fixed argv
        argv,
        env={**env, "FAKE_BLOCK": "gpu_probe"},
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        start_new_session=True,
    )
    deadline = time.monotonic() + 30
    while not (tmp_path / "state/ctr-gpu_probe").exists():
        assert time.monotonic() < deadline and proc.poll() is None
        time.sleep(0.05)
    os.killpg(proc.pid, signal.SIGTERM)
    assert proc.wait(timeout=30) == 143
    assert stop_json(tmp_path)["reason"] == "signal"
    assert ["stop", "--time", "60", "ctr-gpu_probe"] in docker_calls(tmp_path)


# --- input validation (B1): text is checked before it reaches arithmetic or a command line ------


def test_duration_injection_is_refused_before_it_is_evaluated(env, tmp_path):
    marker = tmp_path / "pwned"
    p = run(env, tmp_path, "--duration", f"image[$(touch {marker})]h")
    assert p.returncode == 2 and "--duration" in p.stderr, p.stdout + p.stderr
    assert not marker.exists()
    assert docker_calls(tmp_path) == []


@pytest.mark.parametrize("bad", ["1.5h", "h", "3d", "1h30m", "0", "0h", "-5"])
def test_malformed_duration_is_refused_before_docker(env, tmp_path, bad):
    p = run(env, tmp_path, "--duration", bad)
    assert p.returncode == 2 and "--duration" in p.stderr, p.stdout + p.stderr
    assert docker_calls(tmp_path) == []


@pytest.mark.parametrize(
    "duration,seconds", [("08h", 8 * 3600), ("09", 9), ("010h", 10 * 3600), ("090m", 90 * 60)]
)
def test_leading_zeros_are_decimal(env, tmp_path, duration, seconds):
    p = run(env, tmp_path, "--duration", duration, "--steps", "host_probe")
    assert p.returncode == 0, p.stdout + p.stderr
    assert f"duration={seconds}s" in (tmp_path / "results/h1/run.log").read_text()


def test_a_leading_zero_duration_still_arms_the_spot_watchdog(env, tmp_path, launch):
    # bash read 08h as an invalid octal number: the deadline stayed unset and the watchdog died on
    # set -u, taking the Spot drain with it while the run went on.
    proc = launch("--duration", "08h", FAKE_BLOCK="mfu")
    wait_until(lambda: (tmp_path / "state/ctr-mfu").exists(), "the mfu container")
    Imds.notice = True
    try:
        rc = proc.wait(timeout=20)
    except subprocess.TimeoutExpired:
        pytest.fail("the run ignored the Spot notice: its watchdog is dead")
    assert rc == 143 and stop_json(tmp_path)["reason"] == "spot-interruption"


@pytest.mark.parametrize("poll", ["abc", "0", "0.0", "-1", "60", "5s"])
def test_malformed_poll_interval_is_refused_before_docker(env, tmp_path, poll):
    p = run({**env, "HT_B300_POLL_SECONDS": poll}, tmp_path)
    assert p.returncode == 2 and "HT_B300_POLL_SECONDS" in p.stderr, p.stdout + p.stderr
    assert docker_calls(tmp_path) == []


@pytest.mark.parametrize("grace", ["60s", "-1", "1e3", "0x10", "99999"])
def test_malformed_stop_grace_is_refused_before_docker(env, tmp_path, grace):
    p = run({**env, "HT_B300_STOP_GRACE": grace}, tmp_path)
    assert p.returncode == 2 and "HT_B300_STOP_GRACE" in p.stderr, p.stdout + p.stderr
    assert docker_calls(tmp_path) == []


def test_stop_grace_with_leading_zeros_reaches_docker_as_decimal(env, tmp_path, launch):
    proc = launch(FAKE_BLOCK="gpu_probe", HT_B300_STOP_GRACE="060")
    wait_until(lambda: (tmp_path / "state/ctr-gpu_probe").exists(), "the gpu_probe container")
    proc.send_signal(signal.SIGTERM)
    assert proc.wait(timeout=30) == 143
    assert ["stop", "--time", "60", "ctr-gpu_probe"] in docker_calls(tmp_path)


@pytest.mark.parametrize("prefix", ["executor", "pod_x", "ht b300", "ht-", "../ht-x", "HT-b300"])
def test_container_prefix_must_stay_in_the_ht_namespace(env, tmp_path, prefix):
    p = run({**env, "HT_B300_CONTAINER_PREFIX": prefix}, tmp_path)
    assert p.returncode == 2 and "HT_B300_CONTAINER_PREFIX" in p.stderr, p.stdout + p.stderr
    assert docker_calls(tmp_path) == []


@pytest.mark.parametrize("host", ["..", ".", ".hidden", "a/b", "-x"])
def test_host_name_cannot_leave_the_results_root(env, tmp_path, host):
    p = run(env, tmp_path, "--host", host)
    assert p.returncode == 2 and "bad host name" in p.stderr, p.stdout + p.stderr
    assert docker_calls(tmp_path) == []
    assert not (tmp_path / "steps").exists()  # no directory was made above the results root


def test_a_dead_watchdog_stops_the_run_before_the_next_step(env, tmp_path, launch):
    proc = launch(FAKE_BLOCK="gpu_probe", FAKE_BLOCK_RC="0")
    wait_until(lambda: (tmp_path / "state/ctr-gpu_probe").exists(), "the gpu_probe container")
    os.kill(watchdog_pid(tmp_path), signal.SIGKILL)
    (tmp_path / "state/ctr-gpu_probe.stopped").touch()  # the container then exits 0
    try:
        rc = proc.wait(timeout=30)
    except subprocess.TimeoutExpired:
        pytest.fail("the run went on without a watchdog")
    assert rc == 9 and stop_json(tmp_path)["reason"] == "watchdog-died"
    assert runs_of(tmp_path, "mfu") == []


# --- containers (B2): a run only ever touches containers it started ------------------------------

LIUM = {"com.docker.compose.service": "executor"}
OTHER_RUN = {"io.cortex.b300-run": "20261027T000000Z-1"}


def seed_container(tmp_path: Path, name: str, labels: dict[str, str]) -> Path:
    """A container another process started: the fake docker lists, stops and removes it."""
    ctr = tmp_path / "state" / f"ctr-{name.removeprefix('ht-b300-')}"
    ctr.write_text(json.dumps({"name": name, "labels": labels}))
    return ctr


def untouched(ctr: Path) -> bool:
    return not Path(f"{ctr}.stopped").exists() and not Path(f"{ctr}.removed").exists()


def test_a_running_container_with_the_same_name_is_not_removed(env, tmp_path):
    # Another invocation (other results root, same host) owns ht-b300-gpu_probe. The container
    # names are global to the host and the flock is per results directory: removing by name
    # SIGKILLed that run's container.
    theirs = seed_container(tmp_path, "ht-b300-gpu_probe", OTHER_RUN)
    p = run(env, tmp_path)
    assert p.returncode == 5 and "already in use" in p.stdout + p.stderr, p.stdout + p.stderr
    assert untouched(theirs)
    assert not [c for c in docker_calls(tmp_path) if c[0] == "rm"]
    assert runs_of(tmp_path, "mfu") == []


def test_a_drain_stops_this_runs_containers_only(env, tmp_path, launch):
    executor = seed_container(tmp_path, "executor-executor-1", LIUM)
    other = seed_container(tmp_path, "ht-b300-mfu", OTHER_RUN)
    proc = launch(FAKE_BLOCK="gpu_probe")
    wait_until(lambda: (tmp_path / "state/ctr-gpu_probe").exists(), "the gpu_probe container")
    proc.send_signal(signal.SIGTERM)
    assert proc.wait(timeout=30) == 143
    assert Path(f"{tmp_path}/state/ctr-gpu_probe.stopped").exists()
    assert untouched(executor) and untouched(other)
    log = (tmp_path / "state/destroyed.jsonl").read_text().splitlines()
    assert [json.loads(x)[2] for x in log] == ["ht-b300-gpu_probe"]


# --- upload (B3): the key must not leave the host ------------------------------------------------

SENTINEL = "do-not-upload-this-sentinel"
EVIDENCE = {
    "run.log",
    "steps.tsv",
    "stop.json",
    "nvidia-smi.csv",
    "nvidia-smi.txt",
    "efa-devices.txt",
    "fi_info.txt",
    "df.txt",
    "host_probe.json",
    "pull.log",
    "pull-digests.json",
    "gpu_probe.json",
    "mfu-dp8-flash.json",
    "mine-smoke-v2.json",
    "mine-smoke-v2.stdout",
    *(f"steps/{s}.rc" for s in ("host_probe", "pull", "gpu_probe", "mfu", "miner_test")),
}


def prod_env(cfg_dir: Path, rounds: str = "3") -> dict[str, str]:
    cfg_dir.mkdir(parents=True, exist_ok=True)
    (cfg_dir / "miner.toml").write_text('keyfile = "hk.bin"\nworkdir = "work"\n')
    (cfg_dir / "hk.bin").write_text(SENTINEL)
    return {
        "HT_B300_PROD_ENDPOINT": "https://master.example/challenge/hypertrain",
        "HT_B300_MINER_CONFIG": str(cfg_dir / "miner.toml"),
        "HT_B300_PROD_ROUNDS": rounds,
    }


def test_upload_ships_the_evidence_and_nothing_else(env, tmp_path):
    out = tmp_path / "results/h1"
    (out / "miner").mkdir(parents=True)
    (out / "miner/miner.toml").write_text('keyfile = "hk.bin"\n')
    (out / "hk.bin").write_text(SENTINEL)
    (out / "notes.txt").write_text("stray")
    p = run(env, tmp_path, "--upload", "s3://bucket/b300")
    assert p.returncode == 0, p.stdout + p.stderr
    assert uploaded(tmp_path) == EVIDENCE


@pytest.mark.parametrize("where", ["inside", "parent", "symlink", "same"])
def test_a_miner_config_next_to_the_results_is_refused(env, tmp_path, where):
    out = tmp_path / "results/h1"
    cfg_dir = {
        "inside": out / "miner",
        "parent": tmp_path / "results",
        "symlink": tmp_path / "link",
        "same": out,
    }[where]
    if where == "symlink":
        (out / "miner").mkdir(parents=True)
        cfg_dir.symlink_to(out / "miner")
    prod = prod_env(cfg_dir)
    p = run({**env, **prod}, tmp_path, "--prod", "--upload", "s3://bucket/b300")
    assert p.returncode == 2 and "HT_B300_MINER_CONFIG" in p.stderr, p.stdout + p.stderr
    assert docker_calls(tmp_path) == [] and not (tmp_path / "state/aws.jsonl").exists()


def test_prod_upload_carries_the_round_evidence_but_never_the_key(env, tmp_path):
    prod = prod_env(tmp_path / "miner", rounds="3 4")
    p = run({**env, **prod}, tmp_path, "--prod", "--upload", "s3://bucket/b300")
    assert p.returncode == 0, p.stdout + p.stderr
    rounds = {f"miner-prod/round-{w}{s}.json" for w in "34" for s in ("", "-commitments")}
    assert uploaded(tmp_path) == EVIDENCE | {"steps/miner_prod.rc"} | rounds
    root = tmp_path / "state/s3"
    assert not [f for f in root.rglob("*") if f.is_file() and SENTINEL in f.read_text()]


# --- which code ran (B4) -------------------------------------------------------------------------


def test_the_run_records_the_commit_it_ran(env, tmp_path, checkout):
    assert run(env, tmp_path, script=checkout / "scripts/b300/run_host.sh").returncode == 0
    head = git(checkout, "rev-parse", "HEAD")
    stop = stop_json(tmp_path)
    assert stop["git_rev"] == head and stop["git_dirty"] is False
    assert f"rev={head} dirty=0" in (tmp_path / "results/h1/run.log").read_text()


def test_a_new_commit_reruns_what_passed_with_the_old_one(env, tmp_path, checkout):
    script = checkout / "scripts/b300/run_host.sh"
    assert run(env, tmp_path, script=script).returncode == 0
    (checkout / "scripts/new.txt").write_text("x")
    git(checkout, "add", ".")
    git(checkout, "commit", "-q", "-m", "new")
    assert run(env, tmp_path, script=script).returncode == 0
    assert len(runs_of(tmp_path, "gpu_probe")) == 2 and len(runs_of(tmp_path, "mfu")) == 2
    assert stop_json(tmp_path)["git_rev"] == git(checkout, "rev-parse", "HEAD")


def test_a_modified_checkout_is_flagged_and_never_skipped(env, tmp_path, checkout):
    script = checkout / "scripts/b300/run_host.sh"
    (checkout / "scripts/mfu_profiles.json").write_text("{}")  # tracked file, changed
    for n in (1, 2):  # the same dirty tree twice: still a different, unknown code
        assert run(env, tmp_path, script=script).returncode == 0
        assert stop_json(tmp_path)["git_dirty"] is True
        assert len(runs_of(tmp_path, "gpu_probe")) == n
    (checkout / "scripts/mfu_profiles.json").unlink()
    git(checkout, "checkout", "--", "scripts/mfu_profiles.json")
    (checkout / "scripts/untracked.py").write_text("x = 1\n")  # the mount shows it as well
    assert run(env, tmp_path, script=script).returncode == 0
    assert stop_json(tmp_path)["git_dirty"] is True


def test_a_tree_without_git_is_unknown_and_never_skipped(env, tmp_path):
    script = tmp_path / "plain/scripts/b300/run_host.sh"
    shutil.copytree(ROOT / "scripts", tmp_path / "plain/scripts")
    for n in (1, 2):
        assert run(env, tmp_path, script=script).returncode == 0
        stop = stop_json(tmp_path)
        assert stop["git_rev"] == "unknown" and stop["git_dirty"] is None
        assert len(runs_of(tmp_path, "gpu_probe")) == n


# --- shutdown paths: every way out drains, writes stop.json and uploads --------------------------


def test_sighup_drains_like_sigterm_and_leaves_no_watchdog(env, tmp_path, launch):
    proc = launch(FAKE_BLOCK="gpu_probe")
    wait_until(lambda: (tmp_path / "state/ctr-gpu_probe").exists(), "the gpu_probe container")
    pid = watchdog_pid(tmp_path)
    proc.send_signal(signal.SIGHUP)
    assert proc.wait(timeout=30) == 143
    assert stop_json(tmp_path)["reason"] == "signal"
    assert ["stop", "--time", "60", "ctr-gpu_probe"] in docker_calls(tmp_path)
    wait_until(lambda: not alive(pid), "the watchdog to be gone")


def test_an_unexpected_exit_still_drains_uploads_and_stops_the_watchdog(env, tmp_path, launch):
    # The second miner_prod survives the removal of the first one without --prod, and its unbound
    # HT_B300_MINER_CONFIG kills the shell mid-run; any other unexpected exit takes the same path.
    proc = launch("--steps", "miner_prod miner_prod", "--upload", "s3://bucket/b300")
    assert proc.wait(timeout=30) == 1
    stop = stop_json(tmp_path)
    assert stop["reason"] == "unexpected-exit" and stop["rc"] == 1
    assert "stop.json" in uploaded(tmp_path)
    pid = watchdog_pid(tmp_path)
    wait_until(lambda: not alive(pid), "the watchdog to be gone")


def test_a_stop_during_the_pull_ends_the_pull(env, tmp_path, launch):
    proc = launch(FAKE_PULL_SLEEP="300")  # far longer than the wait below: it must be killed
    pidfile = tmp_path / "state/pull.pid"
    wait_until(lambda: pidfile.exists() and pidfile.read_text(), "the docker pull")
    pid = int(pidfile.read_text())
    proc.send_signal(signal.SIGTERM)
    assert proc.wait(timeout=30) == 143
    wait_until(lambda: not alive(pid), "the docker pull client to be gone", timeout=10)


def test_a_corrupt_rc_file_does_not_stop_the_stop_json_or_the_upload(env, tmp_path):
    (tmp_path / "results/h1/steps").mkdir(parents=True)
    (tmp_path / "results/h1/steps/stray.rc").write_text("")
    p = run(env, tmp_path, "--upload", "s3://bucket/b300")
    assert p.returncode == 0, p.stdout + p.stderr
    stop = stop_json(tmp_path)
    assert "stray" not in stop["steps"] and any("stray.rc" in e for e in stop["errors"])
    assert "stop.json" in uploaded(tmp_path)


def test_a_spot_answer_that_is_not_json_is_kept_as_text(env, tmp_path, monkeypatch):
    def get(self: BaseHTTPRequestHandler) -> None:
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b"<html>ok</html>")

    monkeypatch.setattr(Imds, "do_GET", get)
    p = run({**env, "FAKE_BLOCK": "gpu_probe"}, tmp_path, "--upload", "s3://bucket/b300")
    assert p.returncode == 143, p.stdout + p.stderr
    stop = stop_json(tmp_path)
    assert stop["reason"] == "spot-interruption"
    assert stop["spot_notice"] == {"unparsed": "<html>ok</html>"}
    assert any("spot-instance-action.json" in e for e in stop["errors"])
    assert "stop.json" in uploaded(tmp_path)


def test_the_upload_runs_even_when_stop_json_cannot_be_written(env, tmp_path):
    (tmp_path / "results/h1/stop.json").mkdir(parents=True)
    p = run(env, tmp_path, "--upload", "s3://bucket/b300")
    assert p.returncode == 0, p.stdout + p.stderr
    assert {"run.log", "steps.tsv"} <= uploaded(tmp_path)
    assert "stop.json was not written" in (tmp_path / "results/h1/run.log").read_text()


# --- stale evidence (B6): a step removes what an earlier run left before it starts ---------------

STALE = json.dumps({"ok": True, "stale": True})


@pytest.mark.parametrize(
    "crash,rc,files,extra",
    [
        (
            {"FAKE_SMI_FAIL": "1"},
            3,
            ["host_probe.json", "df.txt", "efa-devices.txt", "fi_info.txt", "nvidia-smi.txt"],
            [],
        ),
        ({"FAKE_PULL_RC": "1"}, 4, ["pull-digests.json"], []),
        ({"FAKE_CRASH": "gpu_probe"}, 5, ["gpu_probe.json"], []),
        ({"FAKE_CRASH": "mfu"}, 6, ["mfu-dp8-flash.json"], []),
        ({"FAKE_CRASH": "miner_test"}, 7, ["mine-smoke-v2.json"], []),
        ({"FAKE_CRASH": "miner_prod"}, 8, ["miner-prod/round-3-commitments.json"], ["--prod"]),
    ],
    ids=["host_probe", "pull", "gpu_probe", "mfu", "miner_test", "miner_prod"],
)
def test_a_step_that_dies_early_leaves_no_evidence_of_an_earlier_run(
    env, tmp_path, crash, rc, files, extra
):
    out = tmp_path / "results/h1"
    (out / "miner-prod").mkdir(parents=True)
    for name in files:
        (out / name).write_text(STALE)
    prod = prod_env(tmp_path / "miner") if extra else {}
    p = run({**env, **crash, **prod}, tmp_path, *extra)
    assert p.returncode == rc, p.stdout + p.stderr
    stale = [n for n in files if (out / n).exists() and "stale" in (out / n).read_text()]
    assert stale == []


# --- the gates (B7): each one stops the run, and a mutant that opens it is caught -----------------


def test_a_failed_gpu_probe_stops_the_run_before_mfu(env, tmp_path):
    p = run({**env, "FAKE_GPU_OK": "0"}, tmp_path)
    assert p.returncode == 5, p.stdout + p.stderr
    stop = stop_json(tmp_path)
    assert stop["reason"] == "gpu-probe-failed" and stop["steps"]["gpu_probe"] != 0
    assert runs_of(tmp_path, "mfu") == [] and runs_of(tmp_path, "miner_test") == []


@pytest.mark.parametrize(
    "avail,passes",
    [("500,500", True), ("100,100", True), ("500,99", False), ("99,500", False)],
)
def test_the_results_and_the_docker_root_each_need_the_free_space(env, tmp_path, avail, passes):
    # Used is 900 GiB on both: reading that column, or the larger of the two, would pass.
    fake = {"HT_B300_MIN_DISK_GB": "100", "FAKE_DF_AVAIL": avail}
    p = run({**env, **fake}, tmp_path, "--steps", "host_probe")
    probe = json.loads((tmp_path / "results/h1/host_probe.json").read_text())
    assert probe["checks"]["disk"] is passes and probe["free_gb"] != {}
    assert p.returncode == (0 if passes else 3), p.stdout + p.stderr


def test_containers_stay_isolated_and_do_not_inherit_the_lock(env, tmp_path):
    prod = prod_env(tmp_path / "miner")
    p = run({**env, **prod}, tmp_path, "--prod")
    assert p.returncode == 0, p.stdout + p.stderr
    runs = [c for c in docker_calls(tmp_path) if c[0] == "run"]
    assert len(runs) == 4  # gpu_probe, mfu, miner_test and one production round
    for c in runs:
        assert {"--rm", "--init"} <= set(c)
        assert c[c.index("--label") + 1].startswith("io.cortex.b300-run=")
    (probe,) = runs_of(tmp_path, "gpu_probe")
    assert probe[probe.index("--network") + 1] == "none"
    for name in ("miner_test", "miner-prod-3"):  # the key file must belong to the invoking user
        (c,) = runs_of(tmp_path, name)
        assert c[c.index("--user") + 1] == f"{os.getuid()}:{os.getgid()}"
    assert not (tmp_path / "state/fd9-leaked").exists()


# --- a step that starts again forgets its old pass, and only its own evidence goes ---------------


def test_an_interrupted_forced_rerun_is_not_remembered_as_a_pass(env, tmp_path, checkout, launch):
    script = checkout / "scripts/b300/run_host.sh"
    assert run(env, tmp_path, script=script).returncode == 0
    proc = launch("--force", "--steps", "gpu_probe", script=script, FAKE_BLOCK="gpu_probe")
    wait_until(lambda: (tmp_path / "state/ctr-gpu_probe").exists(), "the forced gpu_probe")
    proc.send_signal(signal.SIGTERM)
    assert proc.wait(timeout=30) == 143
    assert not (tmp_path / "results/h1/steps/gpu_probe.rc").exists()
    assert "gpu_probe" not in stop_json(tmp_path)["steps"]  # it was cut, it did not pass
    assert run(env, tmp_path, script=script).returncode == 0  # a plain run must do it again
    assert len(runs_of(tmp_path, "gpu_probe")) == 3
    assert (tmp_path / "results/h1/gpu_probe.json").exists()


def test_a_clean_run_does_not_reuse_a_pass_recorded_on_a_modified_tree(env, tmp_path, checkout):
    script = checkout / "scripts/b300/run_host.sh"
    (checkout / "scripts/mfu_profiles.json").write_text("{}")
    assert run(env, tmp_path, script=script).returncode == 0
    git(checkout, "checkout", "--", "scripts/mfu_profiles.json")
    assert run(env, tmp_path, script=script).returncode == 0
    assert stop_json(tmp_path)["git_dirty"] is False
    assert len(runs_of(tmp_path, "gpu_probe")) == 2  # same commit, but not the code that passed


def test_a_pass_recorded_by_the_previous_script_is_not_trusted(env, tmp_path, checkout):
    (tmp_path / "results/h1/steps").mkdir(parents=True)
    (tmp_path / "results/h1/steps/gpu_probe.rc").write_text(f"0 {IMAGE}")  # "<rc> <image>" only
    assert run(env, tmp_path, script=checkout / "scripts/b300/run_host.sh").returncode == 0
    assert len(runs_of(tmp_path, "gpu_probe")) == 1


def test_new_rounds_keep_the_evidence_of_the_rounds_before(env, tmp_path):
    first = {**env, **prod_env(tmp_path / "miner", rounds="3 4")}
    assert run(first, tmp_path, "--prod").returncode == 0
    later = {**env, **prod_env(tmp_path / "miner", rounds="5")}
    p = run(later, tmp_path, "--prod", "--steps", "miner_prod", "--force")
    assert p.returncode == 0, p.stdout + p.stderr
    kept = sorted(f.name for f in (tmp_path / "results/h1/miner-prod").iterdir())
    assert kept == [f"round-{w}{s}.json" for w in "345" for s in ("-commitments", "")]


@pytest.mark.parametrize("gpus", ["8 ../victim", "0", "99x", "100", "08"])
def test_the_expected_gpu_count_is_a_plain_number(env, tmp_path, gpus):
    p = run({**env, "HT_B300_EXPECT_GPUS": gpus}, tmp_path)
    assert p.returncode == 2 and "HT_B300_EXPECT_GPUS" in p.stderr, p.stdout + p.stderr
    assert docker_calls(tmp_path) == []
