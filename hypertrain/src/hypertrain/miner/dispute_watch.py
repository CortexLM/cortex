"""Training-independent signed long-poll watch with durable cursor/outbox.

Crash after delivery reuses the identical signed reply. The service's semantic
reservation makes its effect exactly once; networks cannot promise one send.
"""

from __future__ import annotations

import argparse
import sqlite3
from collections.abc import Callable
from pathlib import Path
from typing import TYPE_CHECKING, Protocol

import httpx
from pydantic import TypeAdapter

from hypertrain.auditor.island_bisect import IslandParty
from hypertrain.auditor.replay import tensor_root, unpack_state
from hypertrain.challenge.disputes_v2 import (
    DisputeError,
    EventAck,
    WatchEvent,
    authentic,
    signed_message,
)
from hypertrain.data.store import Store
from hypertrain.gpu_ops.journal import flock
from hypertrain.miner.island_launch import confined, validate_artifacts
from hypertrain.protocol.envelope_v2 import load_json, seal
from hypertrain.protocol.hashing import MerkleTree
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import LeafPreimage, StateServe
from hypertrain.protocol.messages_v2 import BisectV2, IslandJobV1

if TYPE_CHECKING:
    from hypertrain.miner.core import MinerConfig


class DisputeTransport(Protocol):
    def events(self, party: str, cursor: int, *, timeout: float) -> list[WatchEvent]: ...
    def acknowledge(self, ack: EventAck) -> None: ...
    def submit(self, envelope: bytes) -> None: ...


class HttpDisputeTransport:
    """L0 routes own role authorization. Client bounds response bytes before parsing."""

    def __init__(self, client: httpx.Client, run_url: str) -> None:
        self.client, self.url = client, run_url.rstrip("/")

    def events(self, party: str, cursor: int, *, timeout: float) -> list[WatchEvent]:
        if not 0 <= timeout <= 30:
            raise DisputeError("LONG_POLL_BOUND")
        with self.client.stream(
            "GET",
            self.url + "/disputes",
            params={
                "party": party,
                "cursor": cursor,
                "timeout": timeout,
            },
            timeout=timeout + 5,
        ) as response:
            response.raise_for_status()
            data = bytearray()
            for chunk in response.iter_bytes():
                data.extend(chunk)
                if len(data) > 1_048_576:
                    raise DisputeError("EVENT_RESPONSE_BOUND")
        parsed = load_json(b'{"items":' + bytes(data) + b"}")
        events = TypeAdapter(list[WatchEvent]).validate_python(parsed["items"])
        if len(events) > 64:
            raise DisputeError("EVENT_COUNT_BOUND")
        return events

    def acknowledge(self, ack: EventAck) -> None:
        response = self.client.post(
            self.url + "/disputes/ack", content=canonicalize(ack.body()), timeout=5
        )
        response.raise_for_status()

    def submit(self, envelope: bytes) -> None:
        raw = load_json(envelope)
        if not isinstance(raw, dict):
            raise DisputeError("ENVELOPE_TYPE")
        route = {"BisectV2": "bisect", "StateServe": "state-serve"}.get(str(raw["type"]))
        if route is None:
            raise DisputeError("WATCH_REPLY_TYPE")
        response = self.client.post(self.url + "/" + route, content=envelope, timeout=5)
        response.raise_for_status()


class PublishedStates:
    """Serve validated L1 leaf checkpoints, original Merkle preimages and existing Store."""

    def __init__(self, job: IslandJobV1, directory: Path, objects: Store) -> None:
        artifacts = validate_artifacts(job, directory)
        self.job, self.directory, self.objects = job, directory, objects
        raw = load_json(b'{"items":' + artifacts.leaves.read_bytes() + b"}")["items"]
        if not isinstance(raw, list):
            raise DisputeError("LEAF_ARTIFACT_TYPE")
        self.leaves: list[LeafPreimage] = [LeafPreimage.model_validate(x) for x in raw]
        self.tree: MerkleTree = MerkleTree([bytes.fromhex(leaf.digest()) for leaf in self.leaves])

    def serve(self, event: WatchEvent) -> StateServe:
        leaf = event.checkpoint
        if (
            leaf is None
            or not 0 <= leaf < len(self.leaves)
            or (event.turn.contest.w != self.job.w or event.run_id != self.job.run_id)
        ):
            raise DisputeError("STATE_CHECKPOINT_BINDING")
        t = self.leaves[leaf].t
        blob = confined(self.directory, f"rank-0/checkpoints/{t}.safetensors").read_bytes()
        theta, state = unpack_state(blob)
        if state is None:
            raise DisputeError("CHECKPOINT_OPTIMIZER_MISSING")
        key = self.objects.put(blob)
        return StateServe(
            hotkey=event.party,
            challenge_hash=event.turn.contest.challenge_hash,
            t=t,
            uri=self.objects.presign(key),
            tensor_root=tensor_root(theta, state),
            merkle_proof_leaf_in_leaves_root=[h.hex() for h in self.tree.proof(leaf)],
        )


class DisputeWatch:
    """Mutable local delivery journal; one process owns a watch directory via flock."""

    def __init__(
        self,
        directory: Path,
        key: Keypair,
        transport: DisputeTransport,
        *,
        run_id: str,
        coordinator: str,
        party: Callable[[WatchEvent], IslandParty],
        state_serve: Callable[[WatchEvent], StateServe],
        beacon: Callable[[], int],
        job: IslandJobV1 | None = None,
        jobs: Callable[[int], IslandJobV1] | None = None,
    ) -> None:
        """`job` pins one round (operator pass); `jobs` resolves each contested round (follow)."""
        publication = getattr(state_serve, "__self__", None)
        if job is None and isinstance(publication, PublishedStates):
            job = publication.job
        if jobs is None and (job is None or job.run_id != run_id):
            raise DisputeError("WATCH_PUBLICATION_CONTEXT")
        self.job = None if job is None else IslandJobV1.model_validate(job.model_dump(mode="json"))
        self.jobs = jobs
        directory.mkdir(parents=True, exist_ok=True)
        self.directory, self.key, self.transport = directory, key, transport
        self.run_id, self.coordinator = run_id, coordinator
        self.party, self.state_serve, self.beacon = party, state_serve, beacon
        # One owner at a time (follower thread or caller); flock excludes other processes.
        self.db = sqlite3.connect(
            directory / "watch.db", isolation_level=None, check_same_thread=False
        )
        self.db.execute("PRAGMA journal_mode=WAL")
        self.db.execute("PRAGMA synchronous=FULL")
        self.db.executescript("""
          CREATE TABLE IF NOT EXISTS watch_identity(run TEXT,party TEXT,coordinator TEXT);
          CREATE TABLE IF NOT EXISTS watch_outbox(cursor INTEGER PRIMARY KEY,event TEXT,
              ack TEXT,envelope BLOB,done INTEGER NOT NULL DEFAULT 0);
        """)
        identity = self.db.execute("SELECT * FROM watch_identity").fetchone()
        expected = (run_id, key.ss58, coordinator)
        if identity is not None and identity != expected:
            raise DisputeError("WATCH_IDENTITY_CHANGED")
        if identity is None:
            self.db.execute("INSERT INTO watch_identity VALUES(?,?,?)", expected)

    @property
    def cursor(self) -> int:
        return int(
            self.db.execute(
                "SELECT COALESCE(MAX(cursor),0) FROM watch_outbox WHERE done=1",
            ).fetchone()[0]
        )

    def _validate_event(self, event: WatchEvent) -> None:
        if not authentic(event) or (event.run_id, event.party, event.signer, event.turn.run_id) != (
            self.run_id,
            self.key.ss58,
            self.coordinator,
            self.run_id,
        ):
            raise DisputeError("WATCH_EVENT_AUTHORITY")
        w = event.turn.contest.w
        job = self.jobs(w) if self.jobs is not None else self.job
        if job is None or (job.run_id, job.w) != (self.run_id, w):
            raise DisputeError("WATCH_PUBLICATION_CONTEXT")

    def _prepare(self, event: WatchEvent) -> None:
        self._validate_event(event)
        old = self.db.execute(
            "SELECT event FROM watch_outbox WHERE cursor=?", (event.cursor,)
        ).fetchone()
        if old is not None:
            if WatchEvent.model_validate_json(old[0]) != event:
                raise DisputeError("WATCH_CURSOR_CONFLICT")
            return
        now = self.beacon()
        if (
            not event.issued_beacon
            <= now
            <= min(
                event.turn.turn_deadline,
                event.turn.absolute_deadline,
            )
        ):
            raise DisputeError("WATCH_EVENT_EXPIRED")
        match event.kind:
            case "turn":
                turn = event.turn
                executor = self.party(event)
                if executor.hotkey != self.key.ss58 or turn.expected_party != self.key.ss58:
                    raise DisputeError("WATCH_PARTY_BINDING")
                lo, hi = turn.interval
                at = [lo, -(-(lo + hi) // 2), hi]  # N2 remains three hashes for a unit span
                body = BisectV2(
                    dispute_id=turn.dispute_id,
                    seq=turn.seq,
                    level=turn.level,
                    ctx=turn.ctx,
                    interval=turn.interval,
                    N=2,
                    hashes=executor.hashes(turn.level, tuple(turn.ctx), at),
                    party=self.key.ss58,
                    previous_transcript_hash=turn.transcript_hash,
                )
                envelope = seal(self.key, "BisectV2", self.run_id, body, turn.absolute_deadline)
            case "state":
                serve = self.state_serve(event)
                if (
                    serve.hotkey != self.key.ss58
                    or serve.challenge_hash != event.turn.contest.challenge_hash
                ):
                    raise DisputeError("STATE_SERVE_IDENTITY")
                envelope = seal(
                    self.key, "StateServe", self.run_id, serve, event.turn.absolute_deadline
                )
        ack = EventAck(
            run_id=self.run_id,
            cursor=event.cursor,
            event_hash=event.digest(),
            party=self.key.ss58,
            received_beacon=now,
            sig="",
        )
        ack = ack.model_copy(update={"sig": self.key.sign(signed_message(ack)).hex()})
        self.db.execute(
            "INSERT INTO watch_outbox(cursor,event,ack,envelope) VALUES(?,?,?,?)",
            (
                event.cursor,
                event.model_dump_json(),
                ack.model_dump_json(),
                canonicalize(envelope),
            ),
        )

    def run_once(self, *, timeout: float = 30) -> int:
        """Validate publication context before any durable pending or fresh sends."""
        with flock(self.directory / "watch.lock", blocking=False):
            for row in self.db.execute("SELECT event FROM watch_outbox WHERE done=0"):
                self._validate_event(WatchEvent.model_validate_json(row[0]))
            pending = self.db.execute(
                "SELECT cursor,ack,envelope FROM watch_outbox WHERE done=0 ORDER BY cursor",
            ).fetchall()
            requested_cursor = max([self.cursor, *(row[0] for row in pending)])
            events = self.transport.events(self.key.ss58, requested_cursor, timeout=timeout)
            for event in events:
                self._validate_event(event)
            sent = 0
            for cursor, ack, envelope in pending:
                self.transport.acknowledge(EventAck.model_validate_json(ack))
                self.transport.submit(envelope)
                self.db.execute("UPDATE watch_outbox SET done=1 WHERE cursor=?", (cursor,))
                sent += 1
            if len(events) > 64 or any(
                e.cursor <= self.cursor or (i and e.cursor <= events[i - 1].cursor)
                for i, e in enumerate(events)
            ):
                raise DisputeError("WATCH_EVENT_ORDER_OR_BOUND")
            for event in events:
                self._validate_event(event)
            for event in events:
                self._prepare(event)
                row = self.db.execute(
                    "SELECT ack,envelope FROM watch_outbox WHERE cursor=?", (event.cursor,)
                ).fetchone()
                self.transport.acknowledge(EventAck.model_validate_json(row[0]))
                self.transport.submit(row[1])
                self.db.execute("UPDATE watch_outbox SET done=1 WHERE cursor=?", (event.cursor,))
                sent += 1
            return sent

    def close(self) -> None:
        self.db.close()


def add_watch_args(parser: argparse.ArgumentParser) -> None:
    """`watch --follow` / `run-v2 --watch`: continuous dispute follower flags."""
    parser.add_argument("--follow", action="store_true", help="poll disputes until SIGTERM")
    parser.add_argument("--watch", action="store_true", help="follow disputes alongside training")
    parser.add_argument("--max-backoff", type=float, default=60.0)
    if not any("--timeout" in a.option_strings for a in parser._actions):
        parser.add_argument("--timeout", type=float, default=30)


def run_follow(cfg: MinerConfig, args: argparse.Namespace) -> int:
    from hypertrain.miner.dispute_follow import run_follow as follow

    return follow(cfg, args)
