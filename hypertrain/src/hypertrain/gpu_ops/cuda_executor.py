"""Generic CUDA executor: challenge references and audit replays on any SSH + docker GPU host.

Decision record: docs/adr/0008-generic-cuda-executor.md. A second `RoleLaunch` next to the Vast
`role_dispatch`: no provider API, no lifecycle, no rental. The challenge starts the digest-pinned
GPU image with `docker run --rm` over SSH, streams the island inputs in as a tar on stdin and the
rank artifacts back as a tar on stdout, so nothing persists on the host except the image and one
CUDA compute cache directory per image digest (docs/adr/0012-executor-performance.md). It never
takes the host's word for the result: the artifacts are re-validated here and the observed
environment is checked against the signed manifest before `role-dispatch.json` and `published/`
are written.
"""

from __future__ import annotations

import csv
import hashlib
import io
import json
import os
import re
import shlex
import shutil
import signal
import stat
import subprocess
import tarfile
import tempfile
import threading
import time
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import IO, TYPE_CHECKING, Annotated, Any, Literal, Protocol

from pydantic import (
    BaseModel,
    ConfigDict,
    Field,
    StrictBool,
    StrictInt,
    StringConstraints,
    field_validator,
    model_validator,
)

from hypertrain.gpu_ops import cuda_executor_runner
from hypertrain.gpu_ops.journal import Journal, durable_write, flock, sha256
from hypertrain.gpu_ops.network_qualification import unqualified_device
from hypertrain.gpu_ops.remote import TrustError, valid_endpoint
from hypertrain.miner.island_launch import (
    IslandFailure,
    confined,
    gpu_uuid,
    launch_island,
    validate_artifacts,
)
from hypertrain.protocol import envelope_v2
from hypertrain.protocol.envelope import body_digest
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages import Hex64

if TYPE_CHECKING:
    from hypertrain.gpu_ops.work_screen import IslandLaunch
    from hypertrain.miner.island_launch import IslandArtifacts
    from hypertrain.protocol.messages import ReferenceSpec
    from hypertrain.protocol.messages_v2 import IslandJobV1

RoleLaunch = Callable[[str, "IslandJobV1", Path, Mapping[str, Any]], "IslandLaunch"]

EXECUTOR = "cuda-executor-v1"
ARTIFACTS_ENV = "HYPERTRAIN_CUDA_EXECUTOR_ARTIFACTS"
LABEL = "io.cortex.cuda-executor"
IMAGE_PYTHON = "/opt/hypertrain/venv/bin/python"
RUNNER_SOURCE = Path(cuda_executor_runner.__file__).read_text()
ENV_BASE = {"PYTHONHASHSEED": "0", "OMP_NUM_THREADS": "1", "MKL_NUM_THREADS": "1"}
OPERATIONS = ("reference", "audit", "referee")
SSH_EXIT = 255
RANK = re.compile(r"rank-(0|[1-9][0-9]{0,2})")
MAX_GPUS = 8
TRANSPORT_NAMES = ("job.json", "request.json")
FIELD = 256  # longest host-reported string the provenance record keeps
RUNNER_PROBE_FAILED = 9
MAX_MEMBERS = 16384  # a dp8 layout with a checkpoint per window stays far below this
RUNNER_REFUSED_ENVIRONMENT = 4
STDERR_LIMIT = 16 << 20
SMALL = 1 << 20
POLL = 0.25
MARGIN = 5
LOG_TAIL = 600
MAX_DIGESTS = 32  # RepoDigests entries kept from a host probe
HOST_JSON_DEPTH = 8  # COMPLETE nests two levels and environment.json three: deeper is a bomb
RANK_JSON = ("summary.json", "leaves.json", "trace.json")
RANK_JSON_BYTES = 64 << 20  # auditor.island_bisect.TRACE_BYTES: the largest JSON a reader accepts
# The GPU image runs as root, so the driver keeps its compute cache in /root/.nv. The host side is
# one directory per image digest below the ssh user's home, or below a configured absolute root.
CACHE_TARGET = "/root/.nv"
CACHE_ROOT = ".cortex-cuda-executor/compute-cache"
CACHE_PLAIN = re.compile(r"/[A-Za-z0-9._-]+(?:/[A-Za-z0-9._-]+)*")
CACHE_LEAF = 32  # hex characters of the manifest image digest that name the directory
CACHE_PATH_MAX = 400


def plain_path(value: str) -> bool:
    """An absolute path of plain words: safe inside a `--mount` spec and a shell word."""
    return (
        len(value) <= CACHE_PATH_MAX
        and CACHE_PLAIN.fullmatch(value) is not None
        and not {".", ".."} & set(value.split("/"))
    )


class HostLost(IslandFailure):
    """The host stopped answering or dropped the stream: try another one."""


class HostBusy(IslandFailure):
    """Too few idle GPUs on this host right now."""


class HostRefused(IslandFailure):
    """The host contradicts the manifest or its own earlier identity: quarantine it."""


class AttemptFailed(IslandFailure):
    """The container ran but produced no acceptable result."""


class RemoteTimeout(IslandFailure):
    """A remote command outlived the deadline its caller gave it."""


class Aborted(IslandFailure):
    """The job deadline passed or the lease was cancelled: never retried."""


class Boundary(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)


class HostSpec(Boundary):
    label: str = Field(pattern=r"^[a-z0-9][a-z0-9-]{0,31}$")
    address: str
    port: StrictInt = Field(22, ge=1, le=65535)
    user: str = Field(pattern=r"^[a-z_][a-z0-9_-]{0,31}$")
    identity_file: Path  # path only: the key is never read or copied here
    known_hosts_file: Path  # the operator's pinned host keys; no trust on first use
    docker: tuple[str, ...] = Field(("docker",), min_length=1, max_length=4)
    # Absolute root of the compute cache on the host; None keeps it under the ssh user's home.
    compute_cache_dir: Path | None = None

    @field_validator("address")
    @classmethod
    def _address(cls, value: str) -> str:
        try:
            return valid_endpoint(value, 22)[0]
        except TrustError as error:
            raise ValueError(str(error)) from None

    @field_validator("docker")
    @classmethod
    def _docker(cls, value: tuple[str, ...]) -> tuple[str, ...]:
        if value[0].startswith("-") or not all(
            part and all(c.isalnum() or c in "_./=+-" for c in part) for part in value
        ):
            raise ValueError("docker must be a short argv of plain words")
        return value

    @field_validator("compute_cache_dir")
    @classmethod
    def _cache_root(cls, value: Path | None) -> Path | None:
        if value is not None and (len(str(value)) > 200 or not plain_path(str(value))):
            raise ValueError("compute_cache_dir must be an absolute path of plain words")
        return value

    @model_validator(mode="after")
    def _files(self) -> HostSpec:
        key = self.identity_file.lstat()
        if (
            not stat.S_ISREG(key.st_mode)
            or key.st_uid != os.getuid()
            or stat.S_IMODE(key.st_mode) & 0o077
        ):
            raise ValueError("identity_file must be a regular file of this user with mode 0600")
        # OpenSSH reads UserKnownHostsFile as a list and expands % tokens and a leading ~.
        path = str(self.known_hosts_file)
        if not self.known_hosts_file.is_absolute() or any(c.isspace() or c == "%" for c in path):
            raise ValueError("known_hosts_file must be an absolute path without spaces or '%'")
        pinned = self.known_hosts_file.lstat()
        if not stat.S_ISREG(pinned.st_mode) or pinned.st_size == 0:
            raise ValueError("known_hosts_file must be a non-empty regular file")
        return self


class ExecutorConfig(Boundary):
    run_id: Hex64  # the one run whose CUDA jobs this executor serves
    image_repository: str = Field(pattern=r"^[a-z0-9][a-z0-9./_-]*$")
    state_dir: Path  # journal and remote stderr logs; never read back as authority
    hosts: list[HostSpec] = Field(min_length=1, max_length=8)
    network: Literal["default", "none"] = "default"
    compute_cache: StrictBool = True  # bind-mount one persistent CUDA compute cache per image
    max_attempts: StrictInt = Field(3, ge=1, le=10)
    sweeps: StrictInt = Field(3, ge=1, le=20)
    sweep_pause_seconds: StrictInt = Field(15, ge=0, le=600)
    idle_gpu_memory_mib: StrictInt = Field(2048, ge=0)
    max_clock_skew_seconds: StrictInt = Field(5, ge=1, le=60)
    max_result_bytes: StrictInt = Field(16 << 30, ge=SMALL)
    pull_timeout_seconds: StrictInt = Field(900, ge=30)
    ssh_connect_timeout_seconds: StrictInt = Field(15, ge=1, le=120)

    @model_validator(mode="after")
    def _unique(self) -> ExecutorConfig:
        labels = [h.label for h in self.hosts]
        if len(set(labels)) != len(labels):
            raise ValueError("host labels must be unique")
        return self


@dataclass(frozen=True)
class Gpu:
    uuid: str
    name: str
    driver: str
    capability: tuple[int, int]
    memory_mib: int


@dataclass(frozen=True)
class Snapshot:
    gpus: tuple[Gpu, ...]
    busy: frozenset[str]
    epoch: int
    repo_digests: tuple[str, ...] | None  # None: the image is absent on the host


# `docker image inspect --format {{json .RepoDigests}}` prints a list of references: printable ASCII
# with no space, quote or backslash, so the line needs no JSON parser and carries no escape.
_REFERENCE = rf'"[\x21\x23-\x5b\x5d-\x7e]{{1,{FIELD}}}"'
_REPO_DIGESTS = re.compile(rf"\[(?:{_REFERENCE}(?:, ?{_REFERENCE}){{0,{MAX_DIGESTS - 1}}})?\]")


def snapshot_command(docker: Sequence[str], image: str) -> str:
    inspect = shlex.join([*docker, "image", "inspect", "--format", "{{json .RepoDigests}}", image])
    return (
        "nvidia-smi --query-gpu=uuid,name,driver_version,compute_cap,memory.used "
        "--format=csv,noheader,nounits || exit 41; echo '##apps'; "
        "nvidia-smi --query-compute-apps=gpu_uuid,pid,process_name --format=csv,noheader "
        "|| exit 42; echo '##epoch'; date +%s; echo '##image'; "
        f"{inspect} 2>/dev/null || echo ABSENT"
    )


def parse_snapshot(text: str) -> Snapshot:
    """Strict parse of `snapshot_command` output; anything unexpected refuses the host."""
    sections: dict[str, list[str]] = {"gpus": []}
    current = "gpus"
    for line in text.splitlines():
        if line.startswith("##"):
            current = line[2:].strip()
            sections[current] = []
        elif line.strip():
            sections[current].append(line)
    try:
        gpus = []
        for row in csv.reader(sections["gpus"], skipinitialspace=True):
            major, minor = row[3].strip().split(".")
            gpus.append(
                Gpu(
                    gpu_uuid(row[0]),
                    row[1].strip(),
                    row[2].strip(),
                    (int(major), int(minor)),
                    int(row[4]),
                )
            )
        busy = frozenset(
            gpu_uuid(row[0]) for row in csv.reader(sections["apps"], skipinitialspace=True)
        )
        digests = sections["image"][0].strip()
        absent = digests == "ABSENT"
        epoch = int(sections["epoch"][0])
    except (IndexError, KeyError, ValueError, TypeError, csv.Error, IslandFailure) as error:
        raise HostRefused(f"unparseable host probe: {type(error).__name__}") from error
    if not gpus or len({g.uuid for g in gpus}) != len(gpus):
        raise HostRefused("host probe lists no GPU or a duplicate GPU")
    if not -(1 << 63) <= epoch < 1 << 63:  # `date +%s` prints a time_t; a float holds 308 digits
        raise HostRefused("host probe lists an impossible clock")
    if not absent and not _REPO_DIGESTS.fullmatch(digests):
        raise HostRefused("host probe lists a malformed RepoDigests")
    listed = () if absent else tuple(re.findall(r'"([^"]*)"', digests))
    return Snapshot(tuple(gpus), busy, epoch, None if absent else listed)


def run_command(
    docker: Sequence[str],
    image: str,
    name: str,
    job_digest: str,
    uuids: Sequence[str],
    net: str,
    cache_dir: str | None = None,
) -> str:
    # `--mount` needs the source to exist; `-v` would make a root-owned directory when it does not.
    if cache_dir is not None and not plain_path(cache_dir):
        raise ValueError("compute cache directory must be an absolute path of plain words")
    argv = [
        *docker,
        "run",
        "--rm",
        "-i",
        "--init",
        "--name",
        name,
        "--label",
        f"{LABEL}={job_digest[:16]}",
        "--gpus",
        '"device=' + ",".join(uuids) + '"',
        *(["--network", "none"] if net == "none" else []),
        "--ipc=host",
        "--ulimit",
        "memlock=-1",
        "--ulimit",
        "stack=67108864",
        *(["--mount", f"type=bind,src={cache_dir},dst={CACHE_TARGET}"] if cache_dir else []),
        "--entrypoint",
        IMAGE_PYTHON,
        image,
        "-I",
        "-c",
        RUNNER_SOURCE,
    ]
    return shlex.join(argv)


def cache_location(root: Path | None, image_digest: str) -> tuple[str, re.Pattern[str]]:
    """Shell assignment of the cache directory, and the pattern the host's report must match.

    Without a configured root the directory is below the ssh user's home, which the host's own
    shell expands (`sudo` only starts after, so it is the ssh user's and not root's).
    """
    leaf = image_digest.removeprefix("sha256:")[:CACHE_LEAF]
    if root is None:
        return (
            f'D="$HOME/{CACHE_ROOT}/{leaf}"',
            re.compile(rf"/[A-Za-z0-9._/-]{{1,300}}/{re.escape(CACHE_ROOT)}/{leaf}"),
        )
    path = f"{root}/{leaf}"
    return f"D={shlex.quote(path)}", re.compile(re.escape(path))


def prepare_command(assignment: str) -> str:
    """Create the directory for the ssh user and print the absolute path.

    `umask 077` makes every directory it creates private, so no other user of the host can seed
    the cache. A leaf that is a link is refused (the daemon would resolve it as root), and so is one
    the ssh user does not own: `chmod` would refuse it for an ordinary user but not for root, whose
    chmod would make someone else's directory private and keep it writable by its owner. The reasons
    are printed, because they end up in the journal.
    """
    return (
        f'{assignment}; umask 077; mkdir -p -- "$D" '
        f'&& {{ [ ! -L "$D" ] || {{ echo "compute cache directory is a link" >&2; exit 1; }}; }} '
        f'&& {{ [ -O "$D" ] || {{ echo "compute cache directory is not owned by this user" >&2; '
        f"exit 1; }}; }} "
        f'&& chmod 0700 -- "$D" && printf \'%s\\n\' "$D"'
    )


class Transport(Protocol):
    def run(
        self,
        command: str,
        *,
        stdin: Path | None,
        stdout: Path,
        stderr: Path,
        deadline: float,
        cancel: threading.Event | None,
        max_stdout_bytes: int,
    ) -> int:
        """Run one remote command to exit; HostLost when the host cannot be reached."""


class SshTransport:
    def __init__(self, host: HostSpec, config: ExecutorConfig, ssh: str) -> None:
        self.host, self.ssh = host, ssh
        self.options = [
            "-F",
            "/dev/null",
            "-o",
            "BatchMode=yes",
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            f"UserKnownHostsFile={host.known_hosts_file}",
            "-o",
            "GlobalKnownHostsFile=/dev/null",
            "-o",
            "IdentitiesOnly=yes",
            "-o",
            "IdentityAgent=none",
            "-o",
            "PasswordAuthentication=no",
            "-o",
            "KbdInteractiveAuthentication=no",
            "-o",
            "ForwardAgent=no",
            "-o",
            "ClearAllForwardings=yes",
            "-o",
            "ControlMaster=no",
            "-o",
            "ControlPath=none",
            "-o",
            "LogLevel=ERROR",
            "-o",
            f"ConnectTimeout={config.ssh_connect_timeout_seconds}",
            "-o",
            "ServerAliveInterval=15",
            "-o",
            "ServerAliveCountMax=3",
            "-i",
            str(host.identity_file),
            "-p",
            str(host.port),
            "-T",
        ]

    def argv(self, command: str) -> list[str]:
        return [self.ssh, *self.options, "--", f"{self.host.user}@{self.host.address}", command]

    def run(
        self,
        command: str,
        *,
        stdin: Path | None,
        stdout: Path,
        stderr: Path,
        deadline: float,
        cancel: threading.Event | None,
        max_stdout_bytes: int,
    ) -> int:
        # Nothing of the challenge's environment (tokens, key paths, agent socket) reaches ssh.
        env = {
            "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
            "HOME": os.environ.get("HOME", ""),
            "LC_ALL": "C",
        }
        with (
            open(stdin, "rb") if stdin else open(os.devnull, "rb") as source,
            _private(stdout, os.O_EXCL) as out,
            _private(stderr, os.O_APPEND) as err,
        ):
            proc = subprocess.Popen(  # noqa: S603 - argv list, validated endpoint, no shell
                self.argv(command),
                stdin=source,
                stdout=out,
                stderr=err,
                env=env,
                start_new_session=True,
            )
            try:
                while True:
                    try:
                        code = proc.wait(timeout=POLL)
                        break
                    except subprocess.TimeoutExpired:
                        pass
                    if cancel is not None and cancel.is_set():
                        raise Aborted("lease cancelled")
                    if time.time() >= deadline:
                        raise RemoteTimeout("remote command timed out")
                    self._bounded(out, err, max_stdout_bytes)
            finally:
                if proc.poll() is None:
                    os.killpg(proc.pid, signal.SIGKILL)
                    proc.wait()
            self._bounded(out, err, max_stdout_bytes)  # bytes written since the last poll
        if code == SSH_EXIT:
            raise HostLost(f"ssh could not reach or authenticate the host: {_tail(stderr)}")
        return code

    @staticmethod
    def _bounded(out: IO[bytes], err: IO[bytes], max_stdout_bytes: int) -> None:
        if os.fstat(out.fileno()).st_size > max_stdout_bytes:
            raise AttemptFailed("result exceeds max_result_bytes")
        if os.fstat(err.fileno()).st_size > STDERR_LIMIT:
            raise AttemptFailed("remote stderr exceeds its bound")


def _private(path: Path, flag: int) -> IO[bytes]:
    """A new 0600 file: remote stderr and result bytes are not for other users of the host."""
    return os.fdopen(os.open(path, os.O_WRONLY | os.O_CREAT | flag, 0o600), "wb")


def shallow(raw: bytes) -> str:
    """Decode a document a host wrote; refuse the host when it nests deeper than a runner writes.

    The caller has bounded the size. One linear scan that skips strings counts the nesting, so no
    parser is ever handed a document deep enough to recurse (a count that over-reads a malformed
    document is harmless: json refuses it anyway). The scan reads the decoded text, never bytes
    that json could take for UTF-16.
    """
    text = raw.decode()
    depth, quoted, escaped = 0, False, False
    for char in text:
        if quoted:
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == '"':
                quoted = False
        elif char == '"':
            quoted = True
        elif char in "[{":
            depth += 1
            if depth > HOST_JSON_DEPTH:
                raise HostRefused("host document nests deeper than any runner writes")
        elif char in "]}":
            depth -= 1
    return text


class _CappedReader(io.BufferedReader):
    """tarfile reads the size an extended header claims in one call, and a host may claim 2**63."""

    def read(self, size: int | None = -1) -> bytes:
        if size is None or not 0 <= size <= SMALL:
            raise HostRefused("result tar asks for a read beyond the metadata bound")
        return super().read(size)


def _file_sha(path: Path) -> str:
    with open(path, "rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def _ident(tag: str, text: str) -> int:
    """Positive 52-bit integer (JSON-safe), stable for the same text."""
    value = int.from_bytes(hashlib.sha256(f"{tag}\0{text}".encode()).digest()[:8], "big")
    return (value & ((1 << 52) - 1)) or 1


def _tail(path: Path) -> str:
    try:
        data = path.read_bytes()[-LOG_TAIL:]
    except OSError:
        return ""
    return "".join(c if c.isprintable() else " " for c in data.decode(errors="replace"))


Text = Annotated[str, StringConstraints(max_length=FIELD)]


class ObservedEnvironment(Boundary):
    """What the runner observed in the container; every field is re-checked before use."""

    backend: Literal["cuda"]
    python: Text
    torch: Text
    torch_path: Text
    cuda: Text | None
    gpu_uuids: Annotated[list[Text], Field(max_length=MAX_GPUS)]
    gpu_names: Annotated[list[Text], Field(max_length=MAX_GPUS)]
    sm_counts: Annotated[list[StrictInt], Field(max_length=MAX_GPUS)]
    capabilities: Annotated[list[tuple[StrictInt, StrictInt]], Field(max_length=MAX_GPUS)]
    drivers: Annotated[list[Text], Field(max_length=MAX_GPUS)]


class Executor:
    def __init__(
        self,
        config: ExecutorConfig,
        *,
        transports: Mapping[str, Transport] | None = None,
        sleep: Callable[[float], None] = time.sleep,
    ) -> None:
        self.config, self.sleep = config, sleep
        self.hosts = {h.label: h for h in config.hosts}
        if transports is None:
            ssh = shutil.which("ssh")
            if ssh is None:
                raise FileNotFoundError("ssh client not found")
            transports = {h.label: SshTransport(h, config, ssh) for h in config.hosts}
        self.transports = dict(transports)
        for private in (config.state_dir, config.state_dir / "logs"):
            private.mkdir(mode=0o700, parents=True, exist_ok=True)
            private.chmod(0o700)
        self.journal = Journal(config.state_dir)
        self._locks = {label: threading.Lock() for label in self.hosts}
        self._guard = threading.Lock()
        self._machines: dict[str, int] = {}
        self._quarantined: dict[str, str] = {}
        # (host label, image digest) -> the compute cache directory already created on that host;
        # a host runs one attempt at a time, so no key has two writers
        self._cache_dirs: dict[tuple[str, str], str] = {}

    # -- selection ------------------------------------------------------------------------

    def _order(self, job: IslandJobV1) -> list[str]:
        labels = [h.label for h in self.config.hosts]
        start = int(job.digest()[:8], 16) % len(labels)
        return labels[start:] + labels[:start]

    def _quarantine(self, label: str, why: str) -> None:
        with self._guard:
            self._quarantined[label] = why

    def _refusals(self) -> dict[str, str]:
        with self._guard:
            return dict(self._quarantined)

    def _lease(
        self, order: Sequence[str], skip: set[str], deadline: float, cancel: threading.Event | None
    ) -> str | None:
        """First usable host whose lock is free; wait for a busy one; None when none is left."""
        while True:
            refused = self._refusals()
            usable = [label for label in order if label not in skip and label not in refused]
            if not usable:
                return None
            for label in usable:
                if self._locks[label].acquire(blocking=False):
                    return label
            if cancel is not None and cancel.is_set():
                raise Aborted("lease cancelled")
            if time.time() >= deadline:
                raise Aborted("job deadline expired")
            self.sleep(POLL)

    def _admit(self, label: str, snapshot: Snapshot) -> int:
        """machine_id from every GPU UUID of the host; one machine per label, one label each."""
        machine = _ident("machine", ",".join(sorted(g.uuid for g in snapshot.gpus)))
        with self._guard:
            known = self._machines.get(label)
            owner = next((k for k, v in self._machines.items() if v == machine), None)
            if known is not None and known != machine:
                raise HostRefused("the machine behind this host changed")
            if owner is not None and owner != label:
                raise HostRefused(f"same machine as host {owner}")
            self._machines[label] = machine
        return machine

    # -- one attempt ----------------------------------------------------------------------

    def _remote(
        self,
        label: str,
        command: str,
        *,
        stdout: Path,
        stderr: Path,
        deadline: float,
        cancel: threading.Event | None,
        limit: float | None = None,
        stdin: Path | None = None,
        max_bytes: int = SMALL,
    ) -> int:
        """One remote command, bounded by `limit` seconds and by the job deadline."""
        bound = deadline if limit is None else min(deadline, time.time() + limit)
        try:
            return self.transports[label].run(
                command,
                stdin=stdin,
                stdout=stdout,
                stderr=stderr,
                deadline=bound,
                cancel=cancel,
                max_stdout_bytes=max_bytes,
            )
        except RemoteTimeout:
            if time.time() >= deadline:
                raise Aborted("job deadline expired") from None
            raise HostLost("remote command timed out") from None

    def _snapshot(
        self, label: str, image: str, deadline: float, cancel: threading.Event | None, work: Path
    ) -> Snapshot:
        out, err = work / f"probe-{time.monotonic_ns()}.out", work / "probe.err"
        before = time.time()
        command = snapshot_command(self.hosts[label].docker, image)
        try:
            code = self._remote(
                label, command, stdout=out, stderr=err, deadline=deadline, cancel=cancel, limit=60
            )
        except AttemptFailed as error:  # the transport's output bounds: no honest probe trips them
            raise HostRefused("host probe output exceeds its bound") from error
        after = time.time()
        if code:
            raise HostLost(f"host probe exited {code}: {_tail(err)}")
        snapshot = parse_snapshot(out.read_text(errors="replace"))
        if abs(snapshot.epoch - (before + after) / 2) > self.config.max_clock_skew_seconds:
            raise HostRefused("host clock differs from the challenge clock")
        return snapshot

    def _pull(
        self, label: str, image: str, deadline: float, cancel: threading.Event | None, work: Path
    ) -> None:
        err = work / "pull.err"
        try:
            code = self._remote(
                label,
                shlex.join([*self.hosts[label].docker, "pull", "--quiet", image]),
                stdout=work / f"pull-{time.monotonic_ns()}.out",
                stderr=err,
                deadline=deadline,
                cancel=cancel,
                limit=self.config.pull_timeout_seconds,
            )
        except AttemptFailed as error:  # the transport's output bounds: no honest pull trips them
            raise HostRefused("image pull output exceeds its bound") from error
        if code:
            raise HostLost(f"image pull exited {code}: {_tail(err)}")

    def _cache(
        self,
        label: str,
        ref: ReferenceSpec,
        deadline: float,
        cancel: threading.Event | None,
        work: Path,
    ) -> str | None:
        """The host directory to mount as the container's compute cache, created on first use.

        The host's answer is only accepted when it is the path we asked for. Remembered per host
        and image until a container run fails, so a directory removed by hand costs one attempt.
        """
        if not self.config.compute_cache:
            return None
        key = (label, ref.image_digest)
        if key in self._cache_dirs:
            return self._cache_dirs[key]
        assignment, expected = cache_location(self.hosts[label].compute_cache_dir, ref.image_digest)
        out, err = work / f"cache-{time.monotonic_ns()}.out", work / "cache.err"
        code = self._remote(
            label,
            prepare_command(assignment),
            stdout=out,
            stderr=err,
            deadline=deadline,
            cancel=cancel,
            limit=30,
        )
        if code:
            raise AttemptFailed(f"cannot prepare the compute cache directory: {_tail(err)}")
        reported = out.read_text(errors="replace").splitlines()
        if len(reported) != 1 or not expected.fullmatch(reported[0]) or not plain_path(reported[0]):
            raise AttemptFailed("the host reported an unexpected compute cache directory")
        self._cache_dirs[key] = reported[0]
        return reported[0]

    def _select(self, snapshot: Snapshot, ref: ReferenceSpec, image: str) -> list[Gpu]:
        n = ref.layout.n_gpus
        if image not in (snapshot.repo_digests or ()):
            raise HostRefused("image digest absent from the host's RepoDigests")
        idle = sorted(
            (
                g
                for g in snapshot.gpus
                if g.uuid not in snapshot.busy and g.memory_mib <= self.config.idle_gpu_memory_mib
            ),
            key=lambda g: g.uuid,
        )
        if len(idle) < n:
            raise HostBusy(f"{len(idle)} idle GPUs, {n} needed")
        chosen = idle[:n]
        reason = unqualified_device([g.capability for g in chosen], [ref.sm_count] * n)
        if reason is not None:
            raise HostRefused(reason)
        if len({g.driver for g in chosen}) != 1 or chosen[0].driver not in ref.driver_allowlist:
            raise HostRefused("driver is not the one allowlisted by the manifest")
        return chosen

    def _request(
        self,
        job: IslandJobV1,
        directory: Path,
        chosen: Sequence[Gpu],
        trace: bool,
        input_tar: Path,
        *,
        extra: Mapping[str, Any] | None = None,
    ) -> bytes:
        ref = job.manifest.training.reference_spec
        uuids = [g.uuid for g in chosen]
        files = {"job.json": canonicalize(job.model_dump(mode="json"))}
        for relative, source in _objects(job, directory):
            files[relative] = source.read_bytes()
        budget = int(job.deadline - time.time()) - MARGIN
        if budget < MARGIN:
            raise Aborted("job deadline too close to start")
        request = canonicalize(
            {
                "version": cuda_executor_runner.VERSION,
                "n_gpus": ref.layout.n_gpus,
                "uuids": uuids,
                "job_sha256": sha256(files["job.json"]),
                "budget_seconds": budget,
                "inputs": {name: sha256(raw) for name, raw in files.items()},
                "env": {
                    **ENV_BASE,
                    "CUBLAS_WORKSPACE_CONFIG": ref.env.CUBLAS_WORKSPACE_CONFIG,
                    "CUDA_DISABLE_PTX_JIT": ref.env.CUDA_DISABLE_PTX_JIT,
                    "NVIDIA_TF32_OVERRIDE": "0",
                    "HT_IMAGE_DIGEST": ref.image_digest,
                    "HT_ISLAND_TRACE": "1" if trace else "0",
                    "HT_GPU_UUIDS": ",".join(uuids),
                    "CUDA_VISIBLE_DEVICES": ",".join(uuids),
                },
                **(extra or {}),
            }
        )
        with tarfile.open(input_tar, "x:", format=tarfile.PAX_FORMAT) as archive:
            for name, raw in {"request.json": request, **files}.items():
                info = tarfile.TarInfo(name)
                info.size, info.mode = len(raw), 0o600
                archive.addfile(info, io.BytesIO(raw))
        return request

    def _read_result(
        self, tar_path: Path, staging: Path, n: int, request: bytes
    ) -> tuple[dict[str, str], ObservedEnvironment]:
        """Whitelist reader: rank files, environment.json and a last COMPLETE member."""
        written: dict[str, str] = {}
        small: dict[str, bytes] = {}
        complete, total, members = False, 0, 0
        try:
            with (
                _CappedReader(io.FileIO(tar_path)) as source,
                tarfile.open(fileobj=source, mode="r:") as archive,
            ):
                for member in archive:
                    parts = member.name.split("/")
                    rank = RANK.fullmatch(parts[0])
                    ranked = (
                        len(parts) > 1
                        and rank is not None
                        and int(rank[1]) < n
                        and all(p not in ("", ".", "..") for p in parts)
                        and parts[1] != "windows"  # dispute traces are the challenge's to write
                        and "\0" not in member.name
                    )
                    members += 1
                    total += member.size + tarfile.BLOCKSIZE  # header blocks are bytes too
                    if (
                        complete
                        or member.type not in (tarfile.REGTYPE, tarfile.AREGTYPE)
                        or any(key.startswith("GNU.sparse") for key in member.pax_headers)
                        or total > self.config.max_result_bytes
                        or (
                            len(parts) == 2
                            and parts[1] in RANK_JSON
                            and member.size > RANK_JSON_BYTES
                        )
                        or members > MAX_MEMBERS
                        or member.name in written
                        or member.name in small
                        or "\\" in member.name
                        or not (ranked or member.name in ("environment.json", "COMPLETE"))
                    ):
                        raise AttemptFailed(f"unacceptable result member {member.name!r}")
                    stream = archive.extractfile(member)
                    assert stream is not None
                    if not ranked:
                        if member.size > SMALL:
                            raise AttemptFailed(f"result member {member.name!r} too large")
                        small[member.name] = stream.read()
                        complete = member.name == "COMPLETE"
                        continue
                    target = staging / member.name
                    digest = hashlib.sha256()
                    try:
                        target.parent.mkdir(parents=True, exist_ok=True)
                        with open(target, "xb") as out:
                            while chunk := stream.read(1 << 20):
                                digest.update(chunk)
                                out.write(chunk)
                    except OSError as error:  # a host-chosen name the filesystem cannot hold
                        raise AttemptFailed(
                            f"cannot stage result member {member.name!r}: {type(error).__name__}"
                        ) from error
                    written[member.name] = digest.hexdigest()
        except (tarfile.TarError, ValueError, RecursionError) as error:
            raise AttemptFailed(f"result tar unreadable: {type(error).__name__}") from error
        if not complete or "environment.json" not in small:
            raise AttemptFailed("result is missing environment.json or the COMPLETE member")
        try:
            done = json.loads(shallow(small["COMPLETE"]))
            environment = ObservedEnvironment.model_validate_json(
                shallow(small["environment.json"])
            )
        except ValueError as error:
            raise AttemptFailed("result metadata is malformed") from error
        listed = {**written, "environment.json": sha256(small["environment.json"])}
        if (
            not isinstance(done, dict)
            or done.get("version") != cuda_executor_runner.VERSION
            or done.get("files") != listed
            or done.get("request_sha256") != sha256(request)
        ):
            raise AttemptFailed("COMPLETE does not match the received bytes")
        return written, environment

    def _check_environment(
        self, environment: ObservedEnvironment, ref: ReferenceSpec, chosen: Sequence[Gpu]
    ) -> dict[str, Any]:
        n = len(chosen)
        try:
            uuids = [gpu_uuid(u) for u in environment.gpu_uuids]
        except (IslandFailure, ValueError) as error:
            raise HostRefused("container reported a malformed GPU UUID") from error
        if (
            uuids != [g.uuid for g in chosen]
            or len(environment.gpu_names) != n
            or len(environment.drivers) != n
            or len(environment.sm_counts) != n
            or len(environment.capabilities) != n
        ):
            raise HostRefused("container sees other devices than the ones selected")
        capabilities = environment.capabilities
        reason = unqualified_device(capabilities, environment.sm_counts)
        if (
            reason is not None
            or environment.sm_counts != [ref.sm_count] * n
            or capabilities != [g.capability for g in chosen]
            or environment.drivers != [g.driver for g in chosen]
            or len(set(environment.drivers)) != 1
            or environment.drivers[0] not in ref.driver_allowlist
        ):
            raise HostRefused(reason or "container environment contradicts the host probe")
        return {**environment.model_dump(mode="json"), "image_digest": ref.image_digest}

    def _publish(
        self,
        job: IslandJobV1,
        directory: Path,
        staging: Path,
        record: dict[str, Any],
        trace: bool,
    ) -> IslandArtifacts:
        ranks = job.manifest.training.reference_spec.layout.n_gpus
        published = staging / "published"
        for relative, source in _objects(job, directory):
            target = published / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, target)
        durable_write(published / "job.json", canonicalize(job.model_dump(mode="json")))
        try:
            validate_artifacts(job, published)
            for rank in range(ranks):
                _check_rank(published / f"rank-{rank}", trace)
            record = {**record, "publication_sha256": _publication_sha(published)}
            data = canonicalize(record)
            envelope_v2.load_json(data)  # the bounds _dispatch_env_v2 will apply to it
        except Exception as error:  # noqa: BLE001 - host bytes reach torch/safetensors parsers
            raise AttemptFailed(f"result failed validation: {error}") from error
        if time.time() >= job.deadline:
            raise Aborted("job expired before atomic publication")
        _sync_tree(published)
        provenance = directory / "role-dispatch.json"
        if provenance.exists() or provenance.is_symlink():
            provenance.unlink()  # stale: written by an attempt that never published
        durable_write(provenance, data)
        os.rename(published, directory / "published")
        fd = os.open(directory, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(fd)
        finally:
            os.close(fd)
        return validate_artifacts(job, directory / "published")

    def _attempt(
        self,
        label: str,
        number: int,
        operation: str,
        binding: str,
        job: IslandJobV1,
        directory: Path,
        trace: bool,
        cancel: threading.Event | None,
    ) -> IslandArtifacts:
        host = self.hosts[label]
        ref = job.manifest.training.reference_spec
        image = f"{self.config.image_repository}@{ref.image_digest}"
        name = (
            "ht-exec-" + sha256(f"{job.digest()}:{label}:{number}:{time.time_ns()}".encode())[:16]
        )
        pinned = host.known_hosts_file.read_bytes()  # what ssh will verify the host against
        with tempfile.TemporaryDirectory(prefix="attempt-", dir=directory) as tmp:
            work = Path(tmp)
            snapshot = self._snapshot(label, image, job.deadline, cancel, work)
            if snapshot.repo_digests is None:
                self._pull(label, image, job.deadline, cancel, work)
                snapshot = self._snapshot(label, image, job.deadline, cancel, work)
            machine = self._admit(label, snapshot)
            chosen = self._select(snapshot, ref, image)
            n = len(chosen)
            try:
                cache = self._cache(label, ref, job.deadline, cancel, work)
            except AttemptFailed as error:
                # A speed-up must not fail a job the host can run: go on without it, on the record.
                self.journal.append("exec_cache_unavailable", label=label, reason=str(error))
                cache = None
            self.journal.append(
                "exec_attempt",
                label=label,
                name=name,
                operation=operation,
                job=job.digest(),
                compute_cache=cache is not None,
            )
            input_tar, result_tar = work / "input.tar", work / "result.tar"
            request = self._request(job, directory, chosen, trace, input_tar)
            log = self.config.state_dir / "logs" / f"{name}.stderr"
            try:
                code = self._remote(
                    label,
                    run_command(
                        host.docker,
                        image,
                        name,
                        job.digest(),
                        [g.uuid for g in chosen],
                        self.config.network,
                        cache,
                    ),
                    stdin=input_tar,
                    stdout=result_tar,
                    stderr=log,
                    deadline=job.deadline,
                    cancel=cancel,
                    max_bytes=self.config.max_result_bytes,
                )
            except (HostLost, Aborted, AttemptFailed):
                self._cache_dirs.pop((label, ref.image_digest), None)
                self._remove(label, name, work)
                raise
            if code:
                self._cache_dirs.pop((label, ref.image_digest), None)  # it may be what failed
            if code == RUNNER_REFUSED_ENVIRONMENT:
                raise HostRefused(f"container refused the devices: {_tail(log)}")
            if code == RUNNER_PROBE_FAILED:
                raise AttemptFailed(
                    f"the device probe in the container could not run: {_tail(log)}"
                )
            if code:
                raise AttemptFailed(f"container exited {code}: {_tail(log)}")
            staging = work / "staging"
            try:
                written, environment = self._read_result(
                    result_tar, staging / "published", n, request
                )
                observed = self._check_environment(environment, ref, chosen)
            except (RecursionError, MemoryError, OverflowError) as error:
                # Host bytes exhausted a parser nobody foresaw. The cause may be ours (memory), so
                # the attempt fails and the next host is tried, but nobody is quarantined.
                raise AttemptFailed(f"result exhausted a parser: {type(error).__name__}") from error
            record = {
                "executor": EXECUTOR,
                "operation": operation,
                "binding": binding,
                "run_id": job.run_id,
                "job_sha256": job.digest(),
                "host": {
                    "label": label,
                    "machine_id": machine,
                    "instance_id": _ident(
                        "instance", sha256(pinned) + f"{host.address}:{host.port}"
                    ),
                    "known_hosts_sha256": sha256(pinned),
                },
                "image": {"requested": image, "repo_digests": list(snapshot.repo_digests or ())},
                "runner_sha256": sha256(RUNNER_SOURCE.encode()),
                "result_tar_sha256": _file_sha(result_tar),
                "compute_cache": cache is not None,
                "environment": observed,
            }
            artifacts = self._publish(job, directory, staging, record, trace)
            self.journal.append(
                "exec_published", label=label, name=name, files=len(written), job=job.digest()
            )
            return artifacts

    def _remove(self, label: str, name: str, work: Path) -> None:
        """Best-effort `docker rm -f` of our own container over a fresh connection."""
        try:
            self._remote(
                label,
                shlex.join([*self.hosts[label].docker, "rm", "-f", name]),
                stdout=work / f"rm-{name}.out",
                stderr=work / f"rm-{name}.err",
                deadline=time.time() + 20,
                cancel=None,
            )
        except IslandFailure as error:
            self.journal.append("exec_cleanup_failed", label=label, name=name, error=str(error))

    # -- public ---------------------------------------------------------------------------

    def published(
        self, job: IslandJobV1, directory: Path, trace: bool = False
    ) -> IslandArtifacts | None:
        """A complete earlier publication of this job, or None; a half-written one is refused."""
        published = directory / "published"
        if not published.exists():
            return None
        provenance = directory / "role-dispatch.json"
        if provenance.is_symlink() or not provenance.is_file():
            raise IslandFailure("publication exists without executor provenance")
        try:
            record = json.loads(provenance.read_bytes())
            same = (
                record["executor"] == EXECUTOR
                and record["job_sha256"] == job.digest()
                and record["publication_sha256"] == _publication_sha(published)
            )
        except (ValueError, KeyError, TypeError, OSError) as error:
            raise IslandFailure("executor provenance is unreadable") from error
        if not same:
            raise IslandFailure("executor provenance does not match the publication")
        artifacts = validate_artifacts(job, published)
        try:
            for rank in range(job.manifest.training.reference_spec.layout.n_gpus):
                _check_rank(published / f"rank-{rank}", trace)
        except (IslandFailure, ValueError, KeyError, OSError) as error:
            raise IslandFailure(f"earlier publication is not a usable CUDA one: {error}") from error
        return artifacts

    def launch(
        self,
        operation: str,
        binding: str,
        job: IslandJobV1,
        directory: Path,
        *,
        trace: bool,
        cancel: threading.Event | None,
    ) -> IslandArtifacts:
        if job.manifest.training.reference_spec.layout.n_gpus > MAX_GPUS:
            raise IslandFailure(f"the executor carries layouts of at most {MAX_GPUS} GPUs")
        directory = directory.resolve()
        directory.mkdir(parents=True, exist_ok=True)
        with flock(directory / "launch.lock"):
            if time.time() >= job.deadline or (cancel is not None and cancel.is_set()):
                raise Aborted("job deadline/cancellation")
            earlier = self.published(job, directory, trace)
            if earlier is not None:
                return earlier
            _objects(job, directory)  # refuse a job the transport cannot carry before any ssh
            if job.deadline - time.time() < 2 * MARGIN:
                raise Aborted("job deadline too close to start")
            order, attempts, last = self._order(job), 0, "no host answered"
            for sweep in range(self.config.sweeps):
                skip: set[str] = set()
                while (label := self._lease(order, skip, job.deadline, cancel)) is not None:
                    try:
                        attempts += 1
                        return self._attempt(
                            label, attempts, operation, binding, job, directory, trace, cancel
                        )
                    except HostRefused as error:
                        self._quarantine(label, str(error))
                        self.journal.append("exec_host_refused", label=label, reason=str(error))
                        attempts -= 1
                        last = f"{label} refused: {error}"
                    except (HostLost, HostBusy) as error:
                        self.journal.append("exec_host_unavailable", label=label, reason=str(error))
                        attempts -= 1
                        last = f"{label}: {error}"
                    except AttemptFailed as error:
                        self.journal.append("exec_attempt_failed", label=label, reason=str(error))
                        last = f"{label}: {error}"
                        if attempts >= self.config.max_attempts:
                            raise IslandFailure(f"cuda executor gave up: {last}") from error
                    finally:
                        self._locks[label].release()
                    skip.add(label)
                if all(label in self._refusals() for label in order):
                    break
                if sweep + 1 < self.config.sweeps:
                    self._pause(cancel, job.deadline)
            refused = "".join(
                f"; {label} was refused: {why}" for label, why in self._refusals().items()
            )
            raise IslandFailure(f"cuda executor found no host able to run the job: {last}{refused}")

    def _pause(self, cancel: threading.Event | None, deadline: float) -> None:
        pause = min(self.config.sweep_pause_seconds, max(0.0, deadline - time.time()))
        if cancel is not None:
            if cancel.wait(pause):
                raise Aborted("lease cancelled")
        else:
            self.sleep(pause)


def _objects(job: IslandJobV1, directory: Path) -> list[tuple[str, Path]]:
    """The job's input files as (relative name, confined path); none may shadow the transport."""
    found = []
    for relative in sorted(set(job.object_paths.values())):
        if relative in TRANSPORT_NAMES:
            raise IslandFailure(f"object path {relative!r} collides with the transport")
        found.append((relative, confined(directory, relative)))
    return found


def _publication_sha(published: Path) -> str:
    """Hash of the published files; dispute window traces added later by launch_window are not
    part of what the host returned and do not count."""
    files: dict[str, str] = {}
    for path in sorted(published.rglob("*")):
        if path.is_symlink() or not (path.is_file() or path.is_dir()):
            raise IslandFailure("publication holds a link or a special file")
        relative = path.relative_to(published)
        if path.is_file() and not (len(relative.parts) > 2 and relative.parts[1] == "windows"):
            files[relative.as_posix()] = _file_sha(path)
    return sha256(canonicalize(files))


def _check_rank(rank: Path, trace: bool) -> None:
    if json.loads((rank / "summary.json").read_bytes())["backend"] != "cuda":
        raise IslandFailure("rank summary is not a CUDA publication")
    if trace:
        _check_trace(rank)


def _check_trace(rank: Path) -> None:
    """Accept a trace only if the reader that the audit, the store and a dispute run on it does."""
    from hypertrain.auditor.bisect import RefereeError
    from hypertrain.auditor.island_bisect import IslandParty

    if not (rank / "trace.json").is_file():
        raise IslandFailure("requested trace is absent or malformed")
    try:
        entries = IslandParty.read_trace(rank.parent, f"{rank.name}/trace.json")
    except (RefereeError, ValueError) as error:
        raise IslandFailure("requested trace is absent or malformed") from error
    if any(entry.rank != int(rank.name.removeprefix("rank-")) for entry in entries):
        raise IslandFailure("requested trace names another rank")


def _sync_tree(root: Path) -> None:
    """fsync every file and directory below root so a rename publishes durable bytes."""
    for path in [*sorted(root.rglob("*")), root]:
        fd = os.open(path, os.O_RDONLY)
        try:
            os.fsync(fd)
        finally:
            os.close(fd)


def load(path: Path) -> RoleLaunch:
    config = ExecutorConfig.model_validate_json(path.read_bytes())
    artifacts = os.environ.get(ARTIFACTS_ENV)
    if artifacts:  # opt-in: results through R2 (docs/adr/0013-executor-artifacts-via-r2.md)
        from hypertrain.gpu_ops import artifact_transfer

        executor: Executor = artifact_transfer.build(config, Path(artifacts))
    else:
        executor = Executor(config)

    def factory(
        operation: str, accepted: IslandJobV1, home: Path, context: Mapping[str, Any]
    ) -> IslandLaunch:
        if operation not in OPERATIONS:
            raise RuntimeError(f"cuda executor does not serve {operation!r}")
        binding = body_digest(dict(context))

        def dispatched(
            job: IslandJobV1,
            directory: Path,
            *,
            backend: Literal["cpu", "cuda"],
            cancel: threading.Event | None = None,
            trace: bool = False,
        ) -> IslandArtifacts:
            if job != accepted or directory != home:
                raise RuntimeError("cuda executor launch differs from the accepted descriptor")
            if backend == "cpu":
                return launch_island(job, directory, backend="cpu", cancel=cancel, trace=trace)
            if job.run_id != config.run_id:
                raise RuntimeError("cuda executor job is not the authorized run")
            return executor.launch(operation, binding, job, directory, trace=trace, cancel=cancel)

        return dispatched

    return factory
