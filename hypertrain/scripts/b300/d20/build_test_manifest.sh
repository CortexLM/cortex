#!/usr/bin/env bash
# Build OFFLINE a B300 TEST run manifest (v2) for the S3-qualified dense decoder, then dry-run
# validate it with the repo's own parsers. Nothing is published, POSTed or contacted.
# NOTE: image f4b2885f is built from 2f4406bd, whose Environment.sm_counts is Literal[170], so a
# manifest pinned to f4b2885f can never get a 148-SM CUDA qualification; rerun this script with
# an image that contains commit 8a90d9bc.
#
# Usage: build_test_manifest.sh IMAGE_DIGEST DRIVER SM_COUNT OUT_DIR [EXP_ROUND]
#   OUT_DIR/secrets  disposable seeds (0700 dir, 0600 files, reused if present, never printed)
#   OUT_DIR/inputs   training/policies/relays JSON, public keys, synthetic dataset shard
#   OUT_DIR/out      manifest_v2 build output (must not exist yet)
#   OUT_DIR/state    object store of the policy and relay-registry objects
# EXP_ROUND defaults to the current drand quicknet round (local clock) + 864000 (~30 days).
# Model: the dense-1b-bf16 qualification geometry (scripts/mfu_profiles.json mfu-dense-1b,
# H=8 J=4 micro_batch=2 warmup=2 lr=3e-3, dense-int8), layout pp=1 ep=1 dp=2 n_gpus=2.
# The dataset is a synthetic 4096-sample u32 fixture (no quality claim).
set -euo pipefail

if [ "$#" -lt 4 ] || [ "$#" -gt 5 ]; then
    echo "usage: $0 IMAGE_DIGEST DRIVER SM_COUNT OUT_DIR [EXP_ROUND]" >&2
    exit 2
fi
IMAGE=$1
DRIVER=$2
SM_COUNT=$3
OUT=$4
EXP_ROUND=${5:-}
RELAY_URL=${HT_D20_RELAY_URL:-https://relay.joinbase.ai}
RELAY_PUBKEY=${HT_D20_RELAY_PUBKEY:-5G6pRydHHhv5Hr9PsHoDJdgbMpFJ86a47b5m2i7UYb2GfqCp}
HT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)

case $IMAGE in sha256:*) ;; *) echo "IMAGE_DIGEST must be sha256:<64 hex>" >&2; exit 2 ;; esac
for d in inputs out state; do
    if [ -e "$OUT/$d" ]; then echo "$OUT/$d already exists; use a fresh OUT_DIR" >&2; exit 2; fi
done
mkdir -p "$OUT"
if [ ! -d "$OUT/secrets" ]; then mkdir -m 0700 "$OUT/secrets"; fi
chmod 0700 "$OUT/secrets"
mkdir "$OUT/inputs"

cd "$HT"
uv run --no-sync python -I - "$IMAGE" "$DRIVER" "$SM_COUNT" "$OUT" "$EXP_ROUND" \
    "$RELAY_URL" "$RELAY_PUBKEY" <<'PY'
import json
import os
import secrets
import sys
import time
from pathlib import Path

import numpy as np

from hypertrain.ledger.escrow_v2 import genesis_allocation_hash
from hypertrain.miner.core import load_keyfile
from hypertrain.protocol.example import example_manifest
from hypertrain.protocol.hashing import MerkleTree, sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages import f32hex
from hypertrain.protocol.messages import drand_round_at
from hypertrain.trainer.rng import rng_ctr, uniform_f64

image, driver, sm_count, out, exp_round, relay_url, relay_pubkey = sys.argv[1:]
out_dir = Path(out)
secret_dir, inputs = out_dir / "secrets", out_dir / "inputs"
ROLES = ("owner", "coord", "auditor-0", "auditor-1", "referee", "cold-0", "cold-1")
ss58 = {}
for role in ROLES:
    path = secret_dir / f"{role}.seed"
    if not path.exists():
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, "w") as handle:
            handle.write(secrets.token_bytes(32).hex())
    ss58[role] = load_keyfile(path).ss58
ss58["relay-k1"] = relay_pubkey

SEQ, VOCAB, N = 2048, 32768, 4096
rows = np.empty((N, SEQ + 1), dtype="<u4")
for i in range(N):
    u = uniform_f64(SEQ + 1, rng_ctr("d20-dense-test-data", 0, i, 0))
    rows[i] = np.floor(u * VOCAB).astype("<u4")
(inputs / "samples.u32").write_bytes(rows.tobytes())
tree = MerkleTree([row.tobytes() for row in rows])

reference = canonicalize(
    {
        "purpose": "B300_TEST_RUN_NOT_PUBLIC_QUALITY",
        "qualification_profile": "dense-1b-bf16",
        "geometry_source": "scripts/mfu_profiles.json mfu-dense-1b; S3 dense_qualify.py inner",
        "image_digest": image,
        "driver_allowlist": [driver],
        "sm_count": int(sm_count),
        "dataset": "synthetic philox u32 ids, label d20-dense-test-data, 4096 x 2049",
    }
)
(inputs / "reference-spec.json").write_bytes(reference)

body = json.loads(example_manifest().model_dump_json())
body["model"].update(
    arch="decoder", n_layers=16, d_model=2048, n_heads=16, n_kv_heads=8, d_ff=5632,
    n_experts=1, top_k_experts=1, vocab=VOCAB, seq_len=SEQ, compute_dtype="bf16",
    param_count=889_260_032,
)
body["inner"].update(H=8, J=4, micro_batch=2, grad_accum=1, state_policy="reset", rewarmup_steps=0)
body["inner"]["lr_schedule"].update(peak_lr=f32hex(3e-3), warmup=2, stable=100000, decay=0)
body["tokenizer"] = {
    "name": "ht-synthetic-ids-32768",
    "sha256": sha256_hex(b"d20 synthetic token ids: no tokenizer, vocab 32768"),
}
body["dataset"].update(
    merkle_root=tree.root.hex(),
    depth=12,
    n_samples=N,
    sample_format="u32[seq_len+1] token ids",
    shard_uri_template="samples.u32",
    shard_sha256_root=MerkleTree([bytes.fromhex(sha256_hex(rows.tobytes()))]).root.hex(),
    holdout_commit=sha256_hex(b"D20 synthetic B300 test fixture: no quality holdout"),
)
body["coord_pubkey"] = ss58["coord"]
body["auditors"] = [ss58["auditor-0"], ss58["auditor-1"]]
body["reference_spec"].update(
    image_digest=image,
    driver_allowlist=[driver],
    sm_count=int(sm_count),
    spec_doc_sha256=sha256_hex(reference),
    layout={"pp": 1, "n_gpus": 2, "dp_size": 2, "ep_size": 1, "zero1": False},
)
del body["init_state_hash"]
(inputs / "training.json").write_bytes(canonicalize(body, allow_float=False))

outer = body["outer"]
policies = {
    "admission": {
        "q_base": "0000803e", "E": 4, "clean_finalizations": 12,
        "work_screen_epoch_rounds": 6000, "max_pending": 32, "max_trial_replays": 2,
        "max_trials_per_beacon": 2, "join_per_beacon": 2, "join_burst": 4,
        "ip_prefix_per_beacon": 32, "ip_prefix_burst": 64, "challenge_rounds": 100,
        "artifact_limits": {
            "max_object_bytes": 2147483648, "max_body_bytes": 65536,
            "max_chunk_manifest_bytes": 1048576,
        },
    },
    "economics": {
        "ledger_mode": "test", "G_max_units": 100, "R_collectible_units": 100,
        "S_min_units": 1000, "beta_ppm": 1000000, "gammaV_units": 0, "s_lower_ppm": 1000000,
        "q_floor": 1000000,
        "genesis_allocation_hash": genesis_allocation_hash(
            [(ss58["cold-0"], 20000, 1), (ss58["cold-1"], 20000, 1)]
        ),
        "reward_authority": ss58["coord"],
        "round_reward_units": body["budget"]["epochs_per_round"] * 1_000_000,
        "max_total_issuance": 20000000, "shadow_bootstrap_rounds": 12,
    },
    "aggregation": {
        "arithmetic": "flat-cclip-cap-v1", "order": "utf8", "center": "prev_outer_update",
        "trust_mode": "uniform-verified", "weight_quantum": 16777216,
        "miner_cap_units": 4194304, "probation_cap_units": 4194304, "owner_group_caps": {},
        "preclip_norm": outer["preclip_norm"], "cclip_tau": outer["cclip_tau"],
        "cclip_iters": outer["cclip_iters"],
    },
    "audit": {
        "lease_rounds": 100, "max_attempts": 2, "max_concurrent_per_auditor": 1,
        "max_running_jobs": 2, "max_queued_jobs": 32, "max_anchor_age_rounds": 1,
        "max_steps_per_attempt": 2 * body["inner"]["H"],
    },
    "dispute": {
        "referees": [ss58["referee"]], "max_open": 32, "max_transcript_entries": 256,
        "max_entry_bytes": 65536, "fanout": 2, "max_referee_reassignments": 1,
        "max_referee_cost_units": 100,
    },
}
(inputs / "policies.json").write_bytes(canonicalize(policies, allow_float=False))
relays = {
    "registry_version": 1, "epoch": 0, "previous_registry_hash": None,
    "specs": [{
        "id": "relay-joinbase", "region": "eu", "https_url": relay_url,
        "pubkeys": [{
            "key_id": "k1", "pubkey": relay_pubkey,
            "valid_from_round": 0, "valid_until_round": 1099511627776,
        }],
        "max_object_bytes": 2147483648, "max_inflight_bytes": 8589934592,
        "codecs": ["ht-sparse-v1", "ht-dense-int8-v1"], "mode": "transport",
    }],
}
(inputs / "relays.json").write_bytes(canonicalize(relays, allow_float=False))
(inputs / "public-keys.json").write_bytes(canonicalize(ss58, allow_float=False))
if not exp_round:
    exp_round = str(drand_round_at(int(time.time()), body["beacon"]["genesis_time"]) + 864000)
(inputs / "exp-round.txt").write_text(exp_round + "\n")
print(json.dumps({"public_keys": ss58, "exp_round": int(exp_round)}, sort_keys=True))
PY

uv run --no-sync python -I -m hypertrain.ops.manifest_v2 build \
    --training "$OUT/inputs/training.json" --policies "$OUT/inputs/policies.json" \
    --relays "$OUT/inputs/relays.json" --owner-seed "$OUT/secrets/owner.seed" \
    --exp-round "$(cat "$OUT/inputs/exp-round.txt")" --state-dir "$OUT/state" --out "$OUT/out"

uv run --no-sync python -I - "$OUT" "$IMAGE" "$DRIVER" "$SM_COUNT" <<'PY'
"""Dry-run: the checks POST /v2/admin/runs (store._create_run_v2) runs, minus the database."""
import json
import sys
from pathlib import Path

from hypertrain.data.store import LocalFSStore
from hypertrain.protocol import envelope_v2
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.messages_v2 import (
    AdmissionPolicyV2, AggregationPolicyV2, AuditPolicyV2, DisputePolicyV2,
    EconomicsPolicyV2, RunManifestV2,
)
from hypertrain.protocol.relay_messages import RelayRegistryV1

out, image, driver, sm_count = Path(sys.argv[1]), sys.argv[2], sys.argv[3], int(sys.argv[4])
keys = json.loads((out / "inputs/public-keys.json").read_bytes())
raw = (out / "out/manifest-envelope.json").read_bytes()
index = json.loads((out / "out/index.json").read_bytes())
env = envelope_v2.parse_envelope(raw)
model = envelope_v2.Intake(env.run_id, {"RunManifestV2": keys["owner"].__eq__}).accept(raw, 1)
assert isinstance(model, RunManifestV2)
model.validate_run_id(env.run_id)
store = LocalFSStore(out / "state/objects")
checks = {}
for name, cls in (
    ("admission_policy_hash", AdmissionPolicyV2),
    ("economics_policy_hash", EconomicsPolicyV2),
    ("aggregation_policy_hash", AggregationPolicyV2),
    ("audit_policy_hash", AuditPolicyV2),
    ("dispute_policy_hash", DisputePolicyV2),
):
    policy = cls.model_validate_json(store.get(getattr(model.network, name)))
    checks[name] = policy.digest() == getattr(model.network, name)
    if isinstance(policy, AuditPolicyV2):
        policy.validate_training(model.training)
registry = RelayRegistryV1.model_validate_json(store.get(model.network.relay_registry_hash))
checks["relay_registry_hash"] = registry.digest() == model.network.relay_registry_hash
t, ref = model.training, model.training.reference_spec
admission = AdmissionPolicyV2.model_validate_json(store.get(model.network.admission_policy_hash))
economics = EconomicsPolicyV2.model_validate_json(store.get(model.network.economics_policy_hash))
spec = registry.specs[0]
checks.update(
    envelope_signature=envelope_v2.verify_envelope(raw),
    signer_is_owner=env.signer == keys["owner"],
    run_id_recomputed=model.run_id() == env.run_id == index["run_id"],
    index_file_hashes=all(
        sha256_hex((out / "out" / n).read_bytes()) == h for n, h in index["files"].items()
    ),
    arch_decoder=t.model.arch == "decoder" and t.model.od is None,
    profile_none=ref.profile is None,
    layout=ref.layout.model_dump() == {
        "pp": 1, "n_gpus": 2, "dp_size": 2, "ep_size": 1, "zero1": False
    },
    image_digest=ref.image_digest == image,
    driver_allowlist=ref.driver_allowlist == [driver],
    sm_count=ref.sm_count == sm_count,
    ledger_mode_test=economics.ledger_mode == "test",
    work_screen_epoch_rounds_6000=admission.work_screen_epoch_rounds == 6000,
    relay=len(registry.specs) == 1 and spec.mode == "transport"
    and str(spec.https_url).rstrip("/") == "https://relay.joinbase.ai",
    coord_is_reward_authority=economics.reward_authority == t.coord_pubkey == keys["coord"],
    two_auditors=len(set(t.auditors)) == 2,
)
print(json.dumps({
    "run_id": env.run_id,
    "envelope_sha256": sha256_hex(raw),
    "exp_drand": env.exp_drand,
    "param_count": t.model.param_count,
    "init_state_hash": t.init_state_hash,
    "checks": checks,
}, sort_keys=True, indent=1))
if not all(checks.values()):
    sys.exit("DRY-RUN VALIDATION FAILED")
PY
