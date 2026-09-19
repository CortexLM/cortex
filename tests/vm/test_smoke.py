"""Smoke lifecycle contract with a fake VM boundary; never live KVM evidence."""

import asyncio
import hashlib
import json
import os
import stat
import uuid
from pathlib import Path
from tempfile import TemporaryDirectory

import pytest

from cortex.vm.models import GuestOutput, Resources, VmError
from cortex.vm.smoke import main, probe_scratch, run_smoke


def test_guest_scratch_probe_does_not_collide_with_execution_directory(tmp_path):
    nonce = "ab" * 16
    execution = tmp_path / ("smoke-" + nonce)
    execution.mkdir()

    assert probe_scratch(tmp_path, nonce) is True

    assert list(tmp_path.iterdir()) == [execution]


class ProbeHypervisor:
    def __init__(self, config):
        self.config = config
        self.booted = []
        self.destroyed = []
        self.change = {}
        self.fail_boot = False
        self.confirm = True
        self.wait_probe = False
        self.wait_cleanup = False
        self.probe_entered = asyncio.Event()
        self.cleanup_entered = asyncio.Event()
        self.cleanup_release = asyncio.Event()

    async def ready(self):
        return None

    async def boot(self, vm_id, spec):
        self.booted.append((vm_id, spec))
        if self.fail_boot and spec.kind == "experiment":
            raise VmError("fixture boot failure")
        return 123

    async def execute(self, vm_id, spec, job):
        if self.wait_probe:
            self.probe_entered.set()
            await asyncio.Event().wait()
        probe = {
            "vm_id": vm_id,
            "topic_id": spec.topic_id,
            "image_digest": spec.image_digest,
            "kind": spec.kind,
            "nonce": job.action.argv[-1],
            "boot_id": str(uuid.UUID(int=len(self.booted))),
            "kernel_release": "fixture-kernel-not-live",
            "interfaces": ["lo"],
            "root_read_only": True,
            "scratch_separate": True,
            "scratch_write_ok": True,
        }
        if spec.kind == "experiment":
            probe.update(self.change)
        output = json.dumps(probe) + "\n"
        return GuestOutput(
            context=job.context,
            execution_id=job.execution_id,
            exit_code=0,
            stdout_tail=output,
            report_digest=hashlib.sha256(output.encode()).hexdigest(),
        )

    async def teardown(self, vm_id, retain):
        if self.wait_cleanup:
            self.cleanup_entered.set()
            await self.cleanup_release.wait()
        self.destroyed.append(vm_id)
        return self.confirm


@pytest.fixture
def smoke(tmp_path):
    config = tmp_path / "host.toml"
    config.write_text(
        '[host]\nkernel="/installed/kernel"\nkernel_digest="' + "aa" * 32 + '"\n'
        'state_db="/production/state.sqlite3"\njail_root="/production/jails"\n'
        'retain_root="/production/retained"\n'
        '[images]\n"' + "bb" * 32 + '"="/installed/image.ext4"\n'
        "[caps]\nvcpus=2\nmem_mib=2048\ndisk_mib=16384\n"
    )
    hypervisors = []

    def factory(config):
        hypervisor = ProbeHypervisor(config)
        hypervisors.append(hypervisor)
        return hypervisor

    with TemporaryDirectory(prefix="cvm-", dir="/tmp") as private_root:
        yield (
            {
                "config_path": config,
                "state_dir": Path(private_root) / "run",
                "hypervisor_factory": factory,
            },
            hypervisors,
        )


async def test_smoke_probes_distinct_guests_and_confirms_cleanup_without_using_production_state(
    smoke,
):
    kwargs, hypervisors = smoke

    report = await run_smoke(**kwargs)

    hypervisor = hypervisors[0]
    assert report["state"] == "passed"
    assert report["boundary"] == "injected-hypervisor"
    assert report["scope"] == "vm-lifecycle-only"
    assert report["resources"] == {"vcpus": 1, "mem_mib": 1024, "disk_mib": 16384}
    assert [row["kind"] for row in report["vms"]] == ["topic", "experiment"]
    assert all(row["teardown_confirmed"] for row in report["vms"])
    assert len({row["probe"]["boot_id"] for row in report["vms"]}) == 2
    assert hypervisor.destroyed == [vm_id for vm_id, _ in reversed(hypervisor.booted)]
    state_dir = kwargs["state_dir"]
    assert hypervisor.config.jail_root == state_dir / "jails"
    assert hypervisor.config.retain_root == state_dir / "retained"
    assert hypervisor.config.topic_egress == ()
    assert json.loads((state_dir / "smoke.json").read_text()) == report
    assert stat.S_IMODE(state_dir.stat().st_mode) == 0o700
    assert stat.S_IMODE((state_dir / "smoke.json").stat().st_mode) == 0o600


@pytest.mark.parametrize(
    "change,reason",
    [
        ({"vm_id": "other-vm"}, "identity mismatch"),
        ({"kind": "topic"}, "identity mismatch"),
        ({"nonce": "cc" * 16}, "identity mismatch"),
        ({"image_digest": "dd" * 32}, "identity mismatch"),
        ({"interfaces": ["lo", "eth0"]}, "network interface"),
        ({"root_read_only": False}, "filesystem isolation"),
        ({"scratch_separate": False}, "filesystem isolation"),
        ({"scratch_write_ok": False}, "filesystem isolation"),
        ({"boot_id": str(uuid.UUID(int=1))}, "distinct guest boots"),
    ],
)
async def test_invalid_experiment_probe_rejects_success_and_reaps_both_guests(
    smoke, change, reason
):
    kwargs, hypervisors = smoke
    original = kwargs["hypervisor_factory"]

    def factory(config):
        hypervisor = original(config)
        hypervisor.change = change
        return hypervisor

    with pytest.raises(VmError, match=reason):
        await run_smoke(**(kwargs | {"hypervisor_factory": factory}))

    assert len(hypervisors[0].destroyed) == 2
    state = json.loads((kwargs["state_dir"] / "smoke.json").read_text())
    assert state["state"] == "destroyed"
    assert reason in state["failure"]
    assert all(row["teardown_confirmed"] for row in state["vms"])


async def test_failed_second_boot_is_owned_before_boot_and_reaps_both_guests(smoke):
    kwargs, hypervisors = smoke
    original = kwargs["hypervisor_factory"]

    def factory(config):
        hypervisor = original(config)
        hypervisor.fail_boot = True
        return hypervisor

    with pytest.raises(VmError, match="fixture boot failure"):
        await run_smoke(**(kwargs | {"hypervisor_factory": factory}))

    assert set(hypervisors[0].destroyed) == {vm_id for vm_id, _ in hypervisors[0].booted}
    assert len(hypervisors[0].destroyed) == 2


async def test_repeated_cancellation_waits_for_confirmed_vm_cleanup(smoke):
    kwargs, hypervisors = smoke
    original = kwargs["hypervisor_factory"]
    created = asyncio.Event()

    def factory(config):
        hypervisor = original(config)
        hypervisor.wait_probe = hypervisor.wait_cleanup = True
        created.set()
        return hypervisor

    task = asyncio.create_task(run_smoke(**(kwargs | {"hypervisor_factory": factory})))
    await created.wait()
    hypervisor = hypervisors[0]
    await hypervisor.probe_entered.wait()
    task.cancel()
    await hypervisor.cleanup_entered.wait()
    task.cancel()
    hypervisor.cleanup_release.set()
    with pytest.raises(asyncio.CancelledError):
        await task

    assert hypervisor.destroyed == [hypervisor.booted[0][0]]
    state = json.loads((kwargs["state_dir"] / "smoke.json").read_text())
    assert state["state"] == "destroyed"
    assert state["vms"][0]["teardown_confirmed"] is True


async def test_teardown_refusal_prevents_success_and_still_attempts_every_vm(smoke):
    kwargs, hypervisors = smoke
    original = kwargs["hypervisor_factory"]

    def factory(config):
        hypervisor = original(config)
        hypervisor.confirm = False
        return hypervisor

    with pytest.raises(VmError, match="teardown unconfirmed"):
        await run_smoke(**(kwargs | {"hypervisor_factory": factory}))

    assert len(hypervisors[0].destroyed) == 2
    state = json.loads((kwargs["state_dir"] / "smoke.json").read_text())
    assert state["state"] == "teardown-unconfirmed"
    assert all(not row["teardown_confirmed"] for row in state["vms"])


async def test_existing_state_is_refused_without_touching_it_or_booting(smoke):
    kwargs, hypervisors = smoke
    state_dir = kwargs["state_dir"]
    state_dir.mkdir()
    sentinel = state_dir / "existing-state"
    sentinel.write_text("preserve")

    with pytest.raises(FileExistsError):
        await run_smoke(**kwargs)

    assert sentinel.read_text() == "preserve"
    assert list(state_dir.iterdir()) == [sentinel]
    assert hypervisors == []


async def test_resource_request_above_configured_ceiling_is_rejected_before_boot(smoke):
    kwargs, hypervisors = smoke

    with pytest.raises(ValueError, match="ceilings"):
        await run_smoke(**kwargs, resources=Resources(vcpus=3, mem_mib=1024, disk_mib=16384))

    assert hypervisors == []
    assert not kwargs["state_dir"].exists()


async def test_multiple_images_require_explicit_selection(smoke):
    kwargs, hypervisors = smoke
    path = kwargs["config_path"]
    path.write_text(path.read_text().replace("[caps]", '"' + "cc" * 32 + '"="/other.ext4"\n[caps]'))

    with pytest.raises(ValueError, match="select exactly one"):
        await run_smoke(**kwargs)
    assert hypervisors == []

    report = await run_smoke(**kwargs, image_digest="sha256:" + "cc" * 32)
    assert report["image_digest"] == "cc" * 32


@pytest.mark.parametrize("socket_bytes", [107, 108])
@pytest.mark.parametrize("prefix", ["a", "\u00e9"])
@pytest.mark.parametrize("binary", ["firecracker", "firecracker-reviewed"])
async def test_linux_socket_path_byte_limit_is_checked_before_state_or_boot(
    smoke, socket_bytes, prefix, binary
):
    kwargs, hypervisors = smoke
    config = kwargs["config_path"]
    config.write_text(
        config.read_text().replace("[host]", f'[host]\nfirecracker="/installed/{binary}"')
    )
    parent = kwargs["state_dir"].parent
    suffix = f"/jails/{binary}/smoke-" + "0" * 32 + "/root/v.sock"
    name_bytes = socket_bytes - len(os.fsencode(parent)) - 1 - len(os.fsencode(suffix))
    name = prefix + "x" * (name_bytes - len(os.fsencode(prefix)))
    state_dir = parent / name
    assert len(os.fsencode(str(state_dir) + suffix)) == socket_bytes

    if socket_bytes == 108:
        with pytest.raises(ValueError, match="UNIX socket path exceeds 107 bytes"):
            await run_smoke(**(kwargs | {"state_dir": state_dir}))

        assert hypervisors == []
        assert not state_dir.exists()
    else:
        report = await run_smoke(**(kwargs | {"state_dir": state_dir}))

        assert report["state"] == "passed"
        assert all(row["teardown_confirmed"] for row in report["vms"])
        for vm_id, _ in hypervisors[0].booted:
            socket_path = state_dir / "jails" / binary / vm_id / "root/v.sock"
            assert len(os.fsencode(socket_path)) == 107


def test_cli_requires_explicit_live_opt_in_before_reading_configuration(tmp_path, capsys):
    with pytest.raises(SystemExit) as failure:
        main(["--config", str(tmp_path / "missing"), "--state-dir", str(tmp_path / "smoke")])

    assert failure.value.code == 2
    assert "--run-live is required" in capsys.readouterr().err
    assert not (tmp_path / "smoke").exists()
