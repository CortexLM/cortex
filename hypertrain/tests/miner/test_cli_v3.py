from __future__ import annotations

import functools
import json
import tomllib
from pathlib import Path
from typing import Any

import pytest

from hypertrain.miner import cli
from hypertrain.miner.update import TEST_PUBKEY_ENV, Updater, sign_manifest
from miner.test_update import KEY, MANIFEST_URL, Host, manifest


def _bundle(tmp: Path) -> Path:
    b = tmp / "bundle"
    b.mkdir()
    (b / "server.key").write_text("11" * 32)
    grant = {"server_pub": "5Fake", "key_id": "ab" * 16, "run_id": "*"}
    (b / "grant.json").write_text(json.dumps({"grant": grant}))
    return b


def _out(capsys: pytest.CaptureFixture[str]) -> Any:
    return json.loads(capsys.readouterr().out)


def test_help_lists_new_commands(capsys: pytest.CaptureFixture[str]) -> None:
    with pytest.raises(SystemExit):
        cli.main(["--help"])
    text = capsys.readouterr().out
    for name in ("install", "register", "key", "status", "update", "join-v2", "run"):
        assert name in text


def test_install_writes_config_pointing_at_bundle_key(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    bundle = _bundle(tmp_path)
    config = tmp_path / "host" / "miner.toml"
    assert cli.main(["install", "--bundle", str(bundle), "--config", str(config)]) == 0
    assert _out(capsys)["key_id"] == "ab" * 16
    cfg = tomllib.loads(config.read_text())
    assert Path(cfg["keyfile"]) == (bundle / "server.key").resolve()
    assert (config.stat().st_mode & 0o777) == 0o600
    assert "11" * 32 not in config.read_text()


@pytest.mark.parametrize(
    ("name", "body"),
    [("cold.key", "22" * 32), ("seed.txt", " ".join(["abandon"] * 11 + ["about"]))],
)
def test_install_refuses_coldkey_or_mnemonic(
    tmp_path: Path, capsys: pytest.CaptureFixture[str], name: str, body: str
) -> None:
    bundle = _bundle(tmp_path)
    (bundle / name).write_text(body)
    config = tmp_path / "miner.toml"
    assert cli.main(["install", "--bundle", str(bundle), "--config", str(config)]) == 2
    assert not config.exists()
    assert "abandon" not in capsys.readouterr().err


def test_key_commands_delegate(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    calls: list[tuple[Any, ...]] = []
    monkeypatch.setattr(cli, "create_key", lambda *a, **k: calls.append(a) or tmp_path)
    monkeypatch.setattr(cli, "list_keys", lambda *a: calls.append(a) or [])
    monkeypatch.setattr(cli, "revoke_key", lambda *a: calls.append(a) or {"revoked": True})
    cold = str(tmp_path / "c.key")
    assert cli.main(["key", "create", "--coldkey", cold, "--server", "s", "--api", "u"]) == 0
    assert cli.main(["key", "list", "--coldkey-ss58", "5X"]) == 0
    assert cli.main(["key", "revoke", "kid", "--coldkey", cold, "--api", "u"]) == 0
    assert calls == [(Path(cold), "s", "u"), ("5X", None), (Path(cold), "kid", "u")]


def test_update_check_and_apply(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    monkeypatch.setenv(TEST_PUBKEY_ENV, KEY.public.hex())
    host = Host(sign_manifest(manifest(changeover_at=601, announce_at=1), KEY))
    monkeypatch.setattr(
        cli,
        "Updater",
        functools.partial(Updater, version="0.1.0", fetch=host.fetch, install=host.install),
    )
    monkeypatch.setenv(cli.UPDATE_ROOT_ENV, str(tmp_path / "root"))
    assert cli.main(["update", "--check", "--manifest-url", MANIFEST_URL]) == 0
    assert _out(capsys) == {"update": "0.2.0", "changeover_at": 601}
    assert cli.main(["update", "--apply", "--manifest-url", MANIFEST_URL]) == 0
    out = _out(capsys)
    assert out["staged"] is True and out["current"].endswith("0.2.0")
    assert (tmp_path / "root" / "current").resolve().name == "0.2.0"


def test_update_requires_manifest_url(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv(cli.MANIFEST_URL_ENV, raising=False)
    monkeypatch.setenv(cli.UPDATE_ROOT_ENV, str(tmp_path))
    assert cli.main(["update", "--check"]) == 2
