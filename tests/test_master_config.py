"""Master configuration rejects incomplete private VM-host trust material."""

from pathlib import Path

import pytest

from cortex.config import MasterConfig
from cortex.vm.models import Resources


def configured_environment():
    return {
        "BASE_NETUID": "541",
        "BASE_GATEWAY_SK_FILE": "/run/secrets/gateway",
        "BOUNTY_SK_FILE": "/run/secrets/bounty",
        "PROOF_SK_FILE": "/run/secrets/proof",
        "BOUNTY_SESSION_SECRET_FILE": "/run/secrets/session",
        "BASE_GATEWAY_ADMIN_TOKEN_FILE": "/run/secrets/operator",
        "PROOF_VM_ORCHESTRATOR_URL": "https://vm.example",
        "PROOF_VM_ORCHESTRATOR_TOKEN_FILE": "/run/secrets/vm-token",
        "PROOF_VM_ORCHESTRATOR_CA_FILE": "/run/secrets/vm-ca.pem",
    }


@pytest.mark.parametrize("value", [None, ""])
def test_live_proof_configuration_refuses_missing_explicit_ca(value):
    env = configured_environment()
    if value is None:
        env.pop("PROOF_VM_ORCHESTRATOR_CA_FILE")
    else:
        env["PROOF_VM_ORCHESTRATOR_CA_FILE"] = value

    with pytest.raises(ValueError, match="Proof orchestrator CA file required"):
        MasterConfig.from_env(env)


def test_complete_vm_configuration_retains_separate_ca_and_bearer_files():
    config = MasterConfig.from_env(configured_environment())

    assert config.proof_orchestrator_url == "https://vm.example"
    assert config.proof_orchestrator_token_file == Path("/run/secrets/vm-token")
    assert config.proof_orchestrator_ca_file == Path("/run/secrets/vm-ca.pem")


def test_unwired_proof_configuration_requires_no_vm_credentials():
    env = {
        key: value
        for key, value in configured_environment().items()
        if not key.startswith("PROOF_VM_ORCHESTRATOR_")
    }

    config = MasterConfig.from_env(env)

    assert config.proof_orchestrator_url is None
    assert config.proof_orchestrator_token_file is None
    assert config.proof_orchestrator_ca_file is None


def test_small_host_requests_are_explicit_and_preserved():
    env = {
        **configured_environment(),
        "PROOF_RLM_VM_VCPUS": "1",
        "PROOF_RLM_VM_MEM_MIB": "1024",
        "PROOF_RLM_VM_DISK_MIB": "16384",
    }

    config = MasterConfig.from_env(env)

    assert config.proof_vm_resources == Resources(vcpus=1, mem_mib=1024, disk_mib=16384)


@pytest.mark.parametrize(
    ("name", "value"),
    [
        ("PROOF_RLM_VM_VCPUS", "17"),
        ("PROOF_RLM_VM_VCPUS", "0"),
        ("PROOF_RLM_VM_MEM_MIB", "32769"),
        ("PROOF_RLM_VM_MEM_MIB", "nan"),
        ("PROOF_RLM_VM_DISK_MIB", "16383"),
    ],
)
def test_invalid_host_requests_fail_instead_of_clamping_or_using_defaults(name, value):
    env = {**configured_environment(), name: value}

    with pytest.raises(ValueError):
        MasterConfig.from_env(env)
