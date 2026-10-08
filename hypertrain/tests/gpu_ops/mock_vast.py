"""Loopback mock of the Vast endpoints the launcher uses. Enforces live canonical paths:
non-canonical trailing slash -> 308 (incl. POST /api/v0/instances/N/ssh/), deprecated
GET /api/v0/instances/ -> 410. Instances get a real local sshd with an exec-able "remote".

Fault scripting via CFG: list_429 (n list responses), list_malformed (n), put_429_created (bool),
show_429 (n), delete_429 (n). State at GET /__state.
"""

from __future__ import annotations

import json
import os
import re
import signal
import socket
import subprocess
import sys
import threading
import time
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any

CFG: dict[str, Any] = json.loads(Path(sys.argv[1]).read_text())
STATE_DIR = Path(sys.argv[2])
STATE_DIR.mkdir(mode=0o700, parents=True, exist_ok=True)
LOCK = threading.Lock()
S: dict[str, Any] = {
    "instances": {},
    "events": [],
    "put_count": 0,
    "next_id": 70000000,
    "list_429": CFG.get("list_429", 0),
    "list_malformed": CFG.get("list_malformed", 0),
    "show_429": CFG.get("show_429", 0),
    "delete_429": CFG.get("delete_429", 0),
}
CANON = {"/api/v0/users/current/", "/api/v0/bundles/", "/api/v1/instances/"}


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return int(s.getsockname()[1])


def spawn_sshd(iid: int) -> tuple[int, subprocess.Popen[bytes], Path]:
    d = STATE_DIR / f"sshd-{iid}"
    d.mkdir(mode=0o700)
    subprocess.run(
        ["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(d / "host")], check=True
    )
    (d / "authorized_keys").write_text("")
    os.chmod(d / "authorized_keys", 0o600)
    port = free_port()
    (d / "sshd_config").write_text(
        f"ListenAddress 127.0.0.1\nPort {port}\nHostKey {d}/host\nPidFile {d}/pid\n"
        f"AuthorizedKeysFile {d}/authorized_keys\nStrictModes no\n"
        "PermitRootLogin prohibit-password\n"
        "PasswordAuthentication no\nKbdInteractiveAuthentication no\nUsePAM yes\n"
        f"AllowUsers {os.environ.get('USER', 'root')}\nLogLevel ERROR\nMaxStartups 100\n"
    )
    log = open(d / "sshd.log", "ab")
    proc = subprocess.Popen(
        ["/usr/sbin/sshd", "-D", "-e", "-f", str(d / "sshd_config")],
        stdout=log,
        stderr=log,
        start_new_session=True,
    )
    for _ in range(400):
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=0.2):
                break
        except OSError:
            time.sleep(0.025)
    return port, proc, d


class H(BaseHTTPRequestHandler):
    def log_message(self, *a: Any) -> None:
        pass

    def reply(
        self,
        code: int,
        value: Any,
        headers: tuple[tuple[str, str], ...] = (),
        raw: bytes | None = None,
    ) -> None:
        body = raw if raw is not None else json.dumps(value).encode()
        self.send_response(code)
        for k, v in headers:
            self.send_header(k, v)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def handle_any(self, method: str) -> None:
        url = urllib.parse.urlsplit(self.path)
        path, query = url.path, urllib.parse.parse_qs(url.query)
        body = self.rfile.read(int(self.headers.get("Content-Length") or 0))
        with LOCK:
            S["events"].append(
                {"method": method, "path": path, "query": url.query, "unix": time.time()}
            )
            if path == "/__state":
                inst = {
                    i: {k: v for k, v in r.items() if k != "proc"}
                    for i, r in S["instances"].items()
                }
                return self.reply(
                    200, {**{k: v for k, v in S.items() if k != "instances"}, "instances": inst}
                )
            if self.headers.get("Authorization") != "Bearer " + CFG["key"]:
                return self.reply(401, {"success": False})
            alt = path[:-1] if path.endswith("/") else path + "/"
            if alt in CANON or re.fullmatch(r"/api/v0/(asks|instances)/\d+(/ssh)?", path):
                return self.reply(308, {"detail": "moved"}, (("Location", alt),))
            if path == "/api/v0/instances/" and method == "GET":
                return self.reply(410, {"success": False, "error": "deprecated_endpoint"})
            if path == "/api/v0/users/current/":
                return self.reply(200, {"id": CFG["account_id"], "credit": CFG["credit"]})
            if path == "/api/v0/bundles/":
                q = json.loads(query["q"][0])
                oid = q.get("id", {}).get("eq")
                return self.reply(
                    200, {"offers": [o for o in CFG["offers"] if oid is None or o["id"] == oid]}
                )
            if path == "/api/v1/instances/":
                if S["list_429"] > 0 and S["put_count"] > 0:
                    S["list_429"] -= 1
                    return self.reply(429, {"success": False}, (("Retry-After", "2"),))
                if S["list_malformed"] > 0 and S["put_count"] > 0:
                    S["list_malformed"] -= 1
                    return self.reply(
                        200, None, raw=b'{"success": true, "instances": [{"label": "x"}]'
                    )
                label = (
                    json.loads(query.get("select_filters", ["{}"])[0]).get("label", {}).get("eq")
                )
                rows = [
                    {"id": r["id"], "label": r["label"], "machine_id": r["machine_id"]}
                    for r in S["instances"].values()
                    if not r["deleted"] and (label is None or r["label"] == label)
                ]
                return self.reply(200, {"success": True, "instances": rows})
            parts = path.strip("/").split("/")
            if method == "PUT" and parts[:3] == ["api", "v0", "asks"]:
                S["put_count"] += 1
                now, prev = time.time(), S.get("last_put_unix")
                S["last_put_unix"] = now
                offer = next((o for o in CFG["offers"] if o["id"] == int(parts[3])), None)
                if offer is None:
                    return self.reply(404, {"success": False, "msg": "no_such_ask"})
                iid = S["next_id"]
                S["next_id"] += 1
                port, proc, d = spawn_sshd(iid)
                S["instances"][str(iid)] = {
                    "id": iid,
                    "label": json.loads(body)["label"],
                    "machine_id": offer["machine_id"],
                    "deleted": False,
                    "shows": 0,
                    "ssh_port": port,
                    "sshd_pid": proc.pid,
                    "dir": str(d),
                    "driver_version": offer.get("driver_version"),
                    "image": json.loads(body)["image"],
                }
                if CFG.get("put_429_created") and prev is None:
                    return self.reply(
                        429, {"success": False, "msg": "too frequent"}, (("Retry-After", "1"),)
                    )
                reply = {"success": True, "new_contract": iid}
                if CFG.get("put_secret"):
                    reply["instance_api_key"] = f"{iid:064x}"
                return self.reply(200, reply)
            if parts[:3] == ["api", "v0", "instances"] and len(parts) >= 4:
                r = S["instances"].get(parts[3])
                if method == "POST" and parts[4:] == ["ssh"]:
                    if not r or r["deleted"]:
                        return self.reply(404, {"success": False})
                    with open(Path(r["dir"]) / "authorized_keys", "a") as f:
                        f.write(json.loads(body)["ssh_key"] + "\n")
                    return self.reply(200, {"success": True})
                if method == "DELETE":
                    if S["delete_429"] > 0:
                        S["delete_429"] -= 1
                        return self.reply(429, {"success": False}, (("Retry-After", "1"),))
                    if not r or r["deleted"]:
                        return self.reply(404, {"success": False})
                    r["deleted"] = True
                    try:
                        os.killpg(r["sshd_pid"], signal.SIGTERM)
                    except ProcessLookupError:
                        pass
                    return self.reply(200, {"success": True})
                if method == "GET":
                    if S["show_429"] > 0:
                        S["show_429"] -= 1
                        return self.reply(429, {"success": False}, (("Retry-After", "1"),))
                    if not r or r["deleted"]:
                        return self.reply(200, {"instances": None})
                    r["shows"] += 1
                    return self.reply(
                        200,
                        {
                            "instances": {
                                "id": r["id"],
                                "label": r["label"],
                                "machine_id": r["machine_id"],
                                "actual_status": "running"
                                if r["shows"] > 1 and not CFG.get("never_running")
                                else "loading",
                                "cur_state": "running",
                                "ssh_host": "127.0.0.1",
                                "ssh_port": r["ssh_port"],
                                "driver_version": r["driver_version"],
                            }
                        },
                    )
            return self.reply(404, {"success": False, "error": "mock_unknown_endpoint"})

    def do_GET(self) -> None:
        self.handle_any("GET")

    def do_PUT(self) -> None:
        self.handle_any("PUT")

    def do_POST(self) -> None:
        self.handle_any("POST")

    def do_DELETE(self) -> None:
        self.handle_any("DELETE")


def shutdown(*_: Any) -> None:
    for r in S["instances"].values():
        try:
            os.killpg(r["sshd_pid"], signal.SIGKILL)
        except ProcessLookupError:
            pass
    os._exit(0)


signal.signal(signal.SIGTERM, shutdown)
server = ThreadingHTTPServer(("127.0.0.1", 0), H)
print("PORT", server.server_address[1], flush=True)
server.serve_forever()
