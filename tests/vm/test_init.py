"""Host boot binding drives guest networking; experiments cannot inherit an uplink."""

import pytest

from cortex.vm.init import main, mount_command, network_commands


def test_kernel_premounted_devtmpfs_is_remounted_instead_of_mounted_twice():
    mounts = "22 1 0:5 / /dev rw,relatime - devtmpfs devtmpfs rw,size=501760k\n"

    command = mount_command("devtmpfs", "/dev", "devtmpfs", mounts)

    assert command == ["mount", "-o", "remount,nosuid", "/dev"]


def test_unmounted_virtual_filesystem_receives_its_initial_mount():
    command = mount_command("proc", "/proc", "proc", "")

    assert command == ["mount", "-t", "proc", "-o", "nosuid", "proc", "/proc"]


def test_unexpected_filesystem_at_device_mount_is_refused():
    mounts = "22 1 0:5 / /dev rw,relatime - tmpfs tmpfs rw\n"

    with pytest.raises(ValueError, match="unexpected guest filesystem"):
        mount_command("devtmpfs", "/dev", "devtmpfs", mounts)


def test_guest_init_refuses_to_mount_anything_on_an_ordinary_host():
    with pytest.raises(SystemExit, match="PID 1"):
        main()


def test_topic_static_binding_installs_address_and_default_route():
    commands = network_commands(
        "topic",
        "console=ttyS0 ip=172.30.0.6::172.30.0.5:255.255.255.252::eth0:off",
        nic_present=True,
    )
    assert commands == [
        ["ip", "addr", "replace", "172.30.0.6/30", "dev", "eth0"],
        ["ip", "link", "set", "eth0", "up"],
        ["ip", "route", "replace", "default", "via", "172.30.0.5", "dev", "eth0"],
    ]


@pytest.mark.parametrize(
    "cmdline,nic", [("", True), ("ip=172.30.0.6::172.30.0.5:255.255.255.252::eth0:off", False)]
)
def test_experiment_refuses_an_interface_or_network_boot_binding(cmdline, nic):
    with pytest.raises(ValueError, match="must have no network"):
        network_commands("experiment", cmdline, nic_present=nic)


@pytest.mark.parametrize(
    "binding",
    [
        "dhcp",
        "172.30.0.6::10.0.0.1:255.255.255.252::eth0:off",
        "172.30.0.4::172.30.0.5:255.255.255.252::eth0:off",
    ],
)
def test_invalid_topic_network_binding_fails_before_any_ip_command(binding):
    with pytest.raises(ValueError):
        network_commands("topic", "ip=" + binding, nic_present=True)
