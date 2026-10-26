from __future__ import annotations

import json
import stat
from collections.abc import Iterator
from pathlib import Path
from typing import Any

import pytest
from fastapi.testclient import TestClient

from challenge.conftest import ADMIN, COORD_SEED, INTERNAL, WORKER, Master, make_client
from hypertrain.miner.keys import REGISTRY_ENV, create_key, list_keys, revoke_key
from hypertrain.protocol.keys import Keypair

COLD_SEED = b"\x42" * 32
OTHER_SEED = b"\x43" * 32


def _keyfile(path: Path, seed: bytes) -> Path:
    path.write_text(seed.hex())
    path.chmod(0o600)
    return path


@pytest.fixture
def env(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Iterator[dict[str, Any]]:
    monkeypatch.setenv(REGISTRY_ENV, str(tmp_path / "home" / "keys.json"))
    secrets = tmp_path / "secrets"
    secrets.mkdir()
    for name, value in (
        ("internal.token", INTERNAL),
        ("admin.token", ADMIN),
        ("worker.token", WORKER),
        ("coord.key", COORD_SEED.hex()),
    ):
        (secrets / name).write_text(value)
    master = Master()
    with make_client(tmp_path / "state", secrets, master, [1_800_000_000.0]) as client:
        yield {
            "client": client,
            "master": master,
            "cold": _keyfile(tmp_path / "cold.key", COLD_SEED),
            "other": _keyfile(tmp_path / "other.key", OTHER_SEED),
            "tmp": tmp_path,
        }


def _upload(client: TestClient, hotkey: str) -> int:
    body = {"w": 0, "hotkey": hotkey, "sha256": "0" * 64}
    return client.post("/v1/runs/unknown/uploads", json=body).status_code


def test_create_list_revoke_and_revoked_key_is_rejected(env: dict[str, Any]) -> None:
    client: TestClient = env["client"]
    cold = Keypair(COLD_SEED)
    bundle = create_key(env["cold"], "vast-1", client, out_dir=env["tmp"] / "bundle")

    assert stat.S_IMODE(bundle.stat().st_mode) == 0o700
    assert sorted(p.name for p in bundle.iterdir()) == ["grant.json", "server.key"]
    for item in bundle.iterdir():
        assert stat.S_IMODE(item.stat().st_mode) == 0o600
    grant = json.loads((bundle / "grant.json").read_text())["grant"]
    server = Keypair(bytes.fromhex((bundle / "server.key").read_text()))
    assert grant["server_pub"] == server.ss58 != cold.ss58
    assert grant["coldkey"] == cold.ss58 and grant["scope"] == "mine"

    env["master"].registered.add(server.ss58)
    assert _upload(client, server.ss58) != 403

    listed = list_keys(cold.ss58, client)
    assert [(k["key_id"], k["revoked"]) for k in listed] == [(grant["key_id"], False)]
    assert [k["key_id"] for k in list_keys(cold.ss58)] == [grant["key_id"]]

    record = revoke_key(env["cold"], grant["key_id"], client)
    assert record["revoked"] is True
    assert _upload(client, server.ss58) == 403
    assert list_keys(cold.ss58, client)[0]["revoked"] is True
    assert json.loads(Path(env["tmp"] / "home" / "keys.json").read_text())[grant["key_id"]][
        "revoked"
    ]


def test_wrong_coldkey_cannot_revoke(env: dict[str, Any]) -> None:
    client: TestClient = env["client"]
    bundle = create_key(env["cold"], "vast-2", client, out_dir=env["tmp"] / "b2")
    key_id = json.loads((bundle / "grant.json").read_text())["grant"]["key_id"]
    with pytest.raises(RuntimeError, match="403"):
        revoke_key(env["other"], key_id, client)
    assert list_keys(Keypair(COLD_SEED).ss58, client)[0]["revoked"] is False


def test_forged_grant_is_refused(env: dict[str, Any]) -> None:
    client: TestClient = env["client"]
    bundle = create_key(env["cold"], "vast-3", client, out_dir=env["tmp"] / "b3")
    signed = json.loads((bundle / "grant.json").read_text())
    signed["grant"]["server_name"] = "evil"
    assert client.post("/v2/server-keys", json=signed).status_code == 403


def test_coldkey_material_never_in_bundle(env: dict[str, Any]) -> None:
    bundle = create_key(env["cold"], "vast-4", env["client"], out_dir=env["tmp"] / "b4")
    secret = Keypair(COLD_SEED)._secret
    blob = b"".join(p.read_bytes() for p in bundle.iterdir())
    for needle in (
        COLD_SEED,
        COLD_SEED.hex().encode(),
        bytes(secret),
        bytes(secret).hex().encode(),
    ):
        assert needle not in blob
