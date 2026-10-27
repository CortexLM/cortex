"""scripts/b300/run_host.sh against fake docker / nvidia-smi / aws and a fake IMDSv2 server.

The fakes stand at the script's real boundaries: docker and nvidia-smi on PATH (argv recorded),
EC2 instance metadata over HTTP (session PUT, then spot/instance-action). No GPU, no network.
"""

from __future__ import annotations

import contextlib
import json
import os
import re
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
    n, drv, cc = (os.environ.get(k, d) for k, d in
                  (("FAKE_GPUS", "8"), ("FAKE_DRIVER", "595.91.07"), ("FAKE_CC", "10.3")))
    for i in range(int(n)):
        print(f"{i}, NVIDIA B300 SXM6 AC, {drv}, {cc}, 275040 MiB, 00000000:{i:02X}:00.0")
else:
    print("fake nvidia-smi table")
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

FAKE_AWS = r"""
import json, os, pathlib, sys
with (pathlib.Path(os.environ["FAKE_STATE"]) / "aws.jsonl").open("a") as f:
    f.write(json.dumps(sys.argv[1:]) + "\n")
"""


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
    fakes = {"docker": FAKE_DOCKER, "nvidia-smi": FAKE_SMI, "aws": FAKE_AWS, "sudo": FAKE_SUDO}
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
    }


def run(env: dict[str, str], tmp_path: Path, *args: str) -> subprocess.CompletedProcess[str]:
    argv = ["bash", str(SCRIPT), "--results-root", str(tmp_path / "results"), "--host", "h1"]
    return subprocess.run(
        [*argv, *args], env=env, capture_output=True, text=True, timeout=60, check=False
    )


def docker_calls(tmp_path: Path) -> list[list[str]]:
    trace = tmp_path / "state/docker.jsonl"
    return [json.loads(x) for x in trace.read_text().splitlines()] if trace.exists() else []


def runs_of(tmp_path: Path, name: str) -> list[list[str]]:
    return [c for c in docker_calls(tmp_path) if c[0] == "run" and f"ht-b300-{name}" in c]


def stop_json(tmp_path: Path) -> dict[str, Any]:
    return json.loads((tmp_path / "results/h1/stop.json").read_text())


def wait_until(cond: Callable[[], object], what: str, timeout: float = 30) -> None:
    end = time.monotonic() + timeout
    while not cond():
        assert time.monotonic() < end, f"timed out waiting for {what}"
        time.sleep(0.05)


@pytest.fixture
def launch(env: dict[str, str], tmp_path: Path) -> Iterator[Callable[..., subprocess.Popen[bytes]]]:
    """Start run_host.sh in its own process group; teardown frees fake containers and kills it."""
    procs: list[subprocess.Popen[bytes]] = []

    def start(*args: str, **extra_env: str) -> subprocess.Popen[bytes]:
        argv = ["bash", str(SCRIPT), "--results-root", str(tmp_path / "results"), "--host", "h1"]
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


def test_rerun_skips_passed_steps_but_reprobes_the_host(env, tmp_path):
    assert run(env, tmp_path).returncode == 0
    first = len(docker_calls(tmp_path))
    assert run(env, tmp_path).returncode == 0
    again = docker_calls(tmp_path)[first:]
    assert [c for c in again if c[0] in ("pull", "run")] == []
    assert ["info", "--format", "{{.DockerRootDir}}"] in again  # host probe ran again
    assert run(env, tmp_path, "--force", "--steps", "mfu").returncode == 0
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
    aws = [json.loads(x) for x in (tmp_path / "state/aws.jsonl").read_text().splitlines()]
    assert aws == [
        [
            "s3",
            "cp",
            "--recursive",
            "--only-show-errors",
            str(tmp_path / "results/h1"),
            "s3://bucket/b300/h1/",
        ]
    ]


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
    log = (tmp_path / "results/h1/run.log").read_text()
    match = re.search(r"watchdog pid (\d+)", log)
    assert match, log
    os.kill(int(match.group(1)), signal.SIGKILL)
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
