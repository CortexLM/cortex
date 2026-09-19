"""Signed one-GPU executor offers and fail-closed Lium harvest orchestration."""

from __future__ import annotations

import asyncio
import hashlib
import os
import re
import stat
import threading
import time
from collections.abc import Callable
from pathlib import Path
from typing import Annotated, Literal, Protocol

from pydantic import Field, ValidationError, field_validator, model_validator

from cortex.errors import ServiceError
from cortex.proof.artifacts import MAX_ARTIFACT_BYTES, check_env, verify_artifact
from cortex.proof.material import HarvestMaterial, HarvestMaterialSource
from cortex.proof.models import (
    Document,
    EvaluationReport,
    Hex64,
    Identifier,
    Submission,
    SubmissionLookup,
    Topic,
    canonical_json,
    digest,
)
from cortex.proof.service import SUBMIT_DOMAIN, EvaluationBackend, Readiness
from cortex.protocol.crypto import public_key, sign_raw, verify_raw

EXECUTOR_OFFER_DOMAIN = b"cortex-proof-executor-offer-v1"
MAX_OUTPUT_BYTES = 16_384
_RAW_TEMPLATE_ID = re.compile(
    r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$"
)


class ExecutorPin(Document):
    """Repository/operator ceilings which a live executor can only tighten."""

    schema_version: Literal[1] = 1
    eval_image_digest: Annotated[str, Field(pattern=r"^sha256:[0-9a-f]{64}$")]
    issuer_public_key: Hex64
    gpu_class: Literal["1x"] = "1x"
    max_proof_deadline_s: Annotated[int, Field(gt=0, le=7200, strict=True)] = 7200
    allowed_template_prefixes: tuple[str, ...] = ()

    @field_validator("allowed_template_prefixes")
    @classmethod
    def valid_prefixes(cls, values: tuple[str, ...]) -> tuple[str, ...]:
        if len(values) > 32 or len(set(values)) != len(values):
            raise ValueError("invalid executor template prefixes")
        if any(
            not value
            or len(value) > 128
            or not value.isascii()
            or not value.isprintable()
            or value.strip() != value
            for value in values
        ):
            raise ValueError("invalid executor template prefix")
        return values


class EvalExecutorOffer(Document):
    """Public, signed operator state describing where one Proof eval runs."""

    schema_version: Literal[1] = 1
    offer_id: Annotated[str, Field(pattern=r"^[a-z0-9][a-z0-9-]{1,62}$")]
    lium_template_id: Annotated[str, Field(min_length=1, max_length=128)]
    machine_shape: Annotated[str, Field(min_length=2, max_length=16)]
    max_proof_deadline_s: Annotated[int, Field(gt=0, le=7200, strict=True)]
    eval_image_digest: Annotated[str, Field(pattern=r"^sha256:[0-9a-f]{64}$")]
    config_commitment: Hex64
    issuer_public_key: Hex64
    status: Literal["open", "closed"]
    valid_from_unix: Annotated[int, Field(ge=0, strict=True)]
    valid_until_unix: Annotated[int, Field(gt=0, strict=True)]
    signature: Annotated[str, Field(pattern=r"^[0-9a-f]{128}$")]

    @model_validator(mode="after")
    def valid_window_and_template(self) -> EvalExecutorOffer:
        if self.valid_until_unix <= self.valid_from_unix:
            raise ValueError("invalid executor offer validity window")
        if (
            not self.lium_template_id.isascii()
            or not self.lium_template_id.isprintable()
            or self.lium_template_id.strip() != self.lium_template_id
        ):
            raise ValueError("executor template id must be printable ASCII")
        return self

    def config_material(self) -> dict[str, object]:
        return {
            "eval_image_digest": self.eval_image_digest,
            "lium_template_id": self.lium_template_id,
            "machine_shape": self.machine_shape,
            "max_proof_deadline_s": self.max_proof_deadline_s,
        }

    def expected_config_commitment(self) -> str:
        return digest(self.config_material())

    def signing_payload(self) -> bytes:
        return canonical_json(self.model_dump(mode="json", exclude={"signature"}))

    def commitment(self) -> str:
        return hashlib.sha256(self.signing_payload()).hexdigest()

    def verify(
        self,
        pin: ExecutorPin,
        *,
        now: float | None = None,
        require_open: bool = True,
    ) -> None:
        if self.issuer_public_key != pin.issuer_public_key:
            raise ValueError("executor offer issuer mismatch")
        if not verify_raw(
            bytes.fromhex(self.issuer_public_key),
            EXECUTOR_OFFER_DOMAIN,
            self.signing_payload(),
            bytes.fromhex(self.signature),
        ):
            raise ValueError("executor offer signature invalid")
        moment = time.time() if now is None else now
        if not self.valid_from_unix <= moment < self.valid_until_unix:
            raise ValueError("executor offer outside validity window")
        if self.config_commitment != self.expected_config_commitment():
            raise ValueError("executor config commitment mismatch")
        if self.machine_shape != pin.gpu_class or self.machine_shape != "1x":
            raise ValueError("Proof executor must rent exactly 1x")
        if self.max_proof_deadline_s > pin.max_proof_deadline_s:
            raise ValueError("executor deadline exceeds pin ceiling")
        if self.eval_image_digest != pin.eval_image_digest:
            raise ValueError("executor eval image digest mismatch")
        _validate_template(pin, self.lium_template_id)
        if require_open and self.status != "open":
            raise ValueError("executor offer is closed")

    def public_view(self) -> dict[str, object]:
        return {
            "schema_version": self.schema_version,
            "offer_id": self.offer_id,
            "lium_template_id": self.lium_template_id,
            "machine_shape": self.machine_shape,
            "gpu_count": 1 if self.machine_shape == "1x" else None,
            "max_proof_deadline_s": self.max_proof_deadline_s,
            "eval_image_digest": self.eval_image_digest,
            "config_commitment": self.config_commitment,
            "issuer_public_key": self.issuer_public_key,
            "status": self.status,
            "valid_from_unix": self.valid_from_unix,
            "valid_until_unix": self.valid_until_unix,
            "signature": self.signature,
            "commitment": self.commitment(),
        }

    @classmethod
    def load(cls, path: Path) -> EvalExecutorOffer:
        try:
            descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
            with os.fdopen(descriptor, "rb") as stream:
                metadata = os.fstat(stream.fileno())
                if not stat.S_ISREG(metadata.st_mode) or metadata.st_size > 32_768:
                    raise ValueError("executor offer must be a bounded regular file")
                body = stream.read(32_769)
            return cls.model_validate_json(body)
        except OSError:
            raise ValueError("executor offer file unavailable") from None


def sign_executor_offer(offer: EvalExecutorOffer, seed: bytes) -> EvalExecutorOffer:
    if public_key(seed).hex() != offer.issuer_public_key:
        raise ValueError("executor offer issuer key mismatch")
    return offer.model_copy(
        update={"signature": sign_raw(seed, EXECUTOR_OFFER_DOMAIN, offer.signing_payload()).hex()}
    )


def _validate_template(pin: ExecutorPin, template_id: str) -> None:
    if _RAW_TEMPLATE_ID.fullmatch(template_id):
        raise ValueError("raw Lium template ids are not digest-bound")
    digest_prefix = pin.eval_image_digest.removeprefix("sha256:")[:12]
    if digest_prefix not in template_id:
        raise ValueError("executor template does not carry the eval image digest prefix")
    if pin.allowed_template_prefixes and not any(
        template_id.startswith(prefix) for prefix in pin.allowed_template_prefixes
    ):
        raise ValueError("executor template is outside the pin allowlist")


class HarvestOverrides(Document):
    """Operator hot swaps; every set value is validated and never clamped."""

    template_id: str | None = Field(default=None, min_length=1, max_length=128)
    gpu_count: Annotated[int, Field(gt=0, strict=True)] | None = None
    deadline_s: Annotated[int, Field(gt=0, le=7200, strict=True)] | None = None


class ExecutorPlan(Document):
    offer_id: Identifier
    topic_id: Identifier
    template_id: str
    gpu_count: Literal[1]
    deadline_s: Annotated[int, Field(gt=0, le=7200, strict=True)]
    offer_commitment: Hex64
    config_commitment: Hex64
    overridden: bool


class ExecutorOfferRegistry:
    """Atomic live-offer rotation with optional durable public state."""

    def __init__(
        self,
        pin: ExecutorPin,
        offer: EvalExecutorOffer,
        *,
        state_path: Path | None = None,
        clock: Callable[[], float] = time.time,
    ) -> None:
        self.pin, self.state_path, self.clock = pin, state_path, clock
        self._lock = threading.RLock()
        offer.verify(pin, now=clock(), require_open=False)
        self._offer = offer

    @classmethod
    def from_file(
        cls,
        pin: ExecutorPin,
        path: Path,
        *,
        clock: Callable[[], float] = time.time,
    ) -> ExecutorOfferRegistry:
        return cls(pin, EvalExecutorOffer.load(path), state_path=path, clock=clock)

    def current(self) -> EvalExecutorOffer:
        with self._lock:
            return self._offer

    def require_open(self) -> EvalExecutorOffer:
        offer = self.current()
        offer.verify(self.pin, now=self.clock())
        return offer

    def rotate(self, offer: EvalExecutorOffer) -> EvalExecutorOffer:
        offer.verify(self.pin, now=self.clock(), require_open=False)
        with self._lock:
            if self.state_path is not None:
                self._persist(offer)
            self._offer = offer
        return offer

    def _persist(self, offer: EvalExecutorOffer) -> None:
        path = self.state_path
        if path is None:
            return
        path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        temporary = path.with_name(path.name + ".new")
        descriptor = os.open(
            temporary,
            os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
            0o600,
        )
        try:
            with os.fdopen(descriptor, "wb") as stream:
                stream.write(offer.model_dump_json().encode())
                stream.flush()
                os.fsync(stream.fileno())
            os.replace(temporary, path)
        except BaseException:
            try:
                temporary.unlink()
            except OSError:
                pass
            raise

    def plan(self, topic: Topic, overrides: HarvestOverrides | None = None) -> ExecutorPlan:
        offer = self.require_open()
        if topic.metric.family == "custom":
            raise ValueError("custom topics do not use the Lium executor")
        if topic.eval_image_digest != self.pin.eval_image_digest:
            raise ValueError("topic eval image does not match executor pin")
        constraints = topic.eval_executor
        if (
            constraints.require_offer_commitment is not None
            and constraints.require_offer_commitment != offer.config_commitment
        ):
            raise ValueError("executor offer cannot serve the topic commitment")
        changed = overrides or HarvestOverrides()
        template_id = changed.template_id or offer.lium_template_id
        _validate_template(self.pin, template_id)
        gpu_count = changed.gpu_count or 1
        if gpu_count != 1:
            raise ValueError("Proof executor must rent exactly 1x")
        base_deadline = changed.deadline_s or offer.max_proof_deadline_s
        overridden = (
            template_id != offer.lium_template_id or base_deadline != offer.max_proof_deadline_s
        )
        if overridden and constraints.require_offer_commitment is not None:
            raise ValueError("executor override changes a topic-pinned offer")
        requested_deadline = constraints.max_proof_deadline_s
        if requested_deadline is not None and requested_deadline > base_deadline:
            raise ValueError("topic deadline cannot exceed the live executor deadline")
        deadline = requested_deadline or base_deadline
        if deadline > self.pin.max_proof_deadline_s:
            raise ValueError("executor deadline exceeds pin ceiling")
        config_commitment = digest(
            {
                "eval_image_digest": offer.eval_image_digest,
                "lium_template_id": template_id,
                "machine_shape": offer.machine_shape,
                "max_proof_deadline_s": deadline,
            }
        )
        return ExecutorPlan(
            offer_id=offer.offer_id,
            topic_id=topic.id,
            template_id=template_id,
            gpu_count=1,
            deadline_s=deadline,
            offer_commitment=offer.config_commitment,
            config_commitment=config_commitment,
            overridden=overridden,
        )


class HarvestRequest(Document):
    """Complete policy and private inputs, with a secret-free evidence commitment."""

    schema_version: Literal[1] = 1
    job_id: Hex64
    topic: Topic
    submission: SubmissionLookup
    material: HarvestMaterial = Field(repr=False)
    topic_id: Identifier
    topic_digest: Hex64
    artifact_digest: Hex64
    artifact: Annotated[bytes, Field(min_length=1, max_length=MAX_ARTIFACT_BYTES)] = Field(
        repr=False, exclude=True
    )
    claim: str = Field(min_length=1, max_length=65_536)
    metric: Identifier
    checklist: tuple[Identifier, ...]
    params: dict[str, str]
    env: dict[str, str] = Field(repr=False, exclude=True)
    eval_image_digest: str
    inference_offer_commitment: Hex64
    plan: ExecutorPlan

    @model_validator(mode="after")
    def consistent_envelope(self) -> HarvestRequest:
        if (
            self.topic_id != self.topic.id
            or self.topic_digest != self.topic.content_digest()
            or self.submission.topic_id != self.topic_id
            or self.artifact_digest != self.submission.artifact_digest
            or self.claim != self.submission.claim
            or self.metric != self.topic.metric.primary
            or self.checklist != tuple(rule.id for rule in self.topic.checklist)
            or self.params != self.topic.params
            or self.eval_image_digest != self.topic.eval_image_digest
            or self.inference_offer_commitment != self.topic.inference_offer_commitment
            or self.plan.topic_id != self.topic_id
            or self.material.topic_digest != self.topic_digest
        ):
            raise ValueError("inconsistent harvest envelope")
        return self

    def commitment(self) -> str:
        # BYOK values stay on the private guest channel, as in the miner signature.
        return digest(
            {
                "domain": "cortex-proof-harvest-v1",
                "request": self.model_dump(mode="json"),
                "env_names": sorted(self.env),
            }
        )


class LiumLease(Document):
    instance_id: Annotated[str, Field(min_length=1, max_length=256)]
    template_id: Annotated[str, Field(min_length=1, max_length=128)]
    gpu_count: Annotated[int, Field(gt=0, le=64, strict=True)]
    image_digest: Annotated[str, Field(pattern=r"^sha256:[0-9a-f]{64}$")]


class HarvestExecution(Document):
    schema_version: Literal[1] = 1
    request_commitment: Hex64
    topic_digest: Hex64
    environment_digest: Hex64
    private_holdout_digest: Hex64
    inference_offer_commitment: Hex64
    experiment_vm_id: Identifier
    teardown_confirmed: Literal[True]
    topic_id: Identifier
    job_id: Hex64
    artifact_digest: Hex64
    eval_image_digest: Annotated[str, Field(pattern=r"^sha256:[0-9a-f]{64}$")]
    executor_config_commitment: Hex64
    gpu_count: Literal[1]
    exit_code: int
    verdict: Literal["clean", "suspicious", "reject"]
    reproduced: bool
    claim_holds: bool
    metrics: dict[str, float]
    rule_results: dict[str, bool]
    flops_used: Annotated[int, Field(ge=0, strict=True)]
    wall_seconds: Annotated[float, Field(ge=0, allow_inf_nan=False)]
    evidence_digest: Hex64
    stdout_tail: str = Field(default="", max_length=MAX_OUTPUT_BYTES)
    sandboxed: Literal[True]
    network_enabled: Literal[False] = False

    @field_validator("stdout_tail")
    @classmethod
    def bounded_output(cls, value: str) -> str:
        if len(value.encode()) > MAX_OUTPUT_BYTES:
            raise ValueError("harvest output exceeds byte limit")
        return value


class HarvestFailure(Exception):
    """Private remote failure; its text must not enter a public receipt."""

    def __init__(self, reason: str, stdout_tail: str = "") -> None:
        super().__init__(reason)
        self.reason = reason
        self.stdout_tail = stdout_tail


class LiumAdapter(Protocol):
    """Provider boundary; CI supplies a fake and never rents a live machine."""

    async def probe(self) -> bool: ...

    async def rent(self, request: HarvestRequest) -> LiumLease: ...

    async def execute(self, lease: LiumLease, request: HarvestRequest) -> HarvestExecution: ...

    async def terminate(self, lease: LiumLease) -> None: ...

    async def verify_terminated(self, lease: LiumLease) -> bool: ...


def _bounded_text(value: str, limit: int = MAX_OUTPUT_BYTES) -> str:
    encoded = value.encode(errors="replace")
    if len(encoded) <= limit:
        return value
    return encoded[:limit].decode(errors="ignore")


class LiumBackend:
    """Run standard Proof families on one rented GPU and confirm its teardown."""

    def __init__(
        self,
        *,
        registry: ExecutorOfferRegistry,
        adapter: LiumAdapter,
        inference_offer_commitment: str,
        topic_public_key: bytes,
        material_source: HarvestMaterialSource | None = None,
        overrides: HarvestOverrides | None = None,
        control_timeout_seconds: float = 60,
        teardown_grace_seconds: float = 30,
    ) -> None:
        if not re.fullmatch(r"[0-9a-f]{64}", inference_offer_commitment):
            raise ValueError("inference offer commitment required")
        if control_timeout_seconds <= 0 or teardown_grace_seconds <= 0:
            raise ValueError("Lium control timeouts must be positive")
        if len(topic_public_key) != 32:
            raise ValueError("trusted topic public key required")
        self.registry, self.adapter = registry, adapter
        self.inference_offer_commitment = inference_offer_commitment
        self.topic_public_key = topic_public_key
        self.material_source = material_source
        self.overrides = overrides or HarvestOverrides()
        self.control_timeout_seconds = control_timeout_seconds
        self.teardown_grace_seconds = teardown_grace_seconds

    async def readiness(self) -> Readiness:
        if self.material_source is None:
            raise ServiceError(503, "harvest private material source unavailable")
        try:
            offer = self.registry.require_open()
            async with asyncio.timeout(self.control_timeout_seconds):
                available = await self.adapter.probe()
        except (TimeoutError, ValueError, OSError):
            raise ServiceError(503, "harvest executor unavailable") from None
        except ServiceError:
            raise ServiceError(503, "harvest executor unavailable") from None
        except Exception:
            raise ServiceError(503, "harvest executor unavailable") from None
        if available is not True:
            raise ServiceError(503, "harvest executor unavailable")
        return Readiness(
            self.registry.pin.eval_image_digest,
            self.inference_offer_commitment,
            live_harvest_wired=True,
            executor_config_commitment=offer.config_commitment,
            executor_max_deadline_s=offer.max_proof_deadline_s,
        )

    async def executor_status(self) -> dict[str, object]:
        offer = self.registry.current()
        try:
            await self.readiness()
            ready, reason = True, None
        except ServiceError as error:
            ready, reason = False, error.reason
        return {
            "eval_executor": offer.public_view(),
            "ready": ready,
            "reason": reason,
            "pin": {
                "gpu_class": self.registry.pin.gpu_class,
                "max_proof_deadline_s": self.registry.pin.max_proof_deadline_s,
                "allowed_template_prefixes": list(self.registry.pin.allowed_template_prefixes),
                "eval_image_digest": self.registry.pin.eval_image_digest,
            },
        }

    def rotate_executor(self, offer: EvalExecutorOffer) -> EvalExecutorOffer:
        try:
            return self.registry.rotate(offer)
        except (OSError, ValueError):
            raise ServiceError(400, "invalid eval executor offer") from None

    def prepare(self, topic: Topic) -> ExecutorPlan:
        return self._prepare(topic)[0]

    def _prepare(self, topic: Topic) -> tuple[ExecutorPlan, HarvestMaterial]:
        try:
            plan = self.registry.plan(topic, self.overrides)
        except ValueError as error:
            raise ServiceError(503, str(error)) from None
        return plan, self._material(topic)

    def _material(self, topic: Topic) -> HarvestMaterial:
        if self.material_source is None:
            raise ServiceError(503, "harvest private material source unavailable")
        try:
            material = self.material_source.load(topic)
            material.verified(topic, self.topic_public_key)
        except (OSError, ValueError, ServiceError):
            raise ServiceError(503, "harvest topic or private material invalid") from None
        return material

    def _verify_request(self, request: HarvestRequest, commitment: str) -> None:
        if request.commitment() != commitment:
            raise ServiceError(503, "Lium request changed during execution")
        request.material.verified(request.topic, self.topic_public_key)

    async def evaluate(
        self,
        *,
        job_id: str,
        topic: Topic,
        submission: Submission,
        artifact: bytes | None,
        env: dict[str, str],
    ) -> EvaluationReport:
        await self.readiness()
        plan, material = self._prepare(topic)
        if artifact is None:
            raise ServiceError(503, "verified harvest artifact bytes unavailable")
        verify_artifact(artifact, submission.artifact_digest)
        check_env(topic.params, env)
        if topic.inference_offer_commitment != self.inference_offer_commitment:
            raise ServiceError(503, "harvest inference offer mismatch")
        try:
            envelope = SubmissionLookup.model_validate(
                submission.model_dump(exclude={"artifact_uri"})
            )
            if not verify_raw(
                bytes.fromhex(envelope.miner_hotkey),
                SUBMIT_DOMAIN,
                envelope.signing_payload(),
                bytes.fromhex(envelope.hotkey_signature),
            ):
                raise ValueError("invalid submission signature")
            request = HarvestRequest(
                job_id=job_id,
                topic=Topic.model_validate(topic.model_dump()),
                submission=envelope,
                material=material,
                topic_id=topic.id,
                topic_digest=topic.content_digest(),
                artifact_digest=submission.artifact_digest,
                artifact=artifact,
                claim=submission.claim,
                metric=topic.metric.primary,
                checklist=tuple(rule.id for rule in topic.checklist),
                params=topic.params,
                env=env,
                eval_image_digest=topic.eval_image_digest,
                inference_offer_commitment=topic.inference_offer_commitment,
                plan=plan,
            )
        except ValueError:
            raise ServiceError(503, "invalid harvest request") from None
        request_commitment = request.commitment()
        lease: LiumLease | None = None
        result: HarvestExecution | None = None
        failure: ServiceError | None = None
        cancellation: asyncio.CancelledError | None = None
        try:
            async with asyncio.timeout(self.control_timeout_seconds):
                lease = await self.adapter.rent(request)
            if (
                lease.gpu_count != 1
                or lease.template_id != plan.template_id
                or lease.image_digest != topic.eval_image_digest
            ):
                raise ServiceError(503, "Lium lease does not match the signed executor plan")
            self._verify_request(request, request_commitment)
            try:
                async with asyncio.timeout(plan.deadline_s + self.teardown_grace_seconds):
                    result = await self.adapter.execute(lease, request)
                if not isinstance(result, HarvestExecution):
                    failure = ServiceError(503, "invalid Lium execution response")
                    result = None
                else:
                    result = HarvestExecution.model_validate(result.model_dump())
            except TimeoutError:
                failure = ServiceError(503, "proof deadline exceeded")
            except (HarvestFailure, ServiceError, OSError, ValidationError, ValueError):
                # Guest output can contain private holdouts as well as credentials.
                failure = ServiceError(503, "Lium evaluation failed")
            except Exception:
                failure = ServiceError(503, "Lium evaluation failed")
            except asyncio.CancelledError as error:
                cancellation = error
        except TimeoutError:
            failure = ServiceError(503, "Lium rent timed out")
        except (ServiceError, OSError, ValidationError, ValueError):
            failure = ServiceError(503, "Lium rent failed")
        except Exception:
            failure = ServiceError(503, "Lium rent failed")
        except asyncio.CancelledError as error:
            cancellation = error

        teardown_confirmed = True
        if lease is not None:
            teardown_confirmed, teardown_cancellation = await self._protected_teardown(lease)
            if cancellation is None:
                cancellation = teardown_cancellation
        if cancellation is not None:
            raise cancellation
        if failure is not None and not teardown_confirmed:
            raise ServiceError(503, _bounded_text(f"{failure.reason}; Lium teardown unconfirmed"))
        if failure is not None:
            raise failure
        if not teardown_confirmed:
            raise ServiceError(503, "Lium teardown unconfirmed")
        if result is None or lease is None:
            raise ServiceError(503, "Lium evaluation returned no result")
        self._verify_request(request, request_commitment)
        self._verify_execution(result, request)
        if result.experiment_vm_id == lease.instance_id:
            raise ServiceError(503, "Lium experiment identity is not a fresh VM")
        return EvaluationReport(
            topic_id=topic.id,
            topic_digest=topic.content_digest(),
            submission_id=job_id,
            artifact_digest=submission.artifact_digest,
            verdict=result.verdict,
            reproduced=result.reproduced,
            claim_holds=result.claim_holds,
            rule_results=result.rule_results,
            metrics=result.metrics,
            flops_used=result.flops_used,
            wall_seconds=result.wall_seconds,
            evidence_digest=result.evidence_digest,
            vm_id=result.experiment_vm_id,
            sandboxed=True,
            teardown_confirmed=True,
            executor_offer_id=plan.offer_id,
            executor_offer_commitment=plan.offer_commitment,
            executor_config_commitment=plan.config_commitment,
            rationale="Lium evaluation completed with verified evidence",
        )

    async def _protected_teardown(
        self, lease: LiumLease
    ) -> tuple[bool, asyncio.CancelledError | None]:
        """Finish teardown despite repeated cancellation, then propagate it."""
        teardown = asyncio.create_task(self._teardown(lease))
        cancellation: asyncio.CancelledError | None = None
        while True:
            try:
                return await asyncio.shield(teardown), cancellation
            except asyncio.CancelledError as error:
                if cancellation is None:
                    cancellation = error
                if teardown.done():
                    return (False if teardown.cancelled() else teardown.result()), cancellation

    async def _teardown(self, lease: LiumLease) -> bool:
        try:
            async with asyncio.timeout(self.teardown_grace_seconds):
                await self.adapter.terminate(lease)
                return await self.adapter.verify_terminated(lease) is True
        except (TimeoutError, OSError, ServiceError, ValueError):
            return False
        except Exception:
            return False

    @staticmethod
    def _verify_execution(result: HarvestExecution, request: HarvestRequest) -> None:
        if (
            result.request_commitment != request.commitment()
            or result.topic_digest != request.topic_digest
            or result.environment_digest != request.material.environment_digest
            or result.private_holdout_digest != request.material.private_holdout_digest
            or result.inference_offer_commitment != request.inference_offer_commitment
            or result.teardown_confirmed is not True
            or result.topic_id != request.topic_id
            or result.job_id != request.job_id
            or result.artifact_digest != request.artifact_digest
            or result.eval_image_digest != request.eval_image_digest
            or result.executor_config_commitment != request.plan.config_commitment
            or result.gpu_count != 1
            or result.exit_code != 0
            or result.network_enabled
            or result.wall_seconds > request.plan.deadline_s
            or result.wall_seconds > request.topic.wall_budget_s
            or result.flops_used > request.topic.flops_budget
            or set(result.rule_results) != set(request.checklist)
            or request.metric not in result.metrics
        ):
            raise ServiceError(503, "Lium execution evidence binding mismatch")


class FamilyMux:
    """Keep custom Firecracker and standard Lium readiness independent."""

    def __init__(
        self,
        *,
        custom: EvaluationBackend | None,
        harvest: LiumBackend | None,
    ) -> None:
        self.custom, self.harvest = custom, harvest

    async def readiness(self) -> Readiness:
        custom_result, harvest_result = await asyncio.gather(
            self._readiness(self.custom),
            self._readiness(self.harvest),
        )
        available = [
            item for item in (custom_result, harvest_result) if isinstance(item, Readiness)
        ]
        if not available:
            raise ServiceError(503, "evaluation infrastructure unavailable")
        image = available[0].eval_image_digest
        inference = available[0].inference_offer_commitment
        if any(
            item.eval_image_digest != image or item.inference_offer_commitment != inference
            for item in available[1:]
        ):
            raise ServiceError(503, "evaluation family pins disagree")
        custom_ready = custom_result if isinstance(custom_result, Readiness) else None
        harvest_ready = harvest_result if isinstance(harvest_result, Readiness) else None
        return Readiness(
            image,
            inference,
            custom_ready.custom_ids if custom_ready else frozenset(),
            harvest_ready is not None and harvest_ready.live_harvest_wired,
            executor_config_commitment=(
                harvest_ready.executor_config_commitment if harvest_ready else None
            ),
            executor_max_deadline_s=(
                harvest_ready.executor_max_deadline_s if harvest_ready else None
            ),
            harvest_reason=(
                harvest_result.reason if isinstance(harvest_result, ServiceError) else None
            ),
        )

    @staticmethod
    async def _readiness(backend: EvaluationBackend | None) -> Readiness | ServiceError:
        if backend is None:
            return ServiceError(503, "family is not wired")
        try:
            return await backend.readiness()
        except ServiceError as error:
            return error
        except Exception:
            return ServiceError(503, "family readiness failed")

    async def evaluate(self, **kwargs) -> EvaluationReport:
        topic = kwargs.get("topic")
        if not isinstance(topic, Topic):
            raise ServiceError(503, "evaluation topic unavailable")
        backend: EvaluationBackend | None = (
            self.custom if topic.metric.family == "custom" else self.harvest
        )
        if backend is None:
            raise ServiceError(503, "evaluation family unavailable")
        return await backend.evaluate(**kwargs)

    def prepare(self, topic: Topic) -> ExecutorPlan | None:
        if topic.metric.family == "custom":
            return None
        if self.harvest is None:
            raise ServiceError(503, "harvest executor unavailable")
        return self.harvest.prepare(topic)

    async def run_agent(self, *args, **kwargs):
        run = getattr(self.custom, "run_agent", None)
        if run is None:
            raise ServiceError(503, "topic RLM VM unavailable")
        return await run(*args, **kwargs)

    async def executor_status(self) -> dict[str, object]:
        if self.harvest is None:
            return {"eval_executor": None, "ready": False, "reason": "harvest not wired"}
        return await self.harvest.executor_status()

    def rotate_executor(self, offer: EvalExecutorOffer) -> EvalExecutorOffer:
        if self.harvest is None:
            raise ServiceError(503, "harvest executor is not wired")
        return self.harvest.rotate_executor(offer)
