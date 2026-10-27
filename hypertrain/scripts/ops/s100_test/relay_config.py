"""Re-bind the TEST relay config to a new run: the coordinator re-seals the registry and network
manifest for that run_id and the observers become the run's pinned auditors. Every other field
(relay id, region, key file names, master URL, backing) is kept from the current config.

Usage: relay_config.py CURRENT_RELAY_JSON MANIFEST_OUT_DIR COORD_SEED OUT_JSON
The output is checked with the same signature and linkage rules as relay.app.runtime.
"""

from __future__ import annotations

import hashlib
import json
import os
import sys
from pathlib import Path

from hypertrain.miner.core import load_keyfile
from hypertrain.protocol import envelope_v2, relay_envelope
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages_v2 import RunManifestV2
from hypertrain.protocol.relay_messages import RelayNetworkManifest, RelayRegistryV1
from hypertrain.relay.app import RelayConfig


def main(argv: list[str]) -> int:
    current_path, out_dir, seed, target = (Path(a) for a in argv)
    current = RelayConfig.model_validate(envelope_v2.load_json(current_path.read_bytes()))
    raw = (out_dir / "manifest-envelope.json").read_bytes()
    run_id = envelope_v2.parse_envelope(raw).run_id
    manifest = RunManifestV2.model_validate(envelope_v2.load_json(raw)["body"])
    manifest.validate_run_id(run_id)
    registry = RelayRegistryV1.model_validate_json((out_dir / "relay-registry.json").read_bytes())
    coord = load_keyfile(seed)
    if not coord.ss58 == current.master_public == manifest.training.coord_pubkey:
        raise SystemExit("coordinator key differs from the relay's master or the manifest's")
    if registry.digest() != manifest.network.relay_registry_hash:
        raise SystemExit("relay registry is not the one the manifest pins")
    if current.relay_id not in {s.id for s in registry.specs}:
        raise SystemExit("relay id is not in the run's registry")
    # The challenge binds every RelayAssignment to this manifest's digest built from the
    # training hash (store.relay_assignment_v2), and the relay refuses any other one.
    network = RelayNetworkManifest(
        run_id=run_id,
        base_manifest_hash=manifest.training.training_hash(),
        registry_hash=registry.digest(),
        specs_hash=hashlib.sha256(
            canonicalize([s.body() for s in registry.specs], allow_float=False)
        ).hexdigest(),
        assignment_policy="master-observed-median3-v1",
    )
    config = RelayConfig.model_validate(
        {
            **current.model_dump(mode="json"),
            "run_id": run_id,
            "registry": relay_envelope.seal(
                coord, "RelayRegistryV1", run_id, registry, current.registry.exp_drand
            ),
            "network_manifest": relay_envelope.seal(
                coord, "RelayNetworkManifest", run_id, network, current.network_manifest.exp_drand
            ),
            "observer_public_keys": list(manifest.training.auditors),
        }
    )
    challenge_view = RelayNetworkManifest(
        run_id=run_id,
        base_manifest_hash=manifest.training.training_hash(),
        registry_hash=registry.digest(),
        specs_hash=sha256_hex(canonicalize([s.body() for s in registry.specs])),
        assignment_policy="master-observed-median3-v1",
    )
    sealed = RelayNetworkManifest.model_validate(config.network_manifest.body)
    if sealed.digest() != challenge_view.digest():
        raise SystemExit("relay network manifest differs from the one the challenge signs")
    for env, kind in ((config.registry, "RelayRegistryV1"), (config.network_manifest, None)):
        if not relay_envelope.verify_envelope(env.model_dump()) or env.run_id != run_id:
            raise SystemExit(f"sealed {kind or 'RelayNetworkManifest'} does not verify")
    fd = os.open(target, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "wb") as handle:
        handle.write(canonicalize(config.model_dump(mode="json"), allow_float=False))
    print(json.dumps({"run_id": run_id, "observers": len(config.observer_public_keys)}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
