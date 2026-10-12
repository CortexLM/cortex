"""Operator-side server keys: the coldkey signs grants/revokes locally; it never leaves here."""

from __future__ import annotations

import json
import os
import secrets
import time
from pathlib import Path
from typing import Any

import httpx

from hypertrain.miner.core import load_keyfile
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.server_keys import (
    ServerKeyGrant,
    ServerKeyRevoke,
    SignedGrant,
    SignedRevoke,
    signing_message,
)

REGISTRY_ENV = "HYPERTRAIN_KEYS_FILE"


def registry_path() -> Path:
    return Path(os.environ.get(REGISTRY_ENV) or Path.home() / ".hypertrain" / "keys.json")


def _load_registry() -> dict[str, dict[str, Any]]:
    path = registry_path()
    return json.loads(path.read_text()) if path.exists() else {}


def _save_registry(entries: dict[str, dict[str, Any]]) -> None:
    path = registry_path()
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    tmp = path.with_suffix(".tmp")
    tmp.write_text(json.dumps(entries, indent=2, sort_keys=True))
    tmp.replace(path)


def _write_secret(path: Path, data: str) -> None:
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "w") as f:
        f.write(data)


def _request(api: str | httpx.Client, method: str, path: str, **kw: Any) -> Any:
    if isinstance(api, httpx.Client):
        response = api.request(method, path, **kw)
    else:
        response = httpx.request(method, api.rstrip("/") + path, timeout=30, **kw)
    if response.status_code >= 400:
        raise RuntimeError(f"{path}: {response.status_code} {response.text}")
    return response.json()


def create_key(
    coldkey_path: Path,
    server_name: str,
    api: str | httpx.Client,
    *,
    out_dir: Path | None = None,
    run_id: str = "*",
) -> Path:
    """Grant a fresh server key; returns a 0700 bundle dir holding server.key + grant.json."""
    coldkey = load_keyfile(coldkey_path)
    seed = secrets.token_bytes(32)
    server = Keypair(seed)
    grant = ServerKeyGrant(
        run_id=run_id,
        coldkey=coldkey.ss58,
        server_pub=server.ss58,
        server_name=server_name,
        scope="mine",
        key_id=secrets.token_hex(16),
        issued_at=int(time.time()),
    )
    message = signing_message("grant", grant)
    signed = SignedGrant(
        grant=grant, cold_sig=coldkey.sign(message).hex(), server_sig=server.sign(message).hex()
    )
    record = _request(api, "POST", "/v2/server-keys", json=signed.body())
    bundle = out_dir or registry_path().parent / "bundles" / server_name
    bundle.mkdir(parents=True, mode=0o700)
    bundle.chmod(0o700)
    _write_secret(bundle / "server.key", seed.hex())
    _write_secret(bundle / "grant.json", json.dumps(signed.body(), indent=2, sort_keys=True))
    entries = _load_registry()
    entries[grant.key_id] = {**record, "bundle": str(bundle)}
    _save_registry(entries)
    return bundle


def list_keys(coldkey: str, api: str | httpx.Client | None = None) -> list[dict[str, Any]]:
    """Server list when `api` is given (and mirrored locally), else the local registry."""
    entries = _load_registry()
    if api is not None:
        for record in _request(api, "GET", "/v2/server-keys", params={"coldkey": coldkey}):
            entries[record["key_id"]] = {**entries.get(record["key_id"], {}), **record}
        _save_registry(entries)
    return [e for e in entries.values() if e.get("coldkey") == coldkey]


def revoke_key(coldkey_path: Path, key_id: str, api: str | httpx.Client) -> dict[str, Any]:
    coldkey = load_keyfile(coldkey_path)
    revoke = ServerKeyRevoke(key_id=key_id, coldkey=coldkey.ss58, revoked_at=int(time.time()))
    signed = SignedRevoke(
        revoke=revoke, cold_sig=coldkey.sign(signing_message("revoke", revoke)).hex()
    )
    record = _request(api, "POST", "/v2/server-keys/revoke", json=signed.body())
    entries = _load_registry()
    entries[key_id] = {**entries.get(key_id, {}), **record}
    _save_registry(entries)
    return record
