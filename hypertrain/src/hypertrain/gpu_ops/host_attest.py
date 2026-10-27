"""SSH host-key statements that tie a qualification result to the machine behind it.

A host signs, with its SSH host key, a canonical statement about the result it produced
(docs/adr/0011-audit-integrity.md, part 4). Verification runs `ssh-keygen -Y verify`, which the
server image already carries; nothing here implements a signature scheme. The signature proves that
the holder of the pinned key agreed to the statement, not where the computation ran.
"""

from __future__ import annotations

import base64
import hashlib
import os
import re
import shlex
import shutil
import stat
import subprocess
import tempfile
from collections.abc import Callable
from pathlib import Path
from typing import Annotated, Literal, NamedTuple

from pydantic import (
    BaseModel,
    ConfigDict,
    Field,
    StringConstraints,
    ValidationError,
    field_validator,
)

from hypertrain.gpu_ops.launcher import Reject
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages import Hex64, ImageDigest

NAMESPACE = "hypertrain-qualification-v1"
ATTESTATION = "attestation.json"
SIGNATURE = "attestation.sig"
# ssh-keygen is looked up here only: the process PATH and the cwd cannot substitute it.
SEARCH_PATH = "/usr/local/bin:/usr/bin:/bin"
_UUID = r"[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}"
GPU_UUID = re.compile(f"GPU-{_UUID}")
# Local limits with no consensus effect: 16 GPUs x 44 bytes plus the fixed fields is about 1.2 KiB,
# an Ed25519 SSHSIG is 330 bytes and ssh-keygen verifies in a few milliseconds.
MAX_GPUS = 16
MAX_STATEMENT = 4096
MAX_SIGNATURE = 2048
KEYGEN_TIMEOUT = 10
HOST_KEY_PATH = "/etc/ssh/ssh_host_ed25519_key"
# Run on the host over its pinned SSH channel; nothing else is ever staged or executed there.
FACTS = (
    "nvidia-smi --query-gpu=uuid,driver_version --format=csv,noheader"
    " && cat /proc/sys/kernel/random/boot_id"
)

_FACT_ROW = re.compile(
    r"(GPU-[0-9a-fA-F]{8}(?:-[0-9a-fA-F]{4}){3}-[0-9a-fA-F]{12}), ([0-9A-Za-z._+-]{1,64})"
)
_HOST_KEY = re.compile(r"ssh-ed25519 [A-Za-z0-9+/]{68}")
# SSH wire form of an Ed25519 key: the string "ssh-ed25519", then a 32-byte string.
_ED25519_PREFIX = b"\x00\x00\x00\x0bssh-ed25519\x00\x00\x00\x20"


class HostStatement(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)

    version: Literal[1]
    config_sha256: Hex64
    result_sha256: Hex64
    image_digest: ImageDigest
    # Every GPU of the box, not only the ones the result used.
    gpu_uuids: Annotated[list[str], Field(min_length=1, max_length=MAX_GPUS)]
    drivers: Annotated[
        list[Annotated[str, StringConstraints(min_length=1, max_length=64)]],
        Field(min_length=1, max_length=MAX_GPUS),
    ]
    boot_id: Annotated[str, StringConstraints(pattern=f"^{_UUID}$")]

    @field_validator("gpu_uuids")
    @classmethod
    def _uuids(cls, values: list[str]) -> list[str]:
        if values != sorted(set(values)) or not all(GPU_UUID.fullmatch(v) for v in values):
            raise ValueError("gpu_uuids must be sorted, distinct GPU-<uuid> strings")
        return values

    @field_validator("drivers")
    @classmethod
    def _drivers(cls, values: list[str]) -> list[str]:
        if values != sorted(set(values)):
            raise ValueError("drivers must be sorted and distinct")
        return values


class Attestation(NamedTuple):
    statement: HostStatement
    raw: bytes
    signature: bytes


def statement_bytes(statement: HostStatement) -> bytes:
    return canonicalize(statement.model_dump(mode="json"))


def _blob(key: str) -> bytes:
    """The key goes into allowed_signers: only the exact `ssh-ed25519 <base64>` form passes."""
    if not _HOST_KEY.fullmatch(key):
        raise Reject("qualification_host_key_pin_invalid")
    blob = base64.b64decode(key.removeprefix("ssh-ed25519 "), validate=True)
    if not blob.startswith(_ED25519_PREFIX):
        raise Reject("qualification_host_key_pin_invalid")
    return blob


def pinned_key(known_hosts: bytes) -> str:
    """The one ssh-ed25519 key of a known_hosts file; any other count or form is no pin."""
    keys = set()
    for line in known_hosts.decode("utf-8", "replace").split("\n"):
        # Fields are separated by blanks and tabs only, so a stray CR cannot hide a second key.
        fields = re.split(r"[ \t]+", line.strip(" \t\r"))
        if len(fields) >= 3 and fields[0][0] not in "#@" and fields[1] == "ssh-ed25519":
            keys.add(f"ssh-ed25519 {fields[2]}")
    if len(keys) != 1:
        raise Reject("qualification_host_key_pin_invalid")
    (key,) = keys
    _blob(key)
    return key


def fingerprint(key: str) -> str:
    """`ssh-keygen -l` form, the one a provider console prints."""
    digest = base64.b64encode(hashlib.sha256(_blob(key)).digest()).decode()
    return "SHA256:" + digest.rstrip("=")


def verify(raw: bytes, signature: bytes, key: str) -> None:
    _blob(key)
    keygen = shutil.which("ssh-keygen", path=SEARCH_PATH)
    if keygen is None:
        raise Reject("qualification_ssh_keygen_unavailable")
    with tempfile.TemporaryDirectory(prefix="ht-attest-") as scratch:
        signers, armored = Path(scratch, "allowed_signers"), Path(scratch, "signature")
        signers.write_text(f'host namespaces="{NAMESPACE}" {key}\n')
        armored.write_bytes(signature)
        argv = [keygen, "-Y", "verify", "-f", str(signers), "-I", "host"]
        argv += ["-n", NAMESPACE, "-s", str(armored)]
        try:
            done = subprocess.run(  # noqa: S603 - fixed argv, resolved from a fixed search path
                argv,
                input=raw,
                capture_output=True,
                env={"PATH": SEARCH_PATH, "LC_ALL": "C"},
                timeout=KEYGEN_TIMEOUT,
                check=False,
            )
        except (OSError, subprocess.TimeoutExpired) as exc:
            raise Reject("qualification_ssh_keygen_unavailable") from exc
    if done.returncode == 255 and b"No user exists for uid" in done.stderr:
        raise Reject("qualification_ssh_keygen_unavailable")
    if done.returncode:
        raise Reject("qualification_attestation_signature")


def _no_follow(path: str, flags: int) -> int:
    return os.open(path, flags | os.O_NOFOLLOW | os.O_NONBLOCK)


def _read(path: Path, limit: int) -> bytes:
    """A planted symlink, a FIFO, a directory or an oversized file is no attestation."""
    try:
        with open(path, "rb", opener=_no_follow) as file:
            if not stat.S_ISREG(os.fstat(file.fileno()).st_mode):
                raise Reject("qualification_attestation_missing")
            data = file.read(limit + 1)
    except OSError as exc:
        raise Reject("qualification_attestation_missing") from exc
    if len(data) > limit:
        raise Reject("qualification_attestation_missing")
    return data


def load(result: Path) -> Attestation:
    """The statement and signature beside result.json; the statement must be in canonical form."""
    raw = _read(result.parent / ATTESTATION, MAX_STATEMENT)
    signature = _read(result.parent / SIGNATURE, MAX_SIGNATURE)
    try:
        statement = HostStatement.model_validate_json(raw)
    except ValidationError as exc:
        raise Reject("qualification_attestation_not_canonical") from exc
    if statement_bytes(statement) != raw:
        raise Reject("qualification_attestation_not_canonical")
    return Attestation(statement, raw, signature)


class HostFacts(NamedTuple):
    gpu_uuids: list[str]
    drivers: list[str]
    boot_id: str


def parse_facts(text: str) -> HostFacts:
    """The output of FACTS: one `GPU-<uuid>, <driver>` row per GPU, then the boot id."""
    *rows, boot = text.splitlines() or [""]
    matches = [_FACT_ROW.fullmatch(row) for row in rows]
    uuids = ["GPU-" + m[1][4:].lower() for m in matches if m]
    if (
        not rows
        or len(rows) > MAX_GPUS
        or len(matches) != len(uuids)
        or len(set(uuids)) != len(uuids)
        or not re.fullmatch(_UUID, boot)
    ):
        raise Reject("qualification_host_facts_malformed")
    return HostFacts(uuids, [m[2] for m in matches if m], boot)


def attest(
    run: Callable[[str, bytes], bytes],
    result: Path,
    *,
    key_path: str = HOST_KEY_PATH,
    sudo: bool = True,
    pin: str,
) -> tuple[bytes, bytes]:
    """The statement and signature a host gives for `result`; the caller writes them.

    `run(command, stdin)` executes one fixed command on the host, over the SSH channel pinned by
    the operator, and returns its stdout. The signature is verified against `pin` before it is
    returned, so a wrong host or a wrong key never leaves a file behind."""
    from hypertrain.gpu_ops.network_qualification import Result  # that module imports this one

    data = result.read_bytes()
    recorded = Result.model_validate_json(data)
    facts = parse_facts(run(FACTS, b"").decode("utf-8", "replace"))
    environment = recorded.environment
    if (
        environment.gpu_uuids is None
        or not set(environment.gpu_uuids) <= set(facts.gpu_uuids)
        or not set(environment.drivers) <= set(facts.drivers)
    ):
        raise Reject("qualification_attestation_wrong_host")
    statement = HostStatement(
        version=1,
        config_sha256=recorded.config_sha256,
        result_sha256=hashlib.sha256(data).hexdigest(),
        image_digest=environment.image_digest,
        gpu_uuids=sorted(facts.gpu_uuids),
        drivers=sorted(set(facts.drivers)),
        boot_id=facts.boot_id,
    )
    raw = statement_bytes(statement)
    signing = f"ssh-keygen -Y sign -f {shlex.quote(key_path)} -n {NAMESPACE}"
    signature = run(("sudo -n " if sudo else "") + signing, raw)
    verify(raw, signature, pin)
    return raw, signature


def write_files(result: Path, raw: bytes, signature: bytes) -> None:
    """Create both files beside result.json, never replacing one: a container can plant either
    in its output directory, and a planted file is a reason to investigate, not to overwrite."""
    created: list[Path] = []
    try:
        for name, data in ((ATTESTATION, raw), (SIGNATURE, signature)):
            flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW
            with os.fdopen(os.open(result.parent / name, flags, 0o600), "wb") as file:
                created.append(result.parent / name)
                file.write(data)
    except FileExistsError as exc:
        for path in created:
            path.unlink()
        raise Reject("qualification_attestation_exists") from exc
