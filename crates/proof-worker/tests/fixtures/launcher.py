#!/usr/bin/python3
"""No inference: exercise controller process supervision and Unix IPC."""
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import time


MARKER = "cortex-fixture-" + os.urandom(6).hex()

def descendant(*, detached=False, graceful=False, hold_stdout=True, escape=False):
    ready, notify = os.pipe()
    script = """
import os, signal, sys, time
from pathlib import Path
if sys.argv[2] == "escape":
    # Leave the supervised process group entirely; only a PID namespace
    # can still take this process down with the launcher.
    os.setsid()
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
elif sys.argv[2] == "graceful":
    def stop(*_):
        time.sleep(0.25)
        Path("descendant-cleaned").write_text("done")
        sys.exit(0)
    signal.signal(signal.SIGTERM, stop)
else:
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
os.write(int(sys.argv[1]), b"1")
os.close(int(sys.argv[1]))
time.sleep(60)
"""
    child = subprocess.Popen(
        ["/usr/bin/python3", "-c", script, str(notify),
         "escape" if escape else "graceful" if graceful else "ignore", MARKER],
        pass_fds=(notify,),
        stdin=None if not detached else subprocess.DEVNULL,
        stdout=None if not detached and hold_stdout else subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    os.close(notify)
    assert os.read(ready, 1) == b"1"
    os.close(ready)
    return child


# This mode deliberately never reads the launch input. Its descendant retains
# the pipe reader after the launcher exits, so a large write cannot finish.
if Path("hold-stdin").exists():
    child = descendant(hold_stdout=False)
    Path("processes.json").write_text(json.dumps({
        "pid": os.getpid(), "descendant": child.pid,
    }))
    os._exit(0)

config = json.load(sys.stdin)
mode = config["prompt"]
counts = {"calls": 1, "reservedTokens": 10, "reservedMicroUsd": 0, "children": 0}
report = {
    "pid": os.getpid(),
    "environment": sorted(os.environ),
    "runtime_id": config["runtime_id"],
    "deadline_ms": config["deadline_ms"],
    "resume": config["resume"],
}
if mode == "ignore":
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
if mode in (
    "ignore", "orphan_detached", "orphan_stdout", "failed_orphan",
    "shutdown_child_exits", "shutdown_graceful", "escape_group",
):
    child = descendant(
        detached=mode in ("orphan_detached", "escape_group"),
        graceful=mode == "shutdown_graceful",
        escape=mode == "escape_group",
    )
    report["descendant"] = child.pid
    # Inside a PID namespace these ids are namespaced; the test locates the
    # host processes through this marker instead.
    report["marker"] = MARKER
body = json.dumps({
    "schema_version": 1, "scope": config["scope"], "operation": "report", "arguments": report,
}).encode()
client = socket.socket(socket.AF_UNIX)
client.connect(config["controller_socket"])
client.sendall(
    b"POST /call HTTP/1.1\r\nHost: private\r\nContent-Type: application/json\r\n"
    + f"Content-Length: {len(body)}\r\nConnection: close\r\n\r\n".encode() + body
)
while client.recv(4096):
    pass
client.close()
print(json.dumps({"schema_version": 1, "event": "started", "restored": config["resume"], "counts": counts}), flush=True)
if mode in ("wait", "ignore", "shutdown_child_exits", "shutdown_graceful", "escape_group"):
    time.sleep(60)
elif mode == "oversized":
    print("x" * 10000, flush=True)
elif mode == "extra":
    print(json.dumps({"secret": "synthetic-redaction-sentinel"}), flush=True)
elif mode == "wrong_resume":
    print(json.dumps({"schema_version": 1, "event": "started", "restored": not config["resume"], "counts": counts}), flush=True)
elif mode == "failed_orphan":
    print(json.dumps({"schema_version": 1, "event": "failed", "code": "model_failed", "counts": counts}), flush=True)
    sys.exit(1)
else:
    print(json.dumps({"schema_version": 1, "event": "stopped", "reason": "completed", "counts": counts}), flush=True)
print("synthetic stderr must remain private", file=sys.stderr)
