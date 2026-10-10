"""Independent watch over real service events, restart outbox and published StateServe."""

from __future__ import annotations

import importlib.util
import sys
import threading
from dataclasses import replace
from pathlib import Path
from typing import Any

import httpx
import pytest

from hypertrain.auditor.island_bisect import IslandParty
from hypertrain.challenge.disputes_v2 import (
    DisputeError,
    EventAck,
    WatchEvent,
    authentic,
    signed_message,
)
from hypertrain.data.store import LocalFSStore
from hypertrain.gpu_ops.journal import flock
from hypertrain.miner.dispute_watch import DisputeWatch, PublishedStates
from hypertrain.miner.island_launch import launch_island
from hypertrain.protocol.envelope_v2 import parse_envelope
from hypertrain.protocol.hashing import MerkleTree
from hypertrain.protocol.messages import StateServe
from hypertrain.trainer.island import emulate


def helpers(name: str) -> Any:
    spec = importlib.util.spec_from_file_location(
        "l5_watch_" + name, Path(__file__).parents[1] / name
    )
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


class Transport:
    def __init__(self, service: Any, now: int = 2) -> None:
        self.service, self.now = service, now
        self.crash = False
        self.accepted: list[bytes] = []
        self.serves: list[StateServe] = []

    def events(self, party: str, cursor: int, *, timeout: float) -> list[WatchEvent]:
        return self.service.events(party, cursor, timeout=timeout)

    def acknowledge(self, ack: EventAck) -> None:
        self.service.acknowledge(ack, now=self.now)

    def submit(self, envelope: bytes) -> None:
        env = parse_envelope(envelope)
        if env.type == "BisectV2":
            self.service.bisect(envelope, now=self.now)
        else:
            self.serves.append(StateServe.model_validate(env.body))
        self.accepted.append(envelope)
        if self.crash:
            self.crash = False
            raise ConnectionError("accepted before connection lost")


def setup_watch(tmp_path: Path) -> Any:
    challenge = helpers("challenge/test_disputes_v2.py")
    service, f, ref, _, raw = challenge.service(tmp_path)
    turn = service.open(raw, now=1)
    run = helpers("auditor/test_island_bisect_v2.py").execution()
    traced = emulate(2, run.trace)
    party = IslandParty(f.HOT.ss58, [c for _, c in traced], J=1)
    transport = Transport(service)
    setup = replace(f.setup(), manifest=service.escrow.manifest)
    job = f.stage(
        setup,
        tmp_path / "job-input",
        tuple(range(setup.manifest.training.batch_samples())),
        turn.contest.w,
    )

    def state(event: WatchEvent) -> StateServe:
        raise AssertionError("state not requested")

    def watch() -> DisputeWatch:
        return DisputeWatch(
            tmp_path / "watch",
            f.HOT,
            transport,
            run_id=service.run_id,
            coordinator=f.COORD.ss58,
            party=lambda _: party,
            state_serve=state,
            beacon=lambda: transport.now,
            job=job,
        )

    return service, f, ref, turn, party, transport, watch


def test_cursor_exactly_once_when_restart_after_accepted_send(tmp_path: Path) -> None:
    # Given
    service, _, _, turn, _, transport, factory = setup_watch(tmp_path)
    watch = factory()
    transport.crash = True
    # When
    with pytest.raises(ConnectionError):
        watch.run_once(timeout=0)
    watch.close()
    restored = factory()
    assert restored.run_once(timeout=0) == 1
    # Then
    assert transport.accepted[0] == transport.accepted[1]
    assert service.get(turn.dispute_id).seq == 1
    assert service.db.execute("SELECT COUNT(*) FROM dispute_entries_v2").fetchone()[0] == 1
    assert restored.cursor == 1 and restored.run_once(timeout=0) == 0
    restored.close()


def test_watch_replies_when_layer_and_op_turns_arrive(tmp_path: Path) -> None:
    # Given
    service, f, _, turn, party, transport, factory = setup_watch(tmp_path)
    watch = factory()
    challenge = helpers("challenge/test_disputes_v2.py")
    service.span = lambda c, level, ctx: party.span(level, ctx)
    # When: actual hashes from watch, first differing remote endpoint at every span.
    watch.run_once(timeout=0)
    for level in ("layer", "op"):
        for _ in range(20):
            current = service.get(turn.dispute_id)
            first = service.db.execute(
                "SELECT body FROM dispute_entries_v2 WHERE id=? AND seq=?",
                (
                    turn.dispute_id,
                    current.seq - 1,
                ),
            ).fetchone()[0]
            from hypertrain.protocol.messages_v2 import BisectV2

            claim = BisectV2.model_validate_json(first)
            challenge.reply(service, f.AUDITOR, [claim.hashes[0], "aa" * 32, "bb" * 32])
            current = service.get(turn.dispute_id)
            watch.run_once(timeout=0)
            if current.level == level:
                break
        else:
            raise AssertionError("bounded transcript did not progress")
    # Then
    levels = [parse_envelope(raw).body["level"] for raw in transport.accepted]
    assert list(dict.fromkeys(levels)) == ["step", "layer", "op"]
    watch.close()


def test_watch_lock_when_second_process_owns_directory(tmp_path: Path) -> None:
    # Given
    _, _, _, _, _, _, factory = setup_watch(tmp_path)
    watch = factory()
    # When / Then
    with flock(watch.directory / "watch.lock"), pytest.raises(BlockingIOError):
        watch.run_once(timeout=0)
    assert watch.cursor == 0
    watch.close()


def test_event_rejects_when_coordinator_signature_changed(tmp_path: Path) -> None:
    # Given
    service, f, _, _, _, transport, factory = setup_watch(tmp_path)
    event = service.events(f.HOT.ss58, 0)[0]
    watch = factory()
    transport.events = lambda *args, **kwargs: [event.model_copy(update={"sig": "00" * 64})]
    # When / Then
    with pytest.raises(DisputeError, match="AUTHORITY"):
        watch.run_once(timeout=0)
    assert watch.cursor == 0 and not transport.accepted
    watch.close()


@pytest.mark.parametrize("kind,wrong_round", [("turn", True), ("state", True), ("turn", False)])
def test_watch_rejects_signed_same_run_wrong_round(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, kind: str, wrong_round: bool
) -> None:
    """Authentic event; no emulation, trace replay or training needed for early rejection."""
    challenge = helpers("challenge/test_disputes_v2.py")
    service, f, _, _, raw = challenge.service(tmp_path)
    turn = service.open(raw, now=1)
    event = service.events(f.HOT.ss58, 0, timeout=0)[0]
    wrong = event.model_copy(
        update={
            "kind": kind,
            "checkpoint": 1 if kind == "state" else None,
            "turn": event.turn.model_copy(
                update={
                    "contest": turn.contest.model_copy(
                        update={"w": turn.contest.w + int(wrong_round)}
                    )
                }
            ),
            "sig": "",
        }
    )
    wrong = wrong.model_copy(update={"sig": f.COORD.sign(signed_message(wrong)).hex()})
    assert authentic(wrong)
    transport = Transport(service)
    calls = []
    monkeypatch.setattr(transport, "events", lambda *a, **kw: [wrong])
    monkeypatch.setattr(transport, "submit", lambda raw: calls.append("send"))
    monkeypatch.setattr(transport, "acknowledge", lambda raw: calls.append("ack"))

    def party(dispute: Any) -> Any:
        from types import SimpleNamespace

        calls.append("trace")
        return SimpleNamespace(hotkey=f.HOT.ss58, hashes=lambda *a: ["11" * 32] * 3)

    setup = replace(f.setup(), manifest=service.escrow.manifest)
    watch = DisputeWatch(
        tmp_path / "watch",
        f.HOT,
        transport,
        run_id=service.run_id,
        coordinator=f.COORD.ss58,
        party=party,
        state_serve=lambda event: calls.append("state"),  # type: ignore[arg-type,return-value]
        beacon=lambda: transport.now,
        job=f.stage(
            setup,
            tmp_path / "job-input",
            tuple(range(setup.manifest.training.batch_samples())),
            turn.contest.w,
        ),
    )
    try:
        if wrong_round:
            with pytest.raises(DisputeError, match="WATCH_PUBLICATION_CONTEXT"):
                watch.run_once(timeout=0)
            assert calls == [] and watch.cursor == 0
            assert watch.db.execute("SELECT COUNT(*) FROM watch_outbox").fetchone()[0] == 0
        else:
            assert watch.run_once(timeout=0) == 1
            assert calls == ["trace", "ack", "send"] and watch.cursor == event.cursor
    finally:
        watch.close()


def test_state_serve_when_real_published_checkpoint_requested(tmp_path: Path) -> None:
    # Given
    challenge = helpers("challenge/test_disputes_v2.py")
    service, f, _, _, raw = challenge.service(tmp_path)
    launch = helpers("miner/test_island_launch_v2.py")
    job = launch.staged_job(tmp_path / "runtime", 2, 2)
    artifacts = launch_island(job, tmp_path / "runtime", backend="cpu", trace=True)
    objects = LocalFSStore(tmp_path / "objects")
    states = PublishedStates(job, artifacts.directory, objects)
    party = IslandParty.published(f.HOT.ss58, job, artifacts.directory)
    turn = service.open(raw, now=1)
    event = service.state_request(turn.dispute_id, 1, now=2)
    # Event uses the runtime's actual job identity; production L0 pins the same run/w.
    event = event.model_copy(
        update={
            "run_id": job.run_id,
            "turn": event.turn.model_copy(
                update={
                    "contest": event.turn.contest.model_copy(update={"w": job.w}),
                }
            ),
        }
    )
    # When
    serve = states.serve(event)
    # Then
    assert serve.tensor_root == party.state_root(1)
    assert serve.t == states.leaves[1].t
    assert MerkleTree.verify(
        bytes.fromhex(states.leaves[1].digest()),
        1,
        [bytes.fromhex(h) for h in serve.merkle_proof_leaf_in_leaves_root],
        states.tree.root,
        len(states.leaves),
    )
    assert serve.uri.startswith("file:")
    assert objects.get(Path(serve.uri).name) == party.state_blob(1)
    bad = event.model_copy(update={"checkpoint": 9999})
    with pytest.raises(DisputeError):
        states.serve(bad)


def test_public_watch_layer_after_socket_long_poll(tmp_path: Path) -> None:
    """Actual CLI/socket transport, canonical captured trace, subscribed signed transition."""
    import contextlib
    import io
    import json
    import sqlite3
    import threading
    from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

    from hypertrain.miner.cli import main
    from hypertrain.protocol.envelope_v2 import seal
    from hypertrain.protocol.jcs import canonicalize
    from hypertrain.protocol.keys import decode_hotkey, verify
    from hypertrain.protocol.messages_v2 import BisectV2

    challenge = helpers("challenge/test_disputes_v2.py")
    service, f, _, _, raw = challenge.service(tmp_path)
    setup = replace(f.setup(), manifest=service.escrow.manifest)
    directory = tmp_path / "publication"
    job = f.stage(setup, directory, tuple(range(setup.manifest.training.batch_samples())), 0)
    job_path = directory / "job.json"
    job_path.write_bytes(canonicalize(job.body()))
    artifacts = launch_island(job, directory, backend="cpu", trace=True)
    party = IslandParty.published(f.HOT.ss58, job, artifacts.directory)
    states = PublishedStates(job, artifacts.directory, LocalFSStore(tmp_path / "objects"))
    service.span = lambda contest, level, ctx: party.span(level, ctx)
    turn = service.open(raw, now=1)
    work = tmp_path / "client"
    watch_path = work / service.run_id / f.HOT.ss58 / "watch"
    initial = DisputeWatch(
        watch_path,
        f.HOT,
        Transport(service),
        run_id=service.run_id,
        coordinator=f.COORD.ss58,
        party=lambda _: party,
        state_serve=states.serve,
        beacon=lambda: 2,
        job=job,
    )
    try:
        assert initial.run_once(timeout=0) == 1
        first_cursor = initial.cursor
    finally:
        initial.close()
    step = BisectV2.model_validate_json(
        service.db.execute("SELECT body FROM dispute_entries_v2 WHERE seq=0").fetchone()[0]
    )
    waiting, completed = threading.Event(), threading.Event()

    class SubscribedCondition(threading.Condition):
        def wait(self, timeout: float | None = None) -> bool:
            waiting.set()
            return super().wait(timeout)

    service.condition = SubscribedCondition()
    envelope = seal(f.COORD, "RunManifestV2", service.run_id, job.manifest, 100000)
    run_url = "/v2/runs/" + service.run_id
    requests = []

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *args: Any) -> None:
            pass

        def response(self, body: Any, status: int = 200) -> None:
            encoded = json.dumps(body).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(encoded)))
            self.end_headers()
            self.wfile.write(encoded)

        def do_GET(self) -> None:
            from urllib.parse import parse_qs, urlsplit

            url = urlsplit(self.path)
            requests.append(url.path)
            if url.path == run_url:
                self.response({"manifest_envelope": envelope, "now_round": 2})
                return
            assert url.path == run_url + "/disputes"
            query = parse_qs(url.query)
            identity = query["party"][0]
            assert verify(
                decode_hotkey(identity),
                f"hypertrain/watch/2|{service.run_id}|{identity}".encode(),
                bytes.fromhex(self.headers["X-Dispute-Signature"]),
            )
            events = service.events(
                identity, int(query["cursor"][0]), timeout=float(query["timeout"][0])
            )
            self.response([event.model_dump(mode="json") for event in events])

        def do_POST(self) -> None:
            body = self.rfile.read(int(self.headers["Content-Length"]))
            requests.append(self.path)
            if self.path == run_url + "/disputes/ack":
                ack = EventAck.model_validate_json(body)
                service.acknowledge(ack, now=2)
                self.response({"accepted": ack.cursor})
            else:
                assert self.path == run_url + "/bisect"
                self.response(service.bisect(body, now=2).model_dump(mode="json"))

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    server.daemon_threads = True
    # Binding/listening precedes trigger; serving thread completion is bounded below.
    serving = threading.Thread(target=server.serve_forever, daemon=True)
    key = tmp_path / "miner.key"
    key.write_bytes(b"\x07" * 32)
    key.chmod(0o600)
    config = tmp_path / "miner.toml"
    config.write_text(
        f'api = "http://127.0.0.1:{server.server_port}"\n'
        f'keyfile = "{key}"\nworkdir = "{work}"\nstate_source = "file"\n'
        f'image_digest = "{job.manifest.training.reference_spec.image_digest}"\n'
        f'run_id = "{service.run_id}"\nowner_hotkey = "{f.COORD.ss58}"\n'
        'device = "cpu"\n'
    )
    output, failures, result = io.StringIO(), [], []

    def client() -> Any:
        try:
            with contextlib.redirect_stdout(output):
                result.append(
                    main(
                        ["watch", "--config", str(config), "--job", str(job_path), "--timeout", "5"]
                    )
                )
        except BaseException as error:
            failures.append(error)
        finally:
            completed.set()

    worker = threading.Thread(target=client, daemon=True)
    serving.start()
    try:
        worker.start()
        assert waiting.wait(15), failures
        assert not completed.is_set()
        layer, _ = challenge.reply(service, f.AUDITOR, [step.hashes[0], "aa" * 32, "bb" * 32])
        assert layer.level == "layer" and layer.expected_party == f.HOT.ss58
        assert completed.wait(15)
        worker.join(1)
        assert not worker.is_alive() and not failures and result == [0]
        assert json.loads(output.getvalue()) == {"processed": 1}
        accepted = BisectV2.model_validate_json(
            service.db.execute(
                "SELECT body FROM dispute_entries_v2 WHERE id=? AND seq=?",
                (turn.dispute_id, layer.seq),
            ).fetchone()[0]
        )
        assert (
            accepted.level,
            accepted.ctx,
            accepted.party,
            accepted.previous_transcript_hash,
        ) == ("layer", layer.ctx, f.HOT.ss58, layer.transcript_hash)
        lo, hi = layer.interval
        assert accepted.hashes == party.hashes(
            "layer", tuple(layer.ctx), [lo, -(-(lo + hi) // 2), hi]
        )
        event = service.events(f.HOT.ss58, first_cursor, timeout=0)[0]
        assert (
            service.db.execute(
                "SELECT COUNT(*) FROM dispute_acks_v2 WHERE cursor=? AND party=?",
                (event.cursor, f.HOT.ss58),
            ).fetchone()[0]
            == 1
        )
        with sqlite3.connect(watch_path / "watch.db") as saved:
            cursor, durable, done = saved.execute(
                "SELECT cursor,envelope,done FROM watch_outbox ORDER BY cursor DESC LIMIT 1"
            ).fetchone()
            assert cursor == event.cursor and done == 1
            assert BisectV2.model_validate(parse_envelope(durable).body) == accepted
        assert run_url + "/disputes" in requests
        assert run_url + "/disputes/ack" in requests and run_url + "/bisect" in requests
    finally:
        server.shutdown()
        server.server_close()
        serving.join(5)
        worker.join(15)
        assert not serving.is_alive() and not worker.is_alive()


class RecordingStop(threading.Event):
    def __init__(self) -> None:
        super().__init__()
        self.waits: list[float] = []

    def wait(self, timeout: float | None = None) -> bool:
        self.waits.append(float(timeout or 0))
        return self.is_set()


def test_follow_backs_off_on_transport_failure_then_answers(tmp_path: Path) -> None:
    from hypertrain.miner.dispute_follow import follow

    service, _, _, turn, _, transport, watch_factory = setup_watch(tmp_path)
    watch = watch_factory()
    real, calls = transport.events, []
    stop = RecordingStop()

    def flaky(party: str, cursor: int, *, timeout: float) -> list[WatchEvent]:
        calls.append(cursor)
        if len(calls) <= 3:
            raise httpx.ConnectError("relay down")
        if len(calls) == 5:
            stop.set()
        return real(party, cursor, timeout=timeout)

    transport.events = flaky
    try:
        assert follow(watch, stop, timeout=0, idle=0.5, max_backoff=1.5) == 1
        assert stop.waits == [0.5, 1.0, 1.5, 0.5]
        assert service.get(turn.dispute_id).seq == 1 and watch.cursor > 0
    finally:
        watch.close()


@pytest.mark.parametrize("status,fatal", [(403, True), (409, True), (429, False), (503, False)])
def test_follow_status_policy(tmp_path: Path, status: int, fatal: bool) -> None:
    from hypertrain.miner.dispute_follow import follow

    *_, transport, watch_factory = setup_watch(tmp_path)
    watch = watch_factory()
    stop = RecordingStop()
    request = httpx.Request("GET", "https://service.test/disputes")

    def failing(party: str, cursor: int, *, timeout: float) -> list[WatchEvent]:
        if stop.waits:
            stop.set()
        raise httpx.HTTPStatusError("x", request=request, response=httpx.Response(status))

    transport.events = failing
    try:
        if fatal:
            with pytest.raises(httpx.HTTPStatusError):
                follow(watch, stop, timeout=0)
            assert stop.waits == []
        else:
            assert follow(watch, stop, timeout=0, idle=0.25) == 0
            assert stop.waits == [0.25, 0.5]
    finally:
        watch.close()


def test_sigterm_ends_follow_cleanly_and_restores_handler(tmp_path: Path) -> None:
    import os
    import signal

    from hypertrain.miner.dispute_follow import follow, sigterm_stops

    service, _, _, turn, _, transport, watch_factory = setup_watch(tmp_path)
    watch = watch_factory()
    real = transport.events

    def terminate_after_poll(party: str, cursor: int, *, timeout: float) -> list[WatchEvent]:
        os.kill(os.getpid(), signal.SIGTERM)
        return real(party, cursor, timeout=timeout)

    transport.events = terminate_after_poll
    previous = signal.getsignal(signal.SIGTERM)
    stop = threading.Event()
    try:
        with sigterm_stops(stop):
            assert follow(watch, stop, timeout=0, idle=0) == 1
        assert stop.is_set() and signal.getsignal(signal.SIGTERM) == previous
        assert service.get(turn.dispute_id).seq == 1
    finally:
        watch.close()


def test_follow_rejects_round_without_local_publication(tmp_path: Path) -> None:
    from hypertrain.miner.dispute_follow import Publications

    service, f, _, _, party, transport, _ = setup_watch(tmp_path)
    pubs = Publications(
        f.HOT.ss58, service.run_id, lambda w: tmp_path / str(w), LocalFSStore(tmp_path / "o")
    )
    watch = DisputeWatch(
        tmp_path / "follow-watch",
        f.HOT,
        transport,
        run_id=service.run_id,
        coordinator=f.COORD.ss58,
        party=lambda _: party,
        state_serve=pubs.serve,
        beacon=lambda: transport.now,
        jobs=pubs.job,
    )
    try:
        with pytest.raises(DisputeError, match="WATCH_PUBLICATION_CONTEXT"):
            watch.run_once(timeout=0)
        assert watch.cursor == 0 and not transport.accepted
    finally:
        watch.close()


def test_add_watch_args_extends_existing_parsers() -> None:
    import argparse

    from hypertrain.miner.dispute_watch import add_watch_args

    watch = argparse.ArgumentParser()
    watch.add_argument("--timeout", type=float, default=30)
    add_watch_args(watch)
    run = argparse.ArgumentParser()
    add_watch_args(run)
    a = watch.parse_args(["--follow", "--max-backoff", "5"])
    b = run.parse_args(["--watch", "--timeout", "2"])
    assert (a.follow, a.watch, a.max_backoff, a.timeout) == (True, False, 5.0, 30.0)
    assert (b.follow, b.watch, b.max_backoff, b.timeout) == (False, True, 60.0, 2.0)
