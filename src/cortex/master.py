"""Master composition, live chain epoch clock and durable exact-E emission."""

from __future__ import annotations

import asyncio
import logging
import sqlite3
import time
from collections.abc import Callable
from contextlib import ExitStack, asynccontextmanager
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Protocol, cast
from urllib.parse import urlsplit

import httpx
from fastapi import FastAPI
from fastapi.responses import JSONResponse

from cortex.challenges.client import ChallengeClient, leaf_scores
from cortex.challenges.proxy import create_router as challenge_router
from cortex.challenges.registry import RegistryEntry, load_registry
from cortex.config import MasterConfig, read_seed
from cortex.errors import ServiceError
from cortex.gateway import GatewayService, GatewayStore
from cortex.gateway import create_router as gateway_router
from cortex.http import OperatorAuth, read_private_file
from cortex.proof.api import create_router as proof_router
from cortex.proof.artifacts import FileVault
from cortex.proof.models import Submission
from cortex.proof.scoring import SCORE_MAX
from cortex.proof.service import EvaluationBackend, ProofService, UnwiredBackend
from cortex.proof.store import ProofStore
from cortex.protocol import NoScore, NoScoreReason, Score, TrustRoot, sign_leaf
from cortex.protocol.crypto import decode_hotkey, public_key
from cortex.protocol.merkle import canonical_rows
from cortex.protocol.models import FULL_SHARE_SCORE
from cortex.protocol.scale import uint
from cortex.state import prepare_master_state
from cortex.validator import ChainSnapshot


def _public_chain_endpoint(value: object) -> str:
    if not isinstance(value, str) or not value or len(value) > 2048:
        return ""
    parsed = urlsplit(value)
    if not parsed.scheme:
        return value if len(value) <= 64 and all(c.isalnum() or c in "._-" for c in value) else ""
    try:
        port = parsed.port
    except ValueError:
        return ""
    if parsed.scheme not in {"ws", "wss"} or not parsed.hostname:
        return ""
    hostname = f"[{parsed.hostname}]" if ":" in parsed.hostname else parsed.hostname
    return f"{parsed.scheme}://{hostname}{f':{port}' if port is not None else ''}"


@dataclass(frozen=True)
class EpochState:
    epoch: int
    last_epoch_block: int
    current_block: int

    def validate(self) -> None:
        for value in (self.epoch, self.last_epoch_block, self.current_block):
            uint(value, 8)
        if self.last_epoch_block > self.current_block:
            raise ServiceError(503, "invalid chain epoch boundary")


class EpochProvider(Protocol):
    async def epoch_state(self, netuid: int) -> EpochState: ...


class BittensorEpochProvider:
    def __init__(self, subtensor: Any):
        self.subtensor = subtensor

    async def epoch_state(self, netuid: int) -> EpochState:
        def read():
            tip = self.subtensor.get_current_block()
            before = self.subtensor.get_block_hash(tip)
            state = self.subtensor.get_epoch_schedule_state(netuid, block=tip)
            if before != self.subtensor.get_block_hash(tip):
                raise ServiceError(503, "chain reorganized during epoch read")
            result = EpochState(state.subnet_epoch_index, state.last_epoch_block, tip)
            result.validate()
            return result

        return await asyncio.to_thread(read)

    async def end_block(self, netuid: int, epoch: int, start: int, state: EpochState) -> int:
        """Locate the inclusive end even if a restart skipped several epoch transitions."""

        def read():
            low, high = start, state.last_epoch_block
            anchor = self.subtensor.get_block_hash(high)
            if self.subtensor.get_subnet_epoch_index(netuid, block=low) != epoch:
                raise ServiceError(503, "historical epoch start changed")
            if self.subtensor.get_subnet_epoch_index(netuid, block=high) <= epoch:
                raise ServiceError(503, "epoch has not ended")
            while low + 1 < high:
                middle = (low + high) // 2
                index = self.subtensor.get_subnet_epoch_index(netuid, block=middle)
                if index is None:
                    raise ServiceError(503, "historical epoch state unavailable")
                if index <= epoch:
                    low = middle
                else:
                    high = middle
            if anchor != self.subtensor.get_block_hash(state.last_epoch_block):
                raise ServiceError(503, "chain reorganized during epoch boundary read")
            return low

        return await asyncio.to_thread(read)


class EpochClock:
    """Synchronous intake clock backed by refreshed chain state, never wall-clock epochs."""

    def __init__(
        self,
        provider: EpochProvider,
        netuid: int,
        *,
        stale_seconds: float = 60,
        monotonic: Callable[[], float] = time.monotonic,
    ):
        self.provider, self.netuid = provider, netuid
        self.stale_seconds, self.monotonic = stale_seconds, monotonic
        self.state: EpochState | None = None
        self._observed = 0.0
        self._lock = asyncio.Lock()

    async def refresh(self) -> EpochState:
        async with self._lock:
            state = await self.provider.epoch_state(self.netuid)
            state.validate()
            if self.state and (
                state.epoch < self.state.epoch
                or state.last_epoch_block < self.state.last_epoch_block
            ):
                raise ServiceError(503, "chain epoch moved backwards")
            self.state, self._observed = state, self.monotonic()
            return state

    def __call__(self) -> int:
        if self.state is None or self.monotonic() - self._observed >= self.stale_seconds:
            raise ServiceError(503, "chain epoch unavailable or stale")
        return self.state.epoch


class EmissionJournal:
    def __init__(self, path: Path):
        self.connection = sqlite3.connect(path, isolation_level=None)
        self.connection.execute("PRAGMA journal_mode=WAL")
        self.connection.execute("PRAGMA synchronous=FULL")
        self.connection.execute("""CREATE TABLE IF NOT EXISTS master_epoch_pins (
            epoch TEXT PRIMARY KEY, block TEXT NOT NULL, block_hash BLOB NOT NULL,
            sealed INTEGER NOT NULL DEFAULT 0 CHECK(sealed IN (0,1))
        )""")
        self.connection.execute("""CREATE TABLE IF NOT EXISTS master_epoch_windows (
            epoch TEXT PRIMARY KEY, start_block TEXT NOT NULL
        )""")

    def observe(self, state: EpochState) -> None:
        epoch = f"{state.epoch:020d}"
        latest = self.connection.execute("SELECT MAX(epoch) FROM master_epoch_windows").fetchone()[
            0
        ]
        if latest is not None and epoch < latest:
            raise ServiceError(503, "persisted epoch moved backwards")
        self.connection.execute(
            "INSERT OR IGNORE INTO master_epoch_windows VALUES (?,?)",
            (epoch, str(state.last_epoch_block)),
        )
        previous = self.connection.execute(
            "SELECT start_block FROM master_epoch_windows WHERE epoch=?", (epoch,)
        ).fetchone()
        if previous != (str(state.last_epoch_block),):
            raise ServiceError(503, "epoch start changed")

    def unpinned(self, current_epoch: int) -> list[tuple[int, int]]:
        rows = self.connection.execute(
            "SELECT w.epoch,w.start_block FROM master_epoch_windows w "
            "LEFT JOIN master_epoch_pins p ON p.epoch=w.epoch "
            "WHERE w.epoch < ? AND p.epoch IS NULL ORDER BY w.epoch",
            (f"{current_epoch:020d}",),
        ).fetchall()
        return [(int(epoch), int(start)) for epoch, start in rows]

    def pin(self, epoch_number: int, snapshot: ChainSnapshot) -> None:
        epoch = f"{epoch_number:020d}"
        self.connection.execute(
            "INSERT OR IGNORE INTO master_epoch_pins VALUES (?,?,?,0)",
            (epoch, str(snapshot.block), snapshot.block_hash),
        )
        previous = self.connection.execute(
            "SELECT block,block_hash FROM master_epoch_pins WHERE epoch=?", (epoch,)
        ).fetchone()
        if previous != (str(snapshot.block), snapshot.block_hash):
            raise ServiceError(503, "epoch pin changed")

    def pending(self, current_epoch: int) -> list[tuple[int, int, bytes]]:
        rows = self.connection.execute(
            "SELECT epoch,block,block_hash FROM master_epoch_pins "
            "WHERE epoch < ? AND sealed=0 ORDER BY epoch",
            (f"{current_epoch:020d}",),
        ).fetchall()
        return [(int(epoch), int(block), block_hash) for epoch, block, block_hash in rows]

    def sealed(self, epoch: int) -> None:
        self.connection.execute(
            "UPDATE master_epoch_pins SET sealed=1 WHERE epoch=?", (f"{epoch:020d}",)
        )

    def close(self) -> None:
        self.connection.close()


class ChallengeRegistry:
    """Re-read the operator registry when it changes; an invalid edit burns, never guesses."""

    def __init__(self, path: Path | None):
        self.path = path
        self._stamp: int | None = None
        self._entries: dict[str, RegistryEntry] = {}

    def __call__(self) -> dict[str, RegistryEntry]:
        if self.path is None:
            return {}
        try:
            stamp = self.path.stat().st_mtime_ns
            if stamp != self._stamp:
                self._entries, self._stamp = load_registry(self.path), stamp
        except (OSError, ValueError) as error:
            logging.warning("challenge registry unavailable (%s)", error)
            self._entries, self._stamp = {}, None
        return self._entries


class _EpochTrackedProof(ProofService):
    """Cover the awaited readiness check before intake reaches its durable journal."""

    def __init__(self, **kwargs):
        super().__init__(**kwargs)
        self._admitting: dict[int, int] = {}

    async def submit(self, submission: Submission, artifact: bytes | None = None) -> dict:
        epoch = self.epoch()
        self._admitting[epoch] = self._admitting.get(epoch, 0) + 1
        try:
            return await super().submit(submission, artifact)
        finally:
            self._admitting[epoch] -= 1
            if not self._admitting[epoch]:
                del self._admitting[epoch]

    def has_unfinished(self, epoch: int) -> bool:
        return bool(self._admitting.get(epoch)) or self.store.has_unfinished_jobs(epoch)


class EpochEmitter:
    def __init__(
        self,
        *,
        gateway: GatewayService,
        proof: _EpochTrackedProof,
        clock: EpochClock,
        journal: EmissionJournal,
        challenge_seed: Callable[[bytes], bytes],
        registry: Callable[[], dict[str, RegistryEntry]],
        challenges: ChallengeClient,
    ):
        self.gateway, self.proof, self.clock, self.journal = gateway, proof, clock, journal
        self.challenge_seed, self.registry, self.challenges = challenge_seed, registry, challenges
        self._lock = asyncio.Lock()

    async def _scores(self, challenge: bytes, epoch: int, expected: set[bytes]):
        algorithm = self.gateway.trust.algorithm_version
        try:
            if challenge != b"proof":
                entry = self.registry().get(challenge.decode())
                if entry is None:
                    raise ServiceError(503, "challenge container not registered")
                answer = await self.challenges.weights(entry, epoch)
                return leaf_scores(answer, expected, algorithm_version=algorithm)
            # Backend readiness is still required before old rows can be emitted.
            readiness = await self.proof.backend.readiness()
            active = [topic for topic in self.proof.store.topics_at(epoch) if topic.active(epoch)]
            if not active:
                raise ServiceError(503, "no open topics")
            for topic in active:
                if (
                    not topic.baseline
                    or topic.eval_image_digest != readiness.eval_image_digest
                    or topic.inference_offer_commitment != readiness.inference_offer_commitment
                    or (
                        topic.metric.family == "custom"
                        and topic.metric.custom_id not in readiness.custom_ids
                    )
                    or (topic.metric.family != "custom" and not readiness.live_harvest_wired)
                ):
                    raise ServiceError(503, "Proof topic cannot score")
            scores = self.proof.scores(epoch)
            # Algorithm 3 pays a challenge sum(leaves) / 10^12 of its share, so topic mass
            # without a winner burns instead of moving to other topics.
            scale = FULL_SHARE_SCORE // SCORE_MAX if algorithm == 3 else 1
            return {decode_hotkey(key): Score(value * scale) for key, value in scores.items()}
        except Exception:
            logging.warning(
                "challenge emission unavailable challenge=%s epoch=%d", challenge.decode(), epoch
            )
            return {key: NoScore() for key in expected}

    async def _emit(self, epoch: int, snapshot: ChainSnapshot) -> None:
        self.gateway.refresh_trust(epoch)
        self.proof.topic_public_key = next(
            (entry.public_key for entry in self.gateway.trust.challenges if entry.id == b"proof"),
            self.proof.topic_public_key,
        )
        rows = canonical_rows(snapshot.rows)
        for challenge in self.gateway.trust.challenges:
            expected = challenge.policy.expected(rows)
            seed = self.challenge_seed(challenge.id)
            if public_key(seed) != challenge.public_key:
                raise ServiceError(503, "challenge signing key does not match owner trust")
            outcomes = await self._scores(challenge.id, epoch, expected)
            # No unknown score key can expand E; missing scores are explicit signed absences.
            leaves = []
            for hotkey in sorted(expected):
                score = outcomes.get(hotkey, NoScore(NoScoreReason.NOT_ATTEMPTED))
                try:
                    score.encode()
                except ValueError:
                    score = NoScore()
                leaves.append(sign_leaf(seed, challenge.id, hotkey, epoch, score))
            self.gateway.replace_challenge_leaves(challenge.id, epoch, expected, tuple(leaves))

    async def tick(self) -> list[int]:
        async with self._lock:
            state = await self.clock.refresh()
            await self.proof.resume()
            if state.epoch == 0:
                return []
            self.journal.observe(state)
            for epoch, start in self.journal.unpinned(state.epoch):
                resolve = getattr(self.clock.provider, "end_block", None)
                if resolve is not None:
                    block = await resolve(self.gateway.netuid, epoch, start, state)
                elif state.epoch == epoch + 1:
                    block = state.last_epoch_block - 1
                else:
                    raise ServiceError(503, "historical epoch boundary unavailable")
                if not start <= block < state.last_epoch_block:
                    raise ServiceError(503, "invalid completed epoch boundary")
                snapshot = await self.gateway.chain.snapshot(block, self.gateway.netuid)
                if snapshot.block != block:
                    raise ServiceError(503, "chain epoch snapshot mismatch")
                self.journal.pin(epoch, snapshot)
            completed = []
            for epoch, block, block_hash in self.journal.pending(state.epoch):
                if self.gateway.store.bundle(epoch) is not None:
                    await self.gateway.seal(epoch, block_b=block)
                    self.journal.sealed(epoch)
                    continue
                if self.proof.has_unfinished(epoch):
                    break
                pinned = await self.gateway.chain.snapshot(block, self.gateway.netuid)
                if pinned.block_hash != block_hash or pinned.block != block:
                    raise ServiceError(503, "completed epoch pin changed")
                await self._emit(epoch, pinned)
                await self.gateway.seal(epoch, block_b=block)
                self.journal.sealed(epoch)
                completed.append(epoch)
                # Keep each newly completed seal observable for one polling interval.
                # A backlog must not overwrite it before validators can fetch it.
                break
            return completed


class MasterRuntime:
    def __init__(
        self,
        config: MasterConfig,
        gateway: GatewayService,
        proof: ProofService,
        clock: EpochClock,
        emitter: EpochEmitter,
        http: httpx.AsyncClient,
    ):
        self.config, self.gateway, self.proof = config, gateway, proof
        self.clock, self.emitter, self.http = clock, emitter, http
        self._stop = asyncio.Event()
        self._tasks: list[asyncio.Task] = []
        self.topic_setup = None
        if hasattr(proof.backend, "run_agent"):
            from cortex.proof.setup import SetupBackend, TopicSetup

            self.topic_setup = TopicSetup(
                proof,
                cast(SetupBackend, proof.backend),
                owner_seed=lambda: read_seed(config.proof_seed_file),
            )

    async def _loop(self, operation, seconds: float) -> None:
        while not self._stop.is_set():
            try:
                await operation()
            except Exception as error:
                logging.warning("master background operation failed (%s)", type(error).__name__)
            try:
                await asyncio.wait_for(self._stop.wait(), timeout=seconds)
            except TimeoutError:
                pass

    async def start(self) -> None:
        if self._tasks:
            return
        await self.clock.refresh()
        await self.proof.resume()
        if self.topic_setup is not None:
            await self.topic_setup.resume()
        self._tasks = [
            asyncio.create_task(self._loop(self.clock.refresh, self.config.epoch_refresh_seconds)),
            asyncio.create_task(self._loop(self.emitter.tick, self.config.emit_poll_seconds)),
        ]

    async def close(self) -> None:
        self._stop.set()
        for task in self._tasks:
            task.cancel()
        await asyncio.gather(*self._tasks, return_exceptions=True)
        await self.proof.drain()
        if self.topic_setup is not None:
            await self.topic_setup.drain()
        self.gateway.store.close()
        self.proof.store.close()
        self.emitter.journal.close()
        await self.http.aclose()

    def app(self) -> FastAPI:
        @asynccontextmanager
        async def lifespan(app):
            await self.start()
            try:
                yield
            finally:
                await self.close()

        app = FastAPI(title="Cortex master", lifespan=lifespan)

        @app.exception_handler(ServiceError)
        async def service_error(request, error: ServiceError):
            return JSONResponse({"error": error.reason}, status_code=error.status)

        operator = OperatorAuth(self.config.operator_token_file)
        app.include_router(gateway_router(self.gateway, operator))
        proof = proof_router(self.proof, operator, self.topic_setup)
        app.include_router(proof)
        app.include_router(proof, prefix="/challenge/proof", include_in_schema=False)
        app.include_router(challenge_router(self.emitter.registry, self.http))

        @app.get("/livez")
        async def health():
            return {"ok": True, "role": "master"}

        @app.get("/readyz")
        async def ready():
            try:
                epoch = self.clock()
            except ServiceError as error:
                return JSONResponse(
                    {"ready": False, "role": "master", "reason": str(error)},
                    status_code=503,
                )
            return {"ready": True, "epoch": epoch, "role": "master"}

        return app


async def build_master(
    config: MasterConfig,
    *,
    chain,
    epochs: EpochProvider,
    proof_backend: EvaluationBackend | None = None,
    trust: TrustRoot | None = None,
    challenge_http: httpx.AsyncClient | None = None,
) -> MasterRuntime:
    prepare_master_state(config.state_dir)
    clock = EpochClock(epochs, config.netuid, stale_seconds=config.epoch_stale_seconds)
    state = await clock.refresh()
    local_trust = trust or config.trust_root(state.epoch)
    read_private_file(config.operator_token_file)
    load_registry(config.challenge_registry_file)  # fail fast on an invalid operator registry
    with ExitStack() as cleanup:
        gateway_store = GatewayStore(config.state_dir / "gateway.sqlite3")
        cleanup.callback(gateway_store.close)
        proof_store = ProofStore(config.state_dir / "proof.sqlite3")
        cleanup.callback(proof_store.close)
        journal = EmissionJournal(config.state_dir / "emission.sqlite3")
        cleanup.callback(journal.close)
        journal.observe(state)

        def chain_endpoint() -> str:
            subtensor = getattr(chain, "subtensor", None)
            substrate = getattr(subtensor, "substrate", None)
            value = getattr(substrate, "chain_endpoint", None)
            if not isinstance(value, str) or not value:
                value = getattr(subtensor, "chain_endpoint", None)
            if not isinstance(value, str) or not value:
                value = config.chain_endpoint
            return _public_chain_endpoint(value)

        gateway = GatewayService(
            store=gateway_store,
            trust=local_trust,
            netuid=config.netuid,
            chain=chain,
            gateway_seed=lambda: read_seed(config.gateway_seed_file),
            chain_endpoint=chain_endpoint,
            trust_loader=None if trust is not None else config.trust_root,
        )
        proof = _EpochTrackedProof(
            store=proof_store,
            topic_public_key=next(
                (c.public_key for c in local_trust.challenges if c.id == b"proof"),
                public_key(read_seed(config.proof_seed_file)),
            ),
            vault=FileVault(config.state_dir / "miner-byok"),
            artifact_dir=config.state_dir / "artifacts",
            backend=proof_backend or UnwiredBackend(),
            epoch=clock,
        )
        http = challenge_http or httpx.AsyncClient(trust_env=False, follow_redirects=False)
        emitter = EpochEmitter(
            gateway=gateway,
            proof=proof,
            clock=clock,
            journal=journal,
            challenge_seed=lambda challenge: read_seed(config.challenge_seed_file(challenge)),
            registry=ChallengeRegistry(config.challenge_registry_file),
            challenges=ChallengeClient(http, config.challenge_secrets_dir),
        )
        runtime = MasterRuntime(config, gateway, proof, clock, emitter, http)
        cleanup.pop_all()
        return runtime
