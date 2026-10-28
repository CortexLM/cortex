"""Public join preparation and operator-side coldkey operations; no miner runtime needed."""

from __future__ import annotations

from pathlib import Path
from typing import Any

import httpx
from pydantic import TypeAdapter

from hypertrain.miner.core import MinerConfig, MinerError, canonical, load_keyfile
from hypertrain.protocol import envelope_v2
from hypertrain.protocol.env_pins import EnvPinUnhonorable, require_honorable
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.keys import decode_hotkey, verify
from hypertrain.protocol.messages import Hex64
from hypertrain.protocol.messages_v2 import (
    U64,
    EconomicsPolicyV2,
    EnvObservation,
    EscrowLock,
    HardwareHint,
    JoinRequest,
    Positive,
    RunManifestV2,
)


def required_units(policy: EconomicsPolicyV2, manifest: RunManifestV2, locked: int) -> int:
    denominator = policy.s_lower_ppm * policy.beta_ppm * 1_000_000
    return max(
        policy.S_min_units,
        manifest.training.verify.s_min_units(policy.R_collectible_units),
        (policy.G_max_units * 10**18 + denominator - 1) // denominator
        if denominator
        else policy.G_max_units + locked + 1,
    )


def _request(client: httpx.Client, api: str, method: str, path: str, **kw: Any) -> httpx.Response:
    response = client.request(method, api.rstrip("/") + path, **kw)
    response.raise_for_status()
    return response


def _manifest(summary: dict[str, Any], run_id: str, owner: str | None = None) -> RunManifestV2:
    env = envelope_v2.parse_envelope(summary["manifest_envelope"])
    if (
        env.type != "RunManifestV2"
        or env.run_id != run_id
        or owner is not None
        and env.signer != owner
        or not envelope_v2.verify_envelope(env.model_dump())
    ):
        raise MinerError("v2 manifest authority/signature mismatch")
    manifest = RunManifestV2.model_validate(env.body)
    manifest.validate_run_id(run_id)
    return manifest


def prepare_join(
    client: httpx.Client,
    cfg: MinerConfig,
    coldkey_ss58: str,
    request_id: str,
    expiry: int,
    out: Path,
) -> dict[str, Any]:
    if cfg.run_id is None or cfg.owner_hotkey is None:
        raise MinerError("v2 requires pinned run_id and owner_hotkey")
    TypeAdapter(Hex64).validate_python(cfg.run_id)
    decode_hotkey(coldkey_ss58)
    summary = _request(client, cfg.api, "GET", f"/v2/runs/{cfg.run_id}").json()
    manifest = _manifest(summary, cfg.run_id, cfg.owner_hotkey)
    hotkey = load_keyfile(cfg.keyfile)
    pin = manifest.training.reference_spec.env.pin()
    backend = "cpu" if cfg.device == "cpu" else "cuda"
    try:
        observed = require_honorable(pin, backend)
    except EnvPinUnhonorable as error:
        raise MinerError(
            f"this host cannot honor the manifest's environment pin: {error}"
        ) from error
    request = JoinRequest(
        run_id=cfg.run_id,
        request_id=request_id,
        hotkey=hotkey.ss58,
        coldkey=coldkey_ss58,
        expires_beacon=expiry,
        policy_hash=manifest.network.admission_policy_hash,
        hardware_hint=HardwareHint(
            device_name=cfg.device,
            device_count=manifest.training.reference_spec.layout.n_gpus,
            driver="advisory",
        ),
        env=EnvObservation.model_validate(observed),
        hot_sig="0" * 128,
        cold_sig="0" * 128,
    )
    message = envelope_v2.join_signing_message(request)
    body = {**request.body(), "hot_sig": hotkey.sign(message).hex()}
    with out.open("xb") as stream:
        stream.write(canonical(body))
    return {"prepared": str(out), "message_sha256": sha256_hex(message)}


def cosign_join(client: httpx.Client, api: str, request_path: Path, coldfile: Path) -> Any:
    request = JoinRequest.model_validate(envelope_v2.load_json(request_path.read_bytes()))
    message = envelope_v2.join_signing_message(request)
    if not verify(decode_hotkey(request.hotkey), message, bytes.fromhex(request.hot_sig)):
        raise MinerError("join hotkey signature does not verify")
    coldkey = load_keyfile(coldfile)
    if request.coldkey != coldkey.ss58:
        raise MinerError("join coldkey differs from keyfile owner")
    body = {**request.body(), "cold_sig": coldkey.sign(message).hex()}
    response = _request(
        client, api, "POST", f"/v2/runs/{request.run_id}/join", content=canonical(body)
    )
    try:
        return response.json()
    except ValueError:
        return response.text


def escrow_lock(
    client: httpx.Client,
    api: str,
    run_id: str,
    coldfile: Path,
    hotkey_ss58: str,
    units: int,
    origin_ids: list[str],
    expiry: int | None = None,
    dry_run: bool = False,
) -> dict[str, Any]:
    TypeAdapter(Hex64).validate_python(run_id)
    decode_hotkey(hotkey_ss58)
    TypeAdapter(Positive).validate_python(units)
    origins = sorted(TypeAdapter(list[Hex64]).validate_python(origin_ids))
    if expiry is not None:
        TypeAdapter(Positive).validate_python(expiry)
    coldkey = load_keyfile(coldfile)
    root = f"/v2/runs/{run_id}"
    status_path = root + "/admission/" + hotkey_ss58
    status = _request(client, api, "GET", status_path).json()
    record = status["record"]
    if record["coldkey"] != coldkey.ss58:
        raise MinerError("admission coldkey differs from keyfile owner")
    admission_id = TypeAdapter(Hex64).validate_python(record["admission_id"])
    operation = EscrowLock(
        operation_id=sha256_hex(
            canonical(["ht-escrow-lock-cli", run_id, admission_id, coldkey.ss58, units, origins])
        ),
        owner=coldkey.ss58,
        units=units,
        origin_ids=origins,
        admission_id=admission_id,
        dispute_id=None,
        kind="LOCK_ADMISSION",
    )
    if dry_run:
        return operation.body()
    summary = _request(client, api, "GET", root).json()
    manifest = _manifest(summary, run_id)
    policy_bytes = _request(
        client, api, "GET", root + "/objects/" + manifest.network.economics_policy_hash
    ).content
    if sha256_hex(policy_bytes) != manifest.network.economics_policy_hash:
        raise MinerError("economics policy hash mismatch")
    policy = EconomicsPolicyV2.model_validate(envelope_v2.load_json(policy_bytes))
    if expiry is None:
        expiry = TypeAdapter(U64).validate_python(summary["now_round"]) + 1000
    envelope = envelope_v2.seal(coldkey, "EscrowLock", run_id, operation, expiry)
    result = _request(
        client, api, "POST", root + "/escrow/lock", content=canonical(envelope)
    ).json()
    status = _request(client, api, "GET", status_path).json()
    locked = TypeAdapter(U64).validate_python(status["funding"]["locked_units"])
    return {
        "event_hash": result["event_hash"],
        "locked_units": locked,
        "required_units": required_units(policy, manifest, locked),
        "eligible": status["eligible"],
    }
