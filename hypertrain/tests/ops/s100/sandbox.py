"""A sandbox that runs s100_test/challenge_image.sh against fake docker, curl and python3.

The script hard-codes /root/backups, /data/... and the production master's address and runs as root
on s100. The sandbox runs a COPY with only those path prefixes rewritten into a temp directory
(the rewrite asserts it covered every one), with these first on PATH:
- `docker`: a stateful fake (docker.json: containers, images, flags) of exactly the shapes the
  script uses; any other shape fails. Every mutating call (up, stop, start, restart) is recorded
  with the container name it hit: the tests compare that set with the one the command may touch.
  `up` recreates a container only when its resolved service changed or `--force-recreate` is given,
  as compose does. `exec` takes exactly the shapes of the challenge program (the TEST challenge
  only; `-i` only for the one mode that reads stdin) and records argv, stdin and the files that
  existed at that moment; its answers are canned, except that `post` really compares the variables
  of the fake container with its arguments. The program itself runs for real in test_r2_programs.
  `compose -f A -f B ...` merges the files as compose does for the one override file the script
  writes: `environment` (a list or a mapping) key by key, a mapping recursively, any other value
  replaced by the later file; a service is valid once merged. A recreated container gets the
  image environment overlaid with the merged `environment`. The flag `merge_patch` stands for a
  compose that resolves two files differently from each alone: it is merged over the result.
  A container is found by its name, where compose finds it by project and service labels.
- `curl`: a URL map (urls.json) for the master gate and the challenge API; unknown URLs fail.
- `python3`: the real interpreter, with `datetime.now` frozen for the seal-window code.
- `sleep` (no-op), `stat` (the real one, but the executor directory can be given another owner),
  `install` (without -o/-g, which need root; every call is logged in install.log), and tripwires
  for exactly these
  commands: ssh scp rsync sudo systemctl podman wget nvidia-smi. A call to one is logged and exits
  99. Any other command that could reach out (nc, git, gh, /dev/tcp, python urllib) is NOT
  intercepted: the script uses none of them, and a new one must be reviewed, not assumed caught.
  The fake docker and curl log any invocation shape the script does not use the same way.
The shims come first on a PATH of /usr/bin:/bin; the real docker and curl are never reached.
"""

from __future__ import annotations

import base64
import hashlib
import json
import os
import re
import shutil
import sqlite3
import stat
import subprocess
import sys
import urllib.parse
from dataclasses import dataclass
from datetime import datetime, timedelta
from pathlib import Path

import yaml

SCRIPT = Path(__file__).resolve().parents[3] / "scripts/ops/s100_test/challenge_image.sh"
CHALLENGE, BEACON, STUB, RELAY = (
    f"hypertrain-test-{n}" for n in ("challenge", "beacon", "master-stub", "relay")
)
TEST_NAMES = frozenset(
    {CHALLENGE, BEACON, STUB, RELAY, *(f"hypertrain-test-auditor-{i}" for i in (0, 1))}
)
# Containers the script must never touch: they exist in the fixture so that a call that hits one is
# recorded, and the census (mutated set <= TEST_NAMES) fails on it. The auditors and the executor
# are the ones the runbook says never to restart.
PROD_NAMES = (
    "hypertrain-beacon cortex-challenge-hypertrain hypertrain-auditor-go-0 cortex-master-1 "
    "hypertrain-auditor-0 hypertrain-auditor-1 executor-executor-1".split()
)
REPO = "ghcr.io/cortexlm/cortex/hypertrain"
OLD_ID, NEW_ID, OTHER_ID, BEACON_ID = (f"sha256:{c * 32}" for c in ("0a", "1b", "0c", "2d"))
OLD_REF, NEW_REF = f"{REPO}@{OLD_ID}", f"{REPO}@{NEW_ID}"
EXECUTOR_PATH, EXECUTOR_VAR = "/data/executor/cuda-executor.json", "HYPERTRAIN_CUDA_EXECUTOR"
ARTIFACTS_VAR = "HYPERTRAIN_CUDA_EXECUTOR_ARTIFACTS"
SETTINGS_PATH, TOKEN_PATH = "/data/executor/r2-artifacts.json", "/data/executor/r2.env"
# what the fake answers to a round trip (a line in the grammar of the real program)
ROUNDTRIP_LINE = (
    "r2-check challenge put=200 get=200 delete=204 absent=404 bytes=1048576"
    " put_s=0.1 get_s=0.1 delete_s=0.1"
)
# What the token file holds. None of it may reach an output, a docker call, an argument or a file
# other than the token file itself: Box.run looks for every value after every command.
R2_VALUES = {
    "R2_ACCOUNT_ID": "sentinel-account-id",
    "R2_ACCESS_KEY_ID": "sentinel-access-key-id",
    "R2_SECRET_ACCESS_KEY": "sentinel-secret-access-key",
    "R2_BUCKET": "cortex-v3-test-sentinel",
    "R2_ENDPOINT": "https://sentinel-endpoint.example.test",
}


def _needles() -> dict[str, str]:
    """Every form a value could leave in: as it is, in base64, quoted for a URL, the endpoint
    without its scheme, and any piece of 12 characters of it (printing a prefix leaks too)."""
    found: dict[str, str] = {}
    for name, value in R2_VALUES.items():
        forms = {
            value,
            base64.b64encode(value.encode()).decode().rstrip("="),
            base64.urlsafe_b64encode(value.encode()).decode().rstrip("="),
            urllib.parse.quote(value, safe=""),
            value.split("://", 1)[-1],
        }
        forms |= {value[i : i + 12] for i in range(len(value) - 11)}
        found |= {form: name for form in forms}
    return found


NEEDLES = _needles()
NOW, EPOCH = "2026-10-27T12:00:00+00:00", 1000
SECRET = "env-value-that-must-never-reach-a-backup"
IMAGE_ENV = [f"HYPERTRAIN_ADMIN_TOKEN={SECRET}", "TZ=UTC"]  # what every container starts with
SCRIPT_TEXT = SCRIPT.read_text()
# Pinned here, not read from the script: the expected URLs must not follow an edit of the script.
# API is the TEST challenge port (docs/s100-test-stack.md); MASTER is the production master the
# gate reads. `sandboxed` fails when the script assigns anything else.
API, MASTER = "http://127.0.0.1:18996", "http://10.177.245.39:8080"

COMPOSE = f"""\
name: hypertrain-test
x-common: &common
  image: {OLD_REF}
  restart: unless-stopped
services:
  challenge:
    <<: *common
    container_name: {CHALLENGE}
    ports: ["127.0.0.1:18996:8000"]
  beacon:
    <<: *common
    container_name: {BEACON}
    entrypoint:
      [python, -m, hypertrain.beacon, push, --challenge, "http://challenge:8000", --drand,
       "http://drand:80", --interval, "3",
       --catch-up, "20", --max-failures, "5"]
  master-stub:
    <<: *common
    container_name: {STUB}
"""
RELAY_COMPOSE = f"""\
name: hypertrain-test
services:
  relay:
    image: {OLD_REF}
    container_name: {RELAY}
"""

FAKE_DOCKER = r"""#!@PY@
import hashlib, json, os, re, sys

D = os.environ["FAKE_DIR"]
st = json.load(open(D + "/docker.json"))
a = sys.argv[1:]
st["calls"].append(a)
FORMATS = {
    "{{.Image}}",
    "{{.Config.Image}}",
    "{{.Image}} {{.State.Status}} {{.State.StartedAt}} restarts={{.RestartCount}}",
}


def done(rc=0, msg=""):
    json.dump(st, open(D + "/docker.json", "w"))
    if msg:
        print(msg, file=sys.stderr)
    sys.exit(rc)


def mutate(op, name):  # what the live DB looked like when the container was touched
    db = st["watch"] + "/challenge.db"
    seen = {"files": sorted(os.listdir(st["watch"])),
            "db": hashlib.sha256(open(db, "rb").read()).hexdigest() if os.path.exists(db) else None}
    st["mutations"].append([op, name, seen])


def load(path):  # one compose file's services as written, `environment` as a mapping
    import yaml
    if not os.path.isabs(path) or not os.path.exists(path):
        done(1, "no configuration file provided: " + path)
    try:
        svcs = yaml.safe_load(open(path))["services"]
        for spec in svcs.values():
            env = spec.get("environment") or {}
            if isinstance(env, list):
                env = dict(e.split("=", 1) for e in env)
            if env:
                spec["environment"] = {k: str(v) for k, v in env.items()}
    except (yaml.YAMLError, KeyError, TypeError, AttributeError) as e:
        done(1, "invalid compose file %s: %s" % (path, str(e).replace("\n", " ")))
    return svcs


def merge(base, over):
    if not (isinstance(base, dict) and isinstance(over, dict)):
        return over
    return {**base, **{k: merge(base.get(k), v) for k, v in over.items()}}


def services(paths):
    svcs = {}
    for path in paths:
        svcs = merge(svcs, load(path))
    if len(paths) > 1:
        svcs = merge(svcs, st["merge_patch"])
    for name, spec in svcs.items():
        if "image" not in spec:
            done(1, "service %s has neither an image nor a build context" % name)
    return svcs


def up(paths, names, force):
    svcs = services(paths)
    for s in names:
        if s not in svcs:
            done(1, "no such service: " + s)
    for s in names:
        name, ref = svcs[s].get("container_name", "hypertrain-test-%s-1" % s), svcs[s]["image"]
        if ref not in st["images"]:
            done(1, "pull access denied for " + ref)
        old = st["containers"].get(name, {"Config": {}})
        mutate("up", name)
        st["counter"] += 1
        if st["fail_up"]:
            old["State"] = {"Status": "exited", "StartedAt": "-"}
            st["containers"][name] = old
            done(1, "Error response from daemon: cannot start " + name)
        spec = json.dumps(svcs[s], sort_keys=True)
        if old.get("Spec") == spec and not force:  # compose leaves a container that is as asked
            print(" Container %s  Running" % name, file=sys.stderr)
            continue
        if not st["stale_up"]:  # compose recreated nothing: the old container keeps running
            started = "2026-10-27T12:00:%02dZ" % st["counter"]
            env = dict(e.split("=", 1) for e in st["image_env"]) | svcs[s].get("environment", {})
            st["containers"][name] = {
                "Name": "/" + name, "Image": st["images"][ref], "Spec": spec,
                "Config": {"Image": ref, "Env": ["%s=%s" % kv for kv in env.items()]},
                "State": {"Status": "running", "StartedAt": started}, "RestartCount": 0}
        print(" Container %s  Started" % name, file=sys.stderr)


def seen(name):  # a file the script may have written, as it is at this moment
    path = name if os.path.isabs(name) else st["watch"] + "/executor/" + name
    return hashlib.sha256(open(path, "rb").read()).hexdigest() if os.path.exists(path) else None


ROUNDTRIP = "@ROUNDTRIP@"


def program(args):  # `exec [-i] CHALLENGE python -I -c PROGRAM MODE ARGS`, the one shape the script
    # uses: -i only for `pre`, which reads the settings on stdin; the TEST challenge only
    stdin_flag = args[:1] == ["-i"]
    args = args[1:] if stdin_flag else args
    if args[:4] != ["hypertrain-test-challenge", "python", "-I", "-c"] or len(args) < 6:
        return None
    mode, rest = args[5], args[6:]
    shapes = {("pre", True, 1), ("post", False, 2)} | {("roundtrip", False, n) for n in (1, 3)}
    if (mode, stdin_flag, len(rest)) not in shapes:
        return None
    return args[4], mode, rest, sys.stdin.read() if stdin_flag else ""


def run_program(args):
    shape = program(args)
    if shape is None:
        open(D + "/tripwire.log", "a").write("docker exec " + " ".join(a[1:]) + "\n")
        done(125, "fake docker: unexpected invocation: " + " ".join(a))
    text, mode, rest, stdin = shape
    c = st["containers"]["hypertrain-test-challenge"]
    st["execs"].append({"mode": mode, "args": rest, "program": text, "stdin": stdin,
                        "settings": seen("r2-artifacts.json"), "override": seen(st["override"])})
    if c["State"]["Status"] != "running":
        done(1, "Error response from daemon: container is not running")
    ok = {"pre": "preflight ok", "post": "postflight ok", "roundtrip": ROUNDTRIP}[mode]
    rc, out, *where = st["exec_out"].get(mode, [0, ok])
    if mode == "post":  # the part of the program that only reads this process's environment
        env = dict(e.split("=", 1) for e in c["Config"]["Env"])
        names = ("HYPERTRAIN_CUDA_EXECUTOR", "HYPERTRAIN_CUDA_EXECUTOR_ARTIFACTS")
        for name, want in zip(names, rest):
            if env.get(name) != (None if want == "-" else want):
                rc, out = 1, "postflight refused: %s is not set as the script wrote it" % name
                break
    print(out, file=sys.stderr if where else sys.stdout)  # a traceback is on stderr
    done(rc)


def render(fmt, c):
    def field(m):
        v = c
        for k in m[1].split("."):
            v = v[k]
        return str(v)
    return re.sub(r"\{\{\.([\w.]+)\}\}", field, fmt)


files, rest = [], a[1:]  # `compose -f A [-f B ...] CMD ...`
while a[0] == "compose" and rest[:1] == ["-f"] and len(rest) > 1:
    files, rest = files + [rest[1]], rest[2:]
if files and rest == ["config", "--format", "json"]:
    # as compose does: a service whose profile is off is left out of config (up still starts it)
    active = {n: v for n, v in services(files).items() if "profiles" not in v}
    print(json.dumps({"name": "hypertrain-test", "services": active}))
elif files and rest[:3] == ["up", "-d", "--no-deps"] and len(rest) > 3:
    force = rest[3:4] == ["--force-recreate"]
    up(files, rest[3 + force:], force)
elif a[0] == "exec":
    run_program(a[1:])
elif a[0] == "inspect" and len(a) == 4 and a[2] == "--format" and a[3] in FORMATS:
    if a[1] not in st["containers"]:
        done(1, "Error: No such object: " + a[1])
    print(render(a[3], st["containers"][a[1]]))
elif a[0] == "inspect" and len(a) == 5 and not any(x.startswith("-") for x in a[1:]):
    print(json.dumps([st["containers"][n] for n in a[1:] if n in st["containers"]]))
elif a[:2] == ["image", "inspect"] and len(a) == 5 and a[3:] == ["--format", "{{.Id}}"]:
    if a[2] not in st["images"]:
        done(1, "Error: No such image: " + a[2])
    print(st["images"][a[2]])
elif a[0] in ("stop", "start", "restart") and len(a) == 2:
    mutate(a[0], a[1])  # recorded first: a call that fails is still a call the script made
    if a[1] not in st["containers"]:
        open(D + "/tripwire.log", "a").write("docker %s: unknown container\n" % " ".join(a))
        done(1, "Error response from daemon: No such container: " + a[1])
    st["containers"][a[1]]["State"]["Status"] = "exited" if a[0] == "stop" else "running"
    print(a[1])
elif a[:2] == ["logs", "--since"] and len(a) == 4 and a[2:] == ["25s", "hypertrain-test-beacon"]:
    print(st["logs"])
else:  # logged as well as refused: a script that swallows the exit code is still caught
    open(D + "/tripwire.log", "a").write("docker " + " ".join(a) + "\n")
    done(125, "fake docker: unexpected invocation: " + " ".join(a))
done()
"""

FAKE_CURL = r"""#!@PY@
import json, os, sys

D = os.environ["FAKE_DIR"]
a, i, flags, opt, urls = sys.argv[1:], 0, set(), {}, []


def refuse():
    open(D + "/tripwire.log", "a").write("curl " + " ".join(a) + "\n")
    sys.exit("fake curl: unexpected invocation: " + " ".join(a))


while i < len(a):
    if a[i] in ("-s", "-sf"):
        flags |= set(a[i][1:])
    elif a[i] in ("-m", "-o", "-w") and i + 1 < len(a):
        opt[a[i]] = a[i + 1]
        i += 1
    elif a[i].startswith("http://"):
        urls.append(a[i])
    else:
        refuse()
    i += 1
if "s" not in flags or "-m" not in opt or len(urls) != 1:
    refuse()
url = urls[0]
open(D + "/curl.log", "a").write(url + "\n")
entry = json.load(open(D + "/urls.json")).get(url)
if entry is None:
    open(D + "/unknown-url.log", "a").write(url + "\n")
    sys.exit(6)
seen = open(D + "/curl.log").read().split().count(url)
if entry.get("down") or seen <= entry.get("down_first", 0):
    sys.exit(7)
if entry["status"] >= 400 and "f" in flags:
    sys.exit(22)
if "-o" in opt:
    open(opt["-o"], "w").write(entry["body"])
else:
    sys.stdout.write(entry["body"])
if "-w" in opt:
    print(opt["-w"].replace("%{http_code}", str(entry["status"])).replace("\\n", "\n"), end="")
"""

FAKE_PYTHON3 = r"""#!@PY@
import datetime, os, sys

a = sys.argv[1:]
open(os.environ["FAKE_DIR"] + "/python3.log", "a").write(repr(a) + "\n")
if a[:2] == ["-I", "-c"] and "datetime.datetime.now(" in a[2]:
    frozen = datetime.datetime.fromisoformat(os.environ["FAKE_NOW"])

    class Frozen(datetime.datetime):
        @classmethod
        def now(cls, tz=None):
            return frozen.astimezone(tz)

    datetime.datetime = Frozen
    sys.argv = ["-c", *a[3:]]
    exec(compile(a[2], "<string>", "exec"), {"__name__": "__main__"})
else:
    os.execv("@PY@", ["@PY@", *a])
"""

SHIMS = {
    "docker": FAKE_DOCKER,
    "curl": FAKE_CURL,
    "python3": FAKE_PYTHON3,
    "sleep": "#!/bin/sh\nexit 0\n",
    # stat with FAKE_DIR_UID as the owner of the executor directory: a sandbox that is not root
    # cannot chown, and this is the only way to give the directory another owner than the token
    "stat": (
        f'#!/bin/bash\nout=$({shutil.which("stat")} "$@") || exit $?\n'
        'if [ -n "${FAKE_DIR_UID:-}" ] && [[ ${*: -1} == */state/executor ]]; then\n'
        '  out="${out% *} $FAKE_DIR_UID"; fi\necho "$out"\n'
    ),
    # install -o/-g need root: drop them, keep every other option for the real install
    "install": (
        '#!/bin/bash\necho "$*" >>"$FAKE_DIR/install.log"\nargs=()\n'
        'while [ $# -gt 0 ]; do case $1 in -o|-g) shift 2;; *) args+=("$1"); shift;; esac; done\n'
        f'exec {shutil.which("install")} "${{args[@]}}"\n'
    ),
    **{
        n: '#!/bin/sh\necho "$0 $*" >>"$FAKE_DIR/tripwire.log"\nexit 99\n'
        for n in "ssh scp rsync sudo systemctl podman wget nvidia-smi".split()
    },
}


def sandboxed(text: str, root: Path, uid: int | None = None) -> str:
    """The script with its host paths moved under root; a path it no longer covers fails loudly,
    and so does an API or MASTER address other than the pinned ones. The challenge's uid (CUID) is
    pinned the same way and becomes the uid of the test, so that the files the tests create pass the
    owner check; `uid` asks for another one, to prove that a file of someone else is refused."""
    for name, pinned in (("API", API), ("MASTER", MASTER)):
        found = re.search(rf"\b{name}=(http://[^\s;]+)", text)
        assert found and found[1] == pinned, (
            f"the script assigns {name}={found and found[1]}, the sandbox pins {pinned}: "
            "if the address changed on purpose, update API/MASTER in sandbox.py deliberately"
        )
    assert re.findall(r"^CUID=(\S+)$", text, re.M) == ["65532"], (
        "the challenge uid moved: update CUID"
    )
    text = re.sub(r"^CUID=65532$", f"CUID={os.getuid() if uid is None else uid}", text, flags=re.M)
    for prefix in ("/root/backups", "/data/hypertrain-test", "/data/relay"):
        assert prefix in text, f"{prefix} is gone from the script: update the sandbox rewrite"
        text = text.replace(prefix, f"{root}{prefix}")
    # the `executor` command validates a path INSIDE the challenge container (`/data/....json`): a
    # pattern and its message, not a host path, so the sandbox must not rewrite them
    in_container = {r"/data/[A-Za-z0-9._/-]+\.json$", "/data/....json"}
    # the container's volume is one line of its own (every other container path is $CDATA/...), and
    # a comment line runs nothing
    assert text.count("\nCDATA=/data\n") == 1, "CDATA=/data must be a line of its own, once"
    code = "\n".join(
        line
        for line in text.replace("\nCDATA=/data\n", "\n").splitlines()
        if not line.lstrip().startswith("#")
    )
    stray = [
        p
        for p in re.findall(r"(?<!@)/(?:root|data)\b\S*", code.replace(str(root), "@"))
        if p not in in_container
    ]
    assert not stray, f"host paths the sandbox does not rewrite: {stray}"
    # a temporary file is made under $TMPDIR, which Box.run puts in the sandbox, where it is looked
    # at: a literal /tmp would be written outside it
    shared = re.findall(r"/(?:var/)?tmp\b|/dev/shm", code)
    assert not shared, f"the script names a shared temporary directory: {shared}"
    return text


def program(name: str) -> str:
    """The Python program the script keeps in the variable `name` (`NAME=$(cat <<'PY_NAME' ... )`),
    as the shell hands it to `docker exec`: without the newline before the closing delimiter."""
    found = re.findall(
        rf"^{name}=\$\(cat <<'PY_{name[:-3]}'\n(.*?)\nPY_{name[:-3]}\n\)$", SCRIPT_TEXT, re.M | re.S
    )
    assert len(found) == 1, f"{name} is not a heredoc variable of the script, once"
    return found[0]


def call(cmd: str, backup: Path) -> tuple[str, ...]:
    """The arguments of `cmd` for a backup directory (swap also takes the new image, executor the
    path of its config inside the container)."""
    if cmd == "swap":
        return ("swap", NEW_REF, str(backup))
    return (cmd, str(backup), EXECUTOR_PATH) if cmd == "executor" else (cmd, str(backup))


def make_db(path: Path, rows: tuple[str, ...]) -> None:
    db = sqlite3.connect(path)
    db.execute("create table runs(id integer primary key, v text)")
    db.executemany("insert into runs(v) values (?)", [(r,) for r in rows])
    db.commit()
    db.close()


def rows(path: Path) -> list[str]:
    db = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
    try:
        return [r[0] for r in db.execute("select v from runs order by id")]
    finally:
        db.close()


def container(name: str, image_id: str) -> dict:
    return {
        "Name": "/" + name,
        "Image": image_id,
        "Config": {"Image": OLD_REF, "Env": list(IMAGE_ENV)},
        "State": {"Status": "running", "StartedAt": "t0"},
        "RestartCount": 0,
    }


@dataclass
class Run:
    out: str
    mutations: list


class Box:
    """One sandbox: live files, shims and fake state. `run` executes the script copy."""

    def __init__(self, root: Path, challenge_uid: int | None = None) -> None:
        self.root, self.fake = root, root / "fake"
        self.tokens: set[Path] = (
            set()
        )  # the operator's token files: the only place R2_VALUES may be
        self.t = root / "root/backups/hypertrain-test"
        self.f = self.t / "compose.test.yml"
        self.new = self.t / "compose.test.yml.new"
        self.override = self.t / "compose.executor.yml"
        self.state_dir = root / "data/hypertrain-test/state"
        self.relay_json = root / "data/relay/config/relay.json"
        self.stub_json = root / "data/hypertrain-test/config/metagraph.json"
        (self.fake / "bin").mkdir(parents=True)
        for d in (
            self.t,
            self.state_dir,
            self.relay_json.parent,
            self.stub_json.parent,
            root / "tmp",
        ):
            d.mkdir(parents=True)
        for name, body in SHIMS.items():
            shim = self.fake / "bin" / name
            shim.write_text(
                body.replace("@PY@", sys.executable).replace("@ROUNDTRIP@", ROUNDTRIP_LINE)
            )
            shim.chmod(0o755)
        self.script = root / "challenge_image.sh"
        self.script.write_text(sandboxed(SCRIPT_TEXT, root, challenge_uid))
        self.f.write_text(COMPOSE)
        (self.t / "compose.relay.yml").write_text(RELAY_COMPOSE)
        make_db(self.state_dir / "challenge.db", ("original",))
        make_db(self.state_dir / "server_keys.db", ("key-1",))
        self.relay_json.write_text('{"run": "r1"}\n')
        self.stub_json.write_text('{"hotkeys": {}, "netuid": 100}\n')
        # the beacon runs another image id than the challenge, so a rollback that read the wrong
        # entry of inspect-before.json is caught
        up = {CHALLENGE: OLD_ID, BEACON: BEACON_ID} | dict.fromkeys(
            (STUB, RELAY, *PROD_NAMES), OTHER_ID
        )
        self._write(
            "docker.json",
            {
                "watch": str(self.state_dir),
                "counter": 0,
                "fail_up": False,
                "stale_up": False,
                "image_env": IMAGE_ENV,
                "merge_patch": {},
                "mutations": [],
                "calls": [],
                "execs": [],
                "exec_out": {},
                "override": str(self.override),
                "logs": "beacon: pushed round 1",
                "images": {OLD_REF: OLD_ID},
                "containers": {n: container(n, image_id) for n, image_id in up.items()},
            },
        )
        self._write(
            "urls.json",
            {
                f"{API}/version": {"status": 200, "body": '{"version": "x"}'},
                f"{API}/health": {"status": 200, "body": "ok"},
                f"{API}/public/v1/runs": {"status": 200, "body": "[]"},
                f"{API}/v1/beacon/latest": {"status": 200, "body": '{"round": 1}'},
            },
        )
        self.seal()

    def _write(self, name: str, data: object) -> None:
        (self.fake / name).write_text(json.dumps(data))

    def _read(self, name: str):
        return json.loads((self.fake / name).read_text())

    def docker(self, **flags: object) -> dict:
        """The fake docker state; keyword arguments set flags (fail_up, stale_up) first."""
        st = self._read("docker.json") | flags
        self._write("docker.json", st)
        return st

    def pull(self, ref: str) -> None:
        st = self.docker()
        st["images"][ref] = ref.split("@", 1)[1] if "@" in ref else "sha256:" + "ee" * 32
        self._write("docker.json", st)

    def url(
        self, url: str, status: int = 200, body: str = "", down: bool = False, down_first: int = 0
    ) -> None:
        """Serve `url`; `down` refuses the connection, `down_first` for that many requests."""
        urls = self._read("urls.json")
        urls[url] = {"status": status, "body": body, "down": down, "down_first": down_first}
        self._write("urls.json", urls)

    def seal(
        self,
        age: float = 60,
        epoch: int = EPOCH - 1,
        sealed: object = True,
        ready: int = 200,
        weights: str | bool | None = None,
    ) -> None:
        """Master gate: readyz status, and the latest seal (weights False: unreachable)."""
        self.url(f"{MASTER}/readyz", ready, json.dumps({"epoch": EPOCH}))
        at = (datetime.fromisoformat(NOW) - timedelta(seconds=age)).isoformat()
        seal = {"epoch": epoch, "sealed": sealed, "computed_at": at.replace("+00:00", "Z")}
        body = weights if isinstance(weights, str) else json.dumps(seal)
        self.url(f"{MASTER}/v1/weights/latest", body=body, down=weights is False)

    @property
    def executor_dir(self) -> Path:
        return self.state_dir / "executor"

    @property
    def settings(self) -> Path:
        return self.executor_dir / "r2-artifacts.json"

    def token(self, name: str = "r2.env", mode: int = 0o600) -> Path:
        """The operator's R2 token file in the executor directory (made 0700 on first use), as the
        runbook leaves it: a file of the challenge's user. Returns its container path."""
        self.executor_dir.mkdir(exist_ok=True)
        self.executor_dir.chmod(0o700)
        path = self.executor_dir / name
        path.write_text("".join(f"{k}={v}\n" for k, v in R2_VALUES.items()))
        path.chmod(mode)
        self.tokens.add(path)
        return Path(f"/data/executor/{name}")

    def exec_calls(self, mode: str | None = None) -> list[dict]:
        calls = self.docker()["execs"]
        return [c for c in calls if mode in (None, c["mode"])]

    def exec_out(self, mode: str, rc: int, out: str, *, stderr: bool = False) -> None:
        """What the challenge program answers to `mode` (on stderr, as a traceback is), when the
        fake does not compute it."""
        answer = [rc, out, *(["stderr"] if stderr else [])]
        self.docker(exec_out={**self.docker()["exec_out"], mode: answer})

    def b(self, ts: str = "T1") -> Path:
        return self.t.parent / f"hypertrain-test-s7-{ts}"

    def seed_backup(self, golden: Path, ts: str = "T1") -> Path:
        """A backup dir: the files `backup` wrote in the golden sandbox, copied here."""
        shutil.copytree(golden, self.b(ts), copy_function=shutil.copy2)
        self.b(ts).chmod(0o700)
        return self.b(ts)

    def reseal(self, backup: Path) -> None:
        """Rewrite SHA256SUMS for the files now in the backup: a checksum-consistent forgery."""
        names = [
            line.split("  ", 1)[1] for line in (backup / "SHA256SUMS").read_text().splitlines()
        ]
        (backup / "SHA256SUMS").write_text(
            "".join(f"{digest_of(backup / n)}  {n}\n" for n in names)
        )

    def container(self, name: str) -> dict:
        return self.docker()["containers"][name]

    def env(self, name: str) -> dict[str, str]:
        """The environment a container runs with."""
        return dict(e.split("=", 1) for e in self.container(name)["Config"]["Env"])

    def live_yaml(self) -> dict:
        return yaml.safe_load(self.f.read_text())

    def snapshot(self) -> dict[str, object]:
        """Digest of every live file the script may write, whether it left a $F.new, and the
        executor override with its .new (None: no such file)."""
        files = [self.f, self.relay_json, self.stub_json, *self.state_dir.glob("*.db")]
        pending = self.override.with_name(self.override.name + ".new")
        dry = self.override.with_name(self.override.name + ".dry")
        executor = self.executor_dir
        return {p.name: digest_of(p) for p in files} | {
            "compose.new": self.new.exists(),
            "override": digest_of(self.override) if self.override.exists() else None,
            "override.new": pending.exists(),
            "override.dry": dry.exists(),
            "dry dirs": sorted(p.name for p in self.t.glob("dry.*")),
            # every file of the executor directory: name, content, mode (None: no such directory)
            "executor": executor.exists()
            and {
                str(p.relative_to(executor)): (
                    "link" if p.is_symlink() else digest_of(p) if p.is_file() else "dir",
                    stat.S_IMODE(p.lstat().st_mode),
                )
                for p in sorted(executor.rglob("*"))
            },
        }

    def run(
        self,
        *args: str,
        rc: int = 0,
        mutates: frozenset[str] | set[str] = frozenset(),
        now: str = NOW,
        env: dict[str, str] | None = None,
    ) -> Run:
        """Run the script copy with `args`; assert its exit code, the containers it mutated, and
        that nothing escaped the sandbox. Only the mutations of this run are recorded."""
        self.docker(mutations=[], calls=[])
        for log in ("curl.log", "tripwire.log", "unknown-url.log", "install.log"):
            (self.fake / log).unlink(missing_ok=True)
        env = {
            "PATH": f"{self.fake / 'bin'}:/usr/bin:/bin",
            "HOME": str(self.root),
            "TMPDIR": str(self.root / "tmp"),
            "FAKE_DIR": str(self.fake),
            "FAKE_NOW": now,
            "LC_ALL": "C",
            **(env or {}),
        }
        with self.script.open() as script:  # as over ssh: `bash -s -- CMD ARGS < script`
            p = subprocess.run(
                ["bash", "-s", "--", *map(str, args)],
                stdin=script,
                capture_output=True,
                text=True,
                env=env,
                cwd=self.root,
                timeout=180,
                check=False,
            )
        st = self._read("docker.json")
        hit = {m[1] for m in st["mutations"]}
        out = f"{p.stdout}{p.stderr}"
        report = f"\n{' '.join(map(str, args))}\n{out}\ndocker calls: {st['calls']}"
        for log in ("tripwire.log", "unknown-url.log"):
            assert not (self.fake / log).exists(), f"{log} is not empty{report}"
        self.no_token_values(out, st)
        assert hit <= TEST_NAMES, f"outside the TEST stack: {hit - TEST_NAMES}{report}"
        assert p.returncode == rc, f"exit {p.returncode}, wanted {rc}{report}"
        assert hit == set(mutates), f"mutated {sorted(hit)}, wanted {sorted(mutates)}{report}"
        return Run(out, st["mutations"])

    def no_token_values(self, out: str, st: dict) -> None:
        """No value of the token file, nor a piece of one, in the output, in what docker or python3
        was asked or given, or in the name or content of any file of the sandbox but the token."""
        where = {"output": out, "docker": json.dumps(st["calls"]) + json.dumps(st["execs"])}
        for path in sorted(self.root.rglob("*")):
            if path.is_symlink() or path in self.tokens:
                continue
            where[f"name of {path.relative_to(self.root)}"] = str(path.relative_to(self.root))
            if path.is_file():
                text = path.read_bytes().decode("latin-1")
                if path == self.fake / "docker.json":  # what a test made the fake answer is its own
                    text = json.dumps({k: v for k, v in st.items() if k != "exec_out"})
                where[str(path.relative_to(self.root))] = text
        for name, text in where.items():
            leaked = sorted({label for needle, label in NEEDLES.items() if needle in text})
            assert not leaked, f"{leaked} (a value of the token file) reached {name}"

    def polls(self, url: str) -> int:
        log = self.fake / "curl.log"
        return log.read_text().split().count(url) if log.exists() else 0


def digest_of(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def tree(directory: Path) -> dict[str, str]:
    """Name -> sha256 of every file directly in a directory."""
    return {p.name: digest_of(p) for p in sorted(directory.iterdir()) if p.is_file()}
