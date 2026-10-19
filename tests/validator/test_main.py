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
        hotkey = object()

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
        hotkey = object()

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


def without(arguments, flag):
    """Drop `flag value` from an argument list."""
    index = arguments.index(flag)
    return arguments[:index] + arguments[index + 2 :]


def without_hotkey(tmp_path):
    return without(without(required_arguments(tmp_path), "--wallet-name"), "--wallet-hotkey")


def test_weight_setting_validator_without_hotkey_fails_with_a_clear_error(tmp_path):
    with pytest.raises(SystemExit, match="required to submit weights"):
        validator_main.main(without_hotkey(tmp_path))


def test_peer_consensus_without_hotkey_fails_even_when_verify_only(tmp_path):
    with pytest.raises(SystemExit, match="required to submit weights or run --peer-consensus"):
        validator_main.main([*without_hotkey(tmp_path), "--verify-only", "--peer-consensus"])


def test_verify_only_validator_starts_without_hotkey_gateway_pin_or_trust_files(
    monkeypatch, tmp_path
):
    seen = {}

    class Subtensor:
        def __init__(self, **kwargs):
            pass

        def close(self):
            pass

    bittensor = ModuleType("bittensor")
    bittensor.Subtensor = Subtensor
    wallet_module = ModuleType("bittensor_wallet")
    wallet_module.Wallet = lambda **kwargs: pytest.fail("no wallet must be built")
    monkeypatch.setitem(sys.modules, "bittensor", bittensor)
    monkeypatch.setitem(sys.modules, "bittensor_wallet", wallet_module)

    async def verified(arguments, subtensor, wallet):
        seen["wallet"] = wallet
        return TickResult("verified")

    monkeypatch.setattr(validator_main, "run", verified)
    arguments = without_hotkey(tmp_path)
    validator_main.main([*arguments, "--verify-only", "--once"])
    assert seen == {"wallet": None}


def test_removed_trust_signing_options_are_gone(tmp_path):
    for flag in (
        "--gateway-public",
        "--owner-public",
        "--challenges",
        "--measurements",
        "--minimum-challenges-version",
    ):
        with pytest.raises(SystemExit):
            validator_main.parser().parse_args([*required_arguments(tmp_path), flag, "x"])


@pytest.mark.parametrize(
    ("flag", "value", "message"),
    [
        ("--netuid", "-1", "fit u16"),
        ("--version-key", "-1", "fit u64"),
        ("--min-peer-sample", "65", "between 0 and 64"),
        ("--max-block-lag", "0", "must be positive"),
    ],
)
def test_main_rejects_unsafe_consensus_settings_before_connecting(flag, value, message, tmp_path):
    with pytest.raises(SystemExit, match=message):
        validator_main.main([*required_arguments(tmp_path), flag, value])


@pytest.mark.parametrize("flag", ["--wallet-name", "--wallet-hotkey"])
def test_half_a_wallet_is_refused_instead_of_silently_ignored(flag, monkeypatch, tmp_path):
    fake_chain_modules(monkeypatch)
    monkeypatch.setattr(validator_main, "run", lambda *args: pytest.fail("must not start"))
    with pytest.raises(SystemExit, match="must be given together"):
        validator_main.main([*without(required_arguments(tmp_path), flag), "--verify-only"])


def fake_chain_modules(monkeypatch):
    class Subtensor:
        def __init__(self, **kwargs):
            pass

        def close(self):
            pass

    bittensor = ModuleType("bittensor")
    bittensor.Subtensor = Subtensor
    monkeypatch.setitem(sys.modules, "bittensor", bittensor)


def test_weight_setter_with_a_missing_hotkey_file_fails_at_startup(monkeypatch, tmp_path):
    fake_chain_modules(monkeypatch)  # the real bittensor_wallet reads the empty wallet dir
    monkeypatch.setattr(validator_main, "run", lambda *args: pytest.fail("must not start"))
    arguments = [*required_arguments(tmp_path), "--wallet-path", str(tmp_path / "wallets")]
    with pytest.raises(SystemExit, match="hotkey unreadable") as refused:
        validator_main.main(arguments)
    assert "default" not in str(refused.value) and "wallets" not in str(refused.value)
