"""Deployment contracts that keep signing roles and paid execution fail-closed."""

import runpy
import tomllib
from copy import deepcopy
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
CHECK = runpy.run_path(str(ROOT / "scripts/check_deploy.py"))
PINNED_IMAGE = "fixture.invalid/cortex@sha256:" + "a" * 64


@pytest.mark.parametrize(
    "image",
    [
        "python:3.12-slim-bookworm",
        "python:latest",
        "repo@sha256:" + "x" * 64,
        "repo@sha256:" + "a" * 63,
        "repo@sha256:" + "a" * 64 + ";echo unsafe",
    ],
)
def test_deployment_rejects_floating_or_malformed_image_pins(image):
    with pytest.raises(ValueError):
        CHECK["validate_pin"](image)


def validator_config():
    return {
        "services": {
            "validator": {
                "image": PINNED_IMAGE,
                "init": True,
                "read_only": True,
                "restart": "unless-stopped",
                "user": "65532:65532",
                "cap_drop": ["ALL"],
                "security_opt": ["no-new-privileges:true"],
                "command": [
                    "validator",
                    "--gateway",
                    "https://master.fixture.invalid",
                    "--netuid",
                    "541",
                    "--gateway-public",
                    "e" * 64,
                    "--network",
                    "test",
                    "--owner-public",
                    "/etc/base/config/owner.pubkey",
                    "--challenges",
                    "/etc/base/config/challenges.toml",
                    "--measurements",
                    "/etc/base/config/measurements.toml",
                    "--wallet-name",
                    "fixture",
                    "--wallet-hotkey",
                    "validator",
                    "--wallet-path",
                    "/run/wallets",
                    "--state-db",
                    "/var/lib/base/validator.sqlite3",
                    "--consensus-seed-file",
                    "/run/validator/consensus.key",
                    "--peers",
                    "/etc/base/config/peers.json",
                    "--peer-bind",
                    "0.0.0.0",
                    "--peer-port",
                    "8091",
                    "--peer-tls-certificate",
                    "/run/validator/tls.crt",
                    "--peer-tls-key",
                    "/run/validator/tls.key",
                    "--poll-seconds",
                    "30",
                ],
                "ports": [{"target": 8091, "host_ip": "10.0.0.2"}],
                "tmpfs": ["/tmp:size=32m,mode=1777"],
                "volumes": [
                    {
                        "type": "bind",
                        "source": "/fixture/config",
                        "target": "/etc/base/config",
                        "read_only": True,
                    },
                    {
                        "type": "bind",
                        "source": "/fixture/wallets",
                        "target": "/run/wallets",
                        "read_only": True,
                    },
                    {
                        "type": "bind",
                        "source": "/fixture/validator",
                        "target": "/run/validator",
                        "read_only": True,
                    },
                    {"type": "volume", "target": "/var/lib/base"},
                ],
            }
        }
    }


def master_config():
    return {
        "services": {
            "gateway": {
                "profiles": ["master"],
                "image": PINNED_IMAGE,
                "init": True,
                "read_only": True,
                "restart": "unless-stopped",
                "user": "65532:65532",
                "cap_drop": ["ALL"],
                "security_opt": ["no-new-privileges:true"],
                "command": ["master", "--bind", "0.0.0.0", "--port", "8080"],
                "environment": {
                    "BASE_STATE_DIR": "/var/lib/base",
                    "BASE_OWNER_PUBKEY_FILE": "/etc/base/config/owner.pubkey",
                    "BASE_CHALLENGES_FILE": "/etc/base/config/challenges.toml",
                    "BASE_MEASUREMENTS_FILE": "/etc/base/config/measurements.toml",
                    "BASE_GATEWAY_SK_FILE": "/run/secrets/gateway.key",
                    "BOUNTY_SK_FILE": "/run/secrets/bounty.key",
                    "PROOF_SK_FILE": "/run/secrets/proof.key",
                    "BOUNTY_SESSION_SECRET_FILE": "/run/secrets/bounty-session.key",
                    "BASE_GATEWAY_ADMIN_TOKEN_FILE": "/run/secrets/operator.token",
                },
                "healthcheck": {
                    "test": [
                        "CMD",
                        "python",
                        "-c",
                        "urllib.request.urlopen('http://127.0.0.1:8080/readyz')",
                    ]
                },
                "ports": [{"target": 8080, "host_ip": "10.0.0.1"}],
                "tmpfs": ["/tmp:size=64m,mode=1777"],
                "volumes": [
                    {
                        "type": "bind",
                        "source": "/fixture/config",
                        "target": "/etc/base/config",
                        "read_only": True,
                    },
                    {
                        "type": "bind",
                        "source": "/fixture/secrets",
                        "target": "/run/secrets",
                        "read_only": True,
                    },
                    {"type": "volume", "target": "/var/lib/base"},
                ],
            }
        }
    }


@pytest.mark.parametrize("mutation", ["gateway", "keys", "http", "privileged", "state"])
def test_validator_refuses_control_plane_or_unsafe_runtime(mutation):
    config = deepcopy(validator_config())
    service = config["services"]["validator"]
    if mutation == "gateway":
        config["services"]["gateway"] = {}
    elif mutation == "keys":
        service["volumes"].append({"type": "bind", "read_only": True, "target": "/run/secrets"})
    elif mutation == "http":
        service["command"][2] = "http://master.fixture.invalid"
    elif mutation == "privileged":
        service["privileged"] = True
    else:
        service["volumes"] = []
    with pytest.raises(ValueError):
        CHECK["validate_compose"](config, "validator")


@pytest.mark.parametrize("role", ["master", "validator"])
def test_compose_command_must_match_the_installed_python_entrypoint(role):
    config = master_config() if role == "master" else validator_config()
    service = config["services"]["gateway" if role == "master" else "validator"]
    service["command"] = [role]

    with pytest.raises(ValueError):
        CHECK["validate_compose"](config, role)


@pytest.mark.parametrize("role", ["master", "validator"])
def test_compose_accepts_isolated_python_roles_with_durable_state(role):
    config = master_config() if role == "master" else validator_config()
    CHECK["validate_compose"](config, role)


def master_resource_example(values):
    source = (ROOT / "deploy/env/master.env.example").read_text()
    resource_names = {
        "PROOF_RLM_VM_VCPUS",
        "PROOF_RLM_VM_MEM_MIB",
        "PROOF_RLM_VM_DISK_MIB",
    }
    lines = [line for line in source.splitlines() if line.partition("=")[0] not in resource_names]
    return "\n".join(lines + [f"{name}={value}" for name, value in values.items()])


@pytest.mark.parametrize("resources", [(1, 128, 16384), (1, 1024, 16384), (16, 32768, 1048576)])
def test_master_deployment_accepts_explicit_bounded_vm_resources(resources):
    values = dict(
        zip(
            ("PROOF_RLM_VM_VCPUS", "PROOF_RLM_VM_MEM_MIB", "PROOF_RLM_VM_DISK_MIB"),
            map(str, resources),
            strict=True,
        )
    )
    CHECK["validate_env_examples"](
        master_resource_example(values),
        (ROOT / "deploy/env/python-validator.env.example").read_text(),
    )
    config = master_config()
    config["services"]["gateway"]["environment"].update(values)
    CHECK["validate_compose"](config, "master")


@pytest.mark.parametrize("boundary", ["example", "compose"])
@pytest.mark.parametrize(
    ("name", "invalid"),
    [
        ("PROOF_RLM_VM_VCPUS", None),
        ("PROOF_RLM_VM_VCPUS", "0"),
        ("PROOF_RLM_VM_VCPUS", "17"),
        ("PROOF_RLM_VM_MEM_MIB", None),
        ("PROOF_RLM_VM_MEM_MIB", "127"),
        ("PROOF_RLM_VM_MEM_MIB", "32769"),
        ("PROOF_RLM_VM_DISK_MIB", None),
        ("PROOF_RLM_VM_DISK_MIB", "16383"),
        ("PROOF_RLM_VM_DISK_MIB", "1048577"),
        ("PROOF_RLM_VM_VCPUS", "1.5"),
        ("PROOF_RLM_VM_MEM_MIB", ""),
        ("PROOF_RLM_VM_DISK_MIB", "true"),
    ],
)
def test_master_deployment_refuses_incomplete_or_unsafe_vm_resources(boundary, name, invalid):
    values = {
        "PROOF_RLM_VM_VCPUS": "1",
        "PROOF_RLM_VM_MEM_MIB": "1024",
        "PROOF_RLM_VM_DISK_MIB": "16384",
    }
    if invalid is None:
        del values[name]
    else:
        values[name] = invalid

    with pytest.raises(ValueError, match="VM resource"):
        if boundary == "example":
            CHECK["validate_env_examples"](
                master_resource_example(values),
                (ROOT / "deploy/env/python-validator.env.example").read_text(),
            )
        else:
            config = master_config()
            config["services"]["gateway"]["environment"].update(values)
            CHECK["validate_compose"](config, "master")


def vm_host_config():
    return {
        "host": {
            "bind": "10.0.0.3",
            "port": 8443,
            "token_file": "/etc/proof-vm/token",
            "kernel": "/var/lib/proof/images/vmlinux",
            "kernel_digest": "a" * 64,
            "jail_root": "/var/lib/proof/jails",
            "retain_root": "/var/lib/proof/retained",
            "pack_dir": "/var/lib/proof/packs",
            "state_db": "/var/lib/proof/orchestrator.sqlite3",
            "firecracker": "/usr/local/bin/firecracker",
            "jailer": "/usr/local/bin/jailer",
            "uid": 10000,
            "gid": 10000,
            "uplink": "eth0",
            "max_experiments": 1,
            "max_topics": 64,
            "custom_ids": ["agent"],
        },
        "tls": {
            "certificate": "/etc/proof-vm/tls.crt",
            "private_key": "/etc/proof-vm/tls.key",
            "names": ["proof-vm.internal"],
        },
        "inference": {
            "model": "deepseek/deepseek-v4.1-flash",
            "api_key_file": "/etc/proof-vm/openrouter.key",
            "offer_file": "/etc/proof-vm/inference-offer.json",
            "offer_commitment": "b" * 64,
        },
        "images": {"c" * 64: "/var/lib/proof/images/rootfs.ext4"},
        "caps": {"vcpus": 16, "mem_mib": 32768, "disk_mib": 32768},
        "knowledge": {
            "state_db": "/var/lib/proof/knowledge.sqlite3",
            "owner_public_file": "/etc/proof-vm/owner.pubkey",
        },
    }


@pytest.mark.parametrize(
    "mutation",
    [
        "kernel",
        "offer",
        "image",
        "tls",
        "caps",
        "knowledge",
        "bind",
        "capacity",
        "path_escape",
    ],
)
def test_vm_host_refuses_unpinned_or_incomplete_paid_execution(mutation):
    config = deepcopy(vm_host_config())
    if mutation == "kernel":
        config["host"]["kernel_digest"] = ""
    elif mutation == "offer":
        del config["inference"]["offer_file"]
    elif mutation == "image":
        config["images"] = {}
    elif mutation == "tls":
        config["tls"]["names"] = []
    elif mutation == "caps":
        config["caps"]["vcpus"] = 17
    elif mutation == "knowledge":
        del config["knowledge"]
    elif mutation == "bind":
        config["host"]["bind"] = "0.0.0.0"
    elif mutation == "capacity":
        config["host"]["max_experiments"] = 0
    else:
        config["inference"]["offer_file"] = "/etc/proof-vm/../forged-offer.json"

    with pytest.raises(ValueError):
        CHECK["validate_vm_host"](config)


def test_vm_host_accepts_signed_offer_and_digest_pinned_images():
    CHECK["validate_vm_host"](vm_host_config())


def test_checked_in_vm_host_template_is_complete_and_deliberately_unlaunchable():
    config = tomllib.loads((ROOT / "deploy/env/proof-vm-host.toml.example").read_text())

    CHECK["validate_vm_host_template"](config)
    with pytest.raises(ValueError):
        CHECK["validate_vm_host"](config)


def test_checked_in_python_deployment_files_match_runtime_contracts():
    CHECK["validate_dockerfile"]((ROOT / "deploy/Dockerfile").read_text())
    CHECK["validate_rootfs_builder"]((ROOT / "deploy/guest/bake-rootfs.sh").read_text())
    CHECK["validate_systemd"]((ROOT / "deploy/systemd/proof-vm-orchestrator.service").read_text())


@pytest.mark.parametrize("location", ["absent", "guest_only", "comment_only"])
def test_runtime_requires_installed_ssh_client_for_provider_transport(location):
    source = (ROOT / "deploy/Dockerfile").read_text().replace(" openssh-client", "")
    if location == "guest_only":
        source = source.replace("libseccomp2 bubblewrap", "openssh-client libseccomp2 bubblewrap")
    elif location == "comment_only":
        source += "\n# openssh-client is mentioned but not installed\n"

    with pytest.raises(ValueError, match="runtime must install openssh-client"):
        CHECK["validate_dockerfile"](source)
