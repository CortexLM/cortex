"""The miner side of an admission trial: one bounded publication for the probe's four files
(D94 lot B, docs/adr/0017 at the monorepo root)."""

from __future__ import annotations

import hashlib
import importlib.util
import os
import sys
import threading
import time
from collections import Counter
from collections.abc import Callable
from pathlib import Path

import httpx
import pytest

import hypertrain.miner.core as core
from hypertrain.data.store import PART_BYTES, StoreError, parse_index, put_files
from hypertrain.miner.core import MinerError

_spec = importlib.util.spec_from_file_location(
    "fast_path_client_fixture", Path(__file__).parent / "test_client_network_v2.py"
)
assert _spec is not None and _spec.loader is not None
client = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = client
_spec.loader.exec_module(client)

NAMES = ("state.safetensors", "ef.safetensors", "delta.bin", "leaves.json")


@pytest.fixture(autouse=True)
def no_real_waits(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(core, "RETRY_DELAYS", (0, 0))


def _files(tmp_path: Path, sizes: tuple[int, ...], shared: bytes = b"") -> list[Path]:
    """One file per size, random bytes; `shared` (whole parts) opens every file, so the same
    part digest appears in each of them."""
    folder = tmp_path / "published"
    folder.mkdir()
    paths = []
    for name, size in zip(NAMES, sizes, strict=True):
        path = folder / name
        path.write_bytes(shared + os.urandom(size))
        paths.append(path)
    return paths


class Wire:
    """The object route of `_parts_miner`, behind a latency and an in-flight counter."""

    def __init__(self, miner, latency: float = 0.0) -> None:
        self.route, self.latency = miner.api.c._transport.handler, latency
        self.lock, self.now, self.peak = threading.Lock(), 0, 0
        self.posts: list[tuple[str, bytes]] = []
        self.events: list[tuple[str, str]] = []  # ("start" | "done", digest), in time order
        miner.api.c._transport.handler = self

    def __call__(self, request: httpx.Request) -> httpx.Response:
        if request.method != "POST":
            return self.route(request)
        digest = request.url.path.rsplit("/", 1)[1]
        with self.lock:
            self.now += 1
            self.peak = max(self.peak, self.now)
            self.events.append(("start", digest))
        try:
            time.sleep(self.latency)
            head = int.from_bytes(request.content[:4], "big")
            with self.lock:
                self.posts.append((digest, request.content[4 + head :]))
            response = self.route(request)
            with self.lock:
                self.events.append(("done", digest))
            return response
        finally:
            with self.lock:
                self.now -= 1


def _sequential(paths: list[Path]) -> list[tuple[str, bytes]]:
    """What the publication must store: the bytes the one-worker publisher puts, in its order."""
    out: list[tuple[str, bytes]] = []

    def record(data: bytes) -> str:
        out.append((hashlib.sha256(data).hexdigest(), data))
        return out[-1][0]

    put_files(paths, record, 1)
    return out


def test_the_four_files_share_one_window_and_each_index_follows_its_own_parts(tmp_path: Path):
    # Given: four multi-part files and a route that takes 30 ms per POST.
    miner, store, _ = client._parts_miner(tmp_path)
    paths = _files(tmp_path, (3 * PART_BYTES + 7, 2 * PART_BYTES + 1, 2 * PART_BYTES + 3, 5))
    wire = Wire(miner, latency=0.03)
    # When
    refs = core._upload_refs(paths, miner._bounded_put(time.time() + 3600, None))
    # Then: the refs are the hash-only ones the screen signs, each stored and readable.
    assert refs == [client_ref(path) for path in paths]
    # And: never more than UPLOAD_WORKERS POSTs at once.
    assert 1 < wire.peak <= core.UPLOAD_WORKERS
    events = wire.events
    indexes = [parse_index(store.get(ref.sha256)) for ref in refs]
    for ref, index, path in zip(refs, indexes, paths, strict=True):
        if index is not None:  # each index is POSTed only once every part it lists is stored
            begun = events.index(("start", ref.sha256))
            assert all(events.index(("done", p)) < begun for p in index.parts), path.name
    # And: the later files' parts did not wait for the first file to be published.
    later = {p for index in indexes[1:3] if index is not None for p in index.parts}
    before = events[: events.index(("start", refs[0].sha256))]
    assert len(later & {d for kind, d in before if kind == "start"}) >= 2


def client_ref(path: Path):
    from hypertrain.gpu_ops.work_screen import artifact_ref

    return artifact_ref(path)


def test_the_bytes_stored_are_the_sequential_ones_and_a_shared_part_is_posted_once(
    tmp_path: Path,
):
    # Given: four files that open with the same 1 MiB part.
    miner, _, _ = client._parts_miner(tmp_path)
    paths = _files(
        tmp_path, (PART_BYTES + 9, PART_BYTES + 1, 2 * PART_BYTES, 11), os.urandom(PART_BYTES)
    )
    want = _sequential(paths)
    wire = Wire(miner)
    # When
    core._upload_refs(paths, miner._bounded_put(time.time() + 3600, None))
    # Then: the same distinct (digest, bytes) as the sequential publisher, each POSTed once:
    # the shared part, which the sequential one stores four times, goes out once.
    assert Counter(wire.posts) == Counter(set(want))
    assert len(want) - len(wire.posts) == 3


def _source(*chunks: bytes, before: Callable[[], None] = lambda: None) -> Callable[[], bytes]:
    """A publication source: `before` runs on its first read, then its chunks, then b""."""
    left = [*chunks, b""]

    def read() -> bytes:
        if len(left) == len(chunks) + 1:
            before()
        return left.pop(0)

    return read


def test_queued_puts_are_dropped_once_one_put_has_failed():
    from hypertrain.data.store import _publish_all

    # Given: 2 workers, a window of 4: objects 1 and 2 running, 3 and 4 queued behind them, and
    # the reader held on the fifth source until object 1 failed and the workers were idle.
    put_seen, go = list[bytes](), threading.Event()

    def held() -> None:
        go.set()
        time.sleep(0.2)

    def put(data: bytes) -> str:
        put_seen.append(data)
        assert go.wait(timeout=10)
        if data == b"o1":
            raise StoreError("refused")
        time.sleep(0.05)  # object 2 completes after object 1 failed
        return hashlib.sha256(data).hexdigest()

    sources = [_source(b"o%d" % n) for n in range(1, 5)] + [_source(b"o5", before=held)]
    # When / Then: the failure, and objects 3 and 4 never PUT.
    with pytest.raises(StoreError, match="refused"):
        _publish_all(sources, put, 2)
    assert sorted(put_seen) == [b"o1", b"o2"]


@pytest.mark.parametrize("raised_by", ["the reader", "the end of the publication"])
def test_the_failure_raised_is_the_first_in_time_not_the_first_in_file_order(raised_by: str):
    from hypertrain.data.store import _publish_all

    # Given: the first file's first part fails late with a store error, the second file (one
    # object) fails earlier with a 409; the reader is still at work when both have failed, or
    # every source was read before either failed.
    early_failed, late_raising, read_all = (threading.Event() for _ in range(3))
    late = b"L" * PART_BYTES

    def put(data: bytes) -> str:
        if data == late:
            assert early_failed.wait(timeout=10)
            time.sleep(0.05)  # the 409 is recorded first
            late_raising.set()
            raise StoreError("late")
        if data == b"early":
            if raised_by != "the reader":
                assert read_all.wait(timeout=10)
                time.sleep(0.05)
            early_failed.set()
            raise MinerError("409: early")
        return hashlib.sha256(data).hexdigest()

    def after_both() -> None:
        assert late_raising.wait(timeout=10)
        time.sleep(0.05)

    last = _source(b"last", before=after_both if raised_by == "the reader" else read_all.set)
    sources = [_source(late, b"tail"), _source(b"early"), last]
    # When / Then
    with pytest.raises(MinerError, match="early"):
        _publish_all(sources, put, 4)


class _Entries:
    """A lock that counts how often it was entered."""

    def __init__(self) -> None:
        self.lock, self.entered = threading.Lock(), 0

    def __enter__(self) -> None:
        self.lock.acquire()
        self.entered += 1

    def __exit__(self, *exc: object) -> None:
        self.lock.release()


def test_a_caller_of_a_digest_in_flight_waits_for_the_one_post_and_gets_its_error():
    # Given: one POST of an object, refused after a second caller asked for the same object.
    calls: list[bytes] = []
    started, release = threading.Event(), threading.Event()

    def put(data: bytes) -> str:
        calls.append(data)
        started.set()
        assert release.wait(timeout=10)
        raise StoreError("refused")

    once, errors = core._Once(put), list[BaseException]()
    once.lock = _Entries()  # type: ignore[assignment]

    def send() -> None:
        try:
            once.put(b"same")
        except StoreError as error:
            errors.append(error)

    first = threading.Thread(target=send)
    first.start()
    assert started.wait(timeout=10)
    second = threading.Thread(target=send)
    second.start()
    deadline = time.monotonic() + 10
    while once.lock.entered < 2 and time.monotonic() < deadline:  # the second found it in flight
        time.sleep(0.001)
    release.set()
    first.join(timeout=10)
    second.join(timeout=10)
    # Then: one POST, and both callers see its error.
    assert calls == [b"same"] and len(errors) == 2


def test_the_threads_of_a_publication_read_the_master_round_once(tmp_path: Path):
    # Given: a miner that has not read the round yet, behind a 50 ms run route.
    miner, _, _ = client._parts_miner(tmp_path)
    paths = _files(tmp_path, (4 * PART_BYTES, 4 * PART_BYTES, 4 * PART_BYTES, 4 * PART_BYTES))
    wire, reads = Wire(miner), [0]
    route = wire.route

    def slow_round(request: httpx.Request) -> httpx.Response:
        if request.method == "GET" and "/objects/" not in request.url.path:
            reads[0] += 1
            time.sleep(0.05)
        return route(request)

    wire.route = slow_round
    # When
    core._upload_refs(paths, miner._bounded_put(time.time() + 3600, None))
    # Then: UPLOAD_WORKERS POSTs signed with one read, not one read each.
    assert wire.peak > 1 and reads == [1]


def test_reads_stay_within_the_window_while_the_puts_are_stuck():
    from hypertrain.data.store import _publish_all

    # Given: 64 one-object sources and PUTs that do not complete until released.
    read, release, stuck = [], threading.Event(), [0]

    def source(number: int):
        chunks = iter([b"object %d" % number, b""])

        def chunk() -> bytes:
            data = next(chunks)
            if data:
                read.append(number)
                stuck[0] += not release.is_set()
            return data

        return chunk

    def put(data: bytes) -> str:
        assert release.wait(timeout=10)
        return hashlib.sha256(data).hexdigest()

    threading.Timer(0.3, release.set).start()
    # When: published with 4 workers; the window is 2 * 4 objects read and not yet stored.
    began = time.monotonic()
    keys = _publish_all((source(n) for n in range(64)), put, 4)
    # Then: while nothing was stored, at most the window (plus the one in hand) was read.
    assert time.monotonic() - began >= 0.3
    assert len(keys) == 64 and read == list(range(64))
    assert stuck[0] <= 2 * 4 + 1, stuck


def test_many_small_objects_publish_at_least_5x_faster_than_one_by_one(tmp_path: Path):
    """Client side only (loopback, 20 ms per POST): the S17b path is bound by the relay, not
    by this client (docs/adr/0017)."""
    miner, _, _ = client._parts_miner(tmp_path)
    folder = tmp_path / "many"
    folder.mkdir()
    paths = []
    for number in range(120):
        path = folder / f"o{number}"
        path.write_bytes(b"object %d " % number + os.urandom(64))
        paths.append(path)
    wire = Wire(miner, latency=0.02)
    put = miner._bounded_put(time.time() + 3600, None)
    began = time.monotonic()
    for path in paths[:40]:
        core._upload_ref(path, put)
    one_by_one = (time.monotonic() - began) * 3
    began = time.monotonic()
    core._upload_refs(paths, put)
    shared = time.monotonic() - began
    assert one_by_one / shared >= 5, (one_by_one, shared)
    assert len(wire.posts) == 40 + 120  # 40 single ones, then each of the 120 once


_probe_spec = importlib.util.spec_from_file_location(
    "fast_path_probe_fixture", Path(__file__).parent / "test_probe_trial_v2.py"
)
assert _probe_spec is not None and _probe_spec.loader is not None
probe_v2 = importlib.util.module_from_spec(_probe_spec)
sys.modules[_probe_spec.name] = probe_v2
_probe_spec.loader.exec_module(probe_v2)
network, miner = probe_v2.network, probe_v2.miner


def test_probe_v2_posts_its_four_files_concurrently_and_the_proof_is_accepted(
    miner, monkeypatch, capsys
):
    import json

    from hypertrain.miner import cli

    # Given: the in-process service behind 50 ms per object POST, counting POSTs in flight.
    n, _, config, _, requests, _ = miner
    built, lock, flight = cli.httpx.Client, threading.Lock(), [0, 0]

    def slow(**kwargs):
        c = built(**kwargs)
        route = c._transport.handler

        def handler(request: httpx.Request) -> httpx.Response:
            if request.method != "POST" or "/objects/" not in request.url.path:
                return route(request)
            with lock:
                flight[0] += 1
                flight[1] = max(flight)
            time.sleep(0.05)
            try:
                return route(request)
            finally:
                with lock:
                    flight[0] -= 1

        c._transport.handler = handler
        return c

    monkeypatch.setattr(cli.httpx, "Client", slow)
    # When
    assert cli.main(["probe-v2", "--config", str(config)]) == 0
    signed = json.loads(capsys.readouterr().out)
    # Then: the files' objects were POSTed side by side, each distinct object once ...
    posted = [
        r.url.path.rsplit("/", 1)[1]
        for r in requests
        if r.method == "POST" and "/objects/" in r.url.path
    ]
    refs = [ref["sha256"] for ref in signed["proof"]["body"]["artifact_refs"]]
    assert set(refs) <= set(posted) and len(posted) == len(set(posted))
    assert 2 <= flight[1] <= core.UPLOAD_WORKERS
    # ... and the master accepts the proof they back.
    response = n.client.post(n.url + "/join/proof", json=signed)
    assert response.status_code == 200, response.text
