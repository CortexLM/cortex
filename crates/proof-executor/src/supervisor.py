"""Trusted PID 1. Workload output never becomes a supervisor instruction.

The workload uses a different uid, no network, no host mounts and no capabilities.
Exiting PID 1 kills the entire private PID namespace, including setsid descendants.
No script-authored metric or FLOP count is accepted as a trusted observation.
"""
import hashlib
import json
import os
import selectors
import signal
import subprocess
import sys
import time

payload = json.loads(sys.argv[1])
deadline = payload["deadline_ms"] / 1000
# A backwards wall-clock adjustment must not extend the original grant.
maximum = min(payload["timeout_ms"] / 1000, max(0, deadline - time.time()))
receipt = {
    "schema_version": 1,
    "seed": payload.get("seed"),
    "script_digest": None,
    "exit_code": None,
    "wall_ms": 0,
    "failure": None,
    "log": [],
    "flops_used": None,
    "metrics": None,
}
started = time.monotonic()
interrupted = False
log = bytearray()

def interrupt(_signum, _frame):
    global interrupted
    interrupted = True

signal.signal(signal.SIGTERM, interrupt)
try:
    if time.time() >= deadline:
        raise TimeoutError("deadline")
    kind = payload["kind"]
    env = {"PATH": "/usr/local/bin:/usr/bin:/bin", "HOME": "/work", "LANG": "C.UTF-8"}
    # The artifact volume is root-owned; the workload writes it, the controller reads it.
    if os.path.isdir("/work/artifact"):
        os.chmod("/work/artifact", 0o1777)
        env["PROOF_ARTIFACT_DIR"] = "/work/artifact"
    if kind == "terminal":
        argv = payload["argv"]
    else:
        script = payload["script"].encode("utf-8")
        receipt["script_digest"] = hashlib.sha256(script).hexdigest()
        if receipt["script_digest"] != payload["script_digest"]:
            raise ValueError("commitment")
        with open("/run/proof/program.py", "xb") as out:
            out.write(script)
        os.chmod("/run/proof/program.py", 0o444)
        argv = ["/usr/local/bin/python", "-I", "-u", "/run/proof/program.py"]
        if receipt["seed"] is not None:
            argv.append(str(receipt["seed"]))
            env["PROOF_SEED"] = str(receipt["seed"])
    proc = subprocess.Popen(
        argv, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
        cwd="/work", env=env, user=65532, group=65532, extra_groups=[],
        start_new_session=True, umask=0o077,
    )
    os.set_blocking(proc.stdout.fileno(), False)
    selector = selectors.DefaultSelector()
    selector.register(proc.stdout, selectors.EVENT_READ)
    while True:
        elapsed = time.monotonic() - started
        if interrupted:
            receipt["failure"] = "interrupted"
            break
        if elapsed >= maximum or time.time() >= deadline:
            receipt["failure"] = "deadline"
            break
        # Check the process on every tick even if a descendant holds the pipe.
        exited = proc.poll()
        if selector.select(0.01):
            data = os.read(proc.stdout.fileno(), 4096)
            if len(log) + len(data) > 65536:
                log.extend(data[:65536 - len(log)])
                receipt["failure"] = "log_limit"
                break
            log.extend(data)
            if not data and exited is not None:
                receipt["exit_code"] = exited
                break
        elif exited is not None:
            receipt["exit_code"] = exited
            break
except TimeoutError:
    receipt["failure"] = "deadline"
except Exception:
    receipt["failure"] = "dependency_or_execution"
receipt["log"] = list(log)
receipt["wall_ms"] = max(1, int((time.monotonic() - started) * 1000 + 0.999))
# stdout belongs only to this root supervisor, not to the uid-65532 workload.
data = json.dumps(receipt, separators=(",", ":")).encode() + b"\n"
while data:
    data = data[os.write(1, data):]
os._exit(0)
