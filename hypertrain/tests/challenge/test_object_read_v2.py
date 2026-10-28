"""The anonymous object read streams the stored file and answers a bad key with a 4xx (IMP-8).

`GET /v2/runs/{run_id}/objects/{sha256}` is public. At the 101M profile a trial input is about
0.8 GB: read whole into memory (and hashed again) for every caller, one reader could exhaust the
RAM of the master. The route now sends the file in blocks of OBJECT_READ_BLOCK, refuses an object
above OBJECT_READ_MAX before reading a byte, and a key that is not 64 lowercase hex is a 400.

starlette's TestClient buffers the whole body, so the memory test drives the ASGI app directly
with a `send` that keeps nothing.
"""

# ruff: noqa: F811  (the `network` fixture parameter shadows the imported fixture)
from __future__ import annotations

import asyncio
import hashlib
import tracemalloc
from dataclasses import dataclass, field
from typing import Any

import pytest

from hypertrain.challenge import app as challenge_app

from .test_service_network_v2 import network  # noqa: F401  (pytest fixture import)

SPARSE = (
    256 << 20
)  # a few hundred MiB of file holes: it takes no disk and, read whole, 256 MiB of RAM
MEMORY_BOUND = 8 << 20


@dataclass
class Seen:
    status: int = 0
    headers: dict[str, str] = field(default_factory=dict)
    sizes: list[int] = field(default_factory=list)
    digest: Any = field(default_factory=hashlib.sha256)
    detail: bytes = b""


def drive(app: Any, path: str) -> Seen:
    """GET `path` against the ASGI app; the body is hashed as it arrives and never kept."""
    seen = Seen()
    scope = {
        "type": "http",
        "asgi": {"version": "3.0"},
        "http_version": "1.1",
        "method": "GET",
        "scheme": "http",
        "path": path,
        "raw_path": path.encode(),
        "query_string": b"",
        "headers": [],
        "server": ("test", 80),
        "client": ("127.0.0.1", 1),
    }

    async def serve() -> None:
        requested = False
        gone = asyncio.Event()  # the client never disconnects

        async def receive() -> dict[str, Any]:
            nonlocal requested
            if not requested:
                requested = True
                return {"type": "http.request", "body": b"", "more_body": False}
            await gone.wait()
            return {"type": "http.disconnect"}

        async def send(message: dict[str, Any]) -> None:
            if message["type"] == "http.response.start":
                seen.status = message["status"]
                seen.headers = {k.decode(): v.decode() for k, v in message["headers"]}
            elif message["type"] == "http.response.body":
                body = message.get("body", b"")
                seen.sizes.append(len(body))
                seen.digest.update(body)
                if seen.status != 200:
                    seen.detail += body

        await app(scope, receive, send)

    asyncio.run(serve())
    return seen


def sparse_object(store: Any, size: int) -> str:
    """A file of `size` zero bytes, stored under the sha256 of its content."""
    scratch = store.root / "sparse.tmp"
    with scratch.open("wb") as handle:
        handle.truncate(size)
    with scratch.open("rb") as handle:
        sha = hashlib.file_digest(handle, "sha256").hexdigest()
    target = store._path(sha)
    target.parent.mkdir(exist_ok=True)
    scratch.replace(target)
    return sha


def test_a_large_object_is_streamed_in_bounded_blocks_with_bounded_memory(network):
    store = network.store.for_run(network.manifest.run_id()).objects
    sha = sparse_object(store, SPARSE)
    app = network.client.app
    drive(app, f"{network.url}/objects/{store.put(b'warm up')}")  # imports, thread pool

    tracemalloc.start()
    try:
        tracemalloc.reset_peak()
        before = tracemalloc.get_traced_memory()[0]
        seen = drive(app, f"{network.url}/objects/{sha}")
        peak = tracemalloc.get_traced_memory()[1] - before
    finally:
        tracemalloc.stop()

    assert seen.status == 200
    assert peak < MEMORY_BOUND
    assert seen.headers["content-length"] == str(SPARSE) == str(sum(seen.sizes))
    assert max(seen.sizes) <= challenge_app.OBJECT_READ_BLOCK
    assert len(seen.sizes) >= SPARSE // challenge_app.OBJECT_READ_BLOCK
    assert seen.digest.hexdigest() == sha  # what arrived is the object, byte for byte


@pytest.mark.parametrize("key", ["zz", "a" * 63, "a" * 65, "A" * 64, "g" * 64])
def test_a_key_that_is_not_64_lowercase_hex_is_a_400(network, key):
    response = network.client.get(f"{network.url}/objects/{key}")

    assert response.status_code == 400
    assert "64 lowercase hex" in response.json()["detail"]


def test_an_object_nobody_stored_is_a_404(network):
    response = network.client.get(f"{network.url}/objects/{'ab' * 32}")

    assert response.status_code == 404
    assert response.json() == {"detail": "unknown object"}


def test_an_object_above_the_read_cap_is_refused_before_a_byte_is_read(network, monkeypatch):
    store = network.store.for_run(network.manifest.run_id()).objects
    big = store.put(b"x" * 1025)
    monkeypatch.setattr(challenge_app, "OBJECT_READ_MAX", 1024)

    seen = drive(network.client.app, f"{network.url}/objects/{big}")

    assert seen.status == 413
    assert sum(seen.sizes) < 200  # the refusal, not the object

    monkeypatch.setattr(challenge_app, "OBJECT_READ_MAX", 1025)
    served = network.client.get(f"{network.url}/objects/{big}")
    assert (served.status_code, served.content) == (200, b"x" * 1025)  # the cap itself is served


def test_a_block_of_the_object_is_read_off_the_event_loop(network, monkeypatch):
    # Given: the file of an object, wrapped so that each read notes whether the event loop runs
    store = network.store.for_run(network.manifest.run_id()).objects
    sha = store.put(b"x" * (2 * challenge_app.OBJECT_READ_BLOCK + 1))
    on_loop: list[bool] = []
    real = type(store).open_read

    class Spy:
        def __init__(self, handle: Any) -> None:
            self.handle = handle

        def read(self, size: int) -> bytes:
            try:
                asyncio.get_running_loop()
                on_loop.append(True)
            except RuntimeError:
                on_loop.append(False)
            return self.handle.read(size)  # type: ignore[no-any-return]

        def close(self) -> None:
            self.handle.close()

    def spied(self: Any, key: str) -> Any:
        handle, size = real(self, key)
        return Spy(handle), size

    monkeypatch.setattr(type(store), "open_read", spied)

    # When
    seen = drive(network.client.app, f"{network.url}/objects/{sha}")

    # Then: the blocks (and the final empty read) all ran in a worker thread
    assert seen.status == 200 and sum(seen.sizes) == 2 * challenge_app.OBJECT_READ_BLOCK + 1
    assert on_loop and not any(on_loop)
