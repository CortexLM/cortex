"""A dispute poll pins a worker thread for as long as it waits: bound it, keep it off the pool.

Real uvicorn socket, as in test_subnet_gate_v2.py::test_signed_object_route_over_real_http_chunked:
the thread a wait pins, and a client that goes away without stopping it, only exist on the wire.
`Parked` stands in for the condition wait of `DisputesV2.events` so that nothing depends on a
clock: a poll parks until the test lets it go.
"""

# ruff: noqa: F811  (imported fixtures are asked for by name)
from __future__ import annotations

import json
import socket
import threading
import time
from collections import Counter

import httpx
import pytest

from hypertrain.challenge import app as app_module
from hypertrain.challenge.disputes_v2 import DisputeError
from hypertrain.protocol.keys import Keypair

from .test_event_loop_free_v2 import Probe, served  # noqa: F401
from .test_subnet_gate_v2 import gated, refresh, service, subnet_master  # noqa: F401


class Parked:
    """Replaces `events`: counts the polls that reached it and holds them until released."""

    def __init__(self, real) -> None:
        self.real, self.release = real, threading.Event()
        self.entered, self.guard = 0, threading.Lock()

    def __call__(self, party, cursor, *, timeout):
        with self.guard:
            self.entered += 1
        assert self.release.wait(60)
        return self.real(party, cursor, timeout=0)

    def reached(self, count: int, seconds: float = 10) -> bool:
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            with self.guard:
                if self.entered >= count:
                    return True
            time.sleep(0.01)
        return False


@pytest.fixture
def watched(gated, subnet_master, served, monkeypatch):
    n = gated
    parties = [Keypair(bytes([140 + i]) * 32) for i in range(48)]  # on the subnet, never admitted
    subnet_master.registered.update(p.ss58 for p in parties)
    refresh(n)
    root, _ = served(n.client.app)
    run_id = n.manifest.run_id()
    disputes = n.store.for_run(run_id)._services(run_id)[2]
    parked = Parked(disputes.events)
    monkeypatch.setattr(disputes, "events", parked)
    yield n, parties, root, parked, disputes
    parked.release.set()


def envelope(n, party, wait=0):
    poll = {"party": party.ss58, "cursor": 0, "wait": wait}
    return n.signed(party, "DisputeWatchRequest", poll, n.now + 20)


def poll(root, n, party, wait=0, timeout=5):
    """The status of one poll; 0 when the server had not answered in `timeout` seconds."""
    try:
        return httpx.post(
            root + n.url + "/disputes/watch", json=envelope(n, party, wait), timeout=timeout
        ).status_code
    except httpx.TimeoutException:
        return 0


def long_poll(root, n, party, results, name):
    """A poll that waits (30 s) until the test releases it; its status lands in `results`."""

    def target():
        results[name] = poll(root, n, party, wait=30, timeout=90)

    thread = threading.Thread(target=target)
    thread.start()
    return thread


def settled(parked, results, attempts, seconds=60):
    """True once each of `attempts` polls has reached `events` or been answered (refused)."""
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        with parked.guard:
            if parked.entered + len(results) >= attempts:
                return True
        time.sleep(0.01)
    return False


def test_one_party_is_bounded_and_every_slot_comes_back(watched, monkeypatch):
    n, parties, root, parked, disputes = watched
    monkeypatch.setattr(app_module, "WATCH_PER_PARTY", 2, raising=False)
    results: dict[str, int] = {}
    holders = [long_poll(root, n, parties[0], results, f"hold{i}") for i in range(2)]
    assert parked.reached(2), "two polls of one party must both be allowed to wait"
    # A third and a fourth are refused at once, whatever their wait.
    assert [poll(root, n, parties[0]) for _ in range(2)] == [429, 429]
    # Another party is not charged for it.
    other = long_poll(root, n, parties[1], results, "other")
    assert parked.reached(3)
    parked.release.set()
    for holder in [*holders, other]:
        holder.join(30)
    assert Counter(results.values()) == {200: 3}
    # A finished wait gave its slot back; so does a poll that ends in an error.
    real, failures = parked.real, [1, 2, 3]

    def flaky(party, cursor, *, timeout):
        if failures:
            failures.pop()
            raise DisputeError("BOOM")
        return real(party, cursor, timeout=timeout)

    monkeypatch.setattr(disputes, "events", flaky)
    assert [poll(root, n, parties[0]) for _ in range(4)] == [409, 409, 409, 200]


def test_total_waits_are_bounded_across_parties(watched, monkeypatch):
    n, parties, root, parked, _ = watched
    monkeypatch.setattr(app_module, "WATCH_TOTAL", 3, raising=False)
    results: dict[str, int] = {}
    holders = [long_poll(root, n, parties[i], results, f"hold{i}") for i in range(3)]
    assert parked.reached(3)
    assert poll(root, n, parties[3]) == 429
    parked.release.set()
    for holder in holders:
        holder.join(30)
    assert Counter(results.values()) == {200: 3}
    assert poll(root, n, parties[3]) == 200


def test_subnet_miners_cannot_park_the_polls_the_protocol_roles_need(watched, subnet_master):
    """Registering 64 hotkeys and parking two polls with each used to take all 128 threads: the
    pinned auditors and referee were then refused until a poll timed out, as often as it paid."""
    n, _, root, parked, _ = watched
    miners = [Keypair(bytes([200, i]) * 16) for i in range(app_module.WATCH_MINERS + 1)]
    subnet_master.registered.update(m.ss58 for m in miners)
    refresh(n)
    results: dict[str, int] = {}
    holders = [
        long_poll(root, n, miner, results, f"miner{i}-{j}")
        for i, miner in enumerate(miners[:-1])
        for j in range(app_module.WATCH_PER_PARTY)
    ]
    assert len(holders) == app_module.WATCH_TOTAL  # the whole pool, had nothing capped them
    assert settled(parked, results, len(holders))
    pinned = [service.AUDITORS[0], service.AUDITORS[0], service.AUDITORS[1], service.REFEREE]
    holders += [long_poll(root, n, key, results, f"role{i}") for i, key in enumerate(pinned)]
    assert settled(parked, results, len(holders))
    refused = sorted(name for name in results if name.startswith("role"))
    assert not refused, f"pinned roles refused while miners held the pool: {refused}"
    assert parked.entered == app_module.WATCH_MINERS + len(pinned)
    assert poll(root, n, miners[-1]) == 429  # the next miner is over the miners' share
    parked.release.set()
    for holder in holders:
        holder.join(30)
    total, miners_only = app_module.WATCH_TOTAL, app_module.WATCH_MINERS
    assert Counter(results.values()) == {200: miners_only + len(pinned), 429: total - miners_only}
    assert poll(root, n, miners[-1]) == 200  # and every share came back


def test_roles_are_bounded_by_the_pool_alone_and_each_release_frees_its_own_counter(
    watched, monkeypatch
):
    n, parties, root, miners_parked, disputes = watched
    monkeypatch.setattr(app_module, "WATCH_TOTAL", 5)
    monkeypatch.setattr(app_module, "WATCH_MINERS", 2)
    roles_parked = Parked(miners_parked.real)
    pinned = {key.ss58 for key in (*service.AUDITORS, service.REFEREE)}

    def events(party, cursor, *, timeout):
        parked = roles_parked if party in pinned else miners_parked
        return parked(party, cursor, timeout=timeout)

    monkeypatch.setattr(disputes, "events", events)
    results: dict[str, int] = {}
    try:
        miners = [long_poll(root, n, parties[i], results, f"miner{i}") for i in range(2)]
        assert miners_parked.reached(2)
        assert poll(root, n, parties[2]) == 429  # the miners' share is spent, the pool is not
        # Roles are not held to that share: three polls, one more than WATCH_MINERS.
        keys = [service.AUDITORS[0], service.AUDITORS[0], service.AUDITORS[1]]
        roles = [long_poll(root, n, key, results, f"role{i}") for i, key in enumerate(keys)]
        assert roles_parked.reached(3)
        assert poll(root, n, service.REFEREE) == 429  # the whole pool (5) is theirs to fill
        roles_parked.release.set()
        for holder in roles:
            holder.join(30)
        assert poll(root, n, parties[2]) == 429  # a role's release gave nothing to the miners
        miners_parked.release.set()
        for holder in miners:
            holder.join(30)
        assert poll(root, n, parties[2]) == 200  # a miner's release did
    finally:
        roles_parked.release.set()
    assert Counter(results.values()) == {200: 5}


def test_a_miner_poll_that_ends_early_gives_its_share_back(watched, monkeypatch):
    n, parties, root, parked, disputes = watched
    monkeypatch.setattr(app_module, "WATCH_MINERS", 2)

    def failing(party, cursor, *, timeout):
        raise DisputeError("BOOM")

    monkeypatch.setattr(disputes, "events", failing)
    assert [poll(root, n, parties[i]) for i in range(2)] == [409, 409]
    # Two distinct miners fill the share exactly if the failed polls gave theirs back.
    monkeypatch.setattr(disputes, "events", parked)
    results: dict[str, int] = {}
    holders = [long_poll(root, n, parties[i], results, f"miner{i}") for i in (2, 3)]
    assert parked.reached(2), "a failed poll kept its share of the pool"
    assert poll(root, n, parties[4]) == 429
    parked.release.set()
    for holder in holders:
        holder.join(30)
    for hook in n.client.app.router.on_shutdown:  # the pool is closed, as at shutdown
        hook()
    # Three distinct miners: a share kept by the first two would make the third a 429.
    assert [poll(root, n, parties[i]) for i in range(4, 7)] == [500, 500, 500]


def test_a_stranger_cannot_take_a_slot(watched, monkeypatch):
    """Two-sided boundary control (passes before the bound exists): it kills a claim that is
    taken before the membership check, which would leak one slot per refused stranger."""
    n, _, root, _, _ = watched
    monkeypatch.setattr(app_module, "WATCH_PER_PARTY", 1, raising=False)
    stranger = Keypair(bytes([201]) * 32)  # signs its own poll correctly, is not on the subnet
    codes = [poll(root, n, stranger) for _ in range(3)]
    assert codes == [403, 403, 403]  # never a 429: it held no slot


def test_forty_parked_polls_leave_the_shared_pool_free(watched):
    n, parties, root, parked, _ = watched
    results: dict[str, int] = {}
    holders = [long_poll(root, n, parties[i], results, f"hold{i}") for i in range(40)]
    # On the shared pool (32 threads at most) the last polls would still be queued.
    assert parked.reached(40), "polls are queued behind the shared worker pool"
    try:
        answered = httpx.get(root + "/health", timeout=5) is not None
    except httpx.TimeoutException:
        answered = False
    parked.release.set()
    for holder in holders:
        holder.join(30)
    assert answered, "/health waited behind 40 parked polls"
    assert Counter(results.values()) == {200: 40}  # none lost to a racing read of the connection


def test_a_client_that_leaves_keeps_its_slot_until_the_wait_ends(watched, monkeypatch):
    n, parties, root, parked, _ = watched
    monkeypatch.setattr(app_module, "WATCH_PER_PARTY", 1, raising=False)
    body = json.dumps(envelope(n, parties[2], wait=30)).encode()
    head = (
        f"POST {n.url}/disputes/watch HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\n"
        f"Content-Length: {len(body)}\r\n\r\n"
    ).encode()
    leaver = socket.create_connection(("127.0.0.1", int(root.rsplit(":", 1)[1])))
    leaver.sendall(head + body)
    assert parked.reached(1)
    leaver.close()  # the worker thread keeps waiting: the server never learns it must stop
    assert poll(root, n, parties[2]) == 429
    parked.release.set()
    deadline, status = time.monotonic() + 15, 429
    while status == 429 and time.monotonic() < deadline:
        status = poll(root, n, parties[2])
        time.sleep(0.05)
    assert status == 200


def test_a_poll_the_pool_refuses_gives_its_slot_back(watched, monkeypatch):
    n, parties, root, parked, _ = watched
    parked.release.set()
    monkeypatch.setattr(app_module, "WATCH_PER_PARTY", 1, raising=False)
    for hook in n.client.app.router.on_shutdown:  # the pool is closed, as at shutdown
        hook()
    refused = [poll(root, n, parties[5]) for _ in range(2)]
    assert refused == [500, 500]  # the second would be 429 had the first kept its slot


def test_every_statement_of_a_poll_holds_the_store_lock(watched, monkeypatch):
    """The role lookup for a pinned auditor read the shared connection without the lock."""
    n, _, root, parked, _ = watched
    parked.release.set()
    store = n.store.for_run(n.manifest.run_id())
    probe = Probe(store._db, store._lock)
    monkeypatch.setattr(store, "_db", probe)
    probe.armed = True
    answered = poll(root, n, service.AUDITORS[0])  # a pinned role: no subnet lookup involved
    probe.armed = False
    assert answered == 200
    assert probe.calls
    assert [call for call in probe.calls if not call[1]] == []
