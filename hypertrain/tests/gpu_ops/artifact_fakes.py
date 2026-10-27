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
import json
import threading
import time
from collections import deque
from collections.abc import Callable, Mapping
from pathlib import Path
from typing import Any
from urllib.parse import parse_qsl, unquote

import httpx
from tests.storage.fake_r2 import CFG, FakeR2, _error
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

    def fault_key(self, op: str, suffix: str, *statuses: int | None) -> None:
        """Answer the next calls of `op` on a key ending in `suffix` with these statuses."""
        self.key_faults.append((op, suffix, deque(statuses)))

    def served(self, op: str, suffix: str = "") -> list[tuple[str, str, int, int, int]]:
        return [row for row in self.ledger if row[0] == op and row[1].endswith(suffix)]

    def __call__(self, req: httpx.Request) -> httpx.Response:
        with self.lock:
            raw_path, _, raw_query = req.url.raw_path.decode().partition("?")
            params = dict(parse_qsl(raw_query, keep_blank_values=True))
            key = unquote(raw_path[len(self.bucket) + 2 :])
            if self.clock is not None and self._expired(params):
                self._record("expired", key, 403, req, b"")
                return _error(403, "AccessDenied")
            self._drop_unsigned_condition(req, params)
            try:
                op = self._op(req.method, key, params)
            except KeyError:
                op = "?"
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
        self.send_header("content-length", str(len(response.content)))
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
        prefix: str = "runs/r/exec/j/n/",
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
