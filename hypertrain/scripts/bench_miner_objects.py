"""Loopback benchmark of the miner's object traffic in an admission trial (D94 lot B, ADR 0017).

Times the probe's object POSTs (its four published files) and the trial input GETs against a
stdlib HTTP server on 127.0.0.1 that holds every object request `--latency` seconds, through the
NetworkMiner code of whichever `hypertrain` is imported: run it once on this tree and once with
PYTHONPATH=<older tree>/src for a before/after pair (each tree is driven the way its own probe
and trial fetch call it). The server checks that each POSTed object hashes to its route and
prints the multiset of what it received: two runs printing the same `posted_sha256` put the same
objects on the wire. It is not the master (no signature is checked) and nothing leaves the host.

    uv run --no-sync python scripts/bench_miner_objects.py --work <directory on disk>
"""

from __future__ import annotations

import argparse
import hashlib
import json
import random
import shutil
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any

import httpx

import hypertrain
import hypertrain.miner.core as core
from hypertrain.protocol.keys import Keypair

RUN_ID = hashlib.sha256(b"bench-miner-objects").hexdigest()
URL = f"/v2/runs/{RUN_ID}"
OBJECTS = URL + "/objects/"


class Server(ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self, latency: float, inputs: dict[str, Path]) -> None:
        super().__init__(("127.0.0.1", 0), Handler)
        self.latency, self.inputs = latency, inputs
        self.lock, self.now, self.peak = threading.Lock(), 0, 0
        self.posted: list[tuple[str, int]] = []

    def flight(self, step: int) -> None:
        with self.lock:
            self.now += step
            self.peak = max(self.peak, self.now)


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"  # keep-alive, as the miner's pooled client expects
    server: Server

    def log_message(self, format: str, *args: Any) -> None:
        """Quiet: thousands of requests per run."""

    def do_GET(self) -> None:
        if self.path == URL:
            return self._json({"now_round": 5})
        path = self.server.inputs.get(self.path.removeprefix(OBJECTS))
        if path is None:
            return self._json({"detail": "object not found"}, 404)
        self.server.flight(1)
        try:
            time.sleep(self.server.latency)
            self.send_response(200)
            self.send_header("Content-Length", str(path.stat().st_size))
            self.end_headers()
            with path.open("rb") as f:
                shutil.copyfileobj(f, self.wfile, 1 << 20)
        finally:
            self.server.flight(-1)

    def do_POST(self) -> None:
        digest = self.path.removeprefix(OBJECTS)
        self.server.flight(1)
        try:
            time.sleep(self.server.latency)
            body = self.rfile.read(int(self.headers["Content-Length"]))
        finally:
            self.server.flight(-1)  # before the answer: the client may send its next one then
        data = body[4 + int.from_bytes(body[:4], "big") :]
        if hashlib.sha256(data).hexdigest() != digest:
            return self._json({"detail": "object sha256 mismatch"}, 422)
        with self.server.lock:
            self.server.posted.append((digest, len(data)))
        self._json({"sha256": digest})

    def _json(self, value: object, status: int = 200) -> None:
        raw = json.dumps(value).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)


class Miner(core.NetworkMiner):
    """NetworkMiner's object routes without its manifest read (the fake server holds none)."""

    def __init__(self, client: httpx.Client, base: str, directory: Path) -> None:
        self.api = core.Api(client, base)
        self.kp = Keypair(hashlib.sha256(b"bench-miner-objects-hotkey").digest())
        self.run_id, self.url, self.directory = RUN_ID, URL, directory


def write(folder: Path, sizes: dict[str, int]) -> list[Path]:
    """One file per name, reproducible bytes: every run publishes the same objects."""
    folder.mkdir(parents=True, exist_ok=True)
    paths = []
    for name, size in sizes.items():
        rng, path = random.Random(name), folder / name
        with path.open("wb") as f:
            for start in range(0, size, 64 << 20):
                f.write(rng.randbytes(min(64 << 20, size - start)))
        paths.append(path)
    return paths


def published(params: int, small: int) -> dict[str, int]:
    """The probe's four files at `params` f32 parameters: state (theta, m, v), ef, int8 delta and
    leaves; 100,676,352 gives 1639 objects, 1.71 GB (S17b counted 1640, about 1.7 GB). `small`
    instead gives that many 1 KiB files, one plain object each."""
    if small:
        return {f"object-{n:05d}": 1024 for n in range(small)}
    return {
        "state.safetensors": 12 * params,
        "ef.safetensors": 4 * params,
        "delta.bin": params,
        "leaves.json": 4096,
    }


def trial_inputs(params: int) -> dict[str, int]:
    """The five trial inputs: start_state and ef_in are f32 tensors (805 MB together at 101 M),
    the rest small."""
    return {
        "start_state": 4 * params,
        "ef_in": 4 * params,
        "v0": 64,
        "samples": 1 << 20,
        "sample_proofs": 64 << 10,
    }


def post(miner: Miner, paths: list[Path]) -> str:
    put = miner._bounded_put(time.time() + 3600, None)
    upload = getattr(core, "_upload_refs", None)
    if upload is not None:
        upload(paths, put)
        return "one publication window (_upload_refs)"
    for path in paths:  # the probe before ADR 0017: one window per file, file after file
        core._upload_ref(path, put)
    return "one file at a time (_upload_ref)"


def get(miner: Miner, wanted: dict[Path, str]) -> str:
    fetch = getattr(miner, "_get_all", None)
    if fetch is not None:
        fetch(wanted)
        return "side by side (_get_all)"
    for target, digest in wanted.items():  # _fetch_trial before ADR 0017
        miner._get_whole(digest, target)
    return "one after another (_get_whole)"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--work", type=Path, required=True, help="scratch directory on disk")
    parser.add_argument("--latency", type=float, default=0.02, help="seconds per object request")
    parser.add_argument("--params", type=int, default=100_676_352, help="model parameters")
    parser.add_argument("--small", type=int, default=0, help="N 1 KiB objects instead")
    args = parser.parse_args()
    files = write(args.work / "published", published(args.params, args.small))
    sources = write(args.work / "inputs", trial_inputs(args.params))
    inputs = {hashlib.file_digest(p.open("rb"), "sha256").hexdigest(): p for p in sources}
    fetched = args.work / "fetched"
    shutil.rmtree(fetched, ignore_errors=True)
    fetched.mkdir()
    server = Server(args.latency, inputs)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    report: dict[str, Any] = {
        "hypertrain": str(Path(hypertrain.__file__).parent),
        "latency_s": args.latency,
    }
    with httpx.Client(timeout=120, follow_redirects=False) as client:  # as the miner CLI
        miner = Miner(client, f"http://127.0.0.1:{server.server_port}", args.work)
        began = time.monotonic()
        how = post(miner, files)
        seconds, server.peak = time.monotonic() - began, 0
        posted = sorted(server.posted)
        report["post"] = {
            "how": how,
            "seconds": round(seconds, 2),
            "objects": len(posted),
            "distinct": len(set(posted)),
            "bytes": sum(size for _, size in posted),
            "posted_sha256": hashlib.sha256(json.dumps(posted).encode()).hexdigest(),
        }
        wanted = {fetched / path.name: digest for digest, path in inputs.items()}
        began = time.monotonic()
        how = get(miner, wanted)
        seconds = time.monotonic() - began
        for target, digest in wanted.items():
            if hashlib.file_digest(target.open("rb"), "sha256").hexdigest() != digest:
                raise SystemExit(f"{target.name}: fetched bytes differ")
        report["get"] = {
            "how": how,
            "seconds": round(seconds, 2),
            "objects": len(wanted),
            "bytes": sum(target.stat().st_size for target in wanted),
            "peak_in_flight": server.peak,
        }
    server.shutdown()
    print(json.dumps(report, indent=1))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
