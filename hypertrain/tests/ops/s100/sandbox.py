"""A sandbox that runs s100_test/challenge_image.sh against fake docker, curl and python3.

The script hard-codes /root/backups, /data/... and the production master's address and runs as root
on s100. The sandbox runs a COPY with only those path prefixes rewritten into a temp directory
(the rewrite asserts it covered every one), with these first on PATH:
- `docker`: a stateful fake (docker.json: containers, images, flags) of exactly the shapes the
  script uses; any other shape fails. Every mutating call (up, stop, start, restart) is recorded
  with the container name it hit: the tests compare that set with the one the command may touch.
- `curl`: a URL map (urls.json) for the master gate and the challenge API; unknown URLs fail.
- `python3`: the real interpreter, with `datetime.now` frozen for the seal-window code.
- `sleep` (no-op), `install` (without -o/-g, which need root), and tripwires for exactly these
  commands: ssh scp rsync sudo systemctl podman wget nvidia-smi. A call to one is logged and exits
  99. Any other command that could reach out (nc, git, gh, /dev/tcp, python urllib) is NOT
  intercepted: the script uses none of them, and a new one must be reviewed, not assumed caught.
  The fake docker and curl log any invocation shape the script does not use the same way.
The shims come first on a PATH of /usr/bin:/bin; the real docker and curl are never reached.
"""

from __future__ import annotations

import hashlib
import json
import re
import shutil
import sqlite3
import subprocess
import sys
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
NOW, EPOCH = "2026-10-27T12:00:00+00:00", 1000
SECRET = "env-value-that-must-never-reach-a-backup"
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


def services(path):
    import yaml
    if not os.path.isabs(path) or not os.path.exists(path):
        done(1, "no configuration file provided: " + path)
    try:
        svcs = yaml.safe_load(open(path))["services"]
    except (yaml.YAMLError, KeyError, TypeError) as e:
        done(1, "invalid compose file %s: %s" % (path, str(e).replace("\n", " ")))
    for name, spec in svcs.items():
        if "image" not in spec:
            done(1, "service %s has neither an image nor a build context" % name)
    return svcs


def up(path, names):
    svcs = services(path)
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
        if not st["stale_up"]:  # compose recreated nothing: the old container keeps running
            started = "2026-10-27T12:00:%02dZ" % st["counter"]
            st["containers"][name] = {
                "Name": "/" + name, "Image": st["images"][ref],
                "Config": {"Image": ref, "Env": old["Config"].get("Env", [])},
                "State": {"Status": "running", "StartedAt": started}, "RestartCount": 0}
        print(" Container %s  Started" % name, file=sys.stderr)


def render(fmt, c):
    def field(m):
        v = c
        for k in m[1].split("."):
            v = v[k]
        return str(v)
    return re.sub(r"\{\{\.([\w.]+)\}\}", field, fmt)


if a[:2] == ["compose", "-f"] and a[3:6] == ["config", "--format", "json"] and len(a) == 6:
    # as compose does: a service whose profile is off is left out of config (up still starts it)
    active = {n: v for n, v in services(a[2]).items() if "profiles" not in v}
    print(json.dumps({"name": "hypertrain-test", "services": active}))
elif a[:2] == ["compose", "-f"] and a[3:6] == ["up", "-d", "--no-deps"] and len(a) > 6:
    up(a[2], a[6:])
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
    # install -o/-g need root: drop them, keep every other option for the real install
    "install": (
        "#!/bin/bash\nargs=()\n"
        'while [ $# -gt 0 ]; do case $1 in -o|-g) shift 2;; *) args+=("$1"); shift;; esac; done\n'
        f'exec {shutil.which("install")} "${{args[@]}}"\n'
    ),
    **{
        n: '#!/bin/sh\necho "$0 $*" >>"$FAKE_DIR/tripwire.log"\nexit 99\n'
        for n in "ssh scp rsync sudo systemctl podman wget nvidia-smi".split()
    },
}


def sandboxed(text: str, root: Path) -> str:
    """The script with its host paths moved under root; a path it no longer covers fails loudly,
    and so does an API or MASTER address other than the pinned ones."""
    for name, pinned in (("API", API), ("MASTER", MASTER)):
        found = re.search(rf"\b{name}=(http://[^\s;]+)", text)
        assert found and found[1] == pinned, (
            f"the script assigns {name}={found and found[1]}, the sandbox pins {pinned}: "
            "if the address changed on purpose, update API/MASTER in sandbox.py deliberately"
        )
    for prefix in ("/root/backups", "/data/hypertrain-test", "/data/relay"):
        assert prefix in text, f"{prefix} is gone from the script: update the sandbox rewrite"
        text = text.replace(prefix, f"{root}{prefix}")
    # the `executor` command validates a path INSIDE the challenge container (`/data/....json`): a
    # pattern and its message, not a host path, so the sandbox must not rewrite them
    in_container = {r"/data/[A-Za-z0-9._/-]+\.json$", "/data/....json"}
    stray = [
        p
        for p in re.findall(r"(?<!@)/(?:root|data)\b\S*", text.replace(str(root), "@"))
        if p not in in_container
    ]
    assert not stray, f"host paths the sandbox does not rewrite: {stray}"
    return text


def call(cmd: str, backup: Path) -> tuple[str, ...]:
    """The arguments of `cmd` for a backup directory (swap also takes the new image)."""
    return ("swap", NEW_REF, str(backup)) if cmd == "swap" else (cmd, str(backup))


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
    env = [f"HYPERTRAIN_ADMIN_TOKEN={SECRET}", "TZ=UTC"]
    return {
        "Name": "/" + name,
        "Image": image_id,
        "Config": {"Image": OLD_REF, "Env": env},
        "State": {"Status": "running", "StartedAt": "t0"},
        "RestartCount": 0,
    }


@dataclass
class Run:
    out: str
    mutations: list


class Box:
    """One sandbox: live files, shims and fake state. `run` executes the script copy."""

    def __init__(self, root: Path) -> None:
        self.root, self.fake = root, root / "fake"
        self.t = root / "root/backups/hypertrain-test"
        self.f = self.t / "compose.test.yml"
        self.new = self.t / "compose.test.yml.new"
        self.state_dir = root / "data/hypertrain-test/state"
        self.relay_json = root / "data/relay/config/relay.json"
        self.stub_json = root / "data/hypertrain-test/config/metagraph.json"
        (self.fake / "bin").mkdir(parents=True)
        for d in (self.t, self.state_dir, self.relay_json.parent, self.stub_json.parent):
            d.mkdir(parents=True)
        for name, body in SHIMS.items():
            shim = self.fake / "bin" / name
            shim.write_text(body.replace("@PY@", sys.executable))
            shim.chmod(0o755)
        self.script = root / "challenge_image.sh"
        self.script.write_text(sandboxed(SCRIPT_TEXT, root))
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
                "mutations": [],
                "calls": [],
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

    def live_yaml(self) -> dict:
        return yaml.safe_load(self.f.read_text())

    def snapshot(self) -> dict[str, object]:
        """Digest of every live file the script may write, and whether it left a $F.new."""
        files = [self.f, self.relay_json, self.stub_json, *self.state_dir.glob("*.db")]
        return {p.name: digest_of(p) for p in files} | {"compose.new": self.new.exists()}

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
        for log in ("curl.log", "tripwire.log", "unknown-url.log"):
            (self.fake / log).unlink(missing_ok=True)
        env = {
            "PATH": f"{self.fake / 'bin'}:/usr/bin:/bin",
            "HOME": str(self.root),
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
        assert hit <= TEST_NAMES, f"outside the TEST stack: {hit - TEST_NAMES}{report}"
        assert p.returncode == rc, f"exit {p.returncode}, wanted {rc}{report}"
        assert hit == set(mutates), f"mutated {sorted(hit)}, wanted {sorted(mutates)}{report}"
        return Run(out, st["mutations"])

    def polls(self, url: str) -> int:
        log = self.fake / "curl.log"
        return log.read_text().split().count(url) if log.exists() else 0


def digest_of(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def tree(directory: Path) -> dict[str, str]:
    """Name -> sha256 of every file directly in a directory."""
    return {p.name: digest_of(p) for p in sorted(directory.iterdir()) if p.is_file()}
