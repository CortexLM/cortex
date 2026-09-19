"""The local SSH boundary only trusts operator-pinned host keys and credentials."""

import asyncio
import os
import stat
from pathlib import Path

import pytest

from cortex.proof.executor import HarvestFailure, LiumLease
from cortex.proof.lium import OpenSshTransport, SshTarget

from .test_lium_adapter import (
    IMAGE,
    TEMPLATE_NAME,
    FakeGuestWire,
    FakeSsh,
    adapter_for,
    execution,
    harvest_request,
    pod_row,
)
from .test_lium_adapter import lium_files as lium_files


class CapturedProcess:
    def __init__(self, *, returncode=0, stderr=b"", blocked=False):
        self.returncode = None if blocked else returncode
        self.stdin = None
        self.stdout, self.stderr = asyncio.StreamReader(), asyncio.StreamReader()
        self.stdout.feed_data(b"verified remote output")
        self.stderr.feed_data(stderr)
        self.stdout.feed_eof()
        self.stderr.feed_eof()
        self.finished = asyncio.Event()
        if not blocked:
            self.finished.set()

    async def wait(self):
        await self.finished.wait()
        return self.returncode

    def kill(self):
        self.returncode = -9
        self.finished.set()


@pytest.fixture
def ssh_files(tmp_path):
    key, hosts = tmp_path / "identity", tmp_path / "known_hosts"
    key.write_bytes(b"private-operator-key-fixture")
    key.chmod(0o600)
    hosts.write_bytes(b"[pod.example]:2200 ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAITest\n")
    hosts.chmod(0o644)
    return key, hosts


@pytest.fixture
def subprocess_boundary(monkeypatch):
    calls = []

    async def spawn(*argv, **kwargs):
        calls.append((argv, kwargs))
        return CapturedProcess()

    monkeypatch.setattr("cortex.proof.lium.asyncio.create_subprocess_exec", spawn)
    return calls


async def test_unconfigured_host_trust_refuses_before_connecting(ssh_files, subprocess_boundary):
    key, _ = ssh_files
    transport = OpenSshTransport(key)

    with pytest.raises(HarvestFailure, match="known hosts"):
        await transport.run(SshTarget("root", "pod.example", 2200), "true", timeout_seconds=1)

    assert not subprocess_boundary


async def test_ssh_trust_and_identity_ignore_ambient_settings(ssh_files, monkeypatch):
    key, hosts = ssh_files
    monkeypatch.setenv("SSH_AUTH_SOCK", "/untrusted/agent")
    monkeypatch.setenv("SSH_ASKPASS", "/untrusted/askpass")
    monkeypatch.setenv("LD_PRELOAD", "/untrusted/library.so")
    calls = []
    snapshots = []

    def inspect_material(argv):
        identity = Path(argv[argv.index("-i") + 1])
        options = dict(
            item.split("=", 1) for index, item in enumerate(argv) if argv[index - 1] == "-o"
        )
        trusted = Path(options["UserKnownHostsFile"])
        assert identity.read_bytes() == key.read_bytes()
        assert trusted.read_bytes() == hosts.read_bytes()
        assert stat.S_IMODE(identity.stat().st_mode) == 0o600
        assert stat.S_IMODE(trusted.stat().st_mode) == 0o600
        assert stat.S_IMODE(identity.parent.stat().st_mode) == 0o700
        snapshots.extend([identity, trusted])

    async def spawn(*argv, **kwargs):
        calls.append((argv, kwargs))
        inspect_material(argv)
        return CapturedProcess()

    monkeypatch.setattr("cortex.proof.lium.asyncio.create_subprocess_exec", spawn)
    result = await OpenSshTransport(key, known_hosts_file=hosts).run(
        SshTarget("root", "pod.example", 2200), "printf verified", timeout_seconds=1
    )

    assert result.stdout == "verified remote output"
    argv, kwargs = calls[0]
    assert argv[0] == "/usr/bin/ssh"
    assert argv[argv.index("-F") + 1] == "/dev/null"
    assert argv[-2:] == ("root@pod.example", "printf verified")
    options = dict(item.split("=", 1) for index, item in enumerate(argv) if argv[index - 1] == "-o")
    assert (
        options.items()
        >= {
            "StrictHostKeyChecking": "yes",
            "GlobalKnownHostsFile": "/dev/null",
            "VerifyHostKeyDNS": "no",
            "UpdateHostKeys": "no",
            "KnownHostsCommand": "none",
            "IdentityAgent": "none",
            "IdentitiesOnly": "yes",
            "ProxyCommand": "none",
            "ProxyJump": "none",
            "ControlMaster": "no",
            "ControlPath": "none",
            "ForwardAgent": "no",
            "ForwardX11": "no",
            "ClearAllForwardings": "yes",
            "PermitLocalCommand": "no",
        }.items()
    )
    assert kwargs["env"] == {"PATH": "/usr/bin:/bin", "LC_ALL": "C"}
    assert all(not path.exists() for path in snapshots)


@pytest.mark.parametrize("kind", ["key", "hosts"])
@pytest.mark.parametrize(
    "invalid", ["missing", "empty", "directory", "symlink", "fifo", "mode", "hardlink", "oversized"]
)
async def test_invalid_ssh_files_refuse_before_subprocess(
    ssh_files, subprocess_boundary, kind, invalid
):
    key, hosts = ssh_files
    path = key if kind == "key" else hosts
    original = path.read_bytes()
    path.unlink()
    if invalid == "empty":
        path.touch(mode=0o600)
    elif invalid == "directory":
        path.mkdir()
    elif invalid == "symlink":
        path.symlink_to(hosts if kind == "key" else key)
    elif invalid == "fifo":
        os.mkfifo(path, 0o600)
    elif invalid == "mode":
        path.write_bytes(original)
        path.chmod(0o644 if kind == "key" else 0o666)
    elif invalid == "hardlink":
        source = path.with_suffix(".source")
        source.write_bytes(original)
        source.chmod(0o600)
        path.hardlink_to(source)
    elif invalid == "oversized":
        path.write_bytes(b"x" * (16_385 if kind == "key" else 1024 * 1024 + 1))
        path.chmod(0o600)

    with pytest.raises(HarvestFailure, match="SSH"):
        await OpenSshTransport(key, known_hosts_file=hosts).run(
            SshTarget("root", "pod.example", 2200), "true", timeout_seconds=1
        )

    assert not subprocess_boundary


@pytest.mark.parametrize(
    "target",
    [
        SshTarget("-F", "pod.example", 22),
        SshTarget("root", "pod.example -oProxyCommand=bad", 22),
        SshTarget("root", "bad\nhost", 22),
        SshTarget("root", "pod.example", True),
        SshTarget("root", "pod.example", 65536),
        SshTarget("root", "pod.example", 0),
    ],
)
async def test_direct_transport_revalidates_target(ssh_files, subprocess_boundary, target):
    key, hosts = ssh_files

    with pytest.raises(HarvestFailure, match="SSH target"):
        await OpenSshTransport(key, known_hosts_file=hosts).run(target, "true", timeout_seconds=1)

    assert not subprocess_boundary


async def test_changing_original_paths_during_spawn_does_not_replace_trusted_material(
    ssh_files, monkeypatch
):
    key, hosts = ssh_files
    expected_key, expected_hosts = key.read_bytes(), hosts.read_bytes()

    def replace_and_inspect_material(argv):
        key.write_bytes(b"replaced identity")
        hosts.write_bytes(b"replaced trust")
        identity = Path(argv[argv.index("-i") + 1])
        options = dict(
            item.split("=", 1) for index, item in enumerate(argv) if argv[index - 1] == "-o"
        )
        assert identity.read_bytes() == expected_key
        assert Path(options["UserKnownHostsFile"]).read_bytes() == expected_hosts

    async def spawn(*argv, **kwargs):
        replace_and_inspect_material(argv)
        return CapturedProcess()

    monkeypatch.setattr("cortex.proof.lium.asyncio.create_subprocess_exec", spawn)
    await OpenSshTransport(key, known_hosts_file=hosts).run(
        SshTarget("root", "pod.example", 2200), "true", timeout_seconds=1
    )


async def test_host_key_failure_never_retries_with_weaker_trust(ssh_files, monkeypatch):
    key, hosts = ssh_files
    calls = []

    async def spawn(*argv, **kwargs):
        calls.append(argv)
        return CapturedProcess(returncode=255, stderr=b"Host key verification failed")

    monkeypatch.setattr("cortex.proof.lium.asyncio.create_subprocess_exec", spawn)

    with pytest.raises(HarvestFailure, match="exit 255"):
        await OpenSshTransport(key, known_hosts_file=hosts).run(
            SshTarget("root", "pod.example", 2200), "true", timeout_seconds=1
        )

    assert len(calls) == 1


async def test_cancellation_kills_process_and_removes_material_snapshots(ssh_files, monkeypatch):
    key, hosts = ssh_files
    entered = asyncio.Event()
    process = CapturedProcess(blocked=True)
    snapshots = []

    async def spawn(*argv, **kwargs):
        snapshots.append(Path(argv[argv.index("-i") + 1]))
        entered.set()
        return process

    monkeypatch.setattr("cortex.proof.lium.asyncio.create_subprocess_exec", spawn)
    request = asyncio.create_task(
        OpenSshTransport(key, known_hosts_file=hosts).run(
            SshTarget("root", "pod.example", 2200), "true", timeout_seconds=60
        )
    )
    await entered.wait()
    request.cancel()
    with pytest.raises(asyncio.CancelledError):
        await request

    assert process.returncode == -9
    assert all(not path.parent.exists() for path in snapshots)


@pytest.mark.parametrize("operation", ["probe", "rent", "execute"])
async def test_invalid_trust_refuses_before_provider_spending(lium_files, operation):
    request = harvest_request()
    lium_files["known_hosts"].chmod(0o666)

    def forbid_provider_io(request):
        pytest.fail("Invalid SSH trust must be rejected before contacting the provider")

    adapter, client = await adapter_for(
        lium_files,
        {},
        FakeSsh(),
        guest_wire=FakeGuestWire(execution(request)),
        handler=forbid_provider_io,
    )
    try:
        if operation == "probe":
            assert await adapter.probe() is False
        elif operation == "rent":
            with pytest.raises(HarvestFailure, match="known hosts"):
                await adapter.rent(request)
        else:
            lease = LiumLease(
                instance_id="pod-1", template_id=TEMPLATE_NAME, gpu_count=1, image_digest=IMAGE
            )
            with pytest.raises(HarvestFailure, match="known hosts"):
                await adapter.execute(lease, request)
    finally:
        await client.aclose()


@pytest.mark.parametrize(
    ("field", "value"),
    [
        ("request_commitment", "01" * 32),
        ("topic_digest", "01" * 32),
        ("environment_digest", "01" * 32),
        ("private_holdout_digest", "01" * 32),
        ("inference_offer_commitment", "01" * 32),
        ("teardown_confirmed", False),
        ("network_enabled", True),
        ("sandboxed", False),
    ],
)
async def test_adapter_refuses_unbound_or_unisolated_guest_report(lium_files, field, value):
    request = harvest_request()
    forged = execution(request).model_copy(update={field: value})
    adapter, client = await adapter_for(
        lium_files, {"pods": [pod_row()]}, FakeSsh(), guest_wire=FakeGuestWire(forged)
    )
    lease = LiumLease(
        instance_id="pod-1", template_id=TEMPLATE_NAME, gpu_count=1, image_digest=IMAGE
    )
    try:
        with pytest.raises(HarvestFailure, match="report binding mismatch"):
            await adapter.execute(lease, request)
    finally:
        await client.aclose()
