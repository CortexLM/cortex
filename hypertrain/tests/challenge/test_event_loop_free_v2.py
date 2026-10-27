"""Object and relay-receipt routes never touch the store on the event loop (ADM-04, ADM-04b)."""

# ruff: noqa: F811  (imported fixtures are asked for by name)
from __future__ import annotations

import asyncio
import socket
import threading
import time

import httpx
import pytest
import uvicorn

from hypertrain.protocol import relay_envelope
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.relay_messages import RelayReceipt

from .test_relay_receipt_idempotent import network, submit, uploaded  # noqa: F401
from .test_subnet_gate_v2 import HOT, gated, refresh, subnet_master  # noqa: F401


@pytest.fixture
def served():
    """Serve an app on a real socket; the server thread runs the one event loop."""
    started: list[uvicorn.Server] = []
    threads: list[threading.Thread] = []

    def serve(app) -> tuple[str, uvicorn.Server]:
        sock = socket.socket()
        sock.bind(("127.0.0.1", 0))
        sock.listen(16)
        server = uvicorn.Server(uvicorn.Config(app, lifespan="off", log_level="warning"))
        thread = threading.Thread(target=server.run, kwargs={"sockets": [sock]}, daemon=True)
        thread.start()
        started.append(server)
        threads.append(thread)
        return f"http://127.0.0.1:{sock.getsockname()[1]}", server

    yield serve
    for server in started:
        server.should_exit = True
    for thread in threads:
        thread.join(timeout=30)


class LockHeld:
    """Another request holds the store lock, as a long operation does."""

    def __init__(self, store) -> None:
        self.store, self.held, self.release = store, threading.Event(), threading.Event()
        self.thread = threading.Thread(target=self.hold, daemon=True)

    def hold(self) -> None:
        with self.store._lock:
            self.held.set()
            self.release.wait(60)

    def __enter__(self) -> LockHeld:
        self.thread.start()
        assert self.held.wait(10)
        return self

    def __exit__(self, *exc) -> None:
        self.release.set()
        self.thread.join(30)


def in_flight(server: uvicorn.Server, count: int) -> None:
    """Wait until the server has accepted `count` requests that have not been answered."""
    deadline = time.monotonic() + 10
    while len(server.server_state.tasks) < count:
        assert time.monotonic() < deadline, "the request never reached the server"
        time.sleep(0.01)


def in_background(call, out: dict, name: str) -> threading.Thread:
    def target() -> None:
        try:
            out[name] = call()
        except httpx.HTTPError as error:
            out[name] = error

    thread = threading.Thread(target=target, daemon=True)
    thread.start()
    return thread


def test_forged_object_put_is_refused_while_the_store_lock_is_held(gated, subnet_master, served):
    n = gated
    run_id = n.manifest.run_id()
    subnet_master.registered.add(HOT[1].ss58)  # on the subnet, never admitted to the run
    refresh(n)
    root, server = served(n.client.app)
    sha = "ab" * 32
    out: dict = {}

    def put(signature: str) -> httpx.Response:
        return httpx.put(
            f"{root}{n.url}/objects/{sha}",
            content=b"x",
            headers={"x-hotkey": HOT[1].ss58, "x-object-signature": signature},
            timeout=30,
        )

    genuine = HOT[1].sign(f"hypertrain/object/2|{run_id}|{sha}".encode()).hex()
    with LockHeld(n.store.for_run(run_id)):
        parked = in_background(lambda: put(genuine), out, "genuine")
        in_flight(server, 1)
        forged = in_background(lambda: put("00" * 64), out, "forged")
        # A forged signature is refused without touching the lock at all.
        forged.join(10)
        assert not forged.is_alive(), "forged-signature PUT waited for the store lock"
        assert out["forged"].status_code == 403
        assert out["forged"].json()["detail"] == "object signature mismatch"
        # The loop is free although a request that passed the signature waits for the lock.
        assert httpx.get(root + "/version", timeout=5).status_code == 200
        assert "genuine" not in out
    parked.join(30)
    assert out["genuine"].status_code == 403
    assert out["genuine"].json()["detail"] == "hotkey is not current accepted identity"


def test_anonymous_relay_receipt_does_not_park_the_loop(network, served):
    run_id = network.manifest.run_id()
    root, server = served(network.client.app)
    stranger = Keypair(bytes([201]) * 32)  # unregistered, unknown to the run
    receipt = RelayReceipt(
        w=0,
        hotkey=HOT[0].ss58,
        delta_hash="11" * 32,
        size=10,
        grant_hash="22" * 32,
        chunk_manifest_hash="33" * 32,
        durable_object_key="k",
        key_id="relay-key-1",
        received_drand=5,
        retain_until=50,
    )
    envelope = relay_envelope.seal(stranger, "RelayReceipt", run_id, receipt, 60)
    out: dict = {}
    with LockHeld(network.store.for_run(run_id)):
        posted = in_background(
            lambda: httpx.post(root + "/v2/relay-receipts", json=envelope, timeout=30), out, "r"
        )
        in_flight(server, 1)
        assert httpx.get(root + "/version", timeout=5).status_code == 200
    posted.join(30)
    assert out["r"].status_code == 409, out["r"].text  # no such grant, once the lock is free


class Probe:
    """Stand-in for the shared connection: notes where each statement runs and under what."""

    def __init__(self, db, lock) -> None:
        self.db, self.lock, self.armed, self.calls = db, lock, False, []

    def execute(self, *args, **kwargs):
        if self.armed:
            try:
                asyncio.get_running_loop()
                on_loop = True
            except RuntimeError:
                on_loop = False
            self.calls.append((on_loop, self.lock._is_owned()))
        return self.db.execute(*args, **kwargs)

    def __getattr__(self, name):
        return getattr(self.db, name)


def probed(network, monkeypatch) -> Probe:
    store = network.store.for_run(network.manifest.run_id())
    probe = Probe(store._db, store._lock)
    monkeypatch.setattr(store, "_db", probe)
    return probe


def test_object_read_uses_the_shared_connection_off_the_loop_and_locked(network, monkeypatch):
    store = network.store.for_run(network.manifest.run_id())
    sha = store.objects.put(b"published run state")
    probe = probed(network, monkeypatch)
    probe.armed = True
    response = network.client.get(f"{network.url}/objects/{sha}")
    probe.armed = False
    assert response.status_code == 200
    assert response.content == b"published run state"
    assert probe.calls, "the route must still check that the run exists"
    assert all(call == (False, True) for call in probe.calls), probe.calls


def test_relay_receipt_uses_the_shared_connection_off_the_loop(network, uploaded, monkeypatch):
    _, grant, receipt = uploaded
    probe = probed(network, monkeypatch)
    probe.armed = True
    accepted = submit(network, receipt)
    probe.armed = False
    assert accepted.status_code == 200, accepted.text
    assert accepted.json()["body"]["delta_hash"] == grant.delta_hash
    assert probe.calls
    assert not [call for call in probe.calls if call[0]], "store statements ran on the event loop"
