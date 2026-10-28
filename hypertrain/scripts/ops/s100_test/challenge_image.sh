#!/usr/bin/env bash
# Swap the image of the s100 TEST challenge, retune the TEST beacon pusher, and roll both back.
# Never touches the production challenge or the master; the TEST relay config and stub metagraph
# are only put back by rollback, from the copies backup took.
# Run on s100 over stdin: ssh root@s100 'bash -s -- CMD ARGS' < challenge_image.sh
#   backup TS            snapshot compose files, both SQLite DBs (integrity ok), relay.json,
#                        metagraph.json and inspect JSON (env names only), with SHA256SUMS
#   swap NEW_REF BACKUP  gate, pin NEW_REF on the challenge service, up -d --no-deps challenge
#   beacon BACKUP        gate, set the pusher to --interval 0.2 --catch-up 60 --max-failures 100.
#                        A store lock (a remote reference) hides rounds from the pusher and it
#                        pushes oldest first, at most catch-up of them: 60 skips none up to a lock
#                        of 150 s (a skipped round can be a trial's seed), 100 failures outlast
#                        140 s of refused connections. The gate then stays closed for a few times
#                        the lock. Takes the stock cadence (3 / 20) and the one the previous
#                        version of this script left behind (0.2 / 3); a budget of 100 or more
#                        is kept. docs/adr/0012
#   paper BACKUP         dry rollback: the backed-up compose file must parse and resolve the old image
#   rollback BACKUP      restore the compose file, recreate challenge and beacon as they were, and
#                        put back relay.json / metagraph.json (restarting relay / stub) if changed
#   restore-db BACKUP    keep the live DBs aside, stop the challenge, put the backed-up DBs back
#   executor BACKUP PATH [--r2 FILE [--operations LIST]] [--dry-run]
#                        gate, add HYPERTRAIN_CUDA_EXECUTOR=PATH (a container path) to the challenge
#                        through a compose override file. With --r2 the results come back through R2
#                        (docs/adr/0013): FILE is the bucket token, /data/executor/NAME in the
#                        container, a file of the challenge's uid 65532, mode 0600, which is never
#                        read here. The script writes /data/executor/r2-artifacts.json (FILE and the
#                        operations, default audit,referee) and adds
#                        HYPERTRAIN_CUDA_EXECUTOR_ARTIFACTS; only those variables may change. The
#                        challenge's own loader checks the settings before anything is written and
#                        the recreated container afterwards. --dry-run does all the checks, writes
#                        and recreates nothing, prints variable names only.
#   executor-off BACKUP  drop the override (and the settings file this script wrote) and recreate the
#                        challenge without the variables
#   r2-check [--file FILE] [--node LABEL sha256:DIGEST]
#                        not gated, changes nothing in the stack: put, get and delete a 1 MiB object
#                        in the bucket through the token the challenge is configured with (or FILE,
#                        which is checked like --r2's), then asks for the absent key, as the transfer
#                        does before each attempt. With --node, between get and delete, also a HEAD
#                        and a ranged GET of the object and a GET of an absent key from that executor
#                        host, by presigned URLs on the stdin of a confined --rm container of the GPU
#                        image DIGEST that reports its CA paths and does the TLS handshakes. Prints
#                        HTTP statuses, byte counts and seconds, no URL.
# rollback and restore-db are not gated: they are the way back from a bad change.
# swap, beacon, paper, rollback, executor and executor-off refuse (exit 4) a compose file with a
# service whose container_name is missing or is not one of the six TEST names; executor checks the
# live file and the live file merged with its override. restore-db runs no compose.
# Exit codes: 2 usage (and master readyz), 3 seal gate, 4 refusal, 5 unexpected compose change, 6 DB,
# 7 the challenge is not up, or not configured as asked, after a change, 8 r2-check did not complete.
set -euo pipefail
T=/root/backups/hypertrain-test; F=$T/compose.test.yml; C=hypertrain-test-challenge
STATE=/data/hypertrain-test/state; API=http://127.0.0.1:18996; MASTER=http://10.177.245.39:8080
CDATA=/data
CUID=65532
XD=$STATE/executor; R2S=$CDATA/executor/r2-artifacts.json; R2F=$XD/r2-artifacts.json
cmd=${1:?backup|swap|paper|rollback|restore-db|executor|executor-off|r2-check}
compose() { (cd "$T" && docker compose -f "$1" "${@:2}"); }
images() { # service -> image as compose resolves it
  compose "$1" config --format json | python3 -I -c '
import json,sys
for n,s in sorted(json.load(sys.stdin)["services"].items()): print(n, s["image"])'; }
# compose up acts on the container_name values of the file, so every service of a file about to be
# brought up must declare one of these: a name one token away is a production container
TEST_NAMES="hypertrain-test-challenge hypertrain-test-beacon hypertrain-test-master-stub
  hypertrain-test-relay hypertrain-test-auditor-0 hypertrain-test-auditor-1"
test_names() { # test_names FILE < compose config JSON of FILE
  python3 -I -c '
import json,sys
ok=sys.argv[2].split(); bad=0; svc=json.load(sys.stdin)["services"]
for n,s in sorted(svc.items()):
    c=s.get("container_name")
    if c not in ok:
        bad=1; print("REFUSED %s: service %s %s" % (sys.argv[1], n, "names container "+c if c else "declares no container_name"))
# up -d --no-deps challenge beacon names these two services: compose config leaves out a service whose
# profile is off, which up would still start, and a TEST name on the wrong service is no better
for n,c in (("challenge","hypertrain-test-challenge"),("beacon","hypertrain-test-beacon")):
    if svc.get(n,{}).get("container_name")!=c:
        bad=1; print("REFUSED %s: service %s must be %s" % (sys.argv[1], n, c))
sys.exit(bad)' "$1" "$TEST_NAMES"; }
backup_names() { # backup_names BACKUP: its compose file, which rollback copies over the live one
  # compose() runs in $T, so a relative BACKUP must be made absolute first
  local cfg f; f=$(cd "$1" && pwd)/compose.test.yml
  cfg=$(compose "$f" config --format json) && test_names "$f" <<< "$cfg" || exit 4; }
gate() { # readyz 200, no seal backlog, and a sealed seal younger than 30 min (first half of the
  # 72-min window; a seal delayed by unfinished work would make "young" mean "late", so the
  # sealed epoch must also be the one just before the master's current epoch).
  # HT_GATE_MAX_AGE_S replaces only the 30-min limit (1..4320 s, one epoch), for a change the
  # founder approved outside the first half of the window; it is printed whenever it is set.
  local max=${HT_GATE_MAX_AGE_S:-1800} ready
  [[ $max =~ ^[1-9][0-9]{0,3}$ && $max -le 4320 ]] \
    || { echo "GATE HT_GATE_MAX_AGE_S must be 1..4320 seconds"; exit 4; }
  [ "$max" -eq 1800 ] || echo "GATE age limit overridden: HT_GATE_MAX_AGE_S=$max (default 1800)"
  ready=$(curl -sf -m 10 "$MASTER/readyz") || { echo "GATE readyz not 200"; exit 2; }
  curl -s -m 10 "$MASTER/v1/weights/latest" | python3 -I -c '
import json,sys,datetime
d=json.load(sys.stdin); c=datetime.datetime.fromisoformat(d["computed_at"].replace("Z","+00:00"))
age=(datetime.datetime.now(datetime.timezone.utc)-c).total_seconds(); now=json.loads(sys.argv[1])["epoch"]
print("gate seal epoch",d["epoch"],"master epoch",now,"sealed",d.get("sealed"),"age_s",int(age))
sys.exit(0 if d.get("sealed") is True and 0<=age<int(sys.argv[2]) and d["epoch"]==now-1 else 3)' \
    "$ready" "$max" || { echo "GATE seal not sealed, older than ${max}s, or behind"; exit 3; }; }
live() { # live IMAGE_ID: the challenge answers /version and runs exactly IMAGE_ID
  local i; for i in $(seq 1 30); do curl -sf -m 5 -o /dev/null "$API/version" && break; sleep 1; done
  curl -sf -m 5 -o /dev/null "$API/version" || { echo "challenge /version not 2xx"; exit 7; }
  [ "$(docker inspect "$C" --format '{{.Image}}')" = "$1" ] || { echo "challenge not on $1"; exit 7; }; }
check() { # what an operator reads after any change
  docker inspect "$C" --format '{{.Image}} {{.State.Status}} {{.State.StartedAt}} restarts={{.RestartCount}}'
  for p in version health public/v1/runs; do
    printf '%s ' "$p"; curl -s -m 10 -w ' %{http_code}\n' "$API/$p" | cut -c1-200; done
  printf 'public runs sha '; curl -s -m 10 "$API/public/v1/runs" | sha256sum | cut -c1-64; }
apply() { # apply STEP BACKUP PATH..: $F.new must differ from $F in exactly PATH.., then go live
  # (a PATH written +PATH may or may not differ; if every PATH is optional something must)
  local step=$1 B=$2; shift 2
  compose "$F" config --format json > "$B/config-before-$step.json"
  compose "$F.new" config --format json > "$B/config-after-$step.json"
  { test_names "$F" < "$B/config-before-$step.json" && test_names "$F.new" < "$B/config-after-$step.json"; } \
    || { rm -f "$F.new"; exit 4; }
  diff "$F" "$F.new" || true
  python3 -I - "$B/config-before-$step.json" "$B/config-after-$step.json" "$@" <<'EOF' \
    || { rm -f "$F.new"; echo "unexpected compose change"; exit 5; }
import json, sys
def flat(x, p=""):
    if isinstance(x, dict):
        return {k: v for n, c in x.items() for k, v in flat(c, f"{p}.{n}" if p else n).items()}
    if isinstance(x, list):
        return {k: v for i, c in enumerate(x) for k, v in flat(c, f"{p}.{i}").items()}
    return {p: x}
a, b = (flat(json.load(open(f))) for f in sys.argv[1:3])
changed = sorted(k for k in a.keys() | b.keys() if a.get(k) != b.get(k))
print("changed", changed)
required = {p for p in sys.argv[3:] if not p.startswith("+")}
allowed = required | {p[1:] for p in sys.argv[3:] if p.startswith("+")}
sys.exit(0 if required <= set(changed) <= allowed and (required or changed) else 5)
EOF
  if [ "$step" = beacon ]; then # the cadence the edit was meant to reach, read from the resolved config
    python3 -I -c '
import json,sys
e=json.load(open(sys.argv[1]))["services"]["beacon"]["entrypoint"]
flags=(e[8],e[10],e[12])==("--interval","--catch-up","--max-failures")
sys.exit(0 if flags and (e[9],e[11])==("0.2","60") and int(e[13])>=100 else 5)' "$B/config-after-$step.json" \
      || { rm -f "$F.new"; echo "beacon cadence not reached"; exit 5; }
  fi
  cp -p "$F" "$B/compose.test.yml.before-$step"; mv "$F.new" "$F"; date -u +%FT%TZ
  compose "$F" up -d --no-deps "$step" 2>&1 | tail -3; }
# The challenge's own view of the R2 settings, run in the container as its user (docs/adr/0013). It
# never builds an Executor: its Journal repairs the live journal file. Everything it prints is a fixed
# sentence, a variable name or a class name; a loader message is printed only when it is one of the
# known ones, because the others (a UnicodeDecodeError, a pydantic input) quote the credential file.
#   pre CONFIG           settings JSON on stdin, as ArtifactExecutor would load them
#   post CONFIG SETTINGS the environment of this process names exactly these files ("-": unset) and
#                        the files, read as this user, load
#   roundtrip FILE [LABEL DIGEST]
#                        put, get and delete 1 MiB in the bucket with the token FILE, or with the one
#                        the settings of this process name ("-"), through the client the transfer uses;
#                        with LABEL and DIGEST also, between get and delete, a HEAD and a ranged GET of
#                        that object and a GET of a key that is not there, from the executor host
#                        LABEL, by presigned URLs on its stdin, in a confined --rm container of the GPU
#                        image DIGEST (the host is a rented machine: only what it answers in a closed
#                        shape is printed, never its stderr). After the delete, the key is asked for.
CHALLENGE_PY=$(cat <<'PY_CHALLENGE'
import os
import re
import shutil
import sys
from pathlib import Path

CONFIG_ENV, SETTINGS_ENV = "HYPERTRAIN_CUDA_EXECUTOR", "HYPERTRAIN_CUDA_EXECUTOR_ARTIFACTS"
KIND = {"pre": "preflight", "post": "postflight", "roundtrip": "r2-check"}
SIZE = 1 << 20
KNOWN = (
    "credentials_file must be a regular file of this user with mode 0600",
    "credentials_file holds names other than the R2 settings",
    "the artifact bucket must be a cortex-v3-* bucket",
    "the R2 endpoint must be https (plain http only to a loopback address)",
    "artifacts need network 'default': the container must reach R2",
    "max_artifact_bytes is above the executor's max_result_bytes",
    "invalid bucket name",
)


class Said(Exception):
    """Its message is a sentence written in this program."""


def printable(text):
    return re.sub(r"[^ -~]", "?", str(text))[:120]


def where(found):
    loc = [str(part) for part in found["loc"]]
    if loc and found["type"] == "extra_forbidden":
        loc[-1] = "?"  # the name of a key of the document, which can be anything
    return ".".join(loc) + ": " if loc else ""


def why(error):
    try:
        from pydantic import ValidationError
    except ImportError:
        ValidationError = ()
    text = str(error)
    if isinstance(error, Said):
        return text
    if isinstance(error, ValidationError):  # where and why; its "input" is never read
        return "; ".join(where(e) + printable(e["msg"]) for e in error.errors())
    if text in KNOWN or re.fullmatch(r"missing R2 settings: [A-Z0-9_, ]+", text):
        return text
    if isinstance(error, OSError) and error.filename is not None:  # a path, never file content
        return f"{type(error).__name__}: {printable(error.filename)}"
    return type(error).__name__


def load(config_path, raw):
    import httpx

    from hypertrain.gpu_ops import artifact_transfer as at
    from hypertrain.gpu_ops import cuda_executor as ce
    from hypertrain.storage.r2 import R2Client

    settings = at.ArtifactSettings.model_validate_json(raw)
    config = ce.ExecutorConfig.model_validate_json(Path(config_path).read_bytes())
    credentials = at.load_credentials(settings.credentials_file)
    # what ArtifactExecutor.__init__ refuses besides the above, without building one
    if config.network != "default":
        raise ValueError("artifacts need network 'default': the container must reach R2")
    if settings.max_artifact_bytes > config.max_result_bytes:
        raise ValueError("max_artifact_bytes is above the executor's max_result_bytes")
    with httpx.Client(trust_env=False) as http:
        R2Client(credentials, client=http)
    if shutil.which("ssh") is None:
        raise FileNotFoundError("ssh")
    return settings


def status(error):
    # an R2Error carries the object key in its text: only the HTTP status or a class name leaves
    tail = str(error).rsplit(": ", 1)[-1] if type(error).__name__ == "R2Error" else ""
    found = re.fullmatch(r"HTTP (\d{3})|(\w+)", tail)
    return (found[1] or found[2]) if found else type(error).__name__


# One answer, in the image the runner will use, with the default TLS context the runner uses.
NODE_PY = r"""
import http.client, json, os, ssl, sys, time, urllib.parse

out = {}
try:
    paths = ssl.get_default_verify_paths()
    out.update(
        python=".".join(map(str, sys.version_info[:3])),
        openssl=ssl.OPENSSL_VERSION,
        cafile=bool(paths.cafile),
        openssl_cafile=os.path.exists(paths.openssl_cafile or ""),
        capath_entries=len(os.listdir(paths.capath)) if paths.capath else 0,
        ssl_env=bool(os.environ.get("SSL_CERT_FILE") or os.environ.get("SSL_CERT_DIR")),
    )
    context = ssl.create_default_context()
    urls = sys.stdin.read().split()
    asked = zip(("head", "get", "absent"), ("HEAD", "GET", "GET"), ({}, {"Range": "bytes=0-0"}, {}), urls)
    for name, method, headers, url in asked:
        start, answer = time.monotonic(), {}
        try:
            parts = urllib.parse.urlsplit(url)
            if parts.scheme != "https":
                raise ValueError
            conn = http.client.HTTPSConnection(parts.hostname, parts.port, timeout=20, context=context)
            conn.request(method, parts.path + "?" + parts.query, headers=headers)
            reply = conn.getresponse()
            reply.read()
            answer.update(status=reply.status, bytes=int(reply.getheader("content-length") or 0))
            conn.close()
        except ssl.SSLCertVerificationError as error:
            answer.update(error="SSLCertVerificationError", verify_code=error.verify_code)
        except BaseException as error:
            answer["error"] = type(error).__name__
        answer["elapsed_s"] = round(time.monotonic() - start, 3)
        out[name] = answer
except BaseException as error:
    out["error"] = type(error).__name__
print(json.dumps(out, sort_keys=True))
"""
# The container holds two presigned URLs on a rented machine and needs only to open one https
# connection: no capability, no writable file system, no extra process or memory. The network stays
# the default bridge, which is how it reaches the endpoint (the runner's container does too).
CONFINE = (
    "--cap-drop=ALL",
    "--read-only",
    "--pids-limit=64",
    "--memory=256m",
    "--security-opt=no-new-privileges",
)
# OpenSSL's X509_V_ERR_* codes a first run can meet; any other is printed as its number
VERIFY = {18: "self_signed", 20: "unknown_issuer", 62: "hostname_mismatch", 64: "ip_mismatch"}
WORD = re.compile(r"[A-Za-z]{1,40}")


def small(value, low, high):
    return isinstance(value, int | float) and not isinstance(value, bool) and low <= value <= high


def shown(reply):
    """What a rented host said, rebuilt in a closed shape: a host can print anything, including the
    URL it was handed, so nothing it wrote is passed on as it is."""
    words = []
    version = r"[0-9]{1,4}\.[0-9]{1,4}\.[0-9]{1,4}"
    library = rf"(OpenSSL|LibreSSL) {version}[a-z]?( {{1,2}}[0-9]{{1,2}} [A-Z][a-z]{{2}} [0-9]{{4}})?"
    for key, pattern in (("python", version), ("openssl", library)):
        if isinstance(reply.get(key), str) and re.fullmatch(pattern, reply[key]):
            words.append(f"{key}={reply[key].replace(' ', '_')}")
    for key in ("cafile", "openssl_cafile", "ssl_env"):
        if isinstance(reply.get(key), bool):
            words.append(f"{key}={int(reply[key])}")
    if small(reply.get("capath_entries"), 0, 10**6):
        words.append(f"capath_entries={reply['capath_entries']}")
    if isinstance(reply.get("error"), str) and WORD.fullmatch(reply["error"]):
        words.append(f"error={reply['error']}")
    for name in ("head", "get", "absent"):
        answer = reply.get(name)
        if not isinstance(answer, dict):
            continue
        if small(answer.get("status"), 100, 599):
            words.append(f"{name}={answer['status']}")
        elif isinstance(answer.get("error"), str) and WORD.fullmatch(answer["error"]):
            words.append(f"{name}={answer['error']}")
            if small(answer.get("verify_code"), 0, 100):
                words.append(f"{name}_verify={VERIFY.get(answer['verify_code'], answer['verify_code'])}")
        if small(answer.get("bytes"), 0, 1 << 40):
            words.append(f"{name}_bytes={int(answer['bytes'])}")
        if small(answer.get("elapsed_s"), 0, 1000):
            words.append(f"{name}_s={answer['elapsed_s']:.3f}")
    return " ".join(words)


def resolve(label, digest):
    """The executor host `label` of the config this process runs with: asked before the bucket is."""
    from hypertrain.gpu_ops import cuda_executor as ce

    if CONFIG_ENV not in os.environ:
        raise Said(f"{CONFIG_ENV} is not set")
    config = ce.ExecutorConfig.model_validate_json(Path(os.environ[CONFIG_ENV]).read_bytes())
    host = next((h for h in config.hosts if h.label == label), None)
    if host is None or not re.fullmatch(r"sha256:[0-9a-f]{64}", digest):
        raise Said("no executor host has this label, or the image digest is not sha256:<64 hex>")
    return config, host, digest


def probe(client, key, target):
    """From the host: HEAD and a ranged GET of `key`, and a GET of a key that is not there, the
    request the runner opens every transfer with. Returns (what to print, whether it worked)."""
    import json
    import shlex
    import tempfile
    import time

    from hypertrain.gpu_ops import cuda_executor as ce

    config, host, digest = target
    label = host.label
    command = shlex.join(
        [*host.docker, "run", "--rm", "-i", "--pull", "never", *CONFINE]
        + ["--entrypoint", ce.IMAGE_PYTHON, f"{config.image_repository}@{digest}", "-I", "-c", NODE_PY]
    )
    asked = (("HEAD", key), ("GET", key), ("GET", f"{key}-absent"))
    urls = "".join(client.presign(method, name, expires=120) + "\n" for method, name in asked)
    fd = os.memfd_create("r2-check")  # the URLs are the remote stdin and live in no file
    try:
        os.write(fd, urls.encode())
        with tempfile.TemporaryDirectory(prefix="r2-check-") as work:
            ssh = shutil.which("ssh") or "ssh"
            code = ce.SshTransport(host, config, ssh).run(
                command,
                stdin=Path(f"/proc/self/fd/{fd}"),
                stdout=Path(work, "out"),
                stderr=Path(work, "err"),
                deadline=time.time() + 90,
                cancel=None,
                max_stdout_bytes=4096,
            )
            raw = Path(work, "out").read_text(errors="replace")
    except Exception as error:
        return f"{label} {type(error).__name__}", False
    finally:
        os.close(fd)
    if code:  # 125: docker refused (with --pull never, usually: the image is not on the host)
        return f"{label} exit_{code}", False
    try:
        reply = json.loads(raw)
    except ValueError:
        reply = None
    if not isinstance(reply, dict):
        return f"{label} no_reply", False
    head, get, absent = (reply.get(name) for name in ("head", "get", "absent"))
    ok = (
        isinstance(head, dict) and head.get("status") == 200 and head.get("bytes") == SIZE
        and isinstance(get, dict) and get.get("status") in (200, 206)
        and isinstance(absent, dict) and absent.get("status") == 404
    )  # fmt: skip
    line = f"{label} {shown(reply)}"
    return (line, ok) if re.fullmatch(r"[ -~]{1,300}", line) else (f"{label} unprintable", False)


def roundtrip(file, node):
    import hashlib
    import secrets
    import time

    import httpx

    from hypertrain.gpu_ops import artifact_transfer as at
    from hypertrain.storage.r2 import R2Client

    target = resolve(*node) if node else None  # a host that is not there costs no bucket call
    if file == "-":
        if SETTINGS_ENV not in os.environ:
            raise Said(f"{SETTINGS_ENV} is not set")
        raw = Path(os.environ[SETTINGS_ENV]).read_bytes()
        file = at.ArtifactSettings.model_validate_json(raw).credentials_file
    credentials = at.load_credentials(Path(file))
    seen, row, data = [], {}, os.urandom(SIZE)
    # the client of the transfer: no proxy variable, no CA path from the environment
    hook = {"response": [lambda response: seen.append(response.status_code)]}
    http = httpx.Client(timeout=30, trust_env=False, event_hooks=hook)
    client = R2Client(credentials, client=http)
    key = f"exec/_r2-check/{secrets.token_hex(16)}"

    def step(name, call):
        start = time.monotonic()
        try:
            value, code, ok = call(), str(seen[-1]), True
        except Exception as error:
            value, code, ok = None, status(error), False
        row[name] = [code, time.monotonic() - start, ok]
        return value

    far, far_ok = "", True
    try:
        step("put", lambda: client.put_object(key, data))
        if row["put"][2]:
            got = step("get", lambda: client.get_object(key))
            if row["get"][2] and hashlib.sha256(got).digest() != hashlib.sha256(data).digest():
                row["get"][0], row["get"][2] = "mismatch", False
            if target and row["get"][2]:  # the object is there for the host to look at
                far, far_ok = probe(client, key, target)
                far = f"\nr2-check node {far}"
    finally:
        step("delete", lambda: client.delete_object(key))
        # what Broker.begin asks before every attempt, of a key that is not there: any answer but
        # "absent" (a 403, say) sends the attempt down the ssh path without a word
        if step("absent", lambda: client.head_object(key)) is not None:
            row["absent"][0], row["absent"][2] = "present", False
        http.close()
    names = ("put", "get", "delete", "absent")
    said = " ".join(f"{n}={row[n][0] if n in row else 'skipped'}" for n in names)
    took = " ".join(f"{n}_s={row[n][1]:.3f}" for n in names[:3] if n in row)
    complete = len(row) == 4 and all(r[2] for r in row.values()) and far_ok
    return f"challenge {said} bytes={SIZE} {took}{far}", bool(complete)


def run(mode, args):
    if mode == "pre":
        load(args[0], sys.stdin.buffer.read())
        return "ok", True
    if mode == "roundtrip":
        return roundtrip(args[0], args[1:])
    if mode != "post":
        raise KeyError(mode)
    for name, want in ((CONFIG_ENV, args[0]), (SETTINGS_ENV, args[1])):
        have = os.environ.get(name)
        if have != (None if want == "-" else want):
            raise Said(f"{name} is not set as the script wrote it")
    if args[1] != "-":
        load(args[0], Path(args[1]).read_bytes())
    return "ok", True


mode = sys.argv[1]
try:
    text, complete = run(mode, sys.argv[2:])
except BaseException as error:
    print(KIND.get(mode, "program"), "refused:", why(error))
    sys.exit(1)
print(KIND[mode], text)
sys.exit(0 if complete else 1)
PY_CHALLENGE
)
backup_dir() { local f listed; for f in compose.test.yml challenge.db server_keys.db SHA256SUMS; do
    [ -f "$1/$f" ] || { echo "not a backup dir: $1 (no $f)"; exit 4; }; done
  # --strict fails on a malformed line instead of skipping it. A SHA256SUMS cut short still verifies
  # the files it lists, so every file used later must be listed too ("HASH  NAME" or "HASH *NAME").
  # Files added afterwards (config-*.json, compose.test.yml.before-*, restore-db-aside-*) are never used.
  (cd "$1" && sha256sum -c --strict --quiet SHA256SUMS) || { echo "backup $1 fails SHA256SUMS"; exit 4; }
  listed=$(sed -E 's/^[0-9a-fA-F]{64} [ *]//' "$1/SHA256SUMS")
  for f in compose.test.yml challenge.db server_keys.db compose.relay.yml relay.json metagraph.json \
      inspect-before.json; do
    if [ -e "$1/$f" ] && ! grep -qxF -- "$f" <<< "$listed"; then
      echo "backup $1: $f is not listed in SHA256SUMS"; exit 4; fi; done; }
usage() { echo "usage: $1"; exit 2; }
run_py() { # run_py [-i] MODE ARGS..: CHALLENGE_PY in the running challenge; only its sentences come out
  local i=() out rc=0 said
  if [ "${1:-}" = -i ]; then i=(-i); shift; fi
  out=$(docker exec ${i[@]+"${i[@]}"} "$C" python -I -c "$CHALLENGE_PY" "$@" 2>&1) || rc=$?
  said=$(printf '%s\n' "$out" | grep -E '^(preflight|postflight|r2-check) [ -~]{1,600}$') || true
  [ -z "$said" ] || printf '%s\n' "$said"
  if [ "$rc" -ne 0 ] && [ -z "$said" ]; then
    echo "(no answer from the challenge: docker exec exited $rc; is $C running?)"
  fi
  return "$rc"
}
r2_file() { # r2_file CONTAINER_PATH FLAG: exit 4 unless it is a token file the challenge can load.
  # Metadata only. The content is read by the challenge's own loader (run_py pre), never here.
  local d m H
  [[ $1 =~ ^$CDATA/executor/[A-Za-z0-9_][A-Za-z0-9._-]*$ ]] \
    || { echo "REFUSED $2 FILE must be $CDATA/executor/NAME"; exit 4; }
  H=$XD/${1##*/}
  { [ ! -L "$H" ] && [ -f "$H" ]; } || { echo "REFUSED $1: missing, a link or not a plain file"; exit 4; }
  m=$(stat -c '%a %u' -- "$H") || { echo "REFUSED $1: cannot stat"; exit 4; }
  [ "${m% *}" = 600 ] || { echo "REFUSED $1: mode ${m% *}, want 600"; exit 4; }
  [ "${m#* }" = "$CUID" ] \
    || { echo "REFUSED $1: owner uid ${m#* }, want $CUID (the loader refuses another user)"; exit 4; }
  d=$(stat -c '%a %u' -- "$XD") || { echo "REFUSED $CDATA/executor: cannot stat"; exit 4; }
  [ "$d" = "700 $CUID" ] \
    || { echo "REFUSED $CDATA/executor: mode ${d% *} uid ${d#* }, want 700 uid $CUID"; exit 4; }
}
ours() { # ours FILE: a plain file holding exactly the two keys this script writes
  { [ ! -L "$1" ] && [ -f "$1" ]; } && python3 -I -c '
import json, sys
d = json.load(open(sys.argv[1]))
sys.exit(0 if isinstance(d, dict) and sorted(d) == ["credentials_file", "operations"] else 1)' "$1" \
    2>/dev/null
}
case $cmd in
backup)
  TS=${2:?TS}; B=/root/backups/hypertrain-test-s7-$TS
  [ ! -e "$B" ] || { echo "exists: $B"; exit 4; }
  install -d -m 0700 "$B"; cp -p "$F" "$T/compose.relay.yml" "$B/"
  cp -p /data/relay/config/relay.json /data/hypertrain-test/config/metagraph.json "$B/"
  python3 -I - "$STATE" "$B" <<'EOF' || { echo "DB backup not ok"; exit 6; }
import sqlite3, sys
state, out = sys.argv[1:]
ok = True
for name in ("challenge.db", "server_keys.db"):
    src = sqlite3.connect(f"file:{state}/{name}?mode=ro", uri=True)
    dst = sqlite3.connect(f"{out}/{name}")
    src.backup(dst)
    result = dst.execute("pragma integrity_check").fetchone()[0]
    print(name, result)
    ok = ok and result == "ok"
    dst.close(); src.close()
sys.exit(0 if ok else 6)
EOF
  chmod 0600 "$B"/*.db
  docker inspect "$C" hypertrain-test-beacon hypertrain-test-master-stub hypertrain-test-relay \
    | python3 -I -c '
import json,sys
cs=json.load(sys.stdin)
for c in cs: c["Config"]["Env"]=sorted(e.split("=",1)[0] for e in c["Config"].get("Env") or [])
json.dump(cs,sys.stdout,indent=1)' > "$B/inspect-before.json"
  (cd "$B" && sha256sum compose.test.yml compose.relay.yml challenge.db server_keys.db \
    relay.json metagraph.json inspect-before.json > SHA256SUMS && cat SHA256SUMS); echo "backup $B";;
swap)
  NEW=${2:?NEW_REF}; B=${3:?BACKUP}; backup_dir "$B"; gate
  [[ $NEW =~ ^ghcr\.io/cortexlm/cortex/hypertrain@sha256:[0-9a-f]{64}$ ]] || { echo "bad ref"; exit 4; }
  [ "$(docker image inspect "$NEW" --format '{{.Id}}')" = "${NEW#*@}" ] || { echo "image not pulled"; exit 4; }
  grep -c '^    container_name: hypertrain-test-challenge$' "$F" | grep -qx 1 || { echo "anchor"; exit 4; }
  # a second swap finds the image line the first one added: drop it, or the key would be defined twice
  umask 077; sed '/^    container_name: hypertrain-test-challenge$/{n;/^    image: /d}' "$F" \
    | sed "/^    container_name: hypertrain-test-challenge\$/a\\    image: $NEW" > "$F.new"
  apply challenge "$B" services.challenge.image
  images "$F"; live "${NEW#*@}"; sleep 6; date -u +%FT%TZ; check;;
beacon)
  B=${2:?BACKUP}; backup_dir "$B"; gate
  umask 077; sed -E -e 's#--interval, "(3|0\.2)",$#--interval, "0.2",#' \
    -e 's#--catch-up, "(20|3|60)", --max-failures#--catch-up, "60", --max-failures#' \
    -e 's#(--max-failures, ")([0-9]|[1-9][0-9])"#\1100"#' "$F" > "$F.new"
  apply beacon "$B" +services.beacon.entrypoint.9 +services.beacon.entrypoint.11 \
    +services.beacon.entrypoint.13
  sleep 20; docker logs --since 25s hypertrain-test-beacon 2>&1 | tail -4 | cut -c1-160
  curl -s -m 10 "$API/v1/beacon/latest" | python3 -I -c '
import json,sys,time
r=json.load(sys.stdin)["round"]; print("lag", (int(time.time())-1692803367)//3+1-r)';;
paper)
  B=${2:?BACKUP}; backup_dir "$B"; backup_names "$B"; images "$B/compose.test.yml"
  echo "live: $(docker inspect "$C" --format '{{.Config.Image}}')"
  echo "would run: cp -p $B/compose.test.yml $F && docker compose -f $F up -d --no-deps challenge beacon";;
rollback)
  B=${2:?BACKUP}; backup_dir "$B"; backup_names "$B"; date -u +%FT%TZ
  OLD=$(python3 -I -c 'import json,sys;print(next(c["Image"] for c in json.load(open(sys.argv[1]))
    if c["Name"]=="/hypertrain-test-challenge"))' "$B/inspect-before.json")
  cp -p "$B/compose.test.yml" "$F"; compose "$F" up -d --no-deps challenge beacon 2>&1 | tail -3
  live "$OLD"
  for pair in "relay.json:/data/relay/config/relay.json:hypertrain-test-relay" \
      "metagraph.json:/data/hypertrain-test/config/metagraph.json:hypertrain-test-master-stub"; do
    IFS=: read -r name path container <<< "$pair"
    if [ -f "$B/$name" ] && ! cmp -s "$B/$name" "$path"; then
      cat "$B/$name" > "$path"; docker restart "$container"; echo "restored $path"; fi
  done
  sleep 6; check;;
restore-db)
  B=${2:?BACKUP}; backup_dir "$B"; A=$B/restore-db-aside-$(date -u +%Y%m%dT%H%M%SZ)
  install -d -m 0700 "$A"; cp -p "$STATE"/challenge.db* "$STATE"/server_keys.db* "$A/"
  docker stop "$C"
  for n in challenge.db server_keys.db; do
    rm -f "$STATE/$n-wal" "$STATE/$n-shm"; install -m 0644 -o 65532 -g 65532 "$B/$n" "$STATE/$n"; done
  docker start "$C"; sleep 6; check;;
executor)
  B=${2:?BACKUP}; P=${3:?CONTAINER_CONFIG_PATH}; shift 3
  r2=; ops=audit,referee; opsset=0; dry=0
  while [ $# -gt 0 ]; do
    case $1 in
    --r2) [ $# -ge 2 ] && [ -z "$r2" ] || usage "--r2 FILE, once"; r2=$2; shift 2;;
    --operations) [ $# -ge 2 ] && [ $opsset = 0 ] || usage "--operations LIST, once"
      ops=$2; opsset=1; shift 2;;
    --dry-run) [ $dry = 0 ] || usage "--dry-run, once"; dry=1; shift;;
    *) usage "executor BACKUP PATH [--r2 FILE [--operations LIST]] [--dry-run]";;
    esac
  done
  [ -n "$r2" ] || [ $opsset = 0 ] || usage "--operations needs --r2"
  [[ $ops =~ ^(reference|audit|referee)(,(reference|audit|referee))*$ ]] \
    || usage "--operations is a comma list of audit, referee, reference"
  backup_dir "$B"; gate_rc=0
  if [ $dry = 1 ]; then (gate) || gate_rc=$?; else gate; fi # a dry run reports a closed gate and goes on
  [[ $P =~ ^/data/[A-Za-z0-9._/-]+\.json$ ]] || { echo "PATH must be /data/....json in the container"; exit 4; }
  O=$T/compose.executor.yml; ON=$O.new; W=$B; umask 077
  if [ $dry = 1 ]; then W=$(mktemp -d "$T/dry.XXXXXX"); ON=$O.dry; fi # no file of the real run
  trap 'rm -f "$ON"; [ "$W" = "$B" ] || rm -rf "$W"' EXIT
  vars=(services.challenge.environment.HYPERTRAIN_CUDA_EXECUTOR)
  if [ -n "$r2" ]; then
    r2_file "$r2" --r2
    if [ -e "$R2F" ] || [ -L "$R2F" ]; then
      ours "$R2F" || { echo "REFUSED $R2S: not a file this script wrote; move it away"; exit 4; }
    fi
    json=$(python3 -I -c '
import json, sys
print(json.dumps({"credentials_file": sys.argv[1], "operations": sys.argv[2].split(",")}, indent=1))' \
      "$r2" "$ops")
    vars+=(services.challenge.environment.HYPERTRAIN_CUDA_EXECUTOR_ARTIFACTS)
  fi
  printf 'services:\n  challenge:\n    environment:\n      HYPERTRAIN_CUDA_EXECUTOR: %s\n' "$P" > "$ON"
  [ -z "$r2" ] || printf '      HYPERTRAIN_CUDA_EXECUTOR_ARTIFACTS: %s\n' "$R2S" >> "$ON"
  { compose "$F" config --format json > "$W/config-before-executor.json" \
    && (cd "$T" && docker compose -f "$F" -f "$ON" config --format json) > "$W/config-after-executor.json"; } \
    || { echo "REFUSED $F: compose cannot resolve it with the override"; exit 4; }
  # the file alone is what swap, rollback and executor-off bring up later; the pair is what up gets now
  { test_names "$F" < "$W/config-before-executor.json" \
    && test_names "$F -f $ON" < "$W/config-after-executor.json"; } || exit 4
  if [ -n "$r2" ]; then
    # the checks below run in the running challenge: they say something about the new one only if it
    # is the image compose.test.yml resolves
    ref=$(python3 -I -c 'import json, sys
print(json.load(open(sys.argv[1]))["services"]["challenge"]["image"])' "$W/config-before-executor.json")
    [ "$(docker image inspect "$ref" --format '{{.Id}}')" = "$(docker inspect "$C" --format '{{.Image}}')" ] \
      || { echo "REFUSED: the running challenge is not on the image $F resolves; swap or rollback first"; exit 4; }
    run_py -i pre "$P" <<< "$json" \
      || { echo "REFUSED: the challenge did not accept these settings, or could not be asked; nothing written"; exit 4; }
  fi
  python3 -I - "$W/config-before-executor.json" "$W/config-after-executor.json" "${vars[@]}" <<'EOF' \
    || { echo "unexpected compose change"; exit 5; }
import json, sys
def flat(x, p=""):
    if isinstance(x, dict):
        return {k: v for n, c in x.items() for k, v in flat(c, f"{p}.{n}" if p else n).items()}
    if isinstance(x, list):
        return {k: v for i, c in enumerate(x) for k, v in flat(c, f"{p}.{i}").items()}
    return {p: x}
a, b = (flat(json.load(open(f))) for f in sys.argv[1:3])
changed = sorted(k for k in a.keys() | b.keys() if a.get(k) != b.get(k))
print("changed", changed)
sys.exit(0 if changed == sorted(sys.argv[3:]) else 5)
EOF
  # compose recreates the container only when its resolved config changed, and the challenge reads the
  # settings once per process: new settings behind an unchanged variable need --force-recreate
  force=
  if [ -n "$r2" ] && [ -e "$R2F" ] && ! printf '%s\n' "$json" | cmp -s - "$R2F"; then force=--force-recreate; fi
  if [ $dry = 1 ]; then
    echo "dry-run: nothing written, nothing recreated"
    echo "variables: $(sed -n 's/^      \([A-Z_]*\): .*/\1/p' "$ON" | tr '\n' ' ')"
    [ -z "$r2" ] || echo "settings keys: credentials_file operations; token file: mode 600, uid $CUID"
    echo "recreate: ${force:-only if the resolved config changes}"
    if [ "$gate_rc" = 0 ]; then echo "gate: open"; else echo "gate: closed, a real run would exit $gate_rc"; fi
    exit "$gate_rc"
  fi
  if [ -n "$r2" ]; then # the settings first: the override only points at a file that exists
    printf '%s\n' "$json" | install -m 0600 -o "$CUID" -g "$CUID" /dev/stdin "$R2F.new"
    mv "$R2F.new" "$R2F"
  fi
  mv "$ON" "$O"; date -u +%FT%TZ
  (cd "$T" && docker compose -f "$F" -f "$O" up -d --no-deps ${force:+"$force"} challenge 2>&1 | tail -3) \
    || { echo "up failed after the files were written: bash -s -- executor-off $B"; exit 7; }
  (live "$(docker inspect "$C" --format '{{.Image}}')") \
    || { echo "the challenge is not up as asked: bash -s -- executor-off $B"; exit 7; }
  want=-; [ -z "$r2" ] || want=$R2S
  run_py post "$P" "$want" \
    || { echo "the recreated challenge does not carry the settings: bash -s -- executor-off $B"; exit 7; }
  if [ -z "$r2" ] && [ -e "$R2F" ] && ours "$R2F"; then rm -f "$R2F" "$R2F.new"; echo "removed $R2S"; fi
  sleep 6; check;;
executor-off)
  B=${2:?BACKUP}; backup_dir "$B"
  cfg=$(compose "$F" config --format json) && test_names "$F" <<< "$cfg" || exit 4
  date -u +%FT%TZ
  rm -f "$T/compose.executor.yml"; compose "$F" up -d --no-deps challenge 2>&1 | tail -3
  live "$(docker inspect "$C" --format '{{.Image}}')"
  run_py post - - || { echo "the recreated challenge still carries the variables: see $F"; exit 7; }
  # last, so that a failed up leaves the running challenge a settings file to read
  if [ -e "$R2F" ] || [ -L "$R2F" ]; then
    if ours "$R2F"; then rm -f "$R2F" "$R2F.new"; echo "removed $R2S"; else echo "kept $R2S: not written by this script"; fi
  fi
  sleep 6; check;;
r2-check)
  shift; file=; node=()
  while [ $# -gt 0 ]; do
    case $1 in
    --file) [ $# -ge 2 ] && [ -z "$file" ] || usage "--file FILE, once"; file=$2; shift 2;;
    --node) [ $# -ge 3 ] && [ ${#node[@]} -eq 0 ] || usage "--node LABEL sha256:DIGEST, once"
      [[ $2 =~ ^[a-z0-9][a-z0-9-]{0,31}$ && $3 =~ ^sha256:[0-9a-f]{64}$ ]] \
        || usage "--node LABEL sha256:<64 hex>"
      node=("$2" "$3"); shift 3;;
    *) usage "r2-check [--file FILE] [--node LABEL sha256:DIGEST]";;
    esac
  done
  [ -z "$file" ] || r2_file "$file" --file
  run_py roundtrip "${file:--}" ${node[@]+"${node[@]}"} || { echo "r2-check did not complete"; exit 8; };;
*) echo "unknown $cmd"; exit 2;;
esac
