"""Host diagnostics inspect real files and fake only the KVM syscall boundary."""

import errno
import hashlib
import json
import os
import shutil
import stat
from datetime import UTC, datetime
from pathlib import Path
from types import SimpleNamespace

import pytest
from cryptography import x509
from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric import ed25519
from cryptography.x509.oid import NameOID

from cortex.cli import main
from cortex.protocol.crypto import public_key
from cortex.rlm import AgentLimits
from cortex.rlm.offer import InferenceOffer, sign_offer
from cortex.vm.preflight import check_host


@pytest.fixture
def host_files(tmp_path):
    def write(name, value, mode=0o600):
        path = tmp_path / name
        path.write_bytes(value)
        path.chmod(mode)
        return str(path)

    key = ed25519.Ed25519PrivateKey.generate()
    subject = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "proof.example")])
    certificate = (
        x509.CertificateBuilder()
        .subject_name(subject)
        .issuer_name(subject)
        .public_key(key.public_key())
        .serial_number(1)
        .not_valid_before(datetime(2020, 1, 1, tzinfo=UTC))
        .not_valid_after(datetime(2099, 1, 1, tzinfo=UTC))
        .add_extension(x509.SubjectAlternativeName([x509.DNSName("proof.example")]), False)
        .sign(key, None)
    )
    seed = bytes([61]) * 32
    offer = sign_offer(
        InferenceOffer(
            model="deepseek/deepseek-v4.1-flash",
            limits=AgentLimits(),
            issuer_public_key=public_key(seed).hex(),
            status="open",
            valid_from_unix=0,
            valid_until_unix=4102444800,
            signature="0" * 128,
        ),
        seed,
    )
    config = {
        "host": {
            "kernel": write("kernel", b"fixture kernel"),
            "kernel_digest": hashlib.sha256(b"fixture kernel").hexdigest(),
            "firecracker": write("firecracker", b"fixture binary", 0o700),
            "jailer": write("jailer", b"fixture binary", 0o700),
            "token_file": write("token", b"private-bearer-value"),
            "state_db": str(tmp_path / "orchestrator.sqlite3"),
            "uid": 10000,
            "gid": 10000,
        },
        "images": {
            hashlib.sha256(b"fixture rootfs").hexdigest(): write("rootfs", b"fixture rootfs")
        },
        "caps": {"vcpus": 4, "mem_mib": 4096, "disk_mib": 16384},
        "tls": {
            "certificate": write("cert.pem", certificate.public_bytes(serialization.Encoding.PEM)),
            "private_key": write(
                "key.pem",
                key.private_bytes(
                    serialization.Encoding.PEM,
                    serialization.PrivateFormat.PKCS8,
                    serialization.NoEncryption(),
                ),
            ),
            "names": ["proof.example"],
        },
        "inference": {
            "api_key_file": write("provider-token", b"private-provider-value"),
            "offer_file": write("offer.json", offer.model_dump_json().encode()),
            "offer_commitment": offer.commitment(),
        },
    }
    for name in ("jail_root", "retain_root", "pack_dir"):
        directory = tmp_path / name
        directory.mkdir(mode=0o700)
        config["host"][name] = str(directory)
    return config, tmp_path / "host.toml"


def save_config(config, path):
    rows = []
    for section, values in config.items():
        if section == "egress":
            for value in values:
                rows.append("[[egress]]")
                rows.extend(
                    f"{json.dumps(name)} = {json.dumps(item)}" for name, item in value.items()
                )
            continue
        rows.append(f"[{section}]")
        rows.extend(f"{json.dumps(name)} = {json.dumps(value)}" for name, value in values.items())
    path.write_text("\n".join(rows))
    path.chmod(0o600)


@pytest.fixture
def kvm(monkeypatch, tmp_path):
    import cortex.vm.preflight as module

    actual_open, actual_fstat, actual_close = os.open, os.fstat, os.close
    actual_uid = os.geteuid()
    state = {"api": 12, "maximum": 16, "missing": set(), "closed": False, "euid": 0}
    tools = tmp_path / "host-tools"
    tools.mkdir(mode=0o700)
    for name in ("cp", "chown", "mkfs.ext4", "ip", "nft", "sysctl"):
        binary = tools / name
        binary.write_bytes(b"fixture executable; never invoked")
        binary.chmod(0o755)
    state["tools"] = tools

    def which(name):
        path = tools / name
        return str(path) if path.exists() else None

    def open_device(path, flags, *args, **kwargs):
        if Path(path) == Path("/dev/kvm"):
            assert flags & os.O_NOFOLLOW
            assert flags & os.O_RDWR
            if "error" in state:
                raise state["error"]
            return 123456789
        if Path(path) == Path("/dev/net/tun"):
            assert flags & os.O_NOFOLLOW
            assert flags & os.O_RDWR
            state["tun_opened"] = True
            if "tun_error" in state:
                raise state["tun_error"]
            return 123456788
        return actual_open(path, flags, *args, **kwargs)

    def device_stat(fd):
        if fd == 123456789:
            return SimpleNamespace(st_mode=state.get("mode", stat.S_IFCHR | 0o660))
        if fd == 123456788:
            return SimpleNamespace(st_mode=state.get("tun_mode", stat.S_IFCHR | 0o660))
        info = actual_fstat(fd)
        if info.st_uid == actual_uid:
            fields = list(info)
            fields[4] = state["euid"]
            return os.stat_result(fields)
        return info

    def ioctl(fd, request, argument=0):
        assert fd == 123456789
        assert request in {0xAE00, 0xAE03}, "diagnostic must not issue KVM_CREATE_VM"
        if request == 0xAE00:
            return state["api"]
        if argument == 66:
            return state["maximum"]
        return 0 if argument in state["missing"] else 1

    def close_device(fd):
        if fd == 123456789:
            state["closed"] = True
        elif fd == 123456788:
            state["tun_closed"] = True
        else:
            actual_close(fd)

    monkeypatch.setattr(module.os, "open", open_device)
    monkeypatch.setattr(module.os, "fstat", device_stat)
    monkeypatch.setattr(module.os, "close", close_device)
    monkeypatch.setattr(module.os, "geteuid", lambda: state["euid"])
    monkeypatch.setattr(module.fcntl, "ioctl", ioctl)
    monkeypatch.setattr(shutil, "which", which)
    return state


def test_valid_host_preflight_creates_no_runtime_state(host_files, kvm):
    config, path = host_files
    save_config(config, path)
    before = set(path.parent.rglob("*"))

    report = check_host(path)

    assert report.ready
    assert all(check.ok for check in report.checks)
    assert report.scope == "local prerequisites only; no VM boot or execution verified"
    assert set(path.parent.rglob("*")) == before
    assert kvm["closed"]
    assert "private-bearer-value" not in report.model_dump_json()
    assert "private-provider-value" not in report.model_dump_json()
    assert str(path.parent) not in report.model_dump_json()


@pytest.mark.parametrize(
    "change", ["missing", "permission", "regular", "api", "capability", "vcpus"]
)
def test_kvm_failures_refuse_readiness_and_close_device(host_files, kvm, change):
    config, path = host_files
    save_config(config, path)
    if change in {"missing", "permission"}:
        kvm["error"] = OSError(
            errno.ENOENT if change == "missing" else errno.EACCES, "private path"
        )
    elif change == "regular":
        kvm["mode"] = stat.S_IFREG | 0o600
    elif change == "api":
        kvm["api"] = 11
    elif change == "capability":
        kvm["missing"] = {36}
    else:
        kvm["maximum"] = 2

    report = check_host(path)

    assert not report.ready
    assert any(not item.ok and item.name.startswith("kvm") for item in report.checks)
    assert "private path" not in report.model_dump_json()
    assert kvm["closed"] == (change not in {"missing", "permission"})


@pytest.mark.parametrize(
    "change",
    [
        "kernel",
        "rootfs",
        "executable",
        "tls_mode",
        "tls_pair",
        "tls_san",
        "token",
        "offer",
        "caps",
        "concurrency",
        "state",
    ],
)
def test_invalid_host_inputs_fail_without_exposing_private_details(host_files, kvm, change):
    config, path = host_files
    if change == "kernel":
        Path(config["host"]["kernel"]).write_bytes(b"changed")
    elif change == "rootfs":
        Path(next(iter(config["images"].values()))).write_bytes(b"changed")
    elif change == "executable":
        Path(config["host"]["firecracker"]).chmod(0o600)
    elif change == "tls_mode":
        Path(config["tls"]["private_key"]).chmod(0o644)
    elif change == "tls_pair":
        Path(config["tls"]["private_key"]).write_bytes(
            ed25519.Ed25519PrivateKey.generate().private_bytes(
                serialization.Encoding.PEM,
                serialization.PrivateFormat.PKCS8,
                serialization.NoEncryption(),
            )
        )
    elif change == "tls_san":
        config["tls"]["names"] = ["another.example"]
    elif change == "token":
        Path(config["host"]["token_file"]).chmod(0o644)
    elif change == "offer":
        config["inference"]["offer_commitment"] = "00" * 32
    elif change == "caps":
        config["caps"]["vcpus"] = 17
    elif change == "concurrency":
        config["host"]["max_experiments"] = 0
    else:
        Path(config["host"]["pack_dir"]).chmod(0o755)
    save_config(config, path)

    report = check_host(path)

    assert not report.ready
    assert "private-bearer-value" not in report.model_dump_json()
    assert "private-provider-value" not in report.model_dump_json()
    assert str(path.parent) not in report.model_dump_json()


def test_cli_check_prints_json_and_nonzero_status_without_serving(host_files, kvm, capsys):
    config, path = host_files
    save_config(config, path)
    kvm["api"] = 11

    with pytest.raises(SystemExit) as failure:
        main(["vm-host", "--config", str(path), "--check"])

    assert failure.value.code == 1
    report = json.loads(capsys.readouterr().out)
    assert not report["ready"]
    assert any(not item["ok"] and item["name"] == "kvm_api" for item in report["checks"])


def test_cli_success_does_not_start_a_server(host_files, kvm, capsys, monkeypatch):
    config, path = host_files
    save_config(config, path)

    def forbidden_server(*args, **kwargs):
        pytest.fail("preflight must not start uvicorn")

    monkeypatch.setattr("cortex.cli.uvicorn.run", forbidden_server)

    main(["vm-host", "--config", str(path), "--check"])

    assert json.loads(capsys.readouterr().out)["ready"]


@pytest.mark.parametrize("attack", ["symlink", "fifo", "oversized", "malformed"])
def test_configuration_failures_are_bounded_and_do_not_leak(host_files, kvm, attack):
    config, path = host_files
    save_config(config, path)
    original = path.read_bytes()
    path.unlink()
    if attack == "symlink":
        target = path.with_name("private-config")
        target.write_bytes(original)
        path.symlink_to(target)
    elif attack == "fifo":
        os.mkfifo(path, 0o600)
    elif attack == "oversized":
        with path.open("wb") as source:
            source.truncate(1024 * 1024 + 1)
    else:
        path.write_text("private-secret-invalid-config")

    report = check_host(path)

    assert not report.ready
    assert "private-secret" not in report.model_dump_json()
    assert str(path) not in report.model_dump_json()
    assert any(item.name == "configuration" and not item.ok for item in report.checks)


def test_kvm_group_access_does_not_replace_root_process_identity(host_files, kvm):
    config, path = host_files
    save_config(config, path)
    kvm["euid"] = 1001

    report = check_host(path)

    assert not report.ready
    assert any(item.name == "process_identity" and not item.ok for item in report.checks)


@pytest.mark.parametrize("name", ["cp", "chown", "mkfs.ext4"])
def test_missing_required_host_utility_refuses_readiness(host_files, kvm, name):
    config, path = host_files
    save_config(config, path)
    (kvm["tools"] / name).unlink()

    report = check_host(path)

    assert not report.ready
    assert any(item.name == "utility_" + name and not item.ok for item in report.checks)


def test_networkless_host_does_not_require_network_tools_or_tun(host_files, kvm):
    config, path = host_files
    save_config(config, path)
    for name in ("ip", "nft", "sysctl"):
        (kvm["tools"] / name).unlink()
    kvm["tun_error"] = OSError(errno.ENOENT, "no TUN device")

    report = check_host(path)

    assert report.ready
    assert "tun_opened" not in kvm


@pytest.mark.parametrize("missing", [None, "ip", "nft", "sysctl", "tun", "regular_tun"])
def test_topic_egress_requires_its_tools_and_openable_tun_device(host_files, kvm, missing):
    config, path = host_files
    config["egress"] = [{"cidr": "203.0.113.1/32", "port": 443}]
    save_config(config, path)
    if missing in {"ip", "nft", "sysctl"}:
        (kvm["tools"] / missing).unlink()
    elif missing == "tun":
        kvm["tun_error"] = OSError(errno.EACCES, "private TUN error")
    elif missing == "regular_tun":
        kvm["tun_mode"] = stat.S_IFREG | 0o600

    report = check_host(path)

    assert report.ready == (missing is None)
    assert {"utility_ip", "utility_nft", "utility_sysctl", "tun_device"}.issubset(
        item.name for item in report.checks
    )
    assert kvm.get("tun_closed", False) == (missing != "tun")
    assert "private TUN error" not in report.model_dump_json()
