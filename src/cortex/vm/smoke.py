"""Opt-in KVM lifecycle probe; no inference, scientific score or chain submission."""

from __future__ import annotations

import argparse
import asyncio
import hashlib
import json
import os
import sys
import tomllib
import uuid
from collections.abc import Callable
from pathlib import Path
from typing import Literal

from pydantic import Field, ValidationError

from cortex.rlm.models import Digest, StrictModel, VmAction, VmContext

from .firecracker import FirecrackerHypervisor, HostConfig
from .models import ExecuteRequest, Resources, VmError, VmSpec
from .runtime import Hypervisor

GUEST_PROBE = """import json, os, platform, sys
from pathlib import Path
from cortex.vm.guest import GuestIdentity
from cortex.vm.smoke import probe_scratch
identity = GuestIdentity.from_system()
nonce = sys.argv[1]
scratch_write_ok = probe_scratch(Path('/workspace'), nonce)
print(json.dumps({
    'vm_id': identity.vm_id, 'topic_id': identity.topic_id,
    'image_digest': identity.image_digest, 'kind': identity.kind,
    'nonce': nonce,
    'boot_id': Path('/proc/sys/kernel/random/boot_id').read_text().strip(),
    'kernel_release': platform.release(),
    'interfaces': sorted(p.name for p in Path('/sys/class/net').iterdir()),
    'root_read_only': bool(os.statvfs('/').f_flag & os.ST_RDONLY),
    'scratch_separate': os.stat('/workspace').st_dev != os.stat('/').st_dev,
    'scratch_write_ok': scratch_write_ok,
}, sort_keys=True))
"""


def probe_scratch(workspace: Path, nonce: str) -> bool:
    probe = workspace / ("smoke-probe-" + nonce)
    with probe.open("x") as stream:
        stream.write(nonce)
        stream.flush()
        os.fsync(stream.fileno())
    verified = probe.read_text() == nonce
    probe.unlink()
    return verified


class GuestProbe(StrictModel):
    vm_id: str
    topic_id: str
    image_digest: Digest
    kind: Literal["topic", "experiment"]
    nonce: str = Field(pattern=r"^[0-9a-f]{32}$")
    boot_id: str = Field(pattern=r"^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$")
    kernel_release: str = Field(min_length=1, max_length=256)
    interfaces: list[str] = Field(max_length=16)
    root_read_only: bool
    scratch_separate: bool
    scratch_write_ok: bool


def _configuration(
    path: Path, state_dir: Path, image_digest: str | None, resources: Resources
) -> tuple[HostConfig, str]:
    with path.open("rb") as stream:
        config = tomllib.load(stream)
    host, images = config["host"], config["images"]
    if image_digest is None:
        if len(images) != 1:
            raise ValueError("select exactly one installed image with --image-digest")
        image_digest = next(iter(images))
    image_digest = image_digest.removeprefix("sha256:")
    if image_digest not in images:
        raise ValueError("selected image is not installed in the supplied configuration")
    caps = Resources.model_validate(config.get("caps", {}))
    if any(
        getattr(resources, name) > getattr(caps, name) for name in ("vcpus", "mem_mib", "disk_mib")
    ):
        raise ValueError("smoke resources exceed configured host ceilings")
    installed = {
        "kernel": Path(host["kernel"]),
        "rootfs": Path(images[image_digest]),
        "firecracker": Path(host.get("firecracker", "/usr/local/bin/firecracker")),
        "jailer": Path(host.get("jailer", "/usr/local/bin/jailer")),
    }
    if not all(value.is_absolute() for value in installed.values()):
        raise ValueError("installed VM artifact paths must be absolute")
    socket_path = (
        state_dir / "jails" / installed["firecracker"].name / ("smoke-" + "0" * 32) / "root/v.sock"
    )
    # Linux sun_path is 108 bytes including the pathname's terminating NUL.
    if len(os.fsencode(socket_path)) > 107:
        raise ValueError("smoke UNIX socket path exceeds 107 bytes; use a shorter --state-dir")
    return (
        HostConfig(
            kernel=installed["kernel"],
            kernel_digest=host["kernel_digest"],
            images={image_digest: installed["rootfs"]},
            firecracker=installed["firecracker"],
            jailer=installed["jailer"],
            uid=host.get("uid", 10000),
            gid=host.get("gid", 10000),
            jail_root=state_dir / "jails",
            retain_root=state_dir / "retained",
            pack_dir=state_dir / "packs",
            # Both probes are offline; do not install or change host firewall rules.
            topic_egress=(),
        ),
        image_digest,
    )


def _fresh_state(path: Path) -> None:
    if not path.is_absolute() or path != path.resolve():
        raise ValueError("state directory must be an absolute path without symlink components")
    path.parent.resolve(strict=True)
    path.mkdir(mode=0o700, exist_ok=False)
    path.chmod(0o700)


def _store(path: Path, value: dict) -> None:
    temporary = path.with_suffix(".pending")
    descriptor = os.open(temporary, os.O_CREAT | os.O_EXCL | os.O_WRONLY | os.O_NOFOLLOW, 0o600)
    with os.fdopen(descriptor, "w") as stream:
        json.dump(value, stream, sort_keys=True, indent=2)
        stream.flush()
        os.fsync(stream.fileno())
    temporary.replace(path)
    descriptor = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


async def _probe(hypervisor: Hypervisor, vm_id: str, spec: VmSpec) -> GuestProbe:
    nonce = uuid.uuid4().hex
    job = ExecuteRequest(
        execution_id=f"smoke-{nonce}",
        context=VmContext(
            topic_id=spec.topic_id,
            job_id=f"smoke-{nonce}",
            image_digest=spec.image_digest,
            purpose="setup",
        ),
        action=VmAction(
            operation="run",
            phase="inspect",
            argv=["/opt/cortex/bin/python", "-c", GUEST_PROBE, nonce],
            timeout_seconds=30,
        ),
    )
    output = await hypervisor.execute(vm_id, spec, job)
    if (
        output.context != job.context
        or output.execution_id != job.execution_id
        or output.exit_code != 0
        or output.report_digest != hashlib.sha256(output.stdout_tail.encode()).hexdigest()
    ):
        raise VmError("smoke guest output binding failed")
    try:
        probe = GuestProbe.model_validate_json(output.stdout_tail)
    except ValidationError:
        raise VmError("smoke guest probe is invalid") from None
    if (
        probe.vm_id != vm_id
        or probe.topic_id != spec.topic_id
        or probe.image_digest != spec.image_digest
        or probe.kind != spec.kind
        or probe.nonce != nonce
    ):
        raise VmError("smoke guest identity mismatch")
    if probe.interfaces != ["lo"]:
        raise VmError("smoke guest has a non-loopback network interface")
    if not (probe.root_read_only and probe.scratch_separate and probe.scratch_write_ok):
        raise VmError("smoke guest filesystem isolation failed")
    return probe


async def run_smoke(
    *,
    config_path: Path,
    state_dir: Path,
    image_digest: str | None = None,
    resources: Resources | None = None,
    hypervisor_factory: Callable[[HostConfig], Hypervisor] | None = None,
) -> dict:
    """Use fresh private jails only; injected test boundaries are labelled as such."""
    resources = resources or Resources(vcpus=1, mem_mib=1024, disk_mib=16384)
    config, selected = _configuration(config_path, state_dir, image_digest, resources)
    _fresh_state(state_dir)
    hypervisor = (hypervisor_factory or FirecrackerHypervisor)(config)
    topic_id = "smoke-" + uuid.uuid4().hex
    journal: dict = {
        "scope": "vm-lifecycle-only",
        "boundary": "firecracker-vsock" if hypervisor_factory is None else "injected-hypervisor",
        "topic_id": topic_id,
        "image_digest": selected,
        "kernel_digest": config.kernel_digest,
        "resources": resources.model_dump(mode="json"),
        "state": "pending",
        "vms": [],
    }
    state_path = state_dir / "smoke.json"
    _store(state_path, journal)
    probes: list[GuestProbe] = []
    attempted: list[str] = []

    async def cleanup() -> None:
        unconfirmed = []
        for vm_id in reversed(attempted):
            try:
                confirmed = await hypervisor.teardown(vm_id, retain=False)
            except Exception:
                confirmed = False
            for row in journal["vms"]:
                if row["vm_id"] == vm_id:
                    row["teardown_confirmed"] = confirmed
            if not confirmed:
                unconfirmed.append(vm_id)
        journal["state"] = "teardown-unconfirmed" if unconfirmed else "destroyed"
        _store(state_path, journal)
        if unconfirmed:
            raise VmError("smoke VM teardown unconfirmed; inspect private smoke state")

    try:
        await hypervisor.ready()
        for kind in ("topic", "experiment"):
            spec = VmSpec(topic_id=topic_id, image_digest=selected, kind=kind, resources=resources)
            vm_id = "smoke-" + uuid.uuid4().hex
            attempted.append(vm_id)
            journal["vms"].append({"vm_id": vm_id, "kind": kind, "teardown_confirmed": False})
            journal["state"] = "booting"
            _store(state_path, journal)
            await hypervisor.boot(vm_id, spec)
            probe = await _probe(hypervisor, vm_id, spec)
            probes.append(probe)
            journal["vms"][-1]["probe"] = probe.model_dump(mode="json")
            journal["state"] = "probed"
            _store(state_path, journal)
        if len({probe.boot_id for probe in probes}) != 2:
            raise VmError("smoke probes did not run in distinct guest boots")
    except BaseException as error:
        journal["failure"] = error.reason if isinstance(error, VmError) else type(error).__name__
        raise
    finally:
        # Finish reaping even if the caller cancels again during cleanup.
        task = asyncio.create_task(cleanup())
        cancelled = False
        while not task.done():
            try:
                await asyncio.shield(task)
            except asyncio.CancelledError:
                cancelled = True
        task.result()
        if cancelled:
            raise asyncio.CancelledError
    journal["state"] = "passed"
    _store(state_path, journal)
    return journal


def main(argv: list[str] | None = None) -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--run-live", action="store_true", help="explicitly permit local KVM boot")
    parser.add_argument("--config", type=Path, required=True)
    parser.add_argument("--state-dir", type=Path, required=True, help="new private directory")
    parser.add_argument("--image-digest")
    parser.add_argument("--vcpus", type=int, default=1)
    parser.add_argument("--mem-mib", type=int, default=1024)
    parser.add_argument("--disk-mib", type=int, default=16384)
    args = parser.parse_args(argv)
    if not args.run_live:
        parser.error("--run-live is required; this command boots real local Firecracker VMs")
    try:
        result = asyncio.run(
            run_smoke(
                config_path=args.config,
                state_dir=args.state_dir,
                image_digest=args.image_digest,
                resources=Resources(vcpus=args.vcpus, mem_mib=args.mem_mib, disk_mib=args.disk_mib),
            )
        )
    except (OSError, ValueError, KeyError, TypeError, VmError) as error:
        print(
            f"VM smoke failed ({type(error).__name__}); inspect the private smoke state directory",
            file=sys.stderr,
        )
        raise SystemExit(1) from None
    print(json.dumps(result, indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
