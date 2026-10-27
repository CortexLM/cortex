"""Fake SSH, docker and nvidia-smi host for the generic CUDA executor tests.

No network, no GPU, no real ssh or docker. `Toolbox` generates, under a temporary directory:

- `bin/ssh`: the executable the executor's real `SshTransport` spawns (it must be first on PATH).
  It parses the argv the transport builds, logs one start and one end event per call to
  `ssh.log` (argv, destination, the NAMES of its environment variables, a hash and the head of the
  command, never a key), then runs the command with `sh -c` against `host-bin/`. Child exit code
  199 is a vanished host: ssh exits 255 and the bytes already written stay.
- `host-bin/{docker,sudo,nvidia-smi,date}`: what a GPU host offers. `docker run` executes the exact
  runner source the challenge shipped (`python -I -c SOURCE`) in a namespace of its own and replaces
  only the device probe and the island launch: the runner's tar reading, request checks,
  environment handling and output framing run for real.

The scenario is one JSON file keyed by host address (`hosts`), plus a `global` section:

    hosts[address] = {down, hostkey_ok, port, user, identity, home, clock_skew,
                      gpus[{uuid,name,driver,cc,mem,sm}], busy[uuid], busy_sequence[[uuid]],
                      smi_fail, images[ref], inspect_reports[ref], pull{fails|digest}, runs[run],
                      cold_start, warm_start}
    global = {canned{job_sha256: tree}, runs[run], rm_hold}  # global runs are consumed first
    run = {mode: ok | vanish | truncate | exit | exit-after | probe-fail | env | hang | oversize |
                 flood | mutate, detached, die_after_puts, ...}

`smi_fail` is `query-gpu` or `query-compute-apps`: that nvidia-smi query prints nothing and exits 3.
`home` is the ssh user's home (`$HOME` of the remote shell). A `--mount type=bind` must name an
existing directory, as with the real daemon. A run that mounts `/root/.nv` is `cold` until a run
has left a marker in that directory, then `warm`; a run without the mount is `none` (the cache dies
with the container, so it costs what a cold one does). It sleeps `cold_start` or `warm_start`
seconds (default 0) and leaves the marker when it succeeds, so a persistent cache can be timed.
`port`, `user` and `identity` are what the host's sshd accepts: any other `-p`, user or `-i` file is
refused, and the destination must be pinned in the `UserKnownHostsFile` the caller names, as
`address` for port 22 and `[address]:port` otherwise (StrictHostKeyChecking=yes).
`exit-after`: the container streams a complete valid result, then exits with `code`.
`probe-fail`: the runner's device probe fails (the real runner then exits 9).
`die_after_puts`: with the result going to R2 (docs/adr/0013), the host vanishes (as `vanish`)
when the runner starts its n+1th part upload.
`detached`: the container runs in a session of its own, so killing the local ssh does not stop it;
only `docker rm -f NAME` does, and it waits while the global `rm_hold` flag is set.

Every access takes one flock, so a test may change the scenario between (or during) launches.
"""

from __future__ import annotations

import fcntl
import json
import os
import shutil
import sys
import time
from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

REPOSITORY = "ghcr.io/cortexlm/hypertrain-gpu"
CANARY = "canary-ssh-key-3f9a1c7e5b2d4086"
PEM = "-" * 5  # the dashes of a PEM line, kept apart from BEGIN so the secret scan reads a fixture
DRIVER = "595.71.05"
OTHER_DRIVER = "595.71.06"
# Device classes of docs/adr/0007: (capability) -> SM count, written down here on purpose so the
# fake host does not read the table the executor is checked against.
SM_COUNTS = {"10.3": 148, "12.0": 170}

COMMON = r"""
import fcntl
import hashlib
import json
import os
import sys
import time


def sha(data):
    return hashlib.sha256(data).hexdigest()


def append(path, entry):
    line = (json.dumps(entry, sort_keys=True) + "\n").encode()
    fd = os.open(path, os.O_WRONLY | os.O_APPEND | os.O_CREAT, 0o600)
    try:
        os.write(fd, line)
    finally:
        os.close(fd)


def scenario(change=None):
    with open(SCENARIO + ".lock", "a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        with open(SCENARIO) as stream:
            data = json.load(stream)
        if change is None:
            return data
        result = change(data)
        temporary = "%s.%d" % (SCENARIO, os.getpid())
        with open(temporary, "w") as stream:
            json.dump(data, stream)
        os.replace(temporary, SCENARIO)
        return result
"""

SSH = r"""
import subprocess


def finish(code):
    append(SSH_LOG, {"event": "end", "pid": os.getpid(), "t": time.monotonic(), "exit": code})
    sys.exit(code)


def fail(text):
    sys.stderr.write(text + "\n")
    finish(255)


def pinned_names(path):
    names = set()
    with open(path) as stream:
        for line in stream:
            fields = line.split()
            if fields and not fields[0].startswith("#"):
                names.update(fields[0].split(","))
    return names


def kind(command):
    words = command.split()
    if words[:1] == ["nvidia-smi"]:
        return "probe"
    if command.startswith("D="):  # the compute cache directory command
        return "prepare"
    return next((w for w in words if w in ("run", "pull", "rm")), "other")


def main():
    names = sorted(os.environ)
    argv = sys.argv[1:]
    options, flags, i = {}, {}, 0
    while i < len(argv) and argv[i] != "--":
        if argv[i] in ("-F", "-i", "-p", "-o") and i + 1 < len(argv):
            if argv[i] == "-o":
                key, _, value = argv[i + 1].partition("=")
                options[key] = value
            else:
                flags[argv[i]] = argv[i + 1]
            i += 2
        else:
            i += 1
    rest = argv[i + 1 :]
    if len(rest) < 2:
        fail("usage: ssh [options] -- destination command")
    destination, command = rest[0], " ".join(rest[1:])
    user, _, address = destination.rpartition("@")
    append(
        SSH_LOG,
        {
            "event": "start",
            "pid": os.getpid(),
            "t": time.monotonic(),
            "argv": argv,
            "destination": destination,
            "user": user,
            "host": address,
            "env_names": names,
            "kind": kind(command),
            "command_sha256": sha(command.encode()),
            "command_head": command[:120],
        },
    )
    host = scenario().get("hosts", {}).get(address)
    port = flags.get("-p", "22")
    if host is None or host.get("down") or str(host.get("port", 22)) != port:
        fail("ssh: connect to host %s port %s: Connection refused" % (address, port))
    if not host.get("hostkey_ok", True):
        fail("Host key verification failed.")
    pinned = options.get("UserKnownHostsFile")
    for label, path in (("identity", flags.get("-i")), ("known_hosts", pinned)):
        if not path or not os.path.isfile(path):
            fail("ssh: %s file is not accessible" % label)
    if os.stat(flags["-i"]).st_mode & 0o077:
        fail("Permissions are too open for the identity file")
    if (address if port == "22" else "[%s]:%s" % (address, port)) not in pinned_names(pinned):
        fail("Host key verification failed.")
    if host.get("user", user) != user or host.get("identity", flags["-i"]) != flags["-i"]:
        fail("%s@%s: Permission denied (publickey)." % (user, address))
    env = {
        "PATH": HOST_BIN + os.pathsep + os.environ.get("PATH", "/usr/bin:/bin"),
        "HOME": host.get("home") or os.environ.get("HOME", ""),
        "LC_ALL": "C",
        "TMPDIR": HOST_TMP,
        "FAKE_HOST_ADDRESS": address,
    }
    code = subprocess.run(["sh", "-c", command], env=env, check=False).returncode
    finish(255 if code == 199 else 128 - code if code < 0 else code)


main()
"""

DOCKER = r"""
import csv
import io
import shutil
import signal
import tarfile
import threading
from pathlib import Path

ADDRESS = os.environ["FAKE_HOST_ADDRESS"]
VIA_SUDO = os.environ.get("FAKE_VIA_SUDO") == "1"
VALUE_FLAGS = {
    "--name", "--label", "--gpus", "--network", "--ulimit", "--entrypoint", "-e", "--env",
    "-v", "--volume", "--mount", "-w", "--workdir", "-u", "--user",
}


def log(event, **fields):
    entry = dict(fields, event=event, host=ADDRESS, via_sudo=VIA_SUDO, pid=os.getpid())
    append(DOCKER_LOG, dict(entry, t=time.monotonic()))


def host_of(data):
    return data["hosts"][ADDRESS]


def started():
    with open(DOCKER_LOG) as stream:
        return sum('"event": "run_start"' in line for line in stream)


def usage(text, code=125):
    sys.stderr.write("docker: %s\n" % text)
    sys.exit(code)


def inspect(argv):
    fmt = argv[argv.index("--format") + 1] if "--format" in argv else None
    ref = argv[-1]
    log("image_inspect", ref=ref, format=fmt)
    if fmt != "{{json .RepoDigests}}":
        usage("unsupported inspect format %r" % fmt, 2)
    host = host_of(scenario())
    if ref not in host.get("images", []):
        usage("Error response from daemon: No such image: %s" % ref, 1)
    print(json.dumps(host.get("inspect_reports", host["images"])))


def pull(argv):
    ref = argv[-1]
    log("pull", ref=ref, args=argv)

    def change(data):
        host = host_of(data)
        spec = host.get("pull", {})
        if spec.get("fails"):
            return False
        added = ref.split("@")[0] + "@" + spec["digest"] if spec.get("digest") else ref
        host.setdefault("images", []).append(added)
        return True

    if not scenario(change):
        usage("Error response from daemon: pull access denied", 1)


def container_pid_file(name):
    return Path(HOST_TMP) / ("container-" + name + ".pid")


def remove(argv):
    name = argv[-1]
    log("rm_start", name=name, args=argv)
    give_up = time.monotonic() + 30
    while scenario().get("global", {}).get("rm_hold") and time.monotonic() < give_up:
        time.sleep(0.02)
    pid_file = container_pid_file(name)
    if pid_file.exists():  # a detached container: removed by name, as the daemon would
        try:
            os.killpg(int(pid_file.read_text()), signal.SIGKILL)
        except ProcessLookupError:
            pass
    log("rm", name=name, args=argv)


def detach(name):
    # Split into the docker client (this process, which dies with its ssh session) and the
    # container: a process of its own session that outlives the client until `docker rm -f`.
    child = os.fork()
    if child:
        status = os.waitstatus_to_exitcode(os.waitpid(child, 0)[1])
        sys.exit(128 - status if status < 0 else status)
    os.setsid()
    container_pid_file(name).write_text(str(os.getpid()))
    log("container_start", name=name)


def decode(raw):
    members = []
    with tarfile.open(fileobj=io.BytesIO(raw), mode="r:") as archive:
        for info in archive:
            members.append((info.name, archive.extractfile(info).read(), "file"))
    return members


def encode(members):
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w", format=tarfile.PAX_FORMAT) as archive:
        for name, data, kind in members:
            info = tarfile.TarInfo(name)
            info.mode = 0o600
            if kind == "symlink":
                info.type, info.linkname = tarfile.SYMTYPE, data.decode()
                archive.addfile(info)
            else:
                info.size = len(data)
                archive.addfile(info, io.BytesIO(data))
    return buffer.getvalue()


def edit(body, suffix, change):
    return [
        (n, change(d) if n.endswith(suffix) else d, k) for n, d, k in body
    ]


def dumps(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode()


def mutated(genuine, entry):
    members = decode(genuine)
    body = [m for m in members if m[0] != "COMPLETE"]
    done = json.loads(dict((n, d) for n, d, k in members)["COMPLETE"])
    name, recompute, trailing = entry["name"], False, []
    if name == "truncate":
        return encode(body)
    if name == "symlink-member":
        body.append(("rank-0/link", b"/etc/passwd", "symlink"))
    elif name == "traversal-name":
        body.append(("rank-0/../../escape.txt", b"x", "file"))
    elif name == "foreign-name":
        body.append(("unlisted.txt", b"x", "file"))
    elif name == "duplicate-member":
        body.append(next(m for m in body if m[0].startswith("rank-")))
    elif name == "extra-rank-file":
        body.append(("rank-0/extra.bin", b"extra", "file"))
    elif name == "flipped-byte":
        body = edit(body, "rank-0/delta.bin", lambda d: bytes([d[0] ^ 1]) + d[1:])
    elif name == "wrong-request-hash":
        done["request_sha256"] = "00" * 32
    elif name == "member-after-complete":
        trailing.append(("rank-0/late.bin", b"late", "file"))
    elif name == "cpu-backend":
        body = edit(body, "summary.json", lambda d: dumps(dict(json.loads(d), backend="cpu")))
        recompute = True
    elif name == "no-trace":
        body = [m for m in body if not m[0].endswith("trace.json")]
        recompute = True
    elif name == "corrupt-state":
        body = edit(body, "rank-0/state.safetensors", lambda d: b"not a safetensors file")
        recompute = True
    elif name == "trace-not-a-list":
        body = edit(body, "trace.json", lambda d: dumps({"events": []}))
        recompute = True
    elif name == "trace-garbage":
        body = edit(body, "trace.json", lambda d: b"\xff\xfe not json")
        recompute = True
    elif name == "corrupt-leaves":
        body = edit(body, "rank-0/leaves.json", lambda d: dumps(json.loads(d)[::-1]))
        recompute = True
    elif name == "wrong-job-hash":
        body = edit(body, "summary.json", lambda d: dumps(dict(json.loads(d), job_hash="00" * 32)))
        recompute = True
    elif name == "environment":
        body = edit(body, "environment.json", lambda d: dumps(dict(json.loads(d), **entry["set"])))
        recompute = True
    else:
        usage("unknown mutation %r" % name)
    if recompute:
        done["files"] = {n: sha(d) for n, d, k in body}
    body.append(("COMPLETE", dumps(done), "file"))
    return encode(body + trailing)


def run(argv):
    stdin = sys.stdin.buffer.read()
    options, i = [], 0
    while i < len(argv) and argv[i].startswith("-"):
        if argv[i] in VALUE_FLAGS and i + 1 < len(argv):
            options.append([argv[i], argv[i + 1]])
            i += 2
        else:
            options.append([argv[i]])
            i += 1
    if i >= len(argv):
        usage("docker run: image missing")
    image, tail = argv[i], argv[i + 1 :]
    if tail[:2] != ["-I", "-c"] or len(tail) != 3:
        usage("docker run: expected the runner as -I -c SOURCE")
    source = tail[2]
    given = {flag[0]: flag[1] for flag in options if len(flag) == 2}
    gpus = given.get("--gpus", "")
    fields = next(csv.reader([gpus]), [])
    if not fields or any("=" not in f for f in fields):
        usage("invalid --gpus value %s" % gpus)
    devices = [d for f in fields if f.startswith("device=") for d in f[7:].split(",")]
    name = given.get("--name", "")
    mounts = []
    for flag in options:
        if flag[0] in ("-v", "--volume"):
            usage("this fake host supports --mount only, got %r" % flag)
        if flag[0] == "--mount" and len(flag) == 2:
            mount = dict(part.partition("=")[::2] for part in flag[1].split(","))
            if mount.get("type") != "bind" or not mount.get("src") or not mount.get("dst"):
                usage("invalid mount config %r" % flag[1])
            if not os.path.isdir(mount["src"]):
                usage(
                    'invalid mount config for type "bind": bind source path does not exist: %s'
                    % mount["src"]
                )
            mounts.append(mount)
    cache_dir = next((m["src"] for m in mounts if m["dst"] == "/root/.nv"), None)
    marker = Path(cache_dir) / "ComputeCache" / "index" if cache_dir else None
    temperature = "none" if marker is None else "warm" if marker.exists() else "cold"
    log(
        "run_start",
        name=name,
        gpus_raw=gpus,
        devices=devices,
        network=given.get("--network", "default"),
        options=options,
        mounts=mounts,
        cache=temperature,
        image=image,
        tail_flags=tail[:2],
        source_sha256=sha(source.encode()),
        stdin_bytes=len(stdin),
    )

    def take(data):
        queue = data.get("global", {}).get("runs", [])
        if not queue:
            queue = host_of(data).get("runs", [])
        return queue.pop(0) if queue else {"mode": "ok"}

    data = scenario()
    entry = scenario(take)
    mode = entry.get("mode", "ok")
    host = host_of(data)
    known = {g["uuid"]: g for g in host["gpus"]}
    if not devices or any(d not in known for d in devices):
        usage("Error response from daemon: cannot select the devices %s" % devices)
    if entry.get("detached"):
        detach(name)
    if mode == "exit":
        sys.stderr.write(entry.get("stderr", "") + "\n")
        log("run_end", name=name, code=entry["code"])
        sys.exit(entry["code"])
    if mode == "flood":  # a container that spams stderr: one write, then it stays or leaves
        sys.stderr.write("x" * entry["bytes"])
        sys.stderr.flush()
        time.sleep(entry.get("linger", 0))
        log("run_end", name=name, code=0)
        sys.exit(0)
    canned = data["global"]["canned"]
    namespace = {"__name__": "cuda_executor_runner_under_test"}
    exec(compile(source, "<runner>", "exec"), namespace)
    if entry.get("die_after_puts") is not None:
        real, counted = namespace["http_send"], threading.Lock()
        puts = [0]

        def dying(method, url, headers, body, timeout, limit):
            if method == "PUT" and "partNumber=" in url:
                with counted:
                    puts[0] += 1
                    gone = puts[0] > entry["die_after_puts"]
                if gone:
                    log("run_end", name=name, code=199)
                    os._exit(199)
            return real(method, url, headers, body, timeout, limit)

        namespace["http_send"] = dying

    def probe(request):
        if mode == "probe-fail":
            raise namespace["Refused"](9, "environment probe failed: NVML mismatch")
        chosen = [known[u] for u in request["uuids"]]
        observed = {
            "backend": "cuda",
            "python": "/opt/hypertrain/venv/bin/python",
            "torch": "2.14.0+cu130",
            "torch_path": "/opt/hypertrain/venv/lib/python3.12/site-packages/torch/__init__.py",
            "cuda": "13.0",
            "gpu_uuids": list(request["uuids"]),
            "gpu_names": [g["name"] for g in chosen],
            "sm_counts": [g["sm"] for g in chosen],
            "capabilities": [[int(x) for x in g["cc"].split(".")] for g in chosen],
            "drivers": [g["driver"] for g in chosen],
        }
        observed.update(entry.get("env", {}))
        return observed

    def launch(request, attempt, budget, log_path):
        received = {
            str(p.relative_to(attempt)): sha(p.read_bytes())
            for p in sorted(attempt.rglob("*"))
            if p.is_file()
        }
        tracing = os.environ.get("HT_ISLAND_TRACE") == "1"
        log(
            "launch",
            name=name,
            inputs=received,
            env={k: os.environ.get(k) for k in sorted(namespace["ENV_KEYS"])},
            budget=int(budget),
        )
        give_up = time.monotonic() + 15
        while started() < entry.get("rendezvous", 0) and time.monotonic() < give_up:
            time.sleep(0.05)  # holds this run until that many runs are in flight at once
        time.sleep(entry.get("delay", 0))
        # a container without the mount keeps its cache in its own layer, which --rm throws away
        time.sleep(host.get("warm_start" if temperature == "warm" else "cold_start", 0))
        if mode == "hang":
            time.sleep(60)
        for source_rank in sorted(Path(canned[request["job_sha256"]]).glob("rank-*")):
            shutil.copytree(source_rank, attempt / source_rank.name)
        if not tracing:  # the island writes trace.json only when asked to
            for trace in attempt.glob("rank-*/trace.json"):
                trace.unlink()
        if marker is not None:  # what the driver leaves in /root/.nv after compiling kernels
            marker.parent.mkdir(parents=True, exist_ok=True)
            marker.write_bytes(b"compiled")
        return 0

    captured = io.BytesIO()
    code = namespace["serve"](io.BytesIO(stdin), captured, sys.stderr, probe=probe, launch=launch)
    genuine = captured.getvalue()
    out = sys.stdout.buffer
    if code:
        out.write(genuine)
        out.flush()
        log("run_end", name=name, code=code)
        sys.exit(code)
    if mode == "vanish":
        partial = genuine[: max(1, len(genuine) // 2)]
        out.write(partial)
        out.flush()
        if entry.get("host_down"):
            scenario(lambda d: host_of(d).update(down=True))
        log("run_end", name=name, code=199, stdout_sha256=sha(partial))
        os._exit(199)
    if mode == "exit-after":  # a complete, valid result on the stream; the container still fails
        out.write(genuine)
        out.flush()
        log("run_end", name=name, code=entry["code"], stdout_sha256=sha(genuine))
        sys.exit(entry["code"])
    if mode == "truncate":
        genuine = mutated(genuine, {"name": "truncate"})
    elif mode == "mutate":
        genuine = mutated(genuine, entry)
    out.write(genuine)
    out.flush()
    if mode == "oversize":
        for _ in range(entry["bytes"] >> 16):
            out.write(b"\0" * (1 << 16))
            out.flush()
        time.sleep(entry.get("linger", 20))
    log("run_end", name=name, code=0, stdout_bytes=len(genuine), stdout_sha256=sha(genuine))


def main():
    argv = sys.argv[1:]
    if argv[:2] == ["image", "inspect"]:
        inspect(argv[2:])
    elif argv[:1] == ["pull"]:
        pull(argv[1:])
    elif argv[:2] == ["rm", "-f"]:
        remove(argv[2:])
    elif argv[:1] == ["run"]:
        run(argv[1:])
    else:
        usage("unsupported command %r" % argv, 2)


main()
"""

NVIDIA_SMI = r"""
def main():
    args = sys.argv[1:]
    host = scenario()["hosts"][os.environ["FAKE_HOST_ADDRESS"]]
    query = next((a for a in args if a.startswith("--query-")), "")
    if host.get("smi_fail") and query.startswith("--" + host["smi_fail"] + "="):
        sys.stderr.write("nvidia-smi: Unable to determine the device handle\n")
        sys.exit(3)
    if query.startswith("--query-gpu=") and "--format=csv,noheader,nounits" in args:
        assert query == "--query-gpu=uuid,name,driver_version,compute_cap,memory.used"
        for g in host["gpus"]:
            print("%s, %s, %s, %s, %s" % (g["uuid"], g["name"], g["driver"], g["cc"], g["mem"]))
    elif query.startswith("--query-compute-apps=") and "--format=csv,noheader" in args:
        assert query == "--query-compute-apps=gpu_uuid,pid,process_name"

        def take(data):
            sequence = data["hosts"][os.environ["FAKE_HOST_ADDRESS"]].get("busy_sequence")
            return sequence.pop(0) if sequence else None

        busy = scenario(take)
        for uuid in host.get("busy", []) if busy is None else busy:
            print("%s, 4242, python3" % uuid)
    else:
        sys.stderr.write("unsupported nvidia-smi arguments\n")
        sys.exit(2)


main()
"""

DATE = r"""
if sys.argv[1:] != ["+%s"]:
    sys.exit(2)
skew = scenario()["hosts"][os.environ["FAKE_HOST_ADDRESS"]].get("clock_skew", 0)
print(int(time.time()) + skew)
"""

SUDO = """#!/bin/sh
# Only `sudo -n`: a password prompt would hang a BatchMode session.
if [ "$1" != "-n" ]; then
    echo "sudo: a password is required" >&2
    exit 1
fi
shift
FAKE_VIA_SUDO=1
export FAKE_VIA_SUDO
exec "$@"
"""


class Scenario:
    """The scenario file, shared with the fake processes through one flock."""

    def __init__(self, path: Path) -> None:
        self.path = path
        self._lock = Path(str(path) + ".lock")

    def _transact(self, change: Callable[[dict[str, Any]], Any]) -> None:
        with open(self._lock, "a") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            data = json.loads(self.path.read_text())
            change(data)
            temporary = Path(f"{self.path}.{os.getpid()}")
            temporary.write_text(json.dumps(data))
            os.replace(temporary, self.path)

    def update(self, address: str, **fields: Any) -> None:
        self._transact(lambda data: data["hosts"][address].update(fields))

    def install(self, hosts: dict[str, Any], canned: dict[str, str]) -> None:
        self._transact(
            lambda data: data.update(hosts=hosts, **{"global": {"canned": canned, "runs": []}})
        )

    def global_runs(self, *runs: dict[str, Any]) -> None:
        self._transact(lambda data: data["global"].update(runs=list(runs)))

    def set_global(self, **fields: Any) -> None:
        self._transact(lambda data: data["global"].update(fields))


def _events(path: Path) -> list[dict[str, Any]]:
    if not path.exists():
        return []
    return [json.loads(line) for line in path.read_text().splitlines() if line]


@dataclass
class Toolbox:
    root: Path
    bin_dir: Path  # holds only `ssh`; this is what goes on the challenge's PATH
    host_bin: Path  # what the "remote" shell sees: docker, sudo, nvidia-smi, date
    scenario: Scenario
    ssh_log_path: Path
    docker_log_path: Path

    def activate(self, monkeypatch: Any) -> None:
        monkeypatch.setenv("PATH", f"{self.bin_dir}{os.pathsep}{os.environ.get('PATH', '')}")

    def ssh_log(self) -> list[dict[str, Any]]:
        """One record per ssh call, in start order; `exit` is None when the process was killed."""
        calls: dict[int, dict[str, Any]] = {}
        for event in _events(self.ssh_log_path):
            if event["event"] == "start":
                calls[event["pid"]] = {**event, "exit": None}
            else:
                calls[event["pid"]]["exit"] = event["exit"]
        return list(calls.values())

    def ssh_calls(self, kind: str | None = None, host: str | None = None) -> list[dict[str, Any]]:
        return [
            c
            for c in self.ssh_log()
            if (kind is None or c["kind"] == kind) and (host is None or c["host"] == host)
        ]

    def docker_log(self) -> list[dict[str, Any]]:
        return _events(self.docker_log_path)

    def events(self, event: str, host: str | None = None) -> list[dict[str, Any]]:
        return [e for e in self.docker_log() if e["event"] == event and host in (None, e["host"])]

    def runs(self, host: str | None = None) -> list[dict[str, Any]]:
        """Each `docker run` with its start, its end (absent when killed) and its stdout hash."""
        runs: dict[str, dict[str, Any]] = {}
        for event in self.events("run_start", host):
            runs[event["name"]] = {**event, "start": event["t"], "end": None}
        for event in self.events("run_end", host):
            runs[event["name"]].update(
                end=event["t"], code=event["code"], stdout_sha256=event.get("stdout_sha256")
            )
        return list(runs.values())


def make_toolbox(root: Path) -> Toolbox:
    root.mkdir(parents=True)
    bin_dir, host_bin = root / "bin", root / "host-bin"
    host_tmp = root / "host-tmp"
    for directory in (bin_dir, host_bin, host_tmp):
        directory.mkdir()
    scenario = Scenario(root / "scenario.json")
    scenario.path.write_text(json.dumps({"hosts": {}, "global": {"canned": {}, "runs": []}}))
    toolbox = Toolbox(root, bin_dir, host_bin, scenario, root / "ssh.log", root / "docker.log")
    constants = {
        "SCENARIO": str(scenario.path),
        "SSH_LOG": str(toolbox.ssh_log_path),
        "DOCKER_LOG": str(toolbox.docker_log_path),
        "HOST_BIN": str(host_bin),
        "HOST_TMP": str(host_tmp),
    }
    header = f"#!{sys.executable}\n" + "".join(f"{k} = {v!r}\n" for k, v in constants.items())
    for directory, name, body in (
        (bin_dir, "ssh", SSH),
        (host_bin, "docker", DOCKER),
        (host_bin, "nvidia-smi", NVIDIA_SMI),
        (host_bin, "date", DATE),
    ):
        (directory / name).write_text(header + COMMON + body)
        (directory / name).chmod(0o755)
    (host_bin / "sudo").write_text(SUDO)
    (host_bin / "sudo").chmod(0o755)
    return toolbox


def gpu_uuid(host: int, index: int) -> str:
    return f"GPU-{host:02x}{index:06x}-0000-0000-0000-000000000000"


def gpu(
    host: int,
    index: int,
    *,
    cc: str = "10.3",
    driver: str = DRIVER,
    mem: int = 810,
    sm: int | None = None,
) -> dict[str, Any]:
    return {
        "uuid": gpu_uuid(host, index),
        "name": "NVIDIA B300 SXM6 AC",
        "driver": driver,
        "cc": cc,
        "mem": mem,
        "sm": SM_COUNTS[cc] if sm is None else sm,
    }


def dispatch_manifest(manifest: Any, *, n_gpus: int, sm_count: int, drivers: list[str]) -> Any:
    """The fixture manifest with a CUDA reference: the layout and device class the host reports."""
    ref = manifest.training.reference_spec
    ref = ref.model_copy(
        update={
            "layout": ref.layout.model_copy(update={"n_gpus": n_gpus, "dp_size": n_gpus}),
            "driver_allowlist": drivers,
            "sm_count": sm_count,
        }
    )
    return manifest.model_copy(
        update={"training": manifest.training.model_copy(update={"reference_spec": ref})}
    )


@dataclass(frozen=True)
class Canned:
    """A job, its staged inputs and the CUDA-labelled rank trees a fake island 'trains'."""

    manifest: Any
    job: Any
    inputs: dict[str, bytes]
    tree: Path

    @property
    def image(self) -> str:
        return f"{REPOSITORY}@{self.manifest.training.reference_spec.image_digest}"

    def stage(self, directory: Path) -> Path:
        directory.mkdir(parents=True, exist_ok=True)
        for relative, raw in self.inputs.items():
            (directory / relative).write_bytes(raw)
        return directory


def canned_publication(
    manifest: Any, hotkey: str, saved_geometry: Callable[..., Any], root: Path, *, deadline: int
) -> Canned:
    """Re-label `saved_geometry`'s metadata-valid publication as CUDA for a live deadline.

    The job digest covers the deadline, so every rank summary is rebound to the new digest; the
    result is checked with `validate_artifacts` before it is handed out.
    """
    from hypertrain.miner.island_launch import validate_artifacts
    from hypertrain.protocol.hashing import sha256_hex
    from hypertrain.protocol.jcs import canonicalize

    seed = root / "seed"
    job = saved_geometry(manifest, hotkey, seed, published=True).model_copy(
        update={"deadline": deadline}
    )
    digest = job.digest()
    tree = root / "tree"
    tree.mkdir()
    for source in sorted((seed / "published").glob("rank-*")):
        shutil.copytree(source, tree / source.name)
        path = tree / source.name / "summary.json"
        summary = json.loads(path.read_bytes())
        rank = summary["work"]["rank"]
        summary["backend"] = "cuda"
        summary["job_hash"] = digest
        summary["work"]["fingerprint"] = sha256_hex(canonicalize([digest, rank]))
        path.write_bytes(canonicalize(summary))
    inputs = {rel: (seed / rel).read_bytes() for rel in sorted(set(job.object_paths.values()))}
    check = root / "check"
    shutil.copytree(tree, check)
    for relative, raw in inputs.items():
        (check / relative).write_bytes(raw)
    validate_artifacts(job, check)
    shutil.rmtree(check)
    return Canned(manifest, job, inputs, tree)


@dataclass
class Pool:
    toolbox: Toolbox
    config_path: Path
    config: dict[str, Any]
    identity: Path  # the first host's key
    addresses: dict[str, str]
    known_hosts: dict[str, Path]
    canned: Canned
    specs: dict[str, dict[str, Any]] = field(default_factory=dict)  # label -> the config's host

    @property
    def state_dir(self) -> Path:
        return Path(self.config["state_dir"])

    @property
    def image(self) -> str:
        return self.canned.image

    def address(self, label: str) -> str:
        return self.addresses[label]

    def home(self, label: str) -> Path:
        """The ssh user's home on that host: where `$HOME` points for its remote shell."""
        return self.toolbox.root / "home" / label


def make_pool(
    root: Path,
    monkeypatch: Any,
    canned: Canned,
    *,
    labels: tuple[str, ...] = ("a",),
    gpus: int = 2,
    same_machine: bool = False,
    docker: tuple[str, ...] = ("docker",),
    config: dict[str, Any] | None = None,
    hosts: dict[str, dict[str, Any]] | None = None,
) -> Pool:
    """Toolbox on PATH, one scenario host per label and a valid ExecutorConfig file for them.

    Every host has its own user, port (the second one listens on 2222), key file and pinned host
    key, and its sshd accepts only those: a call that mixes up two hosts' credentials fails.
    """
    toolbox = make_toolbox(root / "fake")
    toolbox.activate(monkeypatch)
    secrets = root / "secrets"
    secrets.mkdir(mode=0o700)
    ref = canned.manifest.training.reference_spec
    addresses, known_hosts, identities, scenario_hosts, specs = {}, {}, {}, {}, []
    for number, label in enumerate(labels):
        address = f"192.0.2.{10 + number}"
        port, user = (22, "ubuntu") if number == 0 else (2221 + number, f"ubuntu-{label}")
        addresses[label] = address
        identity = secrets / ("id_ed25519" if number == 0 else f"id_ed25519_{label}")
        identity.write_text(
            f"{PEM}BEGIN FAKE KEY-----\n{CANARY}\n{label}\n{PEM}END FAKE KEY-----\n"
        )
        identity.chmod(0o600)
        identities[label] = identity
        pinned = secrets / f"known_hosts_{label}"
        name = address if port == 22 else f"[{address}]:{port}"
        pinned.write_text(f"{name} ssh-ed25519 AAAAC3NzaC1lZDI1NTE5{label * 8}\n")
        known_hosts[label] = pinned
        machine = 0 if same_machine else number
        home = toolbox.root / "home" / label
        home.mkdir(parents=True)
        scenario_hosts[address] = {
            "port": port,
            "user": user,
            "identity": str(identity),
            "home": str(home),
            "gpus": [
                gpu(machine, i, cc="10.3", sm=ref.sm_count, driver=ref.driver_allowlist[0])
                for i in range(gpus)
            ],
            "images": [canned.image],
            **(hosts or {}).get(label, {}),
        }
        specs.append(
            {
                "label": label,
                "address": address,
                "port": port,
                "user": user,
                "identity_file": str(identity),
                "known_hosts_file": str(pinned),
                "docker": list(docker),
            }
        )
    toolbox.scenario.install(scenario_hosts, {canned.job.digest(): str(canned.tree)})
    settings = {
        "run_id": canned.job.run_id,
        "image_repository": REPOSITORY,
        "state_dir": str(root / "state"),
        "hosts": specs,
        "sweep_pause_seconds": 0,
        "max_clock_skew_seconds": 20,  # a loaded CI machine must not fail a probe on latency
        **(config or {}),
    }
    config_path = root / "executor.json"
    config_path.write_text(json.dumps(settings))
    return Pool(
        toolbox,
        config_path,
        settings,
        identities[labels[0]],
        addresses,
        known_hosts,
        canned,
        {spec["label"]: spec for spec in specs},
    )


def journal_records(state_dir: Path) -> list[dict[str, Any]]:
    return _events(state_dir / "journal.jsonl")


def wait_for(predicate: Callable[[], Any], timeout: float = 20.0) -> Any:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if value := predicate():
            return value
        time.sleep(0.05)
    raise TimeoutError("condition not met in time")


class SteppedClock:
    """Stands in for the `time` module of cuda_executor: real time plus a movable offset."""

    def __init__(self) -> None:
        self.offset = 0.0

    def time(self) -> float:
        return time.time() + self.offset

    def time_ns(self) -> int:
        return time.time_ns() + int(self.offset * 1e9)

    def __getattr__(self, name: str) -> Any:
        return getattr(time, name)
