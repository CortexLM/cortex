"""Concrete Lium REST and SSH transport for the standard Proof executor.

The provider wire is ported from the former Rust implementation.  The module
does not select or publish an executor offer; :mod:`cortex.proof.executor`
validates that signed policy before this adapter receives a request.
"""

from __future__ import annotations

import asyncio
import hashlib
import json
import math
import os
import re
import shlex
import stat
import tempfile
from collections.abc import AsyncIterator, Awaitable, Callable
from contextlib import asynccontextmanager
from dataclasses import dataclass
from pathlib import Path
from typing import Annotated, Protocol
from urllib.parse import urlsplit

import httpx
from pydantic import Field, ValidationError, field_validator, model_validator

from cortex.errors import ServiceError
from cortex.http import read_private_file
from cortex.proof.executor import (
    MAX_OUTPUT_BYTES,
    HarvestExecution,
    HarvestFailure,
    HarvestRequest,
    LiumBackend,
    LiumLease,
)
from cortex.proof.models import Document

LIUM_API_BASE_URL = "https://lium.io/api"
MAX_PROVIDER_BODY_BYTES = 1024 * 1024
MAX_SSH_CAPTURE_BYTES = 64 * 1024

_RUNNING = frozenset({"RUNNING", "RUNNING_SSH", "READY"})
_TERMINAL = frozenset({"FAILED", "ERROR", "CREATION_FAILED", "TERMINATED", "DELETED", "STOPPED"})
_NAME = re.compile(r"^[a-z0-9][a-z0-9-]{0,31}$")
_POD_NAME = re.compile(r"^[a-z0-9][a-z0-9-]{0,62}$")
_REMOTE_USER = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$")
_REMOTE_HOST = re.compile(r"^[A-Za-z0-9][A-Za-z0-9.-]{0,252}$")
_GHCR_REPOSITORY = re.compile(r"^ghcr\.io/[A-Za-z0-9._/-]+$")
_PROVIDER_ID = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._:-]{0,255}$")
_ENV_NAME = re.compile(r"^[A-Z][A-Z0-9_]{0,63}$")
_RESERVED_ENV = frozenset(
    {
        "PATH",
        "HOME",
        "LANG",
        "XDG_RUNTIME_DIR",
        "LD_PRELOAD",
        "LD_LIBRARY_PATH",
        "PYTHONPATH",
        "PYTHONHOME",
        "BASH_ENV",
        "ENV",
    }
)
_NOT_FOUND = object()


class LiumAdapterConfig(Document):
    """All operator inputs required by the reviewed Lium transport."""

    api_base_url: str = LIUM_API_BASE_URL
    api_key_file: Path
    ssh_private_key_file: Path
    ssh_public_key_file: Path
    ssh_known_hosts_file: Path
    image_repository: str
    gpu_name: Annotated[str, Field(min_length=1, max_length=128)]
    max_price_per_hour: Annotated[float, Field(gt=0, allow_inf_nan=False)]
    max_lifetime_hours: Annotated[float, Field(ge=1, le=24, allow_inf_nan=False)] = 6
    pod_name_prefix: str = "cortex-proof"
    ssh_key_name: str = "proof-eval-worker"
    request_timeout_seconds: Annotated[float, Field(gt=0, le=300)] = 60
    running_timeout_seconds: Annotated[float, Field(gt=0, le=3600)] = 900
    poll_interval_seconds: Annotated[float, Field(ge=0.25, le=60)] = 5
    http_attempts: Annotated[int, Field(ge=1, le=8, strict=True)] = 3
    reconcile_attempts: Annotated[int, Field(ge=1, le=20, strict=True)] = 3
    teardown_attempts: Annotated[int, Field(ge=1, le=20, strict=True)] = 5

    @field_validator("api_base_url")
    @classmethod
    def valid_api_url(cls, value: str) -> str:
        parsed = urlsplit(value)
        if (
            parsed.scheme != "https"
            or not parsed.hostname
            or parsed.username
            or parsed.password
            or parsed.query
            or parsed.fragment
        ):
            raise ValueError("Lium API URL must be credential-free HTTPS")
        return value.rstrip("/")

    @field_validator("image_repository")
    @classmethod
    def valid_repository(cls, value: str) -> str:
        if not _GHCR_REPOSITORY.fullmatch(value) or ".." in value.split("/"):
            raise ValueError("eval image repository must be an untagged GHCR repository")
        return value

    @field_validator("gpu_name")
    @classmethod
    def valid_gpu_name(cls, value: str) -> str:
        if value.strip() != value or not value.isascii() or not value.isprintable():
            raise ValueError("gpu_name must be printable ASCII")
        return value

    @model_validator(mode="after")
    def valid_names(self) -> LiumAdapterConfig:
        if not _NAME.fullmatch(self.pod_name_prefix) or not _NAME.fullmatch(self.ssh_key_name):
            raise ValueError("invalid Lium resource name")
        return self


@dataclass(frozen=True)
class SshTarget:
    user: str
    host: str
    port: int


@dataclass(frozen=True)
class SshResult:
    returncode: int
    stdout: str
    stderr: str


@dataclass
class _RentLockEntry:
    lock: asyncio.Lock
    users: int = 0


class SshTransport(Protocol):
    async def run(
        self,
        target: SshTarget,
        command: str,
        *,
        stdin: bytes | None = None,
        timeout_seconds: float,
        allow_failure: bool = False,
    ) -> SshResult: ...


class LiumGuestWire(Protocol):
    """Versioned eval-image request/result contract, intentionally unwired."""

    async def execute(
        self,
        ssh: SshTransport,
        target: SshTarget,
        request: HarvestRequest,
    ) -> HarvestExecution: ...


async def _drain_tail(stream: asyncio.StreamReader | None, limit: int) -> tuple[bytes, int]:
    if stream is None:
        return b"", 0
    tail = bytearray()
    total = 0
    while chunk := await stream.read(64 * 1024):
        total += len(chunk)
        tail.extend(chunk)
        if len(tail) > limit:
            del tail[: len(tail) - limit]
    return bytes(tail), total


class OpenSshTransport:
    """OpenSSH subprocess boundary with bounded output and no shell-local secrets."""

    def __init__(
        self,
        private_key_file: Path,
        *,
        known_hosts_file: Path | None = None,
        capture_limit: int = MAX_SSH_CAPTURE_BYTES,
    ):
        if capture_limit < 16_384:
            raise ValueError("SSH capture limit is too small")
        self.private_key_file = private_key_file
        self.known_hosts_file = known_hosts_file
        self.capture_limit = capture_limit

    async def run(
        self,
        target: SshTarget,
        command: str,
        *,
        stdin: bytes | None = None,
        timeout_seconds: float,
        allow_failure: bool = False,
    ) -> SshResult:
        _validate_ssh_target(target)
        if self.known_hosts_file is None:
            raise HarvestFailure("SSH known hosts file required")
        if (
            not math.isfinite(timeout_seconds)
            or timeout_seconds <= 0
            or not isinstance(command, str)
            or "\x00" in command
        ):
            raise HarvestFailure("invalid SSH command or deadline")
        key = _read_ssh_file(self.private_key_file, "private key", private=True, limit=16_384)
        hosts = _read_ssh_file(self.known_hosts_file, "known hosts", limit=1024 * 1024)
        # OpenSSH closes inherited descriptors. Private snapshots bind its reads
        # to validated bytes even if an operator rotates the original paths.
        with tempfile.TemporaryDirectory(prefix="cortex-ssh-", dir="/tmp") as directory:
            identity, known_hosts = Path(directory) / "identity", Path(directory) / "known_hosts"
            for path, content in ((identity, key), (known_hosts, hosts)):
                descriptor = os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
                with os.fdopen(descriptor, "wb") as output:
                    output.write(content)
            return await self._run(
                target,
                command,
                identity=identity,
                known_hosts=known_hosts,
                stdin=stdin,
                timeout_seconds=timeout_seconds,
                allow_failure=allow_failure,
            )

    async def _run(
        self,
        target: SshTarget,
        command: str,
        *,
        identity: Path,
        known_hosts: Path,
        stdin: bytes | None,
        timeout_seconds: float,
        allow_failure: bool,
    ) -> SshResult:
        options = {
            "StrictHostKeyChecking": "yes",
            "UserKnownHostsFile": str(known_hosts),
            "GlobalKnownHostsFile": "/dev/null",
            "KnownHostsCommand": "none",
            "VerifyHostKeyDNS": "no",
            "UpdateHostKeys": "no",
            "CanonicalizeHostname": "no",
            "IdentitiesOnly": "yes",
            "IdentityAgent": "none",
            "AddKeysToAgent": "no",
            "CertificateFile": "none",
            "PreferredAuthentications": "publickey",
            "PasswordAuthentication": "no",
            "KbdInteractiveAuthentication": "no",
            "GSSAPIAuthentication": "no",
            "HostbasedAuthentication": "no",
            "ProxyCommand": "none",
            "ProxyJump": "none",
            "ControlMaster": "no",
            "ControlPath": "none",
            "ControlPersist": "no",
            "ForwardAgent": "no",
            "ForwardX11": "no",
            "ClearAllForwardings": "yes",
            "Tunnel": "no",
            "PermitLocalCommand": "no",
            "ConnectTimeout": "15",
            "ServerAliveInterval": "10",
            "ServerAliveCountMax": "60",
            "TCPKeepAlive": "yes",
            "BatchMode": "yes",
        }
        process = await asyncio.create_subprocess_exec(
            "/usr/bin/ssh",
            "-F",
            "/dev/null",
            "-T",
            "-i",
            str(identity),
            *(
                argument
                for name, value in options.items()
                for argument in ("-o", f"{name}={value}")
            ),
            "-p",
            str(target.port),
            "--",
            f"{target.user}@{target.host}",
            command,
            env={"PATH": "/usr/bin:/bin", "LC_ALL": "C"},
            stdin=asyncio.subprocess.PIPE if stdin is not None else asyncio.subprocess.DEVNULL,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )

        async def feed() -> None:
            if stdin is None or process.stdin is None:
                return
            process.stdin.write(stdin)
            try:
                await process.stdin.drain()
            except (BrokenPipeError, ConnectionResetError):
                pass
            process.stdin.close()
            try:
                await process.stdin.wait_closed()
            except (BrokenPipeError, ConnectionResetError):
                pass

        stdout_task = asyncio.create_task(_drain_tail(process.stdout, self.capture_limit))
        stderr_task = asyncio.create_task(_drain_tail(process.stderr, self.capture_limit))
        feed_task = asyncio.create_task(feed())
        try:
            async with asyncio.timeout(timeout_seconds):
                await asyncio.gather(feed_task, process.wait())
                stdout, stdout_size = await stdout_task
                stderr, stderr_size = await stderr_task
        except BaseException:
            if process.returncode is None:
                process.kill()
                await process.wait()
            for task in (feed_task, stdout_task, stderr_task):
                if not task.done():
                    task.cancel()
            await asyncio.gather(feed_task, stdout_task, stderr_task, return_exceptions=True)
            raise

        result = SshResult(
            process.returncode if process.returncode is not None else -1,
            stdout.decode(errors="replace"),
            stderr.decode(errors="replace"),
        )
        if stdout_size > self.capture_limit or stderr_size > self.capture_limit:
            result = SshResult(
                result.returncode,
                "<truncated>\n" + result.stdout,
                "<truncated>\n" + result.stderr,
            )
        if result.returncode != 0 and not allow_failure:
            raise HarvestFailure(
                f"SSH command failed with exit {result.returncode}",
                _bounded(result.stderr or result.stdout),
            )
        return result


def _require_private_regular_file(path: Path) -> None:
    _read_ssh_file(path, "private key", private=True, limit=16_384)


def _read_ssh_file(path: Path, label: str, *, limit: int, private: bool = False) -> bytes:
    try:
        descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        with os.fdopen(descriptor, "rb") as source:
            metadata = os.fstat(source.fileno())
            if (
                not stat.S_ISREG(metadata.st_mode)
                or stat.S_IMODE(metadata.st_mode) & (0o077 if private else 0o022)
                or metadata.st_uid not in {0, os.geteuid()}
                or metadata.st_nlink != 1
                or not 0 < metadata.st_size <= limit
            ):
                raise ValueError("unsafe SSH file")
            value = source.read(limit + 1)
            if not value.strip() or len(value) > limit:
                raise ValueError("empty or oversized SSH file")
            return value
    except (OSError, ValueError):
        raise HarvestFailure(f"SSH {label} unavailable") from None


def _validate_ssh_target(target: SshTarget) -> None:
    if (
        not isinstance(target.user, str)
        or not _REMOTE_USER.fullmatch(target.user)
        or not isinstance(target.host, str)
        or not _REMOTE_HOST.fullmatch(target.host)
        or type(target.port) is not int
        or not 1 <= target.port <= 65535
    ):
        raise HarvestFailure("Lium pod has no valid SSH target")


def _bounded(value: str, limit: int = MAX_OUTPUT_BYTES) -> str:
    encoded = value.encode(errors="replace")
    if len(encoded) <= limit:
        return value
    return encoded[-limit:].decode(errors="ignore")


def _redact(value: str, secrets: tuple[str, ...]) -> str:
    for secret in sorted((secret for secret in secrets if secret), key=len, reverse=True):
        value = value.replace(secret, "<redacted>")
    return value


def _arrays(value: object, *keys: str) -> list[dict[str, object]]:
    candidate = value
    if isinstance(candidate, dict):
        selected = next((candidate[key] for key in keys if key in candidate), _NOT_FOUND)
        if selected is _NOT_FOUND:
            raise HarvestFailure("Lium API returned a malformed collection")
        candidate = selected
    if not isinstance(candidate, list) or any(not isinstance(row, dict) for row in candidate):
        raise HarvestFailure("Lium API returned a malformed collection")
    return [row for row in candidate if isinstance(row, dict)]


def _string(row: dict[str, object], *keys: str) -> str | None:
    for key in keys:
        value = row.get(key)
        if isinstance(value, str) and value:
            return value
    return None


def _number(row: dict[str, object], *keys: str) -> float | None:
    for key in keys:
        value = row.get(key)
        if isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value):
            return float(value)
        if isinstance(value, str):
            try:
                parsed = float(value)
            except ValueError:
                continue
            if math.isfinite(parsed):
                return parsed
    return None


def _offer_label(row: dict[str, object]) -> str:
    direct = _string(row, "gpu_type", "gpu_name", "machine_name")
    machine = row.get("machine")
    nested = _string(machine, "gpu_type") if isinstance(machine, dict) else None
    return direct or nested or ""


def _offer_price(row: dict[str, object]) -> float | None:
    direct = _number(row, "price_per_hour", "price_per_gpu", "price")
    price = row.get("price")
    nested = _number(price, "per_gpu_hour") if isinstance(price, dict) else None
    return direct if direct is not None else nested


def _pod_gpu_label(row: dict[str, object]) -> str:
    direct = _string(row, "gpu_type", "gpu_name", "machine_name")
    executor = row.get("executor")
    nested = _string(executor, "gpu_type") if isinstance(executor, dict) else None
    return direct or nested or ""


def _pod_template_id(row: dict[str, object]) -> str:
    direct = _string(row, "template_id")
    template = row.get("template")
    nested = _string(template, "id", "template_id") if isinstance(template, dict) else None
    scalar = template if isinstance(template, str) and template else None
    return direct or nested or scalar or ""


def _effective_gpu_count(row: dict[str, object]) -> int:
    count = _number(row, "gpu_count", "available_gpu_count", "gpus") or 1
    label = _offer_label(row).lower()
    multipliers = [int(value) for value in re.findall(r"(?:^|\s)(\d{1,2})\s*[x×]", label)]
    multipliers += [int(value) for value in re.findall(r"[x×]\s*(\d{1,2})(?:\s|$)", label)]
    return max(1, int(count), *(value for value in multipliers if 2 <= value <= 16))


def _single_gpu_rentable(row: dict[str, object]) -> bool:
    ncu = row.get("ncu_profiling_enabled") is True or row.get("ncu_profiling_enabled") == 1
    available = _number(row, "available_gpu_count")
    minimum = _number(row, "min_gpu_count_for_rental")
    if not ncu and available is not None and available >= 1 and (minimum is None or minimum <= 1):
        return True
    return _effective_gpu_count(row) == 1


def _parse_target(row: dict[str, object]) -> SshTarget:
    command = _string(row, "ssh_connect_cmd") or ""
    try:
        tokens = shlex.split(command)
    except ValueError:
        tokens = []
    port: int | None = None
    for index, token in enumerate(tokens[:-1]):
        if token == "-p" and tokens[index + 1].isdigit():
            port = int(tokens[index + 1])
    destination = next(
        (token for token in tokens if "@" in token and not token.startswith("-")), None
    )
    if destination is None:
        raise HarvestFailure("Lium pod has no valid SSH target")
    user, host = destination.rsplit("@", 1)
    mapping = row.get("ports_mapping")
    if port is None and isinstance(mapping, dict):
        raw_port = mapping.get("22", mapping.get(22))
        if isinstance(raw_port, int):
            port = raw_port
        elif isinstance(raw_port, str) and raw_port.isdigit():
            port = int(raw_port)
    port = port or 22
    target = SshTarget(user, host, port)
    _validate_ssh_target(target)
    return target


def _provider_id(value: str | None, kind: str) -> str:
    if value is None or not _PROVIDER_ID.fullmatch(value):
        raise HarvestFailure(f"Lium {kind} has an invalid id")
    return value


class LiumRestSshAdapter:
    """One-job/one-pod Lium adapter with reconciliation and verified teardown."""

    def __init__(
        self,
        config: LiumAdapterConfig,
        *,
        http: httpx.AsyncClient | None = None,
        ssh: SshTransport | None = None,
        guest_wire: LiumGuestWire | None = None,
        sleep: Callable[[float], Awaitable[None]] = asyncio.sleep,
    ) -> None:
        self.config = config
        self.http = http or httpx.AsyncClient(timeout=config.request_timeout_seconds)
        self.ssh = ssh or OpenSshTransport(
            config.ssh_private_key_file, known_hosts_file=config.ssh_known_hosts_file
        )
        self.guest_wire = guest_wire
        self.sleep = sleep
        self._owns_http = http is None
        self._rent_locks_guard = asyncio.Lock()
        self._rent_locks: dict[str, _RentLockEntry] = {}

    async def aclose(self) -> None:
        if self._owns_http:
            await self.http.aclose()

    def _url(self, path: str) -> str:
        return self.config.api_base_url + path

    def _api_key(self) -> str:
        try:
            return read_private_file(self.config.api_key_file, 4096)
        except Exception:
            raise HarvestFailure("Lium API key unavailable") from None

    def _public_key(self) -> str:
        try:
            value = (
                _read_ssh_file(self.config.ssh_public_key_file, "public key", limit=16_384)
                .decode()
                .strip()
            )
        except (HarvestFailure, UnicodeError):
            raise HarvestFailure("Lium SSH public key unavailable") from None
        if (
            not value.startswith(("ssh-ed25519 ", "ssh-rsa ", "ecdsa-sha2-"))
            or not value.isascii()
            or not value.isprintable()
        ):
            raise HarvestFailure("Lium SSH public key unavailable")
        return value

    async def _request(
        self,
        method: str,
        path: str,
        *,
        body: dict[str, object] | None = None,
        retryable: bool,
        allow_not_found: bool = False,
    ) -> object:
        attempts = self.config.http_attempts if retryable else 1
        last_reason = "Lium API transport failed"
        for attempt in range(attempts):
            key = self._api_key()
            try:
                async with self.http.stream(
                    method,
                    self._url(path),
                    headers={
                        "X-API-Key": key,
                        "Accept": "application/json",
                        "User-Agent": "cortex-python/0.1",
                    },
                    json=body,
                    timeout=self.config.request_timeout_seconds,
                ) as response:
                    content = bytearray()
                    async for chunk in response.aiter_bytes():
                        if len(content) + len(chunk) > MAX_PROVIDER_BODY_BYTES:
                            raise HarvestFailure("Lium API response exceeds size limit")
                        content.extend(chunk)
                    status = response.status_code
                    retry_after = response.headers.get("retry-after", "")
            except httpx.TransportError:
                last_reason = "Lium API transport failed"
                if attempt + 1 < attempts:
                    await self.sleep(min(2**attempt, 8))
                    continue
                raise HarvestFailure(last_reason) from None
            if allow_not_found and status == 404:
                return _NOT_FOUND
            if status >= 400:
                text = bytes(content[:2048]).decode(errors="replace")
                text = _bounded(_redact(text, (key,)), 512)
                last_reason = f"Lium API {method} {path} returned {status}: {text}"
                transient = status == 429 or status >= 500
                if transient and attempt + 1 < attempts:
                    await self.sleep(self._retry_after(retry_after, attempt))
                    continue
                raise HarvestFailure(last_reason)
            if not content.strip():
                return None
            try:
                return json.loads(content)
            except (json.JSONDecodeError, UnicodeError):
                raise HarvestFailure("Lium API returned invalid JSON") from None
        raise HarvestFailure(last_reason)

    @staticmethod
    def _retry_after(raw: str, attempt: int) -> float:
        try:
            parsed = float(raw)
        except ValueError:
            parsed = float(2**attempt)
        if not math.isfinite(parsed):
            parsed = float(2**attempt)
        return min(max(parsed, 0.05), 30)

    async def probe(self) -> bool:
        if self.guest_wire is None:
            return False
        try:
            self._api_key()
            self._public_key()
            _require_private_regular_file(self.config.ssh_private_key_file)
            _read_ssh_file(self.config.ssh_known_hosts_file, "known hosts", limit=1024 * 1024)
            result = await self._request("GET", "/users/me", retryable=True)
            return isinstance(result, dict)
        except asyncio.CancelledError:
            raise
        except Exception:
            return False

    async def _list(self, path: str, *keys: str) -> list[dict[str, object]]:
        return _arrays(await self._request("GET", path, retryable=True), *keys)

    async def _ensure_ssh_key(self, public_key: str) -> None:
        rows = await self._list("/ssh-keys", "ssh_keys", "data")
        if any((_string(row, "public_key") or "").strip() == public_key for row in rows):
            return
        try:
            await self._request(
                "POST",
                "/ssh-keys",
                body={"public_key": public_key, "name": self.config.ssh_key_name},
                retryable=False,
            )
        except HarvestFailure as error:
            rows = await self._list("/ssh-keys", "ssh_keys", "data")
            if any((_string(row, "public_key") or "").strip() == public_key for row in rows):
                return
            raise error

    def _template_id(self, rows: list[dict[str, object]], request: HarvestRequest) -> str | None:
        named = [row for row in rows if _string(row, "name") == request.plan.template_id]
        for row in named:
            image = _string(row, "docker_image")
            field_digest = _string(row, "docker_image_digest")
            repository = image
            reference_digest: str | None = None
            if image and "@" in image:
                repository, reference_digest = image.rsplit("@", 1)
            digests = tuple(
                digest for digest in (reference_digest, field_digest) if digest is not None
            )
            if (
                repository == self.config.image_repository
                and digests
                and all(digest == request.eval_image_digest for digest in digests)
            ):
                template_id = _string(row, "id")
                if template_id:
                    return _provider_id(template_id, "template")
        if named:
            raise HarvestFailure("Lium template digest binding mismatch")
        return None

    async def _resolve_template(self, request: HarvestRequest) -> str:
        rows = await self._list("/templates", "templates", "data")
        existing = self._template_id(rows, request)
        if existing:
            return existing
        body: dict[str, object] = {
            "name": request.plan.template_id,
            "docker_image": f"{self.config.image_repository}@{request.eval_image_digest}",
            "internal_ports": [22],
            "is_private": True,
            "container_start_immediately": True,
        }
        creation_error: HarvestFailure | None = None
        try:
            await self._request("POST", "/templates", body=body, retryable=False)
        except HarvestFailure as error:
            creation_error = error
        for attempt in range(self.config.reconcile_attempts):
            rows = await self._list("/templates", "templates", "data")
            existing = self._template_id(rows, request)
            if existing:
                return existing
            if attempt + 1 < self.config.reconcile_attempts:
                await self.sleep(self.config.poll_interval_seconds)
        if creation_error is not None:
            raise creation_error
        raise HarvestFailure("created Lium template was not digest-bound in provider state")

    async def _pods(self) -> list[dict[str, object]]:
        return await self._list("/pods", "pods", "data")

    def _pod_name(self, request: HarvestRequest) -> str:
        identity = hashlib.sha256(
            bytes.fromhex(request.job_id + request.plan.config_commitment)
        ).hexdigest()
        suffix_length = 63 - len(self.config.pod_name_prefix) - 1
        name = f"{self.config.pod_name_prefix}-{identity[:suffix_length]}"
        if not _POD_NAME.fullmatch(name):
            raise HarvestFailure("invalid Lium pod name")
        return name

    async def _named_pods(self, name: str) -> list[dict[str, object]]:
        return [row for row in await self._pods() if _string(row, "pod_name", "name") == name]

    async def _cleanup_pods(self, rows: list[dict[str, object]]) -> bool:
        confirmed = True
        for row in rows:
            try:
                instance_id = _provider_id(_string(row, "id", "pod_id"), "reconciled pod")
            except HarvestFailure:
                confirmed = False
                continue
            if not await self._cleanup_instance(instance_id):
                confirmed = False
        return confirmed

    async def _job_pod(self, name: str) -> dict[str, object] | None:
        matches = await self._named_pods(name)
        if len(matches) > 1:
            confirmed, cancellation = await self._protected_rent_cleanup(name)
            if cancellation is not None:
                raise cancellation
            suffix = "" if confirmed else "; cleanup unconfirmed"
            raise HarvestFailure(f"multiple Lium pods exist for one Proof job{suffix}")
        return matches[0] if matches else None

    async def _offers(self) -> list[dict[str, object]]:
        rows = await self._list("/executors", "executors", "data")
        needle = self.config.gpu_name.casefold()
        eligible = []
        for row in rows:
            label = _offer_label(row)
            price = _offer_price(row)
            if (
                needle in label.casefold()
                and price is not None
                and 0 < price <= self.config.max_price_per_hour
                and _single_gpu_rentable(row)
                and _string(row, "id", "executor_id")
            ):
                eligible.append(row)
        eligible.sort(
            key=lambda row: (
                _offer_price(row) or math.inf,
                _string(row, "id", "executor_id") or "",
            )
        )
        return eligible

    async def _pod(self, instance_id: str) -> dict[str, object] | None:
        instance_id = _provider_id(instance_id, "pod")
        value = await self._request(
            "GET", f"/pods/{instance_id}", retryable=True, allow_not_found=True
        )
        if value is _NOT_FOUND:
            return None
        if not isinstance(value, dict):
            raise HarvestFailure("Lium API returned a malformed pod")
        observed_id = _string(value, "id", "pod_id")
        if observed_id is not None and observed_id != instance_id:
            raise HarvestFailure("Lium API returned a mismatched pod")
        return value

    async def _wait_running(
        self,
        instance_id: str,
        *,
        pod_name: str,
        provider_template_id: str,
    ) -> dict[str, object]:
        attempts = max(
            1,
            math.ceil(self.config.running_timeout_seconds / self.config.poll_interval_seconds),
        )
        last = "UNKNOWN"
        for attempt in range(attempts):
            row = await self._pod(instance_id)
            if row is None:
                raise HarvestFailure("Lium pod disappeared before becoming ready")
            last = (_string(row, "status", "state") or "UNKNOWN").upper()
            if last in _RUNNING:
                if _string(row, "pod_name", "name") != pod_name:
                    raise HarvestFailure("Lium pod name binding mismatch")
                if _pod_template_id(row) != provider_template_id:
                    raise HarvestFailure("Lium pod template binding mismatch")
                label = _pod_gpu_label(row)
                if not label or self.config.gpu_name.casefold() not in label.casefold():
                    raise HarvestFailure("Lium pod GPU does not match configured class")
                return row
            if last in _TERMINAL or any(status in last for status in _TERMINAL):
                raise HarvestFailure(f"Lium pod entered terminal status {last}")
            if attempt + 1 < attempts:
                await self.sleep(self.config.poll_interval_seconds)
        raise HarvestFailure(f"Lium pod did not become ready (last status {last})")

    async def _delete(self, instance_id: str) -> None:
        instance_id = _provider_id(instance_id, "pod")
        await self._request("DELETE", f"/pods/{instance_id}", retryable=True, allow_not_found=True)

    async def _cleanup_instance(self, instance_id: str) -> bool:
        try:
            await self._delete(instance_id)
            return await self._instance_absent(instance_id, self.config.teardown_attempts)
        except Exception:
            return False

    async def _protected_cleanup(
        self, instance_id: str
    ) -> tuple[bool, asyncio.CancelledError | None]:
        cleanup = asyncio.create_task(self._cleanup_instance(instance_id))
        cancellation: asyncio.CancelledError | None = None
        while True:
            try:
                return await asyncio.shield(cleanup), cancellation
            except asyncio.CancelledError as error:
                if cancellation is None:
                    cancellation = error
                if cleanup.done():
                    return (
                        False if cleanup.cancelled() else cleanup.result(),
                        cancellation,
                    )

    async def _instance_absent(self, instance_id: str, attempts: int) -> bool:
        for attempt in range(attempts):
            if await self._pod(instance_id) is None:
                return True
            if attempt + 1 < attempts:
                await self.sleep(self.config.poll_interval_seconds)
        return False

    async def _wait_for_lease(
        self,
        instance_id: str,
        request: HarvestRequest,
        *,
        pod_name: str,
        provider_template_id: str,
    ) -> LiumLease:
        try:
            await self._wait_running(
                instance_id,
                pod_name=pod_name,
                provider_template_id=provider_template_id,
            )
            listed = await self._job_pod(pod_name)
            if listed is None or _string(listed, "id", "pod_id") != instance_id:
                raise HarvestFailure("Lium pod identity binding mismatch")
        except BaseException as error:
            confirmed, cleanup_cancellation = await self._protected_cleanup(instance_id)
            if cleanup_cancellation is not None:
                raise cleanup_cancellation from None
            if not confirmed and not isinstance(error, asyncio.CancelledError):
                raise HarvestFailure("Lium rent cleanup is unconfirmed") from error
            raise
        return self._lease(instance_id, request)

    async def _reconcile_rent_cleanup(self, pod_name: str) -> bool:
        try:
            for attempt in range(self.config.reconcile_attempts):
                pods = await self._named_pods(pod_name)
                if pods and not await self._cleanup_pods(pods):
                    return False
                if attempt + 1 < self.config.reconcile_attempts:
                    await self.sleep(self.config.poll_interval_seconds)
            return not await self._named_pods(pod_name)
        except asyncio.CancelledError:
            raise
        except Exception:
            return False

    async def _protected_rent_cleanup(
        self, pod_name: str
    ) -> tuple[bool, asyncio.CancelledError | None]:
        cleanup = asyncio.create_task(self._reconcile_rent_cleanup(pod_name))
        cancellation: asyncio.CancelledError | None = None
        while True:
            try:
                return await asyncio.shield(cleanup), cancellation
            except asyncio.CancelledError as error:
                if cancellation is None:
                    cancellation = error
                if cleanup.done():
                    return (
                        False if cleanup.cancelled() else cleanup.result(),
                        cancellation,
                    )

    @asynccontextmanager
    async def _rent_serialized(self, job_id: str) -> AsyncIterator[None]:
        async with self._rent_locks_guard:
            entry = self._rent_locks.setdefault(job_id, _RentLockEntry(asyncio.Lock()))
            entry.users += 1
        try:
            async with entry.lock:
                yield
        finally:
            async with self._rent_locks_guard:
                entry.users -= 1
                if entry.users == 0 and self._rent_locks.get(job_id) is entry:
                    del self._rent_locks[job_id]

    async def rent(self, request: HarvestRequest) -> LiumLease:
        async with self._rent_serialized(request.job_id):
            return await self._rent(request)

    async def _rent(self, request: HarvestRequest) -> LiumLease:
        if request.plan.gpu_count != 1:
            raise HarvestFailure("Proof executor must rent exactly 1x")
        pod_name = self._pod_name(request)
        return await self._rent_named(request, pod_name)

    async def _rent_named(self, request: HarvestRequest, pod_name: str) -> LiumLease:
        public_key = self._public_key()
        _require_private_regular_file(self.config.ssh_private_key_file)
        _read_ssh_file(self.config.ssh_known_hosts_file, "known hosts", limit=1024 * 1024)
        await self._ensure_ssh_key(public_key)
        provider_template_id = await self._resolve_template(request)
        existing = await self._job_pod(pod_name)
        if existing is not None:
            existing_id = _provider_id(_string(existing, "id", "pod_id"), "reconciled pod")
            status = (_string(existing, "status", "state") or "UNKNOWN").upper()
            if status in _TERMINAL:
                confirmed, cancellation = await self._protected_cleanup(existing_id)
                if cancellation is not None:
                    raise cancellation
                if not confirmed:
                    raise HarvestFailure("terminal Lium pod deletion is unconfirmed")
            else:
                return await self._wait_for_lease(
                    existing_id,
                    request,
                    pod_name=pod_name,
                    provider_template_id=provider_template_id,
                )

        offers = await self._offers()
        if not offers:
            raise HarvestFailure("no exact 1x Lium offer matches the configured GPU and price")
        offer_id = _provider_id(_string(offers[0], "id", "executor_id"), "offer")
        body: dict[str, object] = {
            "pod_name": pod_name,
            "user_public_key": [public_key],
            "termination_hours": math.ceil(self.config.max_lifetime_hours),
            "gpu_count": 1,
            "template_id": provider_template_id,
        }
        return await self._paid_rent(
            request,
            pod_name=pod_name,
            provider_template_id=provider_template_id,
            offer_id=offer_id,
            body=body,
        )

    async def _paid_rent(
        self,
        request: HarvestRequest,
        *,
        pod_name: str,
        provider_template_id: str,
        offer_id: str,
        body: dict[str, object],
    ) -> LiumLease:
        try:
            return await self._paid_rent_inner(
                request,
                pod_name=pod_name,
                provider_template_id=provider_template_id,
                offer_id=offer_id,
                body=body,
            )
        except asyncio.CancelledError as error:
            confirmed, _ = await self._protected_rent_cleanup(pod_name)
            if not confirmed:
                raise HarvestFailure("cancelled Lium rent cleanup is unconfirmed") from error
            raise
        except Exception as error:
            confirmed, cleanup_cancellation = await self._protected_rent_cleanup(pod_name)
            if cleanup_cancellation is not None:
                raise cleanup_cancellation from None
            if not confirmed:
                raise HarvestFailure("Lium rent cleanup is unconfirmed") from error
            raise

    async def _paid_rent_inner(
        self,
        request: HarvestRequest,
        *,
        pod_name: str,
        provider_template_id: str,
        offer_id: str,
        body: dict[str, object],
    ) -> LiumLease:
        rented: object = None
        rent_error: HarvestFailure | None = None
        try:
            rented = await self._request(
                "POST", f"/executors/{offer_id}/rent", body=body, retryable=False
            )
        except HarvestFailure as error:
            rent_error = error

        instance_id: str | None = None
        if isinstance(rented, dict):
            instance_id = _string(rented, "id", "pod_id")
            pod = rented.get("pod")
            if instance_id is None and isinstance(pod, dict):
                instance_id = _string(pod, "id", "pod_id")
        if instance_id is None:
            for attempt in range(self.config.reconcile_attempts):
                reconciled = await self._job_pod(pod_name)
                if reconciled is not None:
                    instance_id = _string(reconciled, "id", "pod_id")
                    if instance_id:
                        break
                if attempt + 1 < self.config.reconcile_attempts:
                    await self.sleep(self.config.poll_interval_seconds)
        if instance_id is None:
            if rent_error is not None:
                raise rent_error
            raise HarvestFailure("Lium rent returned no pod id")
        instance_id = _provider_id(instance_id, "pod")
        return await self._wait_for_lease(
            instance_id,
            request,
            pod_name=pod_name,
            provider_template_id=provider_template_id,
        )

    @staticmethod
    def _lease(instance_id: str, request: HarvestRequest) -> LiumLease:
        return LiumLease(
            instance_id=instance_id,
            template_id=request.plan.template_id,
            gpu_count=1,
            image_digest=request.eval_image_digest,
        )

    async def _target(self, instance_id: str) -> SshTarget:
        row = await self._pod(instance_id)
        if row is None:
            raise HarvestFailure("Lium pod disappeared before execution")
        return _parse_target(row)

    @staticmethod
    def _validate_guest_env(env: dict[str, str]) -> None:
        if len(env) > 8:
            raise HarvestFailure("too many guest environment variables")
        for name, value in sorted(env.items()):
            if not _ENV_NAME.fullmatch(name) or name.startswith("PROOF_") or name in _RESERVED_ENV:
                raise HarvestFailure("invalid guest environment name")
            if not value.strip() or len(value) > 4096 or not value.isprintable():
                raise HarvestFailure("invalid guest environment value")

    @staticmethod
    def _verify_report(report: HarvestExecution, request: HarvestRequest) -> None:
        try:
            verified = HarvestExecution.model_validate(report.model_dump())
            LiumBackend._verify_execution(verified, request)
        except (ValidationError, ServiceError):
            raise HarvestFailure("Lium eval report binding mismatch") from None

    async def execute(self, lease: LiumLease, request: HarvestRequest) -> HarvestExecution:
        if self.guest_wire is None:
            raise HarvestFailure(
                "Lium guest request/result contract unavailable; refusing live execution"
            )
        self._validate_guest_env(request.env)
        _require_private_regular_file(self.config.ssh_private_key_file)
        _read_ssh_file(self.config.ssh_known_hosts_file, "known hosts", limit=1024 * 1024)
        target = await self._target(lease.instance_id)
        try:
            async with asyncio.timeout(request.plan.deadline_s + 60):
                report = await self.guest_wire.execute(self.ssh, target, request)
        except TimeoutError:
            raise HarvestFailure("proof deadline exceeded", "no guest report") from None
        if not isinstance(report, HarvestExecution):
            raise HarvestFailure("Lium guest returned an invalid report type")
        self._verify_report(report, request)
        return report

    async def terminate(self, lease: LiumLease) -> None:
        await self._delete(lease.instance_id)

    async def verify_terminated(self, lease: LiumLease) -> bool:
        return await self._instance_absent(lease.instance_id, self.config.teardown_attempts)
