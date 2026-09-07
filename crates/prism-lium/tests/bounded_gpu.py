#!/usr/bin/env python3
"""Stdlib staging harness. --self-test is local only; --preflight NEVER rents.

ponytail: live writes stay disabled until Lium supplies an atomic <=20 minute
expiry, a binding total-price cap, and a per-pod final billing contract. A local
watchdog cannot guarantee these during host/network failure. No override flag.
"""
import argparse
import json
import math
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

IMAGE = "ghcr.io/cortexlm/proof-eval@sha256:9e32451e178a2e04f33be592ea73f7f89acf723631d53291dbab016c4690ee92"
BASE = "https://lium.io/api"

# Run inside the *verified running container*, not the host/template. No secrets.
# This proves only CUDA tensor execution and invalid-input refusal, not science.
GPU_PROBE = r'''
import json, pathlib, subprocess, tempfile
import torch
driver = subprocess.check_output(["nvidia-smi", "--query-gpu=name,driver_version",
                                 "--format=csv,noheader"], text=True).strip()
if not torch.cuda.is_available() or torch.cuda.device_count() != 1:
    raise RuntimeError("exactly one usable CUDA GPU required")
torch.backends.cuda.matmul.allow_tf32 = False
x = torch.arange(4096, dtype=torch.float32).reshape(64, 64) / 4096
actual = (x.cuda() @ x.T.cuda()).cpu()
torch.cuda.synchronize()
torch.testing.assert_close(actual, x @ x.T, rtol=1e-4, atol=1e-5)
with tempfile.TemporaryDirectory() as d:
    request, out = pathlib.Path(d)/"request.json", pathlib.Path(d)/"metrics.json"
    request.write_text("{}")
    p = subprocess.run(["proof-eval", "score", "--request", str(request),
                        "--out", str(out)], capture_output=True, text=True, timeout=30)
    if p.returncode != 2 or "refused:" not in p.stderr or out.exists():
        raise RuntimeError("expected fail-closed invalid-request refusal")
print(json.dumps(dict(driver_gpu=driver, torch=torch.__version__,
                     cuda=torch.version.cuda, tensor_cpu_reference=True,
                     invalid_request_refused=True, science_validated=False)))
'''


class Refused(RuntimeError):
    pass


def require(condition, message):
    if not condition:
        raise Refused(message)


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):
        return None  # Never forward X-API-Key to another endpoint.


class API:
    def __init__(self, base, key_file, fake=False):
        parsed = urllib.parse.urlsplit(base)
        self.fake = fake and parsed.hostname == "127.0.0.1" and parsed.scheme == "http"
        require(base == BASE or self.fake, "unapproved API origin")
        key_path = Path(key_file)
        require(key_path.is_file() and key_path.stat().st_mode & 0o077 == 0,
                "key file must be private (0600 or stricter)")
        self.key = key_path.read_text().strip()
        require(bool(self.key) and not any(c.isspace() for c in self.key), "invalid key")
        self.base = base
        self.opener = urllib.request.build_opener(NoRedirect)

    def request(self, method, path, body=None):
        require(method == "GET" or self.fake, "live writes disabled: unverified budget/expiry contract")
        request = urllib.request.Request(self.base + path, method=method,
            headers={"X-API-Key": self.key, "Content-Type": "application/json"},
            data=None if body is None else json.dumps(body).encode())
        try:
            with self.opener.open(request, timeout=3) as response:
                raw = response.read(2_000_001)
                require(len(raw) <= 2_000_000, "oversized API response")
                return json.loads(raw) if raw else None
        except urllib.error.HTTPError as error:
            if method in ("GET", "DELETE") and error.code == 404:
                return None
            raise Refused(f"API HTTP {error.code}; body withheld") from None
        except (OSError, ValueError):
            raise Refused("API transport/JSON failure; acknowledgement uncertain") from None


def rows(value, key):
    value = value.get(key) if isinstance(value, dict) else value
    require(isinstance(value, list) and all(isinstance(x, dict) for x in value),
            "malformed list: never treat as absence")
    return value


def choose_offer(value, max_price):
    candidates = []
    for row in rows(value, "executors"):
        try:
            price = float(row["price_per_gpu"])
            compatible = float(row["max_cuda_version"]) >= 13.0
        except (KeyError, TypeError, ValueError):
            continue
        if (row.get("gpu_count") == 1 and row.get("available_gpu_count") == 1
                and row.get("is_whole_host_free") is True
                and row.get("has_no_pending_rental") is True
                and row.get("active") is not False
                and row.get("min_gpu_count_for_rental") in (None, 1)
                and row.get("pending_price_per_hour") is None
                and math.isfinite(price) and 0 < price <= max_price and compatible):
            candidates.append((price, row))
    require(bool(candidates), "no eligible single-GPU whole-host CUDA 13 offer")
    return min(candidates, key=lambda pair: pair[0])[1]


def template_readback(value, template_id, image):
    matches = [r for r in rows(value, "templates") if r.get("id") == template_id]
    require(len(matches) == 1 and matches[0].get("docker_image") == image,
            "template must read back exact full repository@digest")


def runtime_provenance(container, image, expected):
    """Inputs must come from host docker inspect + docker image inspect over SSH.

    Container self-reported env/template pin is not provenance. This validates
    the active image ID against the engine's resolved digest, not an attestation.
    """
    require(container.get("State", {}).get("Running") is True
            and container.get("Image") == image.get("Id")
            and expected in image.get("RepoDigests", []),
            "running container image ID/digest provenance missing or mismatched")


def event(journal, **fields):
    with open(journal / "events.jsonl", "a", encoding="utf-8") as out:
        out.write(json.dumps(dict(at=time.time(), **fields)) + "\n")
        out.flush()
        os.fsync(out.fileno())


def intent(journal, seconds):
    os.mkdir(journal, 0o700)  # Refuse reused journals/names; never overwrite.
    record = dict(name="proof-gpu-" + uuid.uuid4().hex, deadline=time.time() + seconds)
    with open(journal / "intent.json", "x", encoding="utf-8") as out:
        json.dump(record, out)
        out.flush()
        os.fsync(out.fileno())
    for directory in (journal, journal.parent):
        fd = os.open(directory, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(fd)
        finally:
            os.close(fd)
    return record


def cleanup(api, journal, record, seconds=3):
    # Keep reconciling even after initial absence: uncertain rent may appear late.
    end = time.monotonic() + seconds
    absent = False
    while time.monotonic() < end:
        try:
            owned = [p for p in rows(api.request("GET", "/pods"), "pods")
                     if p.get("name", p.get("pod_name")) == record["name"]]
            absent = not owned
            for pod in owned:
                ident = pod.get("id")
                require(isinstance(ident, str) and re.fullmatch(r"[A-Za-z0-9-]+", ident), "invalid owned pod id")
                path = "/pods/" + ident
                detail = api.request("GET", path)
                if detail is None:
                    continue
                require(detail.get("name", detail.get("pod_name")) == record["name"], "pod ownership mismatch")
                api.request("DELETE", path)
                event(journal, action="delete", readback_absent=api.request("GET", path) is None)
            event(journal, action="reconcile", list_absent=absent,
                  billing_stopped_proven=False)
        except Refused:
            absent = False
            event(journal, action="cleanup_unconfirmed", billing_stopped_proven=False)
        time.sleep(0.1)
    event(journal, action="cleanup_finished", list_absent=absent,
          billing_stopped_proven=False, late_rent_excluded=False)
    return absent


def watchdog(api, journal):
    record = json.loads((journal / "intent.json").read_text())
    event(journal, action="watchdog_ready")
    while time.time() < record["deadline"]:
        time.sleep(min(0.1, record["deadline"] - time.time()) if time.time() < record["deadline"] else 0)
    return cleanup(api, journal, record)


def fake_run(base, key_file, journal, crash=False):
    api = API(base, key_file, fake=True)
    record = intent(journal, 0.5)
    # Independent session/process with no inherited credential contents or pipes.
    child = subprocess.Popen([sys.executable, str(Path(__file__).resolve()),
        "--fake-watchdog", base, str(key_file), str(journal)], start_new_session=True,
        stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        limit = time.monotonic() + 3
        while not (journal / "events.jsonl").exists():
            require(child.poll() is None and time.monotonic() < limit, "watchdog failed to arm")
            time.sleep(0.01)
        require(time.time() < record["deadline"], "watchdog deadline already reached")
        event(journal, action="rent_intended", image=IMAGE, hourly_usd=0.32,
              budget_usd=30, expiry_seconds=0.5)
        # Exactly one POST. Any uncertainty proceeds to name reconciliation only.
        api.request("POST", "/executors/fake/rent", {"pod_name": record["name"]})
        if crash:
            os._exit(0)  # Test parent death, bypassing finally on purpose.
    finally:
        cleanup(api, journal, record, seconds=0.3)
    return child


def preflight(args):
    require(args.image == IMAGE, "only the user-approved digest is allowed")
    require(math.isfinite(args.budget_usd) and 0 < args.budget_usd <= 30, "budget must be <=30 USD")
    require(0 < args.max_seconds <= 1200, "runtime must be <=1200 seconds")
    require(math.isfinite(args.max_hourly_usd) and 0 < args.max_hourly_usd <= args.budget_usd,
            "hourly ceiling must fit budget even for one hour")
    api = API(BASE, args.key_file)
    schema = api.request("GET", "/openapi.json")
    properties = schema["components"]["schemas"]["RentExecutorRequest"]["properties"]
    offer = choose_offer(api.request("GET", "/executors"), args.max_hourly_usd)
    gpu = offer.get("specs", {}).get("gpu", {}).get("details", [{}])[0].get("name", "unknown")
    print(json.dumps(dict(gpu=gpu, hourly_usd=offer["price_per_gpu"],
        estimated_20min_usd=float(offer["price_per_gpu"])/3, estimate_not_cap=True,
        termination_schema=properties.get("termination_hours"),
        rent_fields=sorted(properties), live_writes_enabled=False)))
    if args.template_id:
        template_readback(api.request("GET", "/templates"), args.template_id, args.image)
    raise Refused("BLOCKED: rent has integer termination_hours, no verified atomic <=20min expiry or total-price cap; DELETE does not prove final billing")


def self_test():
    state = dict(pods=[dict(id="foreign", name="not-ours")], posts=0, deletes=[], uncertain=False)

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def reply(self, code, body):
            self.send_response(code)
            self.end_headers()
            self.wfile.write(json.dumps(body).encode())

        def do_GET(self):
            if self.path == "/pods":
                return self.reply(200, state["pods"])
            pod = next((p for p in state["pods"] if self.path == "/pods/" + p["id"]), None)
            self.reply(200 if pod else 404, pod)

        def do_POST(self):
            state["posts"] += 1
            body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            pod = dict(id="owned-" + str(state["posts"]), name=body["pod_name"])
            if state["uncertain"]:
                threading.Timer(0.7, lambda: state["pods"].append(pod)).start()
                return self.reply(503, {"secret": "must-not-be-logged"})
            state["pods"].append(pod)
            self.reply(200, pod)

        def do_DELETE(self):
            ident = self.path.rsplit("/", 1)[-1]
            state["deletes"].append(ident)
            state["pods"] = [p for p in state["pods"] if p["id"] != ident]
            self.reply(200, {})

    def refuses(fn):
        try:
            fn()
        except Refused:
            return
        raise AssertionError("expected refusal")

    with tempfile.TemporaryDirectory() as tmp, ThreadingHTTPServer(("127.0.0.1", 0), Handler) as server:
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        base = "http://127.0.0.1:" + str(server.server_port)
        key_file = Path(tmp) / "key"
        key_file.write_text("fake-secret")
        key_file.chmod(0o600)
        try:
            offer = dict(gpu_count=1, available_gpu_count=1, price_per_gpu=0.32,
                         max_cuda_version=13.1, is_whole_host_free=True, has_no_pending_rental=True)
            assert choose_offer([dict(offer, price_per_gpu=1), offer], 2) == offer
            for bad in [dict(offer, gpu_count=8), dict(offer, price_per_gpu=float("nan")),
                        dict(offer, max_cuda_version=12.8), dict(offer, is_whole_host_free=False)]:
                refuses(lambda: choose_offer([bad], 2))
            refuses(lambda: rows({}, "pods"))
            template_readback([dict(id="t", docker_image=IMAGE)], "t", IMAGE)
            refuses(lambda: template_readback([dict(id="t", docker_image="repo:latest")], "t", IMAGE))
            runtime_provenance(dict(State=dict(Running=True), Image="sha256:abc"),
                               dict(Id="sha256:abc", RepoDigests=[IMAGE]), IMAGE)
            refuses(lambda: runtime_provenance(dict(State=dict(Running=True), Image="wrong"),
                                               dict(Id="sha256:abc", RepoDigests=[IMAGE]), IMAGE))
            refuses(lambda: API(BASE, key_file).request("POST", "/pods", {}))
            for index, mode in enumerate(["normal", "uncertain", "crash"]):
                journal = Path(tmp) / str(index)
                state["uncertain"] = mode == "uncertain"
                if mode == "crash":
                    subprocess.run([sys.executable, str(Path(__file__).resolve()),
                        "--fake-crash", base, str(key_file), str(journal)], check=True, timeout=5)
                else:
                    try:
                        fake_run(base, key_file, journal)
                    except Refused:
                        assert mode == "uncertain"
                deadline = time.monotonic() + 7
                while time.monotonic() < deadline:
                    events = (journal / "events.jsonl").read_text()
                    if '"action": "cleanup_finished"' in events and time.time() > json.loads((journal / "intent.json").read_text())["deadline"] + 3.2:
                        break
                    time.sleep(0.05)
                else:
                    raise AssertionError("watchdog did not finish")
                assert state["pods"] == [dict(id="foreign", name="not-ours")]
                assert "fake-secret" not in events and "must-not-be-logged" not in events
            assert state["posts"] == 3 and "foreign" not in state["deletes"]
        finally:
            server.shutdown()
            thread.join()
    print("PASS: offer/pin/provenance guards, live-write refusal, one-shot uncertain rent, late reconciliation, parent-death watchdog, owned-only cleanup, secret redaction")


def main():
    os.umask(0o077)
    if len(sys.argv) == 5 and sys.argv[1] in ("--fake-watchdog", "--fake-crash"):
        _, mode, base, key, journal = sys.argv
        if mode == "--fake-watchdog":
            return watchdog(API(base, key, fake=True), Path(journal))
        return fake_run(base, key, Path(journal), crash=True)
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--self-test", action="store_true")
    mode.add_argument("--preflight", action="store_true")
    parser.add_argument("--key-file", type=Path)
    parser.add_argument("--image", default=IMAGE)
    parser.add_argument("--max-hourly-usd", type=float, default=1)
    parser.add_argument("--budget-usd", type=float, default=30)
    parser.add_argument("--max-seconds", type=int, default=1200)
    parser.add_argument("--template-id")
    args = parser.parse_args()
    if args.self_test:
        return self_test()
    require(args.key_file is not None, "explicit --key-file required")
    preflight(args)


if __name__ == "__main__":
    try:
        main()
    except Refused as error:
        print(str(error), file=sys.stderr)
        sys.exit(2)
