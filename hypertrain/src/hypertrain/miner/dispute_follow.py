"""Continuous miner dispute follower: bounded-backoff polling plus bitwise self-replay.

DisputeWatch owns the durable cursor/outbox; this module keeps it running and answers
STEP/LAYER/OP turns from a fresh replay of the miner's own committed step.
"""

from __future__ import annotations

import argparse
import json
import logging
import signal
import threading
from collections.abc import Callable, Iterator
from contextlib import contextmanager
from pathlib import Path
from types import FrameType
from typing import TYPE_CHECKING, Any

import httpx
import numpy as np

from hypertrain.auditor.island_bisect import IslandExecution, IslandParty
from hypertrain.auditor.replay import unpack_state
from hypertrain.challenge.disputes_v2 import DisputeError, WatchEvent
from hypertrain.data.store import LocalFSStore
from hypertrain.miner.dispute_watch import DisputeWatch, HttpDisputeTransport, PublishedStates
from hypertrain.miner.island_launch import confined, validate_artifacts
from hypertrain.protocol.envelope_v2 import load_json
from hypertrain.protocol.messages import StateServe
from hypertrain.protocol.messages_v2 import IslandJobV1
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.island import emulate
from hypertrain.trainer.loop import Assignment

if TYPE_CHECKING:
    from hypertrain.miner.core import MinerConfig, NetworkMiner

log = logging.getLogger(__name__)
PartyFactory = Callable[[str, IslandJobV1, Path], IslandParty]


def committed_execution(job: IslandJobV1, published: Path) -> IslandExecution:
    """The exact step the miner committed, rebuilt from its validated published inputs."""
    validate_artifacts(job, published)
    cfg = TrainConfig.from_manifest_v2(job.manifest)

    def blob(name: str) -> bytes:
        return confined(published, job.object_paths[name]).read_bytes()

    theta, carry = unpack_state(blob("start_state"))
    ef, _ = unpack_state(blob("ef_in"))
    v0, _ = unpack_state(blob("v0"))
    dtype = "<u2" if job.manifest.training.dataset.sample_format.startswith("u16") else "<u4"
    rows = np.frombuffer(blob("samples"), dtype=dtype).reshape(
        len(job.sample_ids), cfg.model.seq_len + 1
    )
    samples = {i: row.astype(np.uint32) for i, row in zip(job.sample_ids, rows, strict=True)}
    return IslandExecution(
        cfg=cfg,
        layout=job.manifest.training.reference_spec.layout,
        assignment=Assignment(job.run_id, job.w, tuple(job.sample_ids), job.global_step0),
        theta=theta,
        get_sample=samples.__getitem__,
        carry=carry if cfg.inner.state_policy == "carry" else None,
        ef_in=ef,
        v0=v0 if cfg.inner.state_policy == "derived" else None,
    )


def replay_party(hotkey: str, job: IslandJobV1, published: Path) -> IslandParty:
    """Replay every hook; replayed checkpoints must equal the committed bytes exactly."""
    summary = load_json(confined(published, "rank-0/summary.json").read_bytes())
    if not isinstance(summary, dict):
        raise DisputeError("REPLAY_SUMMARY_TYPE")
    if summary.get("backend") != "cpu":
        # ponytail: CPU emulation cannot reproduce CUDA bits, so CUDA publications answer from
        # their actual-path training trace. Upgrade: replay on the pinned GPUs before answering.
        return IslandParty.published(hotkey, job, published)
    run = committed_execution(job, published)
    captures = [c for _, c in emulate(run.layout.n_gpus, run.trace)]
    party = IslandParty(hotkey, captures, J=run.cfg.inner.J)
    for t, state in party.states.items():
        if confined(published, f"rank-0/checkpoints/{t}.safetensors").read_bytes() != state:
            raise DisputeError("REPLAY_COMMITMENT_MISMATCH")
    return party


class Publications:
    """Per-round committed publications of one miner identity, validated once and cached."""

    def __init__(
        self,
        hotkey: str,
        run_id: str,
        locate: Callable[[int], Path],
        objects: LocalFSStore,
        party: PartyFactory = replay_party,
    ) -> None:
        self.hotkey, self.run_id, self.locate, self.objects = hotkey, run_id, locate, objects
        self.make_party = party
        self._jobs: dict[int, IslandJobV1] = {}
        self._states: dict[int, PublishedStates] = {}
        self._parties: dict[int, IslandParty] = {}

    def job(self, w: int) -> IslandJobV1:
        if w not in self._jobs:
            path = self.locate(w) / "job.json"
            if not path.is_file():
                raise DisputeError("WATCH_PUBLICATION_CONTEXT")
            job = IslandJobV1.model_validate_json(path.read_bytes())
            if (job.run_id, job.w) != (self.run_id, w):
                raise DisputeError("WATCH_PUBLICATION_CONTEXT")
            self._jobs[w] = job
        return self._jobs[w]

    def party(self, event: WatchEvent) -> IslandParty:
        w = event.turn.contest.w
        if w not in self._parties:
            self._parties[w] = self.make_party(self.hotkey, self.job(w), self.locate(w))
        return self._parties[w]

    def serve(self, event: WatchEvent) -> StateServe:
        w = event.turn.contest.w
        if w not in self._states:
            self._states[w] = PublishedStates(self.job(w), self.locate(w), self.objects)
        return self._states[w].serve(event)


def follow(
    watch: DisputeWatch,
    stop: threading.Event,
    *,
    timeout: float = 30,
    idle: float = 1.0,
    max_backoff: float = 60.0,
) -> int:
    """Poll until `stop`. Transport/5xx/429 failures back off (doubling, capped). DisputeError,
    other 4xx and a foreign lock owner propagate: retrying cannot fix them, skipping forfeits."""
    processed, delay = 0, idle
    while not stop.is_set():
        try:
            sent = watch.run_once(timeout=timeout)
        except BlockingIOError:
            raise
        except (httpx.HTTPStatusError, httpx.TransportError, OSError) as error:
            if isinstance(error, httpx.HTTPStatusError) and (
                error.response.status_code < 500 and error.response.status_code != 429
            ):
                raise
            log.warning("dispute poll failed (%s); retry in %.2fs", error, delay)
            stop.wait(delay)
            delay = min(max_backoff, delay * 2)
            continue
        processed, delay = processed + sent, idle
        if sent == 0:
            stop.wait(idle)
    return processed


@contextmanager
def sigterm_stops(stop: threading.Event) -> Iterator[None]:
    def handler(signum: int, frame: FrameType | None) -> None:
        stop.set()

    old = {s: signal.signal(s, handler) for s in (signal.SIGTERM, signal.SIGINT)}
    try:
        yield
    finally:
        for s, previous in old.items():
            signal.signal(s, previous)


def network_watch(
    miner: NetworkMiner, job_path: Path | None = None, party: PartyFactory | None = None
) -> DisputeWatch:
    """Signed long-poll transport over the miner's own per-round publications."""
    pinned = None if job_path is None else IslandJobV1.model_validate_json(job_path.read_bytes())

    def locate(w: int) -> Path:
        if pinned is not None and job_path is not None and pinned.w == w:
            return job_path.parent / "published"
        return miner.directory / str(w) / "published"

    pubs = Publications(
        miner.kp.ss58,
        miner.run_id,
        locate,
        LocalFSStore(miner.directory / "state-objects"),
        party or replay_party,
    )
    client = miner.api.c
    client.headers["X-Dispute-Signature"] = miner.kp.sign(
        f"hypertrain/watch/2|{miner.run_id}|{miner.kp.ss58}".encode()
    ).hex()
    return DisputeWatch(
        miner.directory / "watch",
        miner.kp,
        HttpDisputeTransport(client, miner.api.base + miner.url),
        run_id=miner.run_id,
        coordinator=miner.manifest.training.coord_pubkey,
        party=pubs.party,
        state_serve=pubs.serve,
        beacon=lambda: int(miner.api.call("GET", miner.url)["now_round"]),
        jobs=pubs.job,
    )


def run_follow(cfg: MinerConfig, args: argparse.Namespace) -> int:
    """`watch --follow` follows alone; `run-v2 --watch` trains while a follower thread answers."""
    from hypertrain.miner.core import MinerError, NetworkMiner

    stop = threading.Event()
    options: dict[str, float] = {"timeout": args.timeout, "max_backoff": args.max_backoff}
    with sigterm_stops(stop), httpx.Client(timeout=120, follow_redirects=False) as client:
        miner = NetworkMiner(cfg, client)
        watch = network_watch(miner, args.job)
        thread: threading.Thread | None = None
        try:
            if args.command != "run-v2":
                print(json.dumps({"processed": follow(watch, stop, **options)}), flush=True)
                return 0
            failures: list[BaseException] = []

            def background() -> None:
                try:
                    follow(watch, stop, **options)
                except BaseException as error:
                    failures.append(error)
                finally:
                    stop.set()

            thread = threading.Thread(target=background, name="dispute-follow", daemon=True)
            thread.start()
            if args.job is not None:
                result: dict[str, Any] = miner.launch(args.job)
            elif args.round is not None:
                result = {"status": miner.run_round(args.round)}
            else:
                raise MinerError("run-v2 requires --round or --job")
            print(json.dumps(result, sort_keys=True), flush=True)
            stop.wait()
            thread.join()
            if failures:
                raise failures[0]
            return 0
        finally:
            stop.set()
            if thread is not None:
                thread.join()
            watch.close()
