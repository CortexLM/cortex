"""Production jail construction via injected process/KVM boundaries; zero live VMs."""

import hashlib
import json
import shutil

import pytest
from pydantic import ValidationError

from cortex.vm.firecracker import FirecrackerHypervisor, HostConfig, vm_config
from cortex.vm.models import Resources, VmError, VmSpec
from cortex.vm.network import Egress, NetworkPlan


class FakeProcess:
    pid = 1_000_000_000
    returncode = None

    def kill(self):
        self.returncode = -9

    async def wait(self):
        return self.returncode


class RecordingCommands:
    def __init__(self):
        self.commands = []
        self.spawned = []

    async def run(self, argv):
        self.commands.append(argv)
        if argv[0] == "cp":
            shutil.copyfile(argv[-2], argv[-1])

    async def spawn(self, argv, console):
        self.spawned.append(argv)
        console.write_text("fake guest console")
        return FakeProcess()


def config(tmp_path):
    kernel, image = tmp_path / "kernel", tmp_path / "image.ext4"
    kernel.write_bytes(b"operator kernel")
    image.write_bytes(b"operator rootfs")
    firecracker, jailer = tmp_path / "firecracker", tmp_path / "jailer"
    firecracker.write_text("not invoked")
    jailer.write_text("not invoked")
    firecracker.chmod(0o755)
    jailer.chmod(0o755)
    return HostConfig(
        kernel=kernel,
        kernel_digest=hashlib.sha256(kernel.read_bytes()).hexdigest(),
        images={hashlib.sha256(image.read_bytes()).hexdigest(): image},
        jail_root=tmp_path / "jails",
        retain_root=tmp_path / "retained",
        firecracker=firecracker,
        jailer=jailer,
        kvm=tmp_path / "absent-kvm",
    )


async def test_real_backend_fails_closed_without_kvm(tmp_path):
    backend = FirecrackerHypervisor(config(tmp_path))

    with pytest.raises(VmError, match="/dev/kvm"):
        await backend.ready()


async def test_experiment_jail_is_pinned_networkless_and_removed_only_after_reap(tmp_path):
    cfg = config(tmp_path)
    commands = RecordingCommands()
    backend = FirecrackerHypervisor(cfg, commands=commands, kvm_check=lambda path: None)
    spec = VmSpec(topic_id="topic-a", image_digest=next(iter(cfg.images)), kind="experiment")

    await backend.boot("vm-one", spec)
    root = cfg.jail_root / "firecracker" / "vm-one" / "root"
    actual = json.loads((root / "vm-config.json").read_text())
    assert actual["network-interfaces"] == []
    assert actual["machine-config"] == {"vcpu_count": 16, "mem_size_mib": 32768, "smt": False}
    assert "--uid" in commands.spawned[0]
    assert not any(command[0] in {"sh", "bash", "ip", "nft"} for command in commands.commands)
    assert (root / "scratch.ext4").stat().st_size == 32768 * 1024 * 1024

    assert await backend.teardown("vm-one", retain=False)
    assert not root.exists()


async def test_rootfs_changed_after_pinning_never_reaches_jailer(tmp_path):
    cfg = config(tmp_path)
    next(iter(cfg.images.values())).write_bytes(b"different operator image")
    commands = RecordingCommands()
    backend = FirecrackerHypervisor(cfg, commands=commands, kvm_check=lambda path: None)

    with pytest.raises(VmError, match="digest mismatch"):
        await backend.boot(
            "vm-one", VmSpec(topic_id="topic-a", image_digest=next(iter(cfg.images)))
        )

    assert commands.spawned == []


@pytest.mark.parametrize("changes", [{"vcpus": 17}, {"mem_mib": 32769}, {"disk_mib": 16383}])
def test_hard_experiment_resource_limits_are_not_clamped(changes):
    with pytest.raises(ValidationError):
        Resources(**changes)


def test_sister_cannot_receive_even_an_operator_supplied_network():
    plan = NetworkPlan(1, "eth0", (Egress("203.0.113.1/32", 443),))
    with pytest.raises(VmError, match="may not have a network"):
        vm_config(
            "vm-one", VmSpec(topic_id="topic-a", image_digest="ab" * 32, kind="experiment"), plan
        )


def test_topic_firewall_blocks_host_and_nonallowlisted_destinations():
    rules = NetworkPlan(1, "eth0", (Egress("203.0.113.1/32", 443),)).rules()

    assert 'iifname "pfc1" ip daddr 203.0.113.1/32 tcp dport 443 accept' in rules
    assert "chain input" in rules and 'iifname "pfc1" drop' in rules
    assert 'oifname "pfc1" ct state established,related accept' in rules
