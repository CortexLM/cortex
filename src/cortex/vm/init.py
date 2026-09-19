"""Guest PID 1 bootstrap; all mutable files live on the dedicated scratch disk."""

from __future__ import annotations

import ipaddress
import os
import stat
import subprocess
from pathlib import Path


def mount_command(source: str, target: str, kind: str, mountinfo: str) -> list[str]:
    for line in mountinfo.splitlines():
        fields, separator, filesystem = line.partition(" - ")
        values = fields.split()
        if len(values) > 4 and values[4] == target:
            if not separator or filesystem.split()[0] != kind:
                raise ValueError("unexpected guest filesystem at bootstrap mount")
            return ["mount", "-o", "remount,nosuid", target]
    return ["mount", "-t", kind, "-o", "nosuid", source, target]


def network_commands(kind: str, cmdline: str, *, nic_present: bool) -> list[list[str]]:
    """Apply the host's static kernel binding without depending on kernel IP autoconfig."""
    fields = [item.removeprefix("ip=") for item in cmdline.split() if item.startswith("ip=")]
    if kind != "topic":
        if nic_present or fields:
            raise ValueError("experiment guest must have no network interface")
        return []
    if not nic_present:
        if fields:
            raise ValueError("topic network binding has no interface")
        return []
    if len(fields) != 1:
        raise ValueError("topic interface requires one static network binding")
    parts = fields[0].split(":")
    if len(parts) != 7 or parts[1] or parts[4] or parts[5:] != ["eth0", "off"]:
        raise ValueError("invalid topic static network binding")
    interface = ipaddress.IPv4Interface(f"{parts[0]}/{parts[3]}")
    gateway = ipaddress.IPv4Address(parts[2])
    if (
        gateway not in interface.network
        or gateway == interface.ip
        or interface.ip in {interface.network.network_address, interface.network.broadcast_address}
        or gateway in {interface.network.network_address, interface.network.broadcast_address}
    ):
        raise ValueError("invalid topic gateway")
    return [
        ["ip", "addr", "replace", str(interface), "dev", "eth0"],
        ["ip", "link", "set", "eth0", "up"],
        ["ip", "route", "replace", "default", "via", str(gateway), "dev", "eth0"],
    ]


def main() -> None:
    if os.getpid() != 1 or os.geteuid() != 0:
        raise SystemExit("Proof guest init requires PID 1 and guest root")

    def run(*args):
        subprocess.run(args, check=True, stdin=subprocess.DEVNULL)

    for source, target, kind in (
        ("proc", "/proc", "proc"),
        ("sysfs", "/sys", "sysfs"),
        ("devtmpfs", "/dev", "devtmpfs"),
    ):
        try:
            mounted = Path("/proc/self/mountinfo").read_text()
        except FileNotFoundError:
            mounted = ""
        # Some kernels mount devtmpfs before PID 1 starts.
        run(*mount_command(source, target, kind, mounted))
    from cortex.vm.guest import GuestIdentity

    identity = GuestIdentity.from_system()
    if not stat.S_ISBLK(Path("/dev/vdb").stat().st_mode):
        raise SystemExit("Proof guest requires its dedicated writable disk")
    for target, mode in (("/run", "0755"), ("/tmp", "1777")):
        run("mount", "-t", "tmpfs", "-o", f"nosuid,nodev,mode={mode}", "tmpfs", target)
    Path("/dev/pts").mkdir(exist_ok=True)
    run("mount", "-t", "devpts", "-o", "nosuid,noexec,mode=620", "devpts", "/dev/pts")
    run("mount", "-t", "ext4", "-o", "noatime,nosuid,nodev", "/dev/vdb", "/workspace")
    Path("/workspace").chmod(0o700)
    run("ip", "link", "set", "lo", "up")
    for command in network_commands(
        identity.kind,
        Path("/proc/cmdline").read_text(),
        nic_present=Path("/sys/class/net/eth0").exists(),
    ):
        run(*command)
    resolver = Path("/etc/proof/resolver")
    if identity.kind == "topic" and resolver.is_file():
        address = str(ipaddress.IPv4Address(resolver.read_text().strip()))
        Path("/run/resolv.conf").write_text(f"nameserver {address}\n")
        run("mount", "--bind", "/run/resolv.conf", "/etc/resolv.conf")
    environment = {
        "PATH": "/opt/cortex/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
        "LANG": "C.UTF-8",
        "PYTHONDONTWRITEBYTECODE": "1",
        "PYTHONUNBUFFERED": "1",
    }
    os.execve(
        "/usr/bin/tini",
        ["tini", "-g", "--", "/opt/cortex/bin/python", "-m", "cortex.vm.guest_server"],
        environment,
    )


if __name__ == "__main__":
    main()
