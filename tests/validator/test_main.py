import sys
from types import ModuleType

import pytest

from cortex.validator import __main__ as validator_main
from cortex.validator.service import TickResult


def required_arguments(tmp_path):
    return [
        "--gateway",
        "https://master.invalid",
        "--netuid",
        "541",
        "--gateway-public",
        "11" * 32,
        "--minimum-challenges-version",
        "1",
        "--minimum-measurements-version",
        "1",
        "--wallet-name",
        "validator",
        "--wallet-hotkey",
        "default",
        "--state-db",
        str(tmp_path / "state.db"),
        "--version-key",
        "123",
        "--consensus-seed-file",
        str(tmp_path / "consensus.key"),
    ]


def test_fallback_endpoints_reject_non_wss_duplicate_or_oversized_values(tmp_path):
    parse = validator_main.parser().parse_args

    for value in (
        '["https://rpc.invalid"]',
        '["wss://rpc.invalid", "wss://rpc.invalid"]',
        '["wss://rpc.invalid?token=secret"]',
        '["wss://rpc.invalid#fragment"]',
        '["wss://rpc.invalid/provider-secret"]',
        '["wss://rpc.invalid:invalid"]',
        "[" + ",".join(f'"wss://rpc-{index}.invalid"' for index in range(9)) + "]",
    ):
        with pytest.raises(SystemExit):
            parse([*required_arguments(tmp_path), "--fallback-endpoints", value])


@pytest.mark.parametrize(
    "value",
    [
        "unknown-network",
        "https://rpc.invalid",
        "ws://rpc.invalid",
        "wss://user:secret@rpc.invalid",
        "wss://rpc.invalid/provider-secret",
        "wss://rpc.invalid?token=secret",
        "wss://rpc.invalid#fragment",
        "wss://rpc.invalid:invalid",
    ],
)
def test_primary_chain_endpoint_rejects_unknown_alias_or_unsafe_url(value, tmp_path):
    with pytest.raises(SystemExit):
        validator_main.parser().parse_args([*required_arguments(tmp_path), "--network", value])


@pytest.mark.parametrize("value", ["finney", "test", "archive", "local", "wss://rpc.invalid:443"])
def test_primary_chain_endpoint_accepts_sdk_alias_or_safe_wss_origin(value, tmp_path):
    arguments = validator_main.parser().parse_args(
        [*required_arguments(tmp_path), "--network", value]
    )

    assert arguments.network == value


def test_main_passes_validated_fallback_endpoints_to_bittensor(monkeypatch, tmp_path):
    calls = []

    class Subtensor:
        def __init__(self, **kwargs):
            calls.append(kwargs)

        def close(self):
            pass

    class Wallet:
        def __init__(self, **kwargs):
            self.kwargs = kwargs

    bittensor = ModuleType("bittensor")
    bittensor.Subtensor = Subtensor
    wallet_module = ModuleType("bittensor_wallet")
    wallet_module.Wallet = Wallet
    monkeypatch.setitem(sys.modules, "bittensor", bittensor)
    monkeypatch.setitem(sys.modules, "bittensor_wallet", wallet_module)

    async def no_run(arguments, subtensor, wallet):
        assert arguments.fallback_endpoints == ["wss://rpc-a.invalid", "wss://rpc-b.invalid"]

    monkeypatch.setattr(validator_main, "run", no_run)
    validator_main.main(
        [
            *required_arguments(tmp_path),
            "--network",
            "finney",
            "--fallback-endpoints",
            '["wss://rpc-a.invalid", "wss://rpc-b.invalid"]',
        ]
    )

    assert calls == [
        {
            "network": "finney",
            "fallback_endpoints": ["wss://rpc-a.invalid", "wss://rpc-b.invalid"],
        }
    ]


def test_verify_only_once_exits_nonzero_unless_the_outcome_is_verified(monkeypatch, tmp_path):
    class Subtensor:
        def __init__(self, **kwargs):
            pass

        def close(self):
            pass

    class Wallet:
        def __init__(self, **kwargs):
            pass

    bittensor = ModuleType("bittensor")
    bittensor.Subtensor = Subtensor
    wallet_module = ModuleType("bittensor_wallet")
    wallet_module.Wallet = Wallet
    monkeypatch.setitem(sys.modules, "bittensor", bittensor)
    monkeypatch.setitem(sys.modules, "bittensor_wallet", wallet_module)

    async def refused(arguments, subtensor, wallet):
        return TickResult("unsealed")

    monkeypatch.setattr(validator_main, "run", refused)

    with pytest.raises(SystemExit, match="preflight refused: unsealed"):
        validator_main.main([*required_arguments(tmp_path), "--verify-only", "--once"])


@pytest.mark.parametrize("flag", ["--minimum-challenges-version", "--minimum-measurements-version"])
def test_main_rejects_nonpositive_trust_version_pins(flag, tmp_path):
    with pytest.raises(SystemExit, match="minimum trust versions must be positive"):
        validator_main.main([*required_arguments(tmp_path), flag, "0"])


@pytest.mark.parametrize(
    ("flag", "value", "message"),
    [
        ("--netuid", "-1", "fit u16"),
        ("--version-key", "-1", "fit u64"),
        ("--min-peer-sample", "65", "between 0 and 64"),
    ],
)
def test_main_rejects_unsafe_consensus_settings_before_connecting(flag, value, message, tmp_path):
    with pytest.raises(SystemExit, match=message):
        validator_main.main([*required_arguments(tmp_path), flag, value])
