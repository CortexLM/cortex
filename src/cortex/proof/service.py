"""Proof intake and restart-safe evaluation through the VM boundary."""

from __future__ import annotations

import asyncio
import hashlib
import time
import uuid
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path
from typing import Protocol
from urllib.parse import urlsplit

from cortex.errors import ServiceError
from cortex.proof.artifacts import FileVault, check_env, verify_artifact
from cortex.proof.models import EvaluationReport, Submission, SubmissionLookup, Topic
from cortex.proof.scoring import judge, payout
from cortex.proof.store import ProofStore
from cortex.protocol.crypto import public_key, sign_raw, verify_raw

TOPIC_DOMAIN = b"base-proof-topic-v1"
SUBMIT_DOMAIN = b"base-proof-submit-v1"


def _bounded_reason(value: str | None, limit: int = 512) -> str | None:
    if value is None:
        return None
    encoded = value.encode(errors="replace")
    return encoded[:limit].decode(errors="ignore")


@dataclass(frozen=True)
class Readiness:
    eval_image_digest: str
    inference_offer_commitment: str
    custom_ids: frozenset[str] = frozenset()
    live_harvest_wired: bool = False
    executor_config_commitment: str | None = None
    executor_max_deadline_s: int | None = None
    harvest_reason: str | None = None


class EvaluationBackend(Protocol):
    async def readiness(self) -> Readiness: ...

    async def evaluate(
        self,
        *,
        job_id: str,
        topic: Topic,
        submission: Submission,
        artifact: bytes | None,
        env: dict[str, str],
    ) -> EvaluationReport:
        """Idempotent job id; return only authenticated VM evidence after teardown."""
        ...


class UnwiredBackend:
    async def readiness(self) -> Readiness:
        raise ServiceError(503, "UnwiredVmOrchestrator")

    async def evaluate(self, **kwargs) -> EvaluationReport:
        raise ServiceError(503, "UnwiredVmOrchestrator")


class ProofService:
    def __init__(
        self,
        *,
        store: ProofStore,
        topic_public_key: bytes,
        vault: FileVault,
        artifact_dir: Path,
        backend: EvaluationBackend,
        epoch: Callable[[], int],
        clock: Callable[[], float] = time.time,
        max_pending_per_miner: int = 4,
    ):
        self.store = store
        self.topic_public_key = topic_public_key
        self.vault = vault
        self.artifact_dir = artifact_dir
        self.artifact_dir.mkdir(parents=True, exist_ok=True, mode=0o700)
        self.backend = backend
        self.epoch = epoch
        self.clock = clock
        self.max_pending_per_miner = max_pending_per_miner
        self._tasks: dict[str, asyncio.Task] = {}

    async def publish(self, topic: Topic) -> Topic:
        try:
            signature = bytes.fromhex(topic.signature)
        except ValueError:
            raise ServiceError(401, "invalid topic signature") from None
        if not verify_raw(self.topic_public_key, TOPIC_DOMAIN, topic.signing_payload(), signature):
            raise ServiceError(401, "invalid topic signature")
        check_env(
            topic.params,
            {topic.params["miner_byok"]: "shape-check"} if "miner_byok" in topic.params else {},
        )
        if topic.status == "open":
            await self._ready(topic)
            if not topic.checklist:
                raise ServiceError(400, "published checklist required")
        self.store.publish(topic, epoch=self.epoch())
        return topic

    async def _ready(self, topic: Topic, readiness: Readiness | None = None) -> Readiness:
        readiness = readiness or await self.backend.readiness()
        if readiness.eval_image_digest != topic.eval_image_digest:
            raise ServiceError(503, "eval image pin mismatch")
        if readiness.inference_offer_commitment != topic.inference_offer_commitment:
            raise ServiceError(503, "inference offer unavailable")
        if topic.metric.family == "custom":
            if topic.metric.custom_id not in readiness.custom_ids:
                raise ServiceError(503, "custom runner unavailable")
        else:
            if not readiness.live_harvest_wired:
                raise ServiceError(503, "harvest executor unavailable")
            constraints = topic.eval_executor
            if (
                constraints.require_offer_commitment is not None
                and constraints.require_offer_commitment != readiness.executor_config_commitment
            ):
                raise ServiceError(503, "executor offer cannot serve topic commitment")
            if constraints.max_proof_deadline_s is not None and (
                readiness.executor_max_deadline_s is None
                or constraints.max_proof_deadline_s > readiness.executor_max_deadline_s
            ):
                raise ServiceError(503, "topic deadline exceeds live executor deadline")
            prepare = getattr(self.backend, "prepare", None)
            if prepare is not None:
                prepare(topic)
        if not topic.baseline or not topic.holdout_commitment:
            raise ServiceError(503, "baseline is not sealed")
        evidence = self.store.evidence(topic.baseline.evidence_digest, topic.id)
        if (
            evidence.get("metrics") != topic.baseline.metrics
            or evidence.get("script_sha256") != topic.baseline.script_sha256
            or evidence.get("eval_image_digest") != topic.eval_image_digest
            or evidence.get("flops_budget") != topic.flops_budget
            or evidence.get("wall_budget_s") != topic.wall_budget_s
            or evidence.get("sandboxed") is not True
            or evidence.get("teardown_confirmed") is not True
        ):
            raise ServiceError(503, "baseline execution evidence mismatch")
        self.store.holdouts(topic.holdout_commitment)
        return readiness

    async def status(self) -> dict[str, object]:
        topics = [topic for topic in self.store.topics() if topic.active(self.epoch())]
        deferred = [topic for topic in topics if topic.params.get("defer_scoring") == "true"]
        immediate = [topic for topic in topics if topic.params.get("defer_scoring") != "true"]
        readiness: Readiness | None = None
        failures: list[str] = []
        scorable: list[Topic] = []
        try:
            readiness = await self.backend.readiness()
        except ServiceError as error:
            failures.append(error.reason)
        except Exception:
            failures.append("evaluation readiness failed")
        if readiness is not None:
            for topic in immediate:
                try:
                    await self._ready(topic, readiness)
                    scorable.append(topic)
                except ServiceError as error:
                    failures.append(error.reason)
                except Exception:
                    failures.append("topic readiness failed")
        if not topics:
            failures.append("no open topics")
        elif not immediate:
            failures.append("all open topics defer scoring")
        reason = None if scorable else _bounded_reason(failures[0] if failures else "not ready")
        custom = [topic.id for topic in scorable if topic.metric.family == "custom"]
        harvest = [topic.id for topic in scorable if topic.metric.family != "custom"]
        return {
            "challenge_id": "proof",
            "can_score": bool(scorable),
            "reason": reason,
            "open_topics": len(topics),
            "deferred_topics": [topic.id for topic in deferred],
            "scorable_topics": [topic.id for topic in scorable],
            "custom_scorable_topics": custom,
            "harvest_scorable_topics": harvest,
            "live_harvest_wired": readiness.live_harvest_wired if readiness else False,
            "custom_family_wired": bool(readiness and readiness.custom_ids),
            "registered_custom": sorted(readiness.custom_ids) if readiness else [],
            "custom_ready": sorted(readiness.custom_ids) if readiness else [],
            "pins": {
                "eval_image_digest": readiness.eval_image_digest if readiness else None,
                "inference_offer_commitment": (
                    readiness.inference_offer_commitment if readiness else None
                ),
                "executor_config_commitment": (
                    readiness.executor_config_commitment if readiness else None
                ),
                "executor_max_deadline_s": (
                    readiness.executor_max_deadline_s if readiness else None
                ),
            },
            "harvest_reason": _bounded_reason(readiness.harvest_reason)
            if readiness and readiness.harvest_reason
            else None,
        }

    def lookup(self, envelope: SubmissionLookup) -> dict:
        if not verify_raw(
            bytes.fromhex(envelope.miner_hotkey),
            SUBMIT_DOMAIN,
            envelope.signing_payload(),
            bytes.fromhex(envelope.hotkey_signature),
        ):
            raise ServiceError(401, "invalid hotkey signature")
        payload_digest = hashlib.sha256(envelope.signing_payload()).hexdigest()
        row = self.store.lookup_submission(
            envelope.miner_hotkey, envelope.submit_nonce, payload_digest
        )
        if row is None:
            raise ServiceError(404, "submission not found")
        row.setdefault("topic_id", envelope.topic_id)
        row.setdefault("body", envelope.model_dump())
        row["reason"] = _bounded_reason(row.get("reason"))
        return row

    async def submit(self, submission: Submission, artifact: bytes | None = None) -> dict:
        epoch = self.epoch()
        topic = self.store.topic(submission.topic_id)
        if topic is None or not topic.active(epoch):
            raise ServiceError(400, "topic missing, unknown or not open")
        env = check_env(topic.params, submission.env)
        if artifact is not None:
            # An ignored transport URI must not survive even an early recorded rejection.
            submission = submission.model_copy(update={"artifact_uri": None})
        elif not submission.artifact_uri:
            raise ServiceError(400, "artifact required")
        else:
            try:
                parsed = urlsplit(submission.artifact_uri)
                port = parsed.port
                if (
                    parsed.scheme != "https"
                    or not parsed.hostname
                    or parsed.username is not None
                    or parsed.password is not None
                    or parsed.fragment
                    or port == 0
                ):
                    raise ValueError("invalid artifact transport")
            except ValueError:
                raise ServiceError(
                    400, "artifact_uri must be valid HTTPS without credentials or fragments"
                ) from None
        if not verify_raw(
            bytes.fromhex(submission.miner_hotkey),
            SUBMIT_DOMAIN,
            submission.signing_payload(),
            bytes.fromhex(submission.hotkey_signature),
        ):
            raise ServiceError(401, "invalid hotkey signature")
        if artifact is not None:
            verify_artifact(artifact, submission.artifact_digest)
        if submission.artifact_digest in {
            hashlib.sha256(b"").hexdigest(),
            hashlib.sha256(bytes(10240)).hexdigest(),
        }:
            raise ServiceError(400, "artifact has no content")
        await self._ready(topic)
        payload_digest = hashlib.sha256(submission.signing_payload()).hexdigest()
        job_id = hashlib.sha256(
            topic.content_digest().encode() + submission.signing_payload()
        ).hexdigest()
        self.store.reserve_nonce(
            submission.miner_hotkey,
            submission.submit_nonce,
            payload_digest,
            job_id,
            epoch=epoch,
        )
        hashes, datasets = self.store.holdouts(topic.holdout_commitment or "")
        manifest = submission.manifest
        requires_training = (
            topic.params.get(
                "require_training_evidence", str(topic.metric.family != "custom").lower()
            )
            == "true"
        )
        contaminated = bool(
            hashes.intersection(manifest.train_content_hashes)
            or datasets.intersection(manifest.train_dataset_ids)
        )
        empty_training = (
            requires_training
            and not manifest.train_content_hashes
            and not manifest.train_dataset_ids
        )
        if contaminated or empty_training:
            return self.store.record(
                job_id,
                topic,
                submission,
                epoch,
                "rejected",
                reason="contamination or missing training evidence",
            )
        if artifact is not None:
            path = self.artifact_dir / submission.artifact_digest
            try:
                with path.open("xb") as stream:
                    stream.write(artifact)
                    stream.flush()
                    import os

                    os.fsync(stream.fileno())
            except FileExistsError:
                verify_artifact(path.read_bytes(), submission.artifact_digest)
            except OSError:
                raise ServiceError(503, "artifact store unavailable") from None
            submission = submission.model_copy(
                update={"artifact_uri": f"proof-artefact://{submission.artifact_digest}"}
            )
        submission = submission.model_copy(update={"env": {}})
        # Credentials land before the durable row, with no await in between. A
        # crash after the vault write leaves an orphan directory that startup
        # reconciliation removes; a crash after enqueue leaves a complete job.
        # The reverse order left a queued row whose declared credentials did
        # not exist, and reconciliation refused to start anything.
        self.vault.put(job_id, env)
        try:
            self.store.enqueue(
                job_id,
                topic,
                submission,
                epoch,
                sorted(env),
                max_pending=self.max_pending_per_miner,
            )
        except BaseException as caught:
            try:
                self.vault.delete(job_id)
                self.store.discard_unstarted(job_id, missing_ok=True)
            except ServiceError as cleanup:
                raise cleanup from caught
            raise
        if topic.params.get("defer_scoring") == "true":
            return self.store.record(job_id, topic, submission, epoch, "queued")
        task = self._start(job_id)
        # A miner disconnect cannot cancel accepted work. The strong task reference
        # survives until the outcome is committed, and the journal survives restart.
        return await asyncio.shield(task)

    def _start(self, job_id: str) -> asyncio.Task:
        task = self._tasks.get(job_id)
        if task is None:
            task = asyncio.create_task(self._evaluate(job_id))
            self._tasks[job_id] = task
            task.add_done_callback(lambda completed: self._finished(job_id, completed))
        return task

    def _finished(self, job_id: str, task: asyncio.Task) -> None:
        if self._tasks.get(job_id) is task:
            self._tasks.pop(job_id, None)
        if not task.cancelled():
            task.exception()  # retrieve failures even after the HTTP waiter disconnected

    async def _evaluate(self, job_id: str) -> dict:
        owner = uuid.uuid4().hex
        job = self.store.claim(job_id, owner, int(self.clock()), 7300)
        if job is None:
            row = self.store.submission(job_id)
            if row is not None and row["status"] != "queued":
                return row
            raise ServiceError(503, "evaluation already running or requires reconciliation")
        topic = self.store.topic_by_digest(job["topic_digest"])
        submission = Submission.model_validate(job["body"])
        try:
            await self._ready(topic)
            env = self.vault.get(job_id, job["env_names"])
            try:
                check_env(topic.params, env)
            except ServiceError:
                raise ServiceError(503, "required miner credential unavailable") from None
            artifact = None
            if (submission.artifact_uri or "").startswith("proof-artefact://"):
                try:
                    artifact = (self.artifact_dir / submission.artifact_digest).read_bytes()
                except OSError:
                    raise ServiceError(503, "stored artifact unavailable") from None
                verify_artifact(artifact, submission.artifact_digest)
            async with asyncio.timeout(topic.wall_budget_s + 60):
                report = await self.backend.evaluate(
                    job_id=job_id, topic=topic, submission=submission, artifact=artifact, env=env
                )
            if (
                report.topic_id != topic.id
                or report.topic_digest != topic.content_digest()
                or report.artifact_digest != submission.artifact_digest
                or report.submission_id != job_id
            ):
                raise ServiceError(503, "VM evidence binding mismatch")
            failures = judge(topic, report)
        except ServiceError as caught:
            try:
                self._finish_failed(job_id, owner, caught.reason)
            except ServiceError as cleanup:
                raise cleanup from caught
            raise
        except Exception as caught:
            try:
                self._finish_failed(job_id, owner, "evaluation infrastructure failed")
            except ServiceError as cleanup:
                raise cleanup from caught
            raise ServiceError(503, "evaluation infrastructure failed") from None
        self._delete_owned_credentials(job_id, owner)
        return self.store.record(
            job_id,
            topic,
            submission,
            job["epoch"],
            "rejected" if failures else "accepted",
            report.model_dump(),
            "; ".join(failures) or None,
            owner=owner,
        )

    def _delete_owned_credentials(self, job_id: str, owner: str) -> None:
        self.store.protect_finalization(job_id, owner, now=int(self.clock()), lease_seconds=7300)
        self.vault.delete(job_id)

    def _finish_failed(self, job_id: str, owner: str, reason: str) -> None:
        self._delete_owned_credentials(job_id, owner)
        self.store.fail(job_id, owner, reason)

    async def resume(self) -> None:
        self.vault.reconcile(self.store.active_vault_entries())
        for job_id in self.store.pending(int(self.clock())):
            if job_id in self._tasks:
                continue
            job_topic = self.store.job_topic(job_id)
            current = self.store.topic(job_topic.id)
            if current and current.params.get("defer_scoring") != "true":
                self._start(job_id)

    async def drain(self) -> None:
        if self._tasks:
            await asyncio.gather(*self._tasks.values(), return_exceptions=True)

    def scores(self, epoch: int) -> dict[str, int]:
        rows = self.store.submissions(epoch)
        history = self.store.previous_accepted(epoch)
        versions = {
            row["topic_digest"]: self.store.topic_by_digest(row["topic_digest"])
            for row in [*rows, *history]
        }
        topics = self.store.topics_at(epoch)
        by_id = {topic.id: topic for topic in topics}
        champions: dict[str, float] = {}
        seen: dict[str, set[str]] = {}
        for row in history:
            topic = by_id.get(row["topic_id"])
            frozen = versions[row["topic_digest"]]
            if topic is None or frozen.metric != topic.metric or frozen.baseline != topic.baseline:
                continue
            report = EvaluationReport.model_validate(row["report"])
            if judge(frozen, report):
                continue
            value = report.metrics[topic.metric.primary]
            best = max if topic.metric.direction == "max" else min
            champions[topic.id] = best(champions.get(topic.id, value), value)
            seen.setdefault(topic.id, set()).add(report.artifact_digest)
        return payout(
            topics, rows, epoch, champions, frozen_topics=versions, previous_artifacts=seen
        )


def sign_topic(topic: Topic, seed: bytes) -> Topic:
    return topic.model_copy(
        update={"signature": sign_raw(seed, TOPIC_DOMAIN, topic.signing_payload()).hex()}
    )


def sign_submission(body: dict, seed: bytes) -> Submission:
    body = {**body, "miner_hotkey": public_key(seed).hex(), "hotkey_signature": "00" * 64}
    submission = Submission.model_validate(body)
    return submission.model_copy(
        update={
            "hotkey_signature": sign_raw(seed, SUBMIT_DOMAIN, submission.signing_payload()).hex()
        }
    )
