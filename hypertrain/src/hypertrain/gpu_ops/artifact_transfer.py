"""Executor results through R2 (docs/adr/0013-executor-artifacts-via-r2.md).

`ArtifactExecutor` is the generic CUDA executor with one difference. When the settings file named by
HYPERTRAIN_CUDA_EXECUTOR_ARTIFACTS lists the operation, the container does not stream the result tar
on the ssh stdout. It declares the hashes of that tar in R2; this module answers with presigned part
URLs bound to those hashes; the container uploads; the challenge completes the upload, fetches the
object (every 4 MiB block and the whole sha256 are checked) and hands the file to the unchanged
`_read_result`. The host holds URLs only: the R2 credential is read here, from a 0600 file.

Every attempt has its own keys, runs/<run>/exec/<job digest>/<nonce>/, because the tar is never
byte-identical between two attempts (summary.json holds timings, COMPLETE binds the request): a
later attempt cannot adopt the upload of an earlier one, and a stale object cannot reach it.
"""

from __future__ import annotations

import json
import os
import re
import secrets
import shutil
import stat
import threading
import time
from collections.abc import Callable, Mapping
from contextlib import contextmanager
from pathlib import Path
from typing import TYPE_CHECKING, Any, Literal

import httpx
from pydantic import Field, StrictInt, field_validator, model_validator

from hypertrain.data.store import CorruptObjectError, ObjectNotFound, StoreError
from hypertrain.gpu_ops.cuda_executor import (
    SMALL,
    Aborted,
    AttemptFailed,
    Boundary,
    Executor,
    ExecutorConfig,
    HostBusy,
    HostLost,
    HostRefused,
)
from hypertrain.gpu_ops.journal import Journal, sha256
from hypertrain.protocol.jcs import canonicalize
from hypertrain.storage.cas import BLOCK_BYTES, R2_MAX_PARTS, BlockList, R2ObjectStore
from hypertrain.storage.offer import UploadOffer, finish_upload, offer_upload
from hypertrain.storage.r2 import (
    ObjectConflict,
    R2Client,
    R2Config,
    R2Error,
    Retry,
    UploadGone,
    load_config,
    parse_env_file,
)

if TYPE_CHECKING:
    from collections.abc import Iterator

    from hypertrain.protocol.messages_v2 import IslandJobV1

UNREACHABLE = 11  # runner exit code: the host cannot reach the artifact store
STATUS_BYTES = 1 << 10
HEX64 = re.compile(r"[0-9a-f]{64}")
CREDENTIAL_NAMES = frozenset(
    {"R2_ACCOUNT_ID", "R2_ACCESS_KEY_ID", "R2_SECRET_ACCESS_KEY", "R2_BUCKET", "R2_ENDPOINT"}
)
BUCKET_PREFIX = "cortex-v3-"
DISK_RESERVE = 1 << 30
JOIN_SECONDS = 15


class ArtifactSettings(Boundary):
    credentials_file: Path  # a path only: the R2 token is read by this process and nobody else
    operations: tuple[Literal["reference", "audit", "referee"], ...] = ("audit", "referee")
    part_bytes: StrictInt = Field(16 << 20, ge=8 << 20, le=64 << 20)
    max_artifact_bytes: StrictInt = Field(16 << 30, ge=1 << 20)
    workers: StrictInt = Field(8, ge=1, le=32)
    reserve_seconds: StrictInt = Field(60, ge=0, le=3600)
    url_margin_seconds: StrictInt = Field(30, ge=0, le=600)
    url_ttl_cap_seconds: StrictInt = Field(3600, ge=60, le=86400)
    poll_ms: StrictInt = Field(500, ge=10, le=10_000)
    offer_wait_seconds: StrictInt = Field(120, ge=1, le=3600)
    request_timeout_seconds: StrictInt = Field(30, ge=1, le=600)
    retry_attempts: StrictInt = Field(4, ge=1, le=10)
    retry_base_ms: StrictInt = Field(500, ge=1, le=10_000)
    fallback: Literal["classic", "fail"] = "classic"

    @field_validator("operations")
    @classmethod
    def _operations(cls, value: tuple[str, ...]) -> tuple[str, ...]:
        if not value or len(set(value)) != len(value):
            raise ValueError("operations must be a non-empty list without repeats")
        return value

    @model_validator(mode="after")
    def _sizes(self) -> ArtifactSettings:
        if self.part_bytes % BLOCK_BYTES:
            raise ValueError("part_bytes must be a whole number of 4 MiB blocks")
        if self.max_artifact_bytes > R2_MAX_PARTS * self.part_bytes:
            raise ValueError("max_artifact_bytes needs more than 10,000 parts of part_bytes")
        return self


def load_credentials(path: Path) -> R2Config:
    """The R2 settings of a bucket-scoped token: a regular file of this user, mode 0600, holding
    nothing but the five R2 names; the process environment cannot override it."""
    status = path.lstat()
    if not stat.S_ISREG(status.st_mode) or status.st_uid != os.getuid() or status.st_mode & 0o077:
        raise ValueError("credentials_file must be a regular file of this user with mode 0600")
    if set(parse_env_file(path)) - CREDENTIAL_NAMES:
        raise ValueError("credentials_file holds names other than the R2 settings")
    try:
        config = load_config(env={}, env_file=path)
    except R2Error as error:
        raise ValueError(str(error)) from None
    if not config.bucket.startswith(BUCKET_PREFIX):
        raise ValueError(f"the artifact bucket must be a {BUCKET_PREFIX}* bucket")
    secure = config.url.startswith("https://")
    loopback = config.url.startswith("http://127.0.0.1")  # the tests' own fake server
    if not (secure or loopback):
        raise ValueError("the R2 endpoint must be https")
    return config


class Refusal(Exception):
    """A declaration the challenge will not answer; `code` is the fixed word the host is told."""

    def __init__(self, code: str) -> None:
        super().__init__(code)
        self.code = code


class Declared:
    def __init__(
        self, size: int, whole: str, blocks: tuple[str, ...], parts: tuple[str, ...]
    ) -> None:
        self.size, self.sha256, self.blocks, self.parts = size, whole, blocks, parts


def _hashes(value: object, count: int) -> tuple[str, ...]:
    if (
        not isinstance(value, list)
        or len(value) != count
        or not all(isinstance(h, str) and HEX64.fullmatch(h) for h in value)
    ):
        raise Refusal("malformed")
    return tuple(value)


def parse_declaration(raw: bytes, settings: ArtifactSettings, request_sha256: str) -> Declared:
    """Strict: exact keys, integers that are not booleans, lowercase hex, list lengths that follow
    from the size, the part size the challenge asked for, and this attempt's request."""
    try:
        data = json.loads(raw)
    except (ValueError, RecursionError):
        raise Refusal("malformed") from None
    keys = {"version", "request_sha256", "size", "sha256", "part_bytes", "blocks", "parts"}
    if not isinstance(data, dict) or set(data) != keys:
        raise Refusal("malformed")
    version, size, part_bytes = data["version"], data["size"], data["part_bytes"]
    if any(type(n) is not int for n in (version, size, part_bytes)) or version != 1:
        raise Refusal("malformed")
    if data["request_sha256"] != request_sha256:
        raise Refusal("wrong_request")
    if not 1 <= size <= settings.max_artifact_bytes:
        raise Refusal("too_large" if size > 0 else "malformed")
    if part_bytes != settings.part_bytes:
        raise Refusal("malformed")
    whole = data["sha256"]
    if not isinstance(whole, str) or not HEX64.fullmatch(whole):
        raise Refusal("malformed")
    blocks = _hashes(data["blocks"], -(-size // BLOCK_BYTES))
    parts = _hashes(data["parts"], -(-size // part_bytes))
    if whole == BlockList(size, blocks).digest():
        raise Refusal("malformed")  # the object would overwrite its own block list
    return Declared(size, whole, blocks, parts)


class Session:
    """One attempt's exchange with the container: the mailbox URLs it is given, the thread that
    answers its declaration, and the collection of the uploaded object."""

    def __init__(self, broker: Broker, job: IslandJobV1, prefix: str, nonce: str) -> None:
        self.broker, self.job, self.prefix, self.nonce = broker, job, prefix, nonce
        self.store = R2ObjectStore(
            broker.client, prefix + "r/", part_bytes=broker.settings.part_bytes
        )
        self.request_sha256 = ""
        self.declared: Declared | None = None
        self.upload: UploadOffer | None = None
        self.failure = ""
        self.failed = threading.Event()  # set when the answering side cannot go on
        self.stop = threading.Event()
        self.closed = False
        self.state = "waiting"
        self.lock = threading.RLock()
        self.thread: threading.Thread | None = None
        self.offered_at = self.upload_ms = self.download_ms = 0
        self.block = self._block()

    # -- what the container is given ------------------------------------------------------

    def _ttl(self) -> int:
        settings = self.broker.settings
        left = int(self.job.deadline - time.time())
        if left <= 0:
            raise Aborted("job deadline expired")
        return max(1, min(left + settings.url_margin_seconds, settings.url_ttl_cap_seconds))

    def _block(self) -> dict[str, Any]:
        settings, client, ttl = self.broker.settings, self.broker.client, self._ttl()
        return {
            "version": 1,
            "part_bytes": settings.part_bytes,
            "workers": settings.workers,
            "reserve_seconds": settings.reserve_seconds,
            "poll_ms": settings.poll_ms,
            "offer_wait_s": settings.offer_wait_seconds,
            "retry_attempts": settings.retry_attempts,
            "retry_base_ms": settings.retry_base_ms,
            "timeout_s": settings.request_timeout_seconds,
            "declare_put": client.presign(
                "PUT", self.prefix + "declare.json", expires=ttl, headers={"if-none-match": "*"}
            ),
            "offer_get": client.presign("GET", self.prefix + "offer.json", expires=ttl),
        }

    def bind(self, request: bytes) -> None:
        self.request_sha256 = sha256(request)

    # -- answering the declaration --------------------------------------------------------

    def start(self) -> None:
        self.thread = threading.Thread(target=self._answer, name="artifact-offer", daemon=True)
        self.thread.start()

    def stop_thread(self) -> None:
        self.stop.set()
        if self.thread is not None:
            self.thread.join(JOIN_SECONDS)

    def _answer(self) -> None:
        try:
            raw = self._wait_for_declaration()
            if raw is not None:
                self._offer(parse_declaration(raw, self.broker.settings, self.request_sha256), raw)
        except Refusal as refusal:
            self._refuse(refusal.code)
        except Aborted:
            self._refuse("deadline")
        except Exception as error:  # noqa: BLE001 - the thread must report, not die silently
            self._fail(f"offer thread: {type(error).__name__}")

    def _fail(self, reason: str) -> None:
        with self.lock:
            if self.state == "waiting":
                self.state, self.failure = "failed", reason
        self.failed.set()

    def _wait_for_declaration(self) -> bytes | None:
        client, key = self.broker.client, self.prefix + "declare.json"
        settings = self.broker.settings
        cap = self.broker.declaration_cap
        while not self.stop.is_set():
            info = client.head_object(key)
            if info is not None:
                if not 0 < info.size <= cap:
                    raise Refusal("too_large" if info.size else "malformed")
                return client.get_range(key, 0, info.size)  # read once; never re-read
            self.stop.wait(settings.poll_ms / 1000)
        return None

    def _refuse(self, code: str) -> None:
        body = json.dumps({"version": 1, "refused": code}).encode()
        try:
            self.broker.client.put_object(self.prefix + "offer.json", body)
        except Exception as error:  # noqa: BLE001 - the container then waits; the attempt must end
            self._fail(f"refusal could not be written: {type(error).__name__}")
            return
        with self.lock:
            self.state, self.failure = "refused", f"declaration refused ({code})"

    def _offer(self, declared: Declared, raw: bytes) -> None:
        with self.lock:
            if self.closed:
                return
        blocks = BlockList(declared.size, declared.blocks)
        offer = offer_upload(
            self.store, declared.sha256, blocks, declared.parts, expires=self._ttl()
        )
        with self.lock:
            if self.closed:
                self.broker.abort(self.store.key(declared.sha256), offer.upload_id)
                return
            self.declared, self.upload = declared, offer
        self.broker.journal.append(
            "exec_artifact_offered", nonce=self.nonce, size=declared.size, parts=len(declared.parts)
        )
        body = canonicalize(
            {
                "version": 1,
                "declare_sha256": sha256(raw),
                "ttl_s": self._ttl(),
                "parts": [part.url for part in offer.parts],
                "list": list(offer.list_urls),
            }
        )
        self.broker.client.put_object(self.prefix + "offer.json", body)
        with self.lock:
            self.state, self.offered_at = "offered", time.monotonic_ns()

    # -- the result -----------------------------------------------------------------------

    def parts_held(self) -> bool:
        """Every planned part is stored at its planned size (a host lost after its last byte)."""
        if self.upload is None:
            return False
        plan = self.upload.plan
        listed = self.broker.client.list_parts(self.upload.object_key, self.upload.upload_id)
        held = {p.number: p.size for p in listed or ()}
        return all(held.get(n) == plan.span(n)[1] for n in range(1, plan.count + 1))

    def collect(self, status: Path, stop: Callable[[], None], *, lost: bool) -> None:
        """Complete the upload, fetch the object over `status` and verify it. `lost`: the ssh
        session ended without a status line, so only the stored parts vouch for the upload."""
        with self.lock:
            declared, offer, state = self.declared, self.upload, self.state
        if state != "offered" or declared is None or offer is None:
            raise AttemptFailed(self.failure or "the container never received an offer")
        if not lost:
            _check_status(status, declared)
        client, started = self.broker.client, time.monotonic_ns()
        self.upload_ms = (started - self.offered_at) // 1_000_000
        with _storage("artifact upload"):
            finish_upload(self.store, offer)
            info = client.head_object(offer.object_key)
        blocks = BlockList(declared.size, declared.blocks)
        if (
            info is None
            or info.size != declared.size
            or info.sha256 != declared.sha256
            or info.meta.get("blocks") != blocks.digest()
        ):
            raise AttemptFailed("the stored object is not the one that was declared")
        if shutil.disk_usage(status.parent).free < 2.2 * declared.size + DISK_RESERVE:
            raise AttemptFailed("not enough disk for the artifact")
        try:
            with _storage("artifact download"):
                self.store.fetch_to(declared.sha256, status, check=stop)
        except CorruptObjectError:
            raise HostRefused(
                "the uploaded bytes differ from the hashes the host declared"
            ) from None
        self.download_ms = (time.monotonic_ns() - started) // 1_000_000
        self.broker.journal.append(
            "exec_artifact_done",
            nonce=self.nonce,
            sha256=declared.sha256,
            size=declared.size,
            upload_ms=self.upload_ms,
            download_ms=self.download_ms,
        )

    def record(self) -> dict[str, Any]:
        """For role-dispatch.json: strings and integers only (envelope_v2 rejects floats)."""
        declared = self.declared
        assert declared is not None
        return {
            "transport": "r2",
            "sha256": declared.sha256,
            "size": declared.size,
            "part_bytes": self.broker.settings.part_bytes,
            "parts": len(declared.parts),
            "upload_ms": self.upload_ms,
            "download_ms": self.download_ms,
        }

    def close(self) -> None:
        """End of the attempt: no more offers, the open upload aborted, the object and its block
        list deleted. Never raises; the mailbox objects stay for the bucket's lifecycle rule."""
        with self.lock:
            self.closed = True
            declared, offer = self.declared, self.upload
        self.stop_thread()
        broker = self.broker
        if offer is not None and declared is not None:
            key = offer.object_key
            broker.abort(key, offer.upload_id)
            blocks = BlockList(declared.size, declared.blocks)
            broker.delete(key)
            broker.delete(self.store.key(blocks.digest()))
        broker.journal.append("exec_artifact_end", nonce=self.nonce)


def _check_status(status: Path, declared: Declared) -> None:
    """The container's one line: it must say what was declared."""
    with status.open("rb") as stream:
        raw = stream.read(STATUS_BYTES + 1)
    try:
        said = json.loads(raw) if len(raw) <= STATUS_BYTES else None
    except ValueError:
        said = None
    if (
        not isinstance(said, dict)
        or said.get("artifact") != "uploaded"
        or said.get("sha256") != declared.sha256
        or said.get("size") != declared.size
    ):
        raise AttemptFailed("the container's status line does not match its declaration")


@contextmanager
def _storage(what: str) -> Iterator[None]:
    """One boundary for storage errors: an attempt failure that names the class, never a URL."""
    try:
        yield
    except CorruptObjectError:
        raise
    except (R2Error, StoreError, ObjectNotFound, UploadGone, ObjectConflict, OSError) as error:
        raise AttemptFailed(f"{what} failed: {type(error).__name__}") from None


class Broker:
    """The R2 side of the executor: one credential, the settings, and the sessions it opens."""

    def __init__(
        self,
        settings: ArtifactSettings,
        credentials: R2Config,
        journal: Journal,
        *,
        transport: httpx.BaseTransport | None = None,
        sleep: Callable[[float], None] = time.sleep,
    ) -> None:
        self.settings, self.journal = settings, journal
        retry = Retry(
            attempts=settings.retry_attempts,
            base=settings.retry_base_ms / 1000,
            cap=8.0,
            sleep=sleep,
        )
        http = httpx.Client(timeout=settings.request_timeout_seconds, transport=transport)
        self.client = R2Client(credentials, client=http, retry=retry)
        self._probe = R2Client(
            credentials, client=http, retry=Retry(attempts=2, cap=2.0, sleep=sleep)
        )
        quick = httpx.Client(timeout=5, transport=transport)
        self._cleanup = R2Client(
            credentials, client=quick, retry=Retry(attempts=2, cap=1.0, sleep=sleep)
        )
        size, part = settings.max_artifact_bytes, settings.part_bytes
        self.declaration_cap = 4096 + 72 * (-(-size // BLOCK_BYTES) + -(-size // part))

    def begin(self, job: IslandJobV1, operation: str | None) -> Session | None:
        """A session for this attempt, or None when the attempt runs the classic way."""
        if operation not in self.settings.operations:
            return None
        nonce = secrets.token_hex(16)
        prefix = f"runs/{job.run_id}/exec/{job.digest()}/{nonce}/"
        try:
            self._probe.head_object(prefix + "declare.json")
        except R2Error as error:
            self.journal.append("exec_artifact_unavailable", reason=type(error).__name__)
            if self.settings.fallback == "classic":
                return None
            raise AttemptFailed("the artifact store is unavailable") from None
        self.journal.append(  # before any URL exists, so a crash leaves a record to sweep by
            "exec_artifact_begin",
            nonce=nonce,
            prefix=prefix,
            expires_unix=int(job.deadline) + self.settings.url_margin_seconds,
        )
        return Session(self, job, prefix, nonce)

    def abort(self, key: str, upload_id: str) -> None:
        self._best_effort(lambda: self._cleanup.abort_multipart(key, upload_id))

    def delete(self, key: str) -> None:
        self._best_effort(lambda: self._cleanup.delete_object(key))

    def _best_effort(self, call: Callable[[], None]) -> None:
        try:
            call()
        except Exception as error:  # noqa: BLE001 - cleanup must not mask the attempt's outcome
            self.journal.append("exec_artifact_cleanup_failed", error=type(error).__name__)


class _Either(threading.Event):
    """Set when the lease is cancelled or the answering thread has failed."""

    def __init__(self, *events: threading.Event | None) -> None:
        super().__init__()
        self._events = [event for event in events if event is not None]

    def is_set(self) -> bool:
        return any(event.is_set() for event in self._events)


class ArtifactExecutor(Executor):
    def __init__(
        self,
        config: ExecutorConfig,
        settings: ArtifactSettings,
        credentials: R2Config,
        *,
        artifact_transport: httpx.BaseTransport | None = None,
        **kwargs: Any,
    ) -> None:
        if config.network != "default":
            raise ValueError("artifacts need network 'default': the container must reach R2")
        super().__init__(config, **kwargs)
        self.broker = Broker(
            settings, credentials, self.journal, transport=artifact_transport, sleep=self.sleep
        )
        self._ctx = threading.local()

    def launch(
        self, operation: str, binding: str, job: IslandJobV1, directory: Path, **kwargs: Any
    ) -> Any:
        self._ctx.operation = operation
        try:
            return super().launch(operation, binding, job, directory, **kwargs)
        finally:
            self._ctx.operation = None

    def _request(
        self,
        job: IslandJobV1,
        directory: Path,
        chosen: Any,
        trace: bool,
        input_tar: Path,
        *,
        extra: Mapping[str, Any] | None = None,
    ) -> bytes:
        self._ctx.session = self._ctx.collected = None
        session = self.broker.begin(job, getattr(self._ctx, "operation", None))
        if session is None:
            return super()._request(job, directory, chosen, trace, input_tar, extra=extra)
        try:
            request = super()._request(
                job,
                directory,
                chosen,
                trace,
                input_tar,
                extra={**(extra or {}), "artifact": session.block},
            )
        except BaseException:
            session.close()
            raise
        session.bind(request)
        self._ctx.session = session
        return request

    def _remote(self, label: str, command: str, **kwargs: Any) -> int:
        session: Session | None = getattr(self._ctx, "session", None)
        if kwargs.get("stdin") is None or session is None:  # the probe, the pull, the cleanup
            return super()._remote(label, command, **kwargs)
        self._ctx.session = None
        try:
            return self._run(session, label, command, **kwargs)
        finally:
            session.close()

    def _run(self, session: Session, label: str, command: str, **kwargs: Any) -> int:
        status, deadline, cancel = kwargs["stdout"], kwargs["deadline"], kwargs["cancel"]

        def stop() -> None:
            if cancel is not None and cancel.is_set():
                raise Aborted("lease cancelled")
            if time.time() >= deadline:
                raise Aborted("job deadline expired")

        lost = False
        session.start()
        try:
            code = super()._remote(
                label,
                command,
                **{**kwargs, "cancel": _Either(cancel, session.failed), "max_bytes": SMALL},
            )
        except Aborted:
            session.stop_thread()
            if session.failed.is_set() and not (cancel is not None and cancel.is_set()):
                raise AttemptFailed(session.failure) from None
            raise
        except HostLost:
            session.stop_thread()
            if not session.parts_held():
                raise
            self.journal.append("exec_artifact_salvaged", nonce=session.nonce)
            code, lost = 0, True
        session.stop_thread()
        if code == UNREACHABLE:
            raise HostBusy("the host cannot reach the artifact store")
        if code:
            if session.failure:
                raise AttemptFailed(session.failure)
            return code
        session.collect(status, stop, lost=lost)
        self._ctx.collected = session.record()
        return 0

    def _publish(
        self, job: IslandJobV1, directory: Path, staging: Path, record: dict[str, Any], trace: bool
    ) -> Any:
        collected, self._ctx.collected = getattr(self._ctx, "collected", None), None
        if collected is not None:
            record = {**record, "artifact": collected}
            (staging.parent / "result.tar").unlink(missing_ok=True)  # peak disk: one copy fewer
        return super()._publish(job, directory, staging, record, trace)


def build(config: ExecutorConfig, path: Path) -> ArtifactExecutor:
    settings = ArtifactSettings.model_validate_json(path.read_bytes())
    return ArtifactExecutor(config, settings, load_credentials(settings.credentials_file))
