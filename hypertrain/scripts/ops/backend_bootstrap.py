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
from hypertrain.gpu_ops import network_qualification as engine
from hypertrain.gpu_ops.journal import fsha
from hypertrain.miner.core import load_keyfile
from hypertrain.protocol import envelope_v2
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages_v2 import RunManifestV2

p = argparse.ArgumentParser(description=__doc__)
p.add_argument("--manifest-envelope", type=Path, required=True)
p.add_argument("--owner-seed", type=Path, required=True)
p.add_argument("--config", type=Path, nargs=2, required=True, help="h0 h1: <output>/config.json")
p.add_argument("--result", type=Path, nargs=2, required=True, help="h0 h1: <output>/result.json")
a = p.parse_args()

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
verdict = engine.compare(list(a.result), manifest, host_keys=None)
if verdict != {"two_host_bitwise_match": True, "workload_allowed": False}:
    sys.exit(f"two-host comparison did not match: {verdict}")

ref = manifest.training.reference_spec
evidence = {
    "run_id": manifest.run_id(),
    "backend": "cuda",
    "reference_hash": hashlib.sha256(canonicalize(ref.model_dump(mode="json"))).hexdigest(),
    "layout_hash": ref.layout.model_dump_json(),
    "configs": [fsha(c) for c in a.config],
    "results": [fsha(r) for r in a.result],
    "summaries": [
        fsha(r.parent / f"round-{w}/published/rank-{k}/summary.json")
        for r in a.result
        for w in range(2)
        for k in range(ref.layout.n_gpus)
    ],
}
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
    store._bootstrap_backend_v2(manifest, tuple(a.config), tuple(a.result), canonicalize(receipt))
    print(
        json.dumps(
            {"run_id": manifest.run_id(), "received_round": now, "authority_hash": authority}
        )
    )
finally:
    store.close()
