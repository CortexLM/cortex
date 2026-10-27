"""Doubles for the executor artifact transfer (docs/adr/0013-executor-artifacts-via-r2.md).

- `Clock`: one movable clock shared by the signer (`hypertrain.storage.r2._now`) and the fake.
- `TimedFakeR2`: tests/storage/fake_r2.FakeR2 behind one lock, with presigned URL expiry on that
  clock, faults aimed at one key, and a ledger of every authenticated request.
- `bridge`: the runner's one HTTP seam (`http_send`) wired straight to a fake, no socket.
- `serve_http`: the same fake behind a loopback HTTP/1.1 server, for the tests that must prove the
  stdlib client speaks real HTTP.
- `Challenge`: the challenge half of the mailbox contract written by hand, so the runner is tested
  against the contract and not against the broker that implements the other side.
"""

from __future__ import annotations

import datetime as dt
import hashlib
import http.server
import io
import json
import os
import random
import signal
import tarfile
import tempfile
import threading
import time
import uuid as uuidlib
from collections import deque
from collections.abc import Callable, Iterator, Mapping
from pathlib import Path
from typing import Any
from urllib.parse import parse_qsl, unquote

import httpx
import pytest
from tests.storage.fake_r2 import CFG, FakeR2, _error, _xml
from tests.storage.fakes import SECRET

from hypertrain.gpu_ops import cuda_executor_runner as runner
from hypertrain.storage.cas import BlockList, R2ObjectStore
from hypertrain.storage.offer import offer_upload
from hypertrain.storage.r2 import R2Client, R2Config

BUCKET = "cortex-v3-test-fake"
AMZ = "%Y%m%dT%H%M%SZ"


class Clock:
    def __init__(self, start: float | None = None) -> None:
        self.now = time.time() if start is None else start

    def time(self) -> float:
        return self.now

    def amz(self) -> str:
        return dt.datetime.fromtimestamp(self.now, dt.UTC).strftime(AMZ)

    def advance(self, seconds: float) -> None:
        self.now += seconds


class TimedFakeR2(FakeR2):
    def __init__(self, clock: Clock | None = None) -> None:
        super().__init__(BUCKET)
        self.clock = clock
        self.lock = threading.RLock()
        self.ledger: list[tuple[str, str, int, int, int]] = []  # op, key, status, bytes in, out
        self.key_faults: list[tuple[str, str, deque[int | None]]] = []
        self.gates: dict[str, tuple[threading.Event, threading.Event]] = {}
        self.signatures: set[str] = set()  # of every presigned request the fake was asked to serve

    def fault_key(self, op: str, suffix: str, *statuses: int | None) -> None:
        """Answer the next calls of `op` on a key ending in `suffix` with these statuses."""
        self.key_faults.append((op, suffix, deque(statuses)))

    def served(self, op: str, suffix: str = "") -> list[tuple[str, str, int, int, int]]:
        return [row for row in self.ledger if row[0] == op and row[1].endswith(suffix)]

    def gate(self, op: str) -> tuple[threading.Event, threading.Event]:
        """Hold every request of `op` before it reaches the fake: (entered, release)."""
        gate = self.gates[op] = (threading.Event(), threading.Event())
        return gate

    def __call__(self, req: httpx.Request) -> httpx.Response:
        raw_path, _, raw_query = req.url.raw_path.decode().partition("?")
        params = dict(parse_qsl(raw_query, keep_blank_values=True))
        key = unquote(raw_path[len(self.bucket) + 2 :])
        try:
            op = self._op(req.method, key, params)
        except KeyError:
            op = "?"
        if op in self.gates:  # outside the lock, so the rest of the fake keeps serving
            entered, release = self.gates[op]
            entered.set()
            release.wait(20)
        with self.lock:
            if "X-Amz-Signature" in params:
                self.signatures.add(params["X-Amz-Signature"])
            if self.clock is not None and self._expired(params):
                self._record("expired", key, 403, req, b"")
                return _error(403, "AccessDenied")
            self._drop_unsigned_condition(req, params)
            for fault_op, suffix, queue in self.key_faults:
                if fault_op == op and key.endswith(suffix) and queue:
                    status = queue.popleft()
                    if status is not None:
                        self._record(op, key, status, req, b"")
                        return _error(status, "Injected")
            before = len(self.ops)
            response = super().__call__(req)
            if len(self.ops) > before:
                self._record(self.ops[-1][0], key, response.status_code, req, response.content)
            return response

    def _ListObjects(self, req: httpx.Request, key: str, params: dict[str, str]) -> httpx.Response:
        """ListObjectsV2 in one page: what R2Client.list_prefix reads."""
        prefix = params.get("prefix", "")
        rows = "".join(
            f"<Contents><Key>{k}</Key><Size>{len(v[0])}</Size></Contents>"
            for k, v in sorted(self.objects.items())
            if k.startswith(prefix)
        )
        return httpx.Response(
            200, content=_xml("ListBucketResult", f"<IsTruncated>false</IsTruncated>{rows}")
        )

    def _record(self, op: str, key: str, status: int, req: httpx.Request, out: bytes) -> None:
        self.ledger.append((op, key, status, len(req.content), len(out)))

    def _expired(self, params: Mapping[str, str]) -> bool:
        if "X-Amz-Signature" not in params:
            return False
        signed = dt.datetime.strptime(params["X-Amz-Date"], AMZ).replace(tzinfo=dt.UTC)
        deadline = signed.timestamp() + int(params["X-Amz-Expires"])
        assert self.clock is not None
        return self.clock.time() > deadline

    @staticmethod
    def _drop_unsigned_condition(req: httpx.Request, params: Mapping[str, str]) -> None:
        """R2 only honours a condition the signature covers; an unsigned one is ignored."""
        signed = params.get("X-Amz-SignedHeaders", "").split(";")
        if "X-Amz-Signature" in params and "if-none-match" not in signed:
            req.headers.pop("if-none-match", None)


def config(base: str = "https://r2.test") -> R2Config:
    return R2Config("acct", CFG.access_key_id, SECRET, BUCKET, endpoint=base)


def client_for(fake: TimedFakeR2, base: str = "https://r2.test") -> R2Client:
    return R2Client(config(base), client=httpx.Client(transport=httpx.MockTransport(fake)))


def bridge(fake: FakeR2) -> Callable[..., tuple[int, dict[str, str], bytes]]:
    """`runner.http_send` served in-process by a fake: the same contract as the socket client."""

    def send(
        method: str,
        url: str,
        headers: Mapping[str, str],
        body: bytes | None,
        timeout: float,
        limit: int,
    ) -> tuple[int, dict[str, str], bytes]:
        response = fake(httpx.Request(method, url, headers=dict(headers), content=body or b""))
        if len(response.content) > limit:
            raise runner.TransferError("response too large", fatal=True)
        return (
            response.status_code,
            {k.lower(): v for k, v in response.headers.items()},
            response.content,
        )

    return send


class _Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    fake: FakeR2
    early: dict[
        str, int
    ]  # method -> status answered before the body is read, then the socket closes

    def _serve(self) -> None:
        length = int(self.headers.get("content-length", "0"))
        status = self.early.get(self.command)
        if status is not None:
            self.send_response(status)
            self.send_header("content-length", "0")
            self.send_header("connection", "close")
            self.end_headers()
            self.close_connection = True
            return
        body = self.rfile.read(length) if length else b""
        request = httpx.Request(
            self.command,
            f"http://{self.headers['host']}{self.path}",
            headers={k: v for k, v in self.headers.items() if k.lower() != "content-length"},
            content=body,
        )
        response = self.fake(request)
        self.send_response(response.status_code)
        for name, value in response.headers.items():
            if name.lower() not in ("content-length", "connection", "transfer-encoding"):
                self.send_header(name, value)
        length = response.headers.get("content-length") if self.command == "HEAD" else None
        self.send_header("content-length", length or str(len(response.content)))
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(response.content)

    do_GET = do_PUT = do_POST = do_DELETE = do_HEAD = _serve

    def log_message(self, format: str, *args: Any) -> None:  # noqa: A002 - silence the access log
        return


class Front:
    def __init__(self, fake: FakeR2, early: dict[str, int] | None = None) -> None:
        handler = type("Handler", (_Handler,), {"fake": fake, "early": early or {}})
        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
        self.server.daemon_threads = True
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()

    @property
    def base(self) -> str:
        return f"http://127.0.0.1:{self.server.server_address[1]}"

    def close(self) -> None:
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(10)


def serve_http(fake: FakeR2, early: dict[str, int] | None = None) -> Front:
    return Front(fake, early)


def credentials_file(directory: Path, base: str, bucket: str = BUCKET) -> Path:
    """The env file the challenge reads its R2 token from (a test token, built at run time)."""
    path = directory / "r2-exec.env"
    path.write_text(
        "\n".join(
            [
                "R2_ACCOUNT_ID=acct",
                f"R2_ACCESS_KEY_ID={CFG.access_key_id}",
                f"R2_SECRET_ACCESS_KEY={SECRET}",
                f"R2_BUCKET={bucket}",
                f"R2_ENDPOINT={base}",
                "",
            ]
        )
    )
    path.chmod(0o600)
    return path


class Challenge:
    """The challenge half of the mailbox contract, by hand: presign the two mailbox URLs, then
    answer a declaration with an offer (or a refusal)."""

    def __init__(
        self,
        fake: TimedFakeR2,
        client: R2Client,
        *,
        prefix: str = "exec/r/j/n/",
        ttl: int = 600,
        part_bytes: int = 8 << 20,
    ) -> None:
        self.fake, self.client, self.prefix = fake, client, prefix
        self.ttl, self.part_bytes = ttl, part_bytes
        self.raw = b""  # the request the runner was given, for tests that need the expected bytes
        self.declared: bytes | None = None
        self.offer: dict[str, Any] | None = None
        self.upload: Any = None  # (R2ObjectStore, UploadOffer) once the upload is open
        self.error: BaseException | None = None

    def request_block(self, **over: Any) -> dict[str, Any]:
        block: dict[str, Any] = {
            "version": 1,
            "part_bytes": self.part_bytes,
            "workers": 3,
            "reserve_seconds": 30,
            "poll_ms": 10,
            "offer_wait_s": 20,
            "retry_attempts": 3,
            "retry_base_ms": 1,
            "timeout_s": 20,
            "declare_put": self.client.presign(
                "PUT",
                self.prefix + "declare.json",
                expires=self.ttl,
                headers={"if-none-match": "*"},
            ),
            "offer_get": self.client.presign("GET", self.prefix + "offer.json", expires=self.ttl),
        }
        block.update(over)
        return block

    def answer(
        self,
        *,
        refuse: str | None = None,
        edit: Callable[[dict[str, Any]], None] | None = None,
        after_open: Callable[[Challenge], None] | None = None,
        wait: float = 20.0,
    ) -> threading.Thread:
        """Start answering in the background; the runner's serve() must own the main thread."""

        def run() -> None:
            try:
                self._answer(refuse, edit, after_open, wait)
            except BaseException as error:  # noqa: BLE001 - reported to the test by join()
                self.error = error

        thread = threading.Thread(target=run, daemon=True)
        thread.start()
        return thread

    def _answer(
        self,
        refuse: str | None,
        edit: Callable[[dict[str, Any]], None] | None,
        after_open: Callable[[Challenge], None] | None,
        wait: float,
    ) -> None:
        key = self.prefix + "declare.json"
        give_up = time.monotonic() + wait
        while time.monotonic() < give_up:
            with self.fake.lock:
                held = self.fake.objects.get(f"{key}")
            if held is not None:
                break
            time.sleep(0.005)
        else:
            raise TimeoutError("no declaration")
        raw = held[0]
        self.declared = raw
        if refuse is not None:
            body: dict[str, Any] = {"version": 1, "refused": refuse}
        else:
            declared = json.loads(raw)
            store = R2ObjectStore(self.client, self.prefix + "r/", part_bytes=self.part_bytes)
            blocks = BlockList(declared["size"], tuple(declared["blocks"]))
            offer = offer_upload(
                store, declared["sha256"], blocks, declared["parts"], expires=self.ttl
            )
            self.upload = (store, offer)
            if after_open is not None:
                after_open(self)
            body = {
                "version": 1,
                "declare_sha256": hashlib.sha256(raw).hexdigest(),
                "ttl_s": self.ttl,
                "parts": [part.url for part in offer.parts],
                "list": list(offer.list_urls),
            }
        if edit is not None:
            edit(body)
        self.offer = body
        self.client.put_object(self.prefix + "offer.json", json.dumps(body).encode())


# -- a tree the fake island "trains", and the request that asks for it ---------------------------

MiB = 1 << 20
U0 = "GPU-" + str(uuidlib.UUID(int=(1 << 120) | 0xABCDEF0123))


def sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def isolated_process(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Iterator[None]:
    """For an autouse fixture of the test module: `serve` takes SIGTERM, mutates os.environ and
    makes its work directory in tempdir; all three are restored."""
    scratch = tmp_path / "tmp"
    scratch.mkdir()
    monkeypatch.setattr(tempfile, "tempdir", str(scratch))
    term, environment = signal.getsignal(signal.SIGTERM), dict(os.environ)
    signal.signal(signal.SIGTERM, signal.SIG_DFL)
    yield
    signal.signal(signal.SIGTERM, term if term is not None else signal.SIG_DFL)
    os.environ.clear()
    os.environ.update(environment)


def moved_clock(monkeypatch: pytest.MonkeyPatch) -> Clock:
    """The runner's monotonic clock and sleep on one movable clock: nothing really waits."""
    clock = Clock(1000.0)
    monkeypatch.setattr(runner, "_monotonic", clock.time)
    monkeypatch.setattr(runner, "_sleep", clock.advance)
    return clock


# -- a tree the fake island 'trains' -----------------------------------------------------------


def write_tree(attempt: Path, size: int = 17 * MiB + 5, ranks: int = 1) -> None:
    """Rank files of every shape the tar writer meets: empty, odd, a 512 multiple, a long path."""
    for rank in range(ranks):
        base = attempt / f"rank-{rank}"
        (base / "checkpoints").mkdir(parents=True)
        (base / "state.safetensors").write_bytes(random.Random(rank).randbytes(size))
        (base / "summary.json").write_bytes(f"summary {rank}".encode())
        (base / "empty.bin").write_bytes(b"")
        (base / "block.bin").write_bytes(b"b" * 1024)
        (base / "checkpoints" / ("c" * 120 + ".safetensors")).write_bytes(b"c" * 513)


def launch_tree(request: dict[str, Any], attempt: Path, budget: float, log: Path) -> int:
    write_tree(attempt, ranks=request["n_gpus"])
    return 0


def probe_ok(request: dict[str, Any]) -> dict[str, Any]:
    return {"backend": "cuda", "gpu_uuids": list(request["uuids"]), "sm_counts": [148]}


# The package files the fake container's image holds. serve reports their digest in
# environment.json, so the tar it sends carries it; a test never measures the installed package.
MEASURED = {"src/hypertrain/a.py": "11" * 32}
MEASURED_SHA256 = sha(json.dumps(MEASURED, sort_keys=True, separators=(",", ":")).encode())


def measured() -> dict[str, str]:
    return dict(MEASURED)


def request_bytes(files: dict[str, bytes], artifact: dict[str, Any] | None) -> bytes:
    body: dict[str, Any] = {
        "version": 1,
        "n_gpus": 1,
        "uuids": [U0],
        "job_sha256": sha(files["job.json"]),
        "budget_seconds": 600,
        "inputs": {name: sha(raw) for name, raw in files.items()},
        "env": {"PYTHONHASHSEED": "0", "HT_GPU_UUIDS": U0, "CUDA_VISIBLE_DEVICES": U0},
    }
    if artifact is not None:
        body["artifact"] = artifact
    return json.dumps(body, indent=1).encode()


def package(files: dict[str, bytes], raw: bytes) -> bytes:
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w", format=tarfile.PAX_FORMAT) as archive:
        for name, payload in {"request.json": raw, **files}.items():
            info = tarfile.TarInfo(name)
            info.size = len(payload)
            archive.addfile(info, io.BytesIO(payload))
    return buffer.getvalue()


FILES = {"job.json": b'{"job": 1}', "start_state": b"s" * 3000}


def oracle(raw: bytes) -> bytes:
    """What the classic path streams for the same tree, environment and request."""
    with tempfile.TemporaryDirectory() as scratch:
        attempt = Path(scratch)
        write_tree(attempt)
        out = io.BytesIO()
        environment = {**probe_ok({"uuids": [U0]}), "image_sources_sha256": MEASURED_SHA256}
        runner.write_outputs(out, attempt, environment, raw)
        return out.getvalue()
