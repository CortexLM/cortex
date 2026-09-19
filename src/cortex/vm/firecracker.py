"""Real KVM backend. Every subprocess is argv-only and belongs to a tracked jail."""

from __future__ import annotations

import asyncio
import base64
import fcntl
import hashlib
import json
import os
import re
import shutil
import signal
import stat
from dataclasses import dataclass
from pathlib import Path
from typing import Protocol

from cortex.proof.artifacts import verify_artifact

from .models import MAX_ARTIFACT, ExecuteRequest, GuestOutput, VmError, VmSpec
from .network import Egress, NetworkPlan
from .transport import VsockTransport


@dataclass(frozen=True)
class HostConfig:
    kernel: Path
    kernel_digest: str
    images: dict[str, Path]
    jail_root: Path
    retain_root: Path
    firecracker: Path = Path("/usr/local/bin/firecracker")
    jailer: Path = Path("/usr/local/bin/jailer")
    kvm: Path = Path("/dev/kvm")
    uid: int = 10000
    gid: int = 10000
    boot_timeout: float = 60
    uplink: str = "eth0"
    topic_egress: tuple[Egress, ...] = ()
    pack_dir: Path | None = None

    def __post_init__(self):
        if self.uid <= 0 or self.gid <= 0:
            raise ValueError("jailer must drop privileges")
        if not re.fullmatch(r"[A-Za-z0-9_.-]{1,15}", self.uplink):
            raise ValueError("invalid uplink")
        for digest in [self.kernel_digest, *self.images]:
            if not re.fullmatch(r"[0-9a-f]{64}", digest):
                raise ValueError("kernel and rootfs require actual sha256 pins")
        if not self.images:
            raise ValueError("at least one pinned rootfs is required")


class CommandRunner(Protocol):
    async def run(self, argv: list[str]) -> None: ...
    async def spawn(self, argv: list[str], console: Path) -> asyncio.subprocess.Process: ...


class SystemCommands:
    async def run(self, argv: list[str]) -> None:
        process = await asyncio.create_subprocess_exec(
            *argv,
            stdin=asyncio.subprocess.DEVNULL,
            stdout=asyncio.subprocess.DEVNULL,
            stderr=asyncio.subprocess.DEVNULL,
            start_new_session=True,
        )
        try:
            async with asyncio.timeout(60):
                status = await process.wait()
        except BaseException:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            await process.wait()
            raise
        if status:
            raise VmError("host VM preparation command failed")

    async def spawn(self, argv: list[str], console: Path) -> asyncio.subprocess.Process:
        with console.open("xb") as stream:
            return await asyncio.create_subprocess_exec(
                *argv,
                stdin=asyncio.subprocess.DEVNULL,
                stdout=stream,
                stderr=stream,
                start_new_session=True,
            )


def verify_pin(path: Path, expected: str) -> None:
    try:
        if path.is_symlink() or not path.is_file():
            raise VmError("pinned VM file unavailable")
        with path.open("rb") as stream:
            actual = hashlib.file_digest(stream, "sha256").hexdigest()
    except OSError:
        raise VmError("pinned VM file unavailable") from None
    if actual != expected:
        raise VmError("pinned VM file digest mismatch")


def verify_kvm(path: Path) -> None:
    fd = None
    try:
        if not stat.S_ISCHR(path.stat().st_mode):
            raise VmError("working /dev/kvm required")
        fd = os.open(path, os.O_RDWR | os.O_CLOEXEC)
        if fcntl.ioctl(fd, 0xAE00, 0) != 12:
            raise VmError("unsupported KVM API")
    except OSError:
        raise VmError("working /dev/kvm required") from None
    finally:
        if fd is not None:
            os.close(fd)


def vm_config(vm_id: str, spec: VmSpec, network: NetworkPlan | None = None) -> dict:
    boot = (
        f"console=ttyS0 reboot=k panic=1 pci=off root=/dev/vda ro "
        f"proof_vm={vm_id} proof_topic={spec.topic_id} proof_image={spec.image_digest} "
        f"proof_kind={spec.kind}"
    )
    if network is not None:
        if spec.kind != "topic":
            raise VmError("experiment VMs may not have a network")
        boot += f" ip={network.guest}::{network.host}:255.255.255.252::eth0:off"
    return {
        "boot-source": {"kernel_image_path": "/vmlinux", "boot_args": boot},
        "drives": [
            {
                "drive_id": "rootfs",
                "path_on_host": "/rootfs.ext4",
                "is_root_device": True,
                "is_read_only": True,
            },
            {
                "drive_id": "scratch",
                "path_on_host": "/scratch.ext4",
                "is_root_device": False,
                "is_read_only": False,
            },
        ],
        "machine-config": {
            "vcpu_count": spec.resources.vcpus,
            "mem_size_mib": spec.resources.mem_mib,
            "smt": False,
        },
        "vsock": {"guest_cid": 3, "uds_path": "/v.sock"},
        "network-interfaces": []
        if network is None
        else [
            {
                "iface_id": "eth0",
                "host_dev_name": network.tap,
                "guest_mac": f"AA:FC:00:00:{network.index >> 8:02x}:{network.index & 255:02x}",
            }
        ],
    }


class FirecrackerHypervisor:
    def __init__(
        self,
        config: HostConfig,
        *,
        commands: CommandRunner | None = None,
        transport: VsockTransport | None = None,
        kvm_check=verify_kvm,
    ):
        self.config = config
        self.commands = commands or SystemCommands()
        self.transport = transport or VsockTransport(boot_timeout=config.boot_timeout)
        self._processes: dict[str, asyncio.subprocess.Process] = {}
        self.kvm_check = kvm_check

    def _directory(self, vm_id: str) -> Path:
        if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,95}", vm_id):
            raise VmError("invalid VM identifier", 400)
        return self.config.jail_root / self.config.firecracker.name / vm_id

    async def ready(self) -> None:
        cfg = self.config
        self.kvm_check(cfg.kvm)
        if not all(
            path.is_file() and os.access(path, os.X_OK) for path in (cfg.firecracker, cfg.jailer)
        ):
            raise VmError("Firecracker and jailer executables required")
        verify_pin(cfg.kernel, cfg.kernel_digest)

    def prepare(self, job: ExecuteRequest) -> ExecuteRequest:
        if job.action.phase not in {"preflight", "experiment"}:
            return job
        digest = job.params.get("experiment_pack_digest", "").removeprefix("sha256:")
        if not re.fullmatch(r"[0-9a-f]{64}", digest) or self.config.pack_dir is None:
            raise VmError("signed experiment pack is not installed")
        path = self.config.pack_dir / f"{digest}.tar"
        verify_pin(path, digest)
        with path.open("rb") as stream:
            raw = stream.read(MAX_ARTIFACT + 1)
        verify_artifact(raw, digest, limit=MAX_ARTIFACT)
        return job.model_copy(update={"pack_b64": base64.b64encode(raw).decode()})

    def install_setup(self, raw: bytes, digest: str) -> None:
        directory = self.config.pack_dir
        if directory is None or directory.is_symlink():
            raise VmError("private setup pack directory required")
        directory.mkdir(parents=True, mode=0o700, exist_ok=True)
        if directory.stat().st_mode & 0o077:
            raise VmError("setup pack directory must be private (0700)")
        verify_artifact(raw, digest, limit=MAX_ARTIFACT)
        target = directory / f"{digest}.tar"
        try:
            fd = os.open(target, os.O_CREAT | os.O_EXCL | os.O_WRONLY | os.O_NOFOLLOW, 0o600)
        except FileExistsError:
            verify_pin(target, digest)
            return
        with os.fdopen(fd, "wb") as stream:
            stream.write(raw)
            stream.flush()
            os.fsync(stream.fileno())

    async def boot(self, vm_id: str, spec: VmSpec) -> int:
        await self.ready()
        cfg = self.config
        source = cfg.images.get(spec.image_digest)
        if source is None:
            raise VmError("rootfs digest not installed")
        verify_pin(source, spec.image_digest)
        directory = self._directory(vm_id)
        root = directory / "root"
        root.mkdir(parents=True, exist_ok=False, mode=0o700)
        (root / "run").mkdir(mode=0o700)
        try:
            # Re-hash staged bytes so a source change during copying cannot change the pin.
            for origin, name, digest in (
                (cfg.kernel, "vmlinux", cfg.kernel_digest),
                (source, "rootfs.ext4", spec.image_digest),
            ):
                await self.commands.run(
                    ["cp", "--reflink=auto", "--", str(origin), str(root / name)]
                )
                verify_pin(root / name, digest)
            scratch = root / "scratch.ext4"
            with scratch.open("xb") as stream:
                stream.truncate(spec.resources.disk_mib * 1024 * 1024)
            await self.commands.run(["mkfs.ext4", "-q", "-F", str(scratch)])
            network = None
            if spec.kind == "topic" and cfg.topic_egress:
                used = {
                    json.loads(path.read_text())["index"]
                    for path in (cfg.jail_root / cfg.firecracker.name).glob("*/network.json")
                }
                index = next(
                    (candidate for candidate in range(1, 16383) if candidate not in used), None
                )
                if index is None:
                    raise VmError("topic network capacity exhausted")
                network = NetworkPlan(index, cfg.uplink, cfg.topic_egress)
                (directory / "network.json").write_text(json.dumps({"index": index}))
                await network.up(self.commands, directory, cfg.uid)
            (root / "vm-config.json").write_text(json.dumps(vm_config(vm_id, spec, network)))
            await self.commands.run(["chown", "-R", f"{cfg.uid}:{cfg.gid}", str(root)])
            argv = [
                str(cfg.jailer),
                "--id",
                vm_id,
                "--exec-file",
                str(cfg.firecracker),
                "--uid",
                str(cfg.uid),
                "--gid",
                str(cfg.gid),
                "--chroot-base-dir",
                str(cfg.jail_root),
                "--",
                "--config-file",
                "/vm-config.json",
                "--api-sock",
                "/run/firecracker.socket",
            ]
            process = await self.commands.spawn(argv, directory / "console.log")
            self._processes[vm_id] = process
            # Persist identity to safely reap a VM after the orchestrator itself restarts.
            (directory / "process.json").write_text(
                json.dumps({"pid": process.pid, "start": self._process_start(process.pid)})
            )
            return process.pid
        except BaseException:
            await asyncio.shield(self.teardown(vm_id, retain=True))
            raise

    async def execute(self, vm_id: str, spec: VmSpec, job: ExecuteRequest) -> GuestOutput:
        if spec.topic_id != job.context.topic_id or spec.image_digest != job.context.image_digest:
            raise VmError("topic_mismatch", 409)
        return await self.transport.execute(self._directory(vm_id) / "root" / "v.sock", job)

    def socket_for(self, vm_id: str) -> Path:
        return self._directory(vm_id) / "root" / "v.sock"

    def network_enabled(self, spec: VmSpec) -> bool:
        return spec.kind == "topic" and bool(self.config.topic_egress)

    @staticmethod
    def _process_start(pid: int) -> str | None:
        try:
            return Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[19]
        except (OSError, IndexError):
            return None

    async def teardown(self, vm_id: str, retain: bool) -> bool:
        directory = self._directory(vm_id)
        process = self._processes.pop(vm_id, None)
        if process is not None:
            if process.returncode is None:
                try:
                    process.kill()
                except ProcessLookupError:
                    pass
            try:
                async with asyncio.timeout(10):
                    await process.wait()
            except TimeoutError:
                self._processes[vm_id] = process
                return False
        elif (directory / "process.json").exists():
            try:
                identity = json.loads((directory / "process.json").read_text())
                pid, start = identity["pid"], identity["start"]
                current = self._process_start(pid)
                if current is not None:
                    if not start or current != start:
                        return False
                    os.kill(pid, signal.SIGKILL)
                    for _ in range(100):
                        if self._process_start(pid) != start:
                            break
                        await asyncio.sleep(0.1)
                    else:
                        return False
            except (OSError, ValueError, KeyError, TypeError):
                return False
        if not directory.exists():
            return True
        try:
            network_path = directory / "network.json"
            if network_path.exists():
                network = NetworkPlan(
                    json.loads(network_path.read_text())["index"],
                    self.config.uplink,
                    self.config.topic_egress,
                )
                await network.down(self.commands)
                network_path.unlink()
            if retain:
                self.config.retain_root.mkdir(parents=True, exist_ok=True, mode=0o700)
                destination = self.config.retain_root / vm_id
                if destination.exists():
                    return False
                directory.rename(destination)
                return destination.exists() and not directory.exists()
            shutil.rmtree(directory)
            return not directory.exists()
        except (OSError, VmError, ValueError, KeyError):
            return False
