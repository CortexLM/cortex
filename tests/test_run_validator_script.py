import json
import os
import subprocess
from pathlib import Path

import pytest

SCRIPT = Path(__file__).resolve().parents[1] / "scripts" / "run-validator.sh"
LATEST = {"protocol_version": "1.0", "netuid": 100, "epoch": 4242, "sealed": True}


@pytest.fixture
def env(tmp_path):
    fake_bin = tmp_path / "bin"
    fake_bin.mkdir()
    (tmp_path / "latest.json").write_text(json.dumps(LATEST))
    curl = fake_bin / "curl"
    curl.write_text(
        "#!/usr/bin/env bash\n"
        'while (($#)); do [[ $1 == -o ]] && { cp "$LATEST_JSON" "$2"; shift; }; shift; done\n'
        "printf 200\n"
    )
    curl.chmod(0o755)
    return {
        key: value
        for key, value in os.environ.items()
        if not key.startswith(("BASE_", "CORTEX_", "WALLET_", "GATEWAY"))
    } | {
        "PATH": f"{fake_bin}:{os.environ['PATH']}",
        "HOME": str(tmp_path),
        "LATEST_JSON": str(tmp_path / "latest.json"),
        "WALLET_NAME": "coldwallet-secret",
        "WALLET_HOTKEY": "hotkey-secret",
        "GATEWAY_PUBLIC": "5GatewayPinnedKey",
        "VERSION_KEY": "7",
    }


def run(env, *args):
    return subprocess.run(
        [str(SCRIPT), *args], env=env, capture_output=True, text=True, timeout=120
    )


@pytest.mark.parametrize(
    ("args", "extra"),
    [
        (["master"], {}),
        (["challenge-supervisor", "--dry-run"], {}),
        (["--dry-run"], {"CORTEX_MASTER": "1"}),
        (["--dry-run"], {"BASE_MASTER_SECRETS_HOST_DIR": "/srv/secrets"}),
        (["--dry-run"], {"BASE_CHALLENGE_SECRETS_HOST_DIR": ""}),
    ],
)
def test_master_intent_is_refused(env, args, extra):
    result = run(env | extra, *args)
    assert result.returncode == 2
    assert "refused" in result.stderr
    assert "cortex master" not in result.stdout


def test_missing_gateway_public_fails(env):
    del env["GATEWAY_PUBLIC"]
    result = run(env, "--dry-run")
    assert result.returncode != 0
    assert "GATEWAY_PUBLIC is not set" in result.stderr


def test_netuid_mismatch_fails(env):
    result = run(env | {"NETUID": "541"}, "--dry-run")
    assert result.returncode != 0
    assert "netuid" in result.stderr


def test_dry_run_prints_masked_validator_command(env):
    result = run(env, "--dry-run", "--verify-only")
    assert result.returncode == 0, result.stderr
    command = result.stdout.strip()
    assert "cortex validator" in command
    assert "cortex master" not in command + result.stderr
    assert "secret" not in command
    assert "--wallet-name \\*\\*\\* --wallet-hotkey \\*\\*\\*" in command
    for expected in (
        "--netuid 100",
        "--gateway-public 5GatewayPinnedKey",
        "--version-key 7",
        "--verify-only --once",
    ):
        assert expected in command
    assert "epoch=4242 sealed=true" in result.stderr
