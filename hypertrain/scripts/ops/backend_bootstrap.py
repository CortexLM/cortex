#!/usr/bin/env python3
"""Bootstrap the live CUDA backend from verified two-host qualification, before run POST."""

import argparse
import hashlib
import json
import sys
from pathlib import Path

import hypertrain.trainer  # noqa: F401  determinism setup before any torch import
from hypertrain.beacon import parse_round
from hypertrain.challenge.app import Config, load_coord_key
from hypertrain.challenge.store import ChallengeStore
from hypertrain.data.store import LocalFSStore
from hypertrain.gpu_ops import host_attest
from hypertrain.gpu_ops import network_qualification as engine
from hypertrain.miner.core import load_keyfile
from hypertrain.protocol import envelope_v2
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages_v2 import RunManifestV2

p = argparse.ArgumentParser(description=__doc__)
p.add_argument("--manifest-envelope", type=Path, required=True)
p.add_argument("--owner-seed", type=Path, required=True)
p.add_argument("--config", type=Path, nargs=2, required=True, help="h0 h1: <output>/config.json")
p.add_argument("--result", type=Path, nargs=2, required=True, help="h0 h1: <output>/result.json")
mode = p.add_mutually_exclusive_group(required=True)
mode.add_argument(
    "--label-only",
    action="store_true",
    help="results without a host attestation (the Vast lifecycle): nothing proves two machines",
)
mode.add_argument(
    "--known-hosts",
    type=Path,
    nargs=2,
    metavar=("KH0", "KH1"),
    help="h0 h1: each host's known_hosts file; its hash is the config's known_hosts_sha256",
)
p.add_argument(
    "--host-fingerprint",
    nargs=2,
    metavar=("FP0", "FP1"),
    help="h0 h1: SHA256:... fingerprint of each host's SSH key, typed from the provider console",
)
a = p.parse_args()
if bool(a.known_hosts) != bool(a.host_fingerprint):
    p.error("--known-hosts and --host-fingerprint go together")

raw = a.manifest_envelope.read_bytes()
env = envelope_v2.parse_envelope(raw)
if env.type != "RunManifestV2" or not envelope_v2.verify_envelope(raw):
    sys.exit("manifest envelope is not a verified RunManifestV2")
manifest = RunManifestV2.model_validate(env.body)
cfg = Config.from_env()
owner = load_keyfile(a.owner_seed)
if cfg.owner_hotkey is None or owner.ss58 != cfg.owner_hotkey or env.signer != cfg.owner_hotkey:
    sys.exit("owner seed / manifest signer is not HYPERTRAIN_OWNER_HOTKEY")

# The existing verifier runs before any Receipt is sealed or store opened.
for config_path in a.config:
    job = engine.check_inputs(engine.Qualification.model_validate_json(config_path.read_bytes()))
    if job.manifest != manifest:
        sys.exit(f"{config_path}: qualification seed job binds another manifest")
config_bytes = [c.read_bytes() for c in a.config]
pins = None
if a.known_hosts:
    pins = [engine.HostPin(h, fp) for h, fp in zip(a.known_hosts, a.host_fingerprint, strict=True)]
keys = None if pins is None else engine.pinned_keys(config_bytes, pins)
verdict = engine.compare(list(a.result), manifest, host_keys=keys)
if verdict != {"two_host_bitwise_match": True, "workload_allowed": False}:
    sys.exit(f"two-host comparison did not match: {verdict}")

evidence = ChallengeStore._qualification_evidence_v2(
    manifest,
    config_bytes,
    [r.read_bytes() for r in a.result],
    a.result,
    keys,
    [] if keys is None else [host_attest.load(r) for r in a.result],
)
authority = hashlib.sha256(canonicalize(evidence)).hexdigest()

store = ChallengeStore(
    cfg.state_dir,
    cfg.params,
    load_coord_key(cfg.coord_key_file),
    cfg.owner_hotkey,
    parse_round,
    LocalFSStore(cfg.state_dir / "objects"),
)
try:
    now = store._now(store._db)
    receipt = envelope_v2.seal(
        owner,
        "Receipt",
        manifest.run_id(),
        {"w": 0, "commit_hash": authority, "received_round": now},
        now + 100,
    )
    store._bootstrap_backend_v2(
        manifest, tuple(a.config), tuple(a.result), canonicalize(receipt), pins=pins
    )
    print(
        json.dumps(
            {
                "run_id": manifest.run_id(),
                "received_round": now,
                "authority_hash": authority,
                "mode": "label-only" if keys is None else "strict",
                "host_keys": [] if keys is None else [host_attest.fingerprint(k) for k in keys],
            }
        )
    )
finally:
    store.close()
