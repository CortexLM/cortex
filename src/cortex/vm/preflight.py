"""Read-only local host diagnostics; a passing check is not VM execution evidence."""

from __future__ import annotations

import errno
import fcntl
import hashlib
import os
import shutil
import stat
import tomllib
from collections.abc import Callable, Iterator
from contextlib import contextmanager
from datetime import UTC, datetime
from functools import partial
from ipaddress import ip_address
from pathlib import Path
from typing import BinaryIO, Literal

from cryptography import x509
from cryptography.hazmat.primitives import serialization

from cortex.proof.models import Document
from cortex.rlm import AgentLimits
from cortex.rlm.offer import InferenceOffer

from .firecracker import HostConfig
from .models import Resources
from .network import Egress

# Linux UAPI linux/kvm.h: these inspect the KVM device and never create a VM.
KVM_GET_API_VERSION = 0xAE00
KVM_CHECK_EXTENSION = 0xAE03
KVM_CAP_MAX_VCPUS = 66
REQUIRED_KVM_CAPABILITIES = {"user_memory": 3, "irqfd": 32, "ioeventfd": 36, "immediate_exit": 136}


class PreflightCheck(Document):
    name: str
    ok: bool
    detail: str


class PreflightReport(Document):
    schema_version: Literal[1] = 1
    scope: Literal["local prerequisites only; no VM boot or execution verified"] = (
        "local prerequisites only; no VM boot or execution verified"
    )
    ready: bool
    checks: list[PreflightCheck]


def _check(name: str, action: Callable[[], str], failure: str) -> PreflightCheck:
    try:
        return PreflightCheck(name=name, ok=True, detail=action())
    except Exception:
        return PreflightCheck(name=name, ok=False, detail=failure)


def _kvm_checks(vcpus: int) -> list[PreflightCheck]:
    descriptor = None
    checks = []
    try:
        descriptor = os.open("/dev/kvm", os.O_RDWR | os.O_CLOEXEC | os.O_NOFOLLOW | os.O_NONBLOCK)
        if not stat.S_ISCHR(os.fstat(descriptor).st_mode):
            raise ValueError("not a KVM device")
        checks.append(PreflightCheck(name="kvm_device", ok=True, detail="character device opened"))
        version = fcntl.ioctl(descriptor, KVM_GET_API_VERSION, 0)
        checks.append(
            PreflightCheck(name="kvm_api", ok=version == 12, detail=f"API {version}; required 12")
        )
        if version != 12:
            return checks
        for name, capability in REQUIRED_KVM_CAPABILITIES.items():
            value = fcntl.ioctl(descriptor, KVM_CHECK_EXTENSION, capability)
            checks.append(
                PreflightCheck(
                    name="kvm_" + name,
                    ok=value > 0,
                    detail="supported" if value > 0 else "required capability unavailable",
                )
            )
        maximum = fcntl.ioctl(descriptor, KVM_CHECK_EXTENSION, KVM_CAP_MAX_VCPUS)
        checks.append(
            PreflightCheck(
                name="kvm_vcpus",
                ok=maximum >= vcpus,
                detail=f"KVM maximum {maximum}; configured ceiling {vcpus}",
            )
        )
    except (OSError, ValueError) as error:
        reason = "KVM device or ioctl unavailable"
        if isinstance(error, OSError) and error.errno in {errno.EACCES, errno.EPERM}:
            reason = "KVM device access denied for this process"
        elif isinstance(error, OSError) and error.errno == errno.ENOENT:
            reason = "KVM device missing"
        checks.append(PreflightCheck(name="kvm_device", ok=False, detail=reason))
    finally:
        if descriptor is not None:
            os.close(descriptor)
    return checks


@contextmanager
def _regular_file(path: Path, *, private: bool = False) -> Iterator[BinaryIO]:
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC)
    with os.fdopen(descriptor, "rb") as source:
        info = os.fstat(source.fileno())
        if (
            not stat.S_ISREG(info.st_mode)
            or info.st_uid not in {0, os.geteuid()}
            or stat.S_IMODE(info.st_mode) & (0o077 if private else 0o022)
            or (private and info.st_nlink != 1)
        ):
            raise ValueError("unsafe file")
        yield source


def _read_file(path: Path, *, limit: int, private: bool = False) -> bytes:
    with _regular_file(path, private=private) as source:
        if not 0 < os.fstat(source.fileno()).st_size <= limit:
            raise ValueError("invalid file size")
        raw = source.read(limit + 1)
    if not raw.strip() or len(raw) > limit:
        raise ValueError("invalid file size")
    return raw


def _pin(path: Path, expected: str) -> str:
    measured = hashlib.sha256()
    with _regular_file(path) as source:
        while chunk := source.read(1024 * 1024):
            measured.update(chunk)
    if measured.hexdigest() != expected:
        raise ValueError("digest mismatch")
    return "exact SHA-256 verified"


def _executable(path: Path) -> str:
    with _regular_file(path) as source:
        if not os.fstat(source.fileno()).st_mode & 0o111 or not os.access(path, os.X_OK):
            raise ValueError("not executable")
    return "executable file available; binary not executed"


def _utility(name: str) -> str:
    selected = shutil.which(name)
    if selected is None:
        raise ValueError("host utility unavailable")
    # Distribution tools such as mkfs.ext4 commonly link to one shared binary.
    return _executable(Path(selected).resolve(strict=True))


def _tun() -> str:
    descriptor = os.open("/dev/net/tun", os.O_RDWR | os.O_CLOEXEC | os.O_NOFOLLOW | os.O_NONBLOCK)
    try:
        if not stat.S_ISCHR(os.fstat(descriptor).st_mode):
            raise ValueError("not a TUN device")
    finally:
        os.close(descriptor)
    return "TUN character device opened; no interface created"


def _private_directory(path: Path) -> None:
    descriptor = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
    try:
        info = os.fstat(descriptor)
        if info.st_uid != os.geteuid() or stat.S_IMODE(info.st_mode) != 0o700:
            raise ValueError("unsafe directory")
    finally:
        os.close(descriptor)


def _state(config: dict) -> str:
    host = config["host"]
    for name in ("jail_root", "retain_root", "pack_dir"):
        _private_directory(Path(host[name]))
    paths = [Path(host["state_db"])]
    if "knowledge" in config:
        paths.append(Path(config["knowledge"]["state_db"]))
        public = _read_file(Path(config["knowledge"]["owner_public_file"]), limit=256)
        if len(bytes.fromhex(public.decode().strip())) != 32:
            raise ValueError("invalid knowledge owner")
    for path in paths:
        _private_directory(path.parent)
        try:
            path.lstat()
        except FileNotFoundError:
            continue
        with _regular_file(path, private=True):
            pass
    return "private directories and state paths verified; no state created"


def _credential(path: Path) -> str:
    value = _read_file(path, private=True, limit=4096).decode().strip()
    if not value or any(char.isspace() for char in value):
        raise ValueError("invalid credential")
    return "private credential file available; value withheld"


def _tls(config: dict) -> str:
    tls = config["tls"]
    certificate = x509.load_pem_x509_certificate(
        _read_file(Path(tls["certificate"]), limit=256 * 1024)
    )
    key = serialization.load_pem_private_key(
        _read_file(Path(tls["private_key"]), private=True, limit=16_384), password=None
    )
    encoding = serialization.Encoding.DER
    form = serialization.PublicFormat.SubjectPublicKeyInfo
    if key.public_key().public_bytes(encoding, form) != certificate.public_key().public_bytes(
        encoding, form
    ):
        raise ValueError("TLS key mismatch")
    now = datetime.now(UTC)
    if not certificate.not_valid_before_utc <= now < certificate.not_valid_after_utc:
        raise ValueError("TLS validity window")
    names = tls["names"]
    if not isinstance(names, list) or not names:
        raise ValueError("TLS SAN names required")
    san = certificate.extensions.get_extension_for_class(x509.SubjectAlternativeName).value
    for name in names:
        try:
            address = ip_address(name)
        except ValueError:
            if name not in san.get_values_for_type(x509.DNSName):
                raise ValueError("TLS SAN mismatch") from None
        else:
            if address not in san.get_values_for_type(x509.IPAddress):
                raise ValueError("TLS SAN mismatch")
    return "private key matches current certificate and configured SANs; trust chain not verified"


def _inference(config: dict) -> str:
    inference = config["inference"]
    offer = InferenceOffer.model_validate_json(
        _read_file(Path(inference["offer_file"]), limit=16_384)
    )
    limits = AgentLimits.model_validate(config.get("limits", offer.limits.model_dump()))
    offer.verify_runtime(inference.get("model", offer.model), limits, inference["offer_commitment"])
    return "open signed offer, commitment and runtime limits verified; provider not contacted"


def _host(config: dict) -> HostConfig:
    host = config["host"]
    for name in ("uid", "gid"):
        if type(host.get(name, 10000)) is not int:
            raise ValueError("invalid jailer identity")
    return HostConfig(
        kernel=Path(host["kernel"]),
        kernel_digest=host["kernel_digest"],
        images={key: Path(value) for key, value in config["images"].items()},
        jail_root=Path(host["jail_root"]),
        retain_root=Path(host["retain_root"]),
        pack_dir=Path(host["pack_dir"]),
        firecracker=Path(host.get("firecracker", "/usr/local/bin/firecracker")),
        jailer=Path(host.get("jailer", "/usr/local/bin/jailer")),
        uid=host.get("uid", 10000),
        gid=host.get("gid", 10000),
        uplink=host.get("uplink", "eth0"),
        topic_egress=tuple(Egress(**row) for row in config.get("egress", [])),
    )


def _resources(config: dict) -> Resources:
    host = config["host"]
    for name, default, maximum in (("max_experiments", 1, 128), ("max_topics", 64, 1024)):
        value = host.get(name, default)
        if type(value) is not int or not 1 <= value <= maximum:
            raise ValueError("invalid concurrency")
    return Resources.model_validate(config.get("caps", {}), strict=True)


def check_host(path: Path) -> PreflightReport:
    """Inspect prerequisites without creating state, booting a VM or calling a provider."""
    root = os.geteuid() == 0
    checks = [
        PreflightCheck(
            name="process_identity",
            ok=root,
            detail="effective UID is root" if root else "root required for jailer and host setup",
        )
    ]
    for utility in ("cp", "chown", "mkfs.ext4"):
        checks.append(
            _check(
                "utility_" + utility,
                partial(_utility, utility),
                "required host utility unavailable, unsafe or not executable",
            )
        )
    config: dict = {}
    host = None
    caps = None
    try:
        config = tomllib.loads(_read_file(path, limit=1024 * 1024).decode())
        host = _host(config)
        checks.append(PreflightCheck(name="configuration", ok=True, detail="host shape validated"))
    except Exception:
        checks.append(
            PreflightCheck(
                name="configuration", ok=False, detail="host configuration invalid or unavailable"
            )
        )
    try:
        caps = _resources(config)
        checks.append(
            PreflightCheck(
                name="resources",
                ok=True,
                detail=(
                    f"per-VM ceilings: {caps.vcpus} vCPU, {caps.mem_mib} MiB RAM, "
                    f"{caps.disk_mib} MiB disk; concurrency valid"
                ),
            )
        )
    except Exception:
        checks.append(
            PreflightCheck(
                name="resources", ok=False, detail="invalid resource ceilings or concurrency"
            )
        )
    checks.extend(_kvm_checks(caps.vcpus if caps is not None else 16))
    if host is not None:
        if host.topic_egress:
            for utility in ("ip", "nft", "sysctl"):
                checks.append(
                    _check(
                        "utility_" + utility,
                        partial(_utility, utility),
                        "required topic-network utility unavailable, unsafe or not executable",
                    )
                )
            checks.append(_check("tun_device", _tun, "TUN device unavailable or access denied"))
        checks.extend(
            [
                _check(
                    "kernel",
                    lambda: _pin(host.kernel, host.kernel_digest),
                    "kernel unavailable, unsafe or SHA-256 mismatch",
                ),
                _check(
                    "firecracker",
                    lambda: _executable(host.firecracker),
                    "Firecracker executable unavailable or unsafe",
                ),
                _check(
                    "jailer",
                    lambda: _executable(host.jailer),
                    "jailer executable unavailable or unsafe",
                ),
            ]
        )
        for index, (expected, image) in enumerate(host.images.items(), 1):
            checks.append(
                _check(
                    f"rootfs_{index}",
                    partial(_pin, image, expected),
                    "rootfs unavailable, unsafe or SHA-256 mismatch",
                )
            )
        checks.extend(
            [
                _check(
                    "state",
                    lambda: _state(config),
                    "state paths unavailable or not privately owned",
                ),
                _check(
                    "bearer",
                    lambda: _credential(Path(config["host"]["token_file"])),
                    "private bearer file unavailable or invalid",
                ),
                _check(
                    "provider_key",
                    lambda: _credential(Path(config["inference"]["api_key_file"])),
                    "private provider key file unavailable or invalid",
                ),
                _check(
                    "tls",
                    lambda: _tls(config),
                    "TLS files, private key, validity or SAN binding invalid",
                ),
                _check(
                    "inference_offer",
                    lambda: _inference(config),
                    "signed inference offer unavailable, closed or runtime mismatched",
                ),
            ]
        )
    return PreflightReport(ready=all(check.ok for check in checks), checks=checks)
