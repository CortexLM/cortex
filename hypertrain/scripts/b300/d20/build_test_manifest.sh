#!/usr/bin/env bash
# Build OFFLINE a B300 TEST run manifest (v2) with S3's dense decoder geometry, then dry-run
# validate it with the repo's own parsers. Nothing is published, POSTed or contacted.
# NOTE: image f4b2885f is built from 2f4406bd, whose Environment.sm_counts is Literal[170], so a
# manifest pinned to f4b2885f can never get a 148-SM CUDA qualification; rerun this script with
# an image that contains commit 8a90d9bc.
#
# Usage: build_test_manifest.sh IMAGE_DIGEST DRIVER SM_COUNT OUT_DIR [EXP_ROUND]
#        build_test_manifest.sh --clean OUT_DIR
#   OUT_DIR/secrets  disposable seeds (0700 dir, 0600 files, reused if present, never printed)
#   OUT_DIR/inputs   training/policies/relays JSON, public keys, synthetic dataset shard
#   OUT_DIR/out      manifest_v2 build output (must not exist yet)
#   OUT_DIR/state    object store of the policy and relay-registry objects
# IMAGE_DIGEST is sha256: and 64 lowercase hex, DRIVER digits and dots (or cpu-test, the tiny-cpu
# value), SM_COUNT and EXP_ROUND digits: all checked before anything is created (exit 2).
# EXP_ROUND defaults to the current drand quicknet round (local clock) + 864000 (~30 days).
# A build that fails or is interrupted (SIGINT, SIGTERM, SIGHUP) removes what it created and nothing
# else: OUT_DIR and its missing parents if it made them, secrets/ if it made it, the seeds it made
# (pre-staged seeds stay), inputs, out and state. The exit code is the failure's. SIGKILL cannot be
# trapped: clean that OUT_DIR with --clean.
# --clean OUT_DIR removes secrets, inputs, out and state under OUT_DIR, then OUT_DIR if it is empty,
# when the campaign is over. It exits 2 and removes nothing for a relative path, / , a symlinked
# OUT_DIR, secrets, inputs, out or state (a trailing slash is stripped first), and a directory it
# cannot prove is a builder output: it needs inputs/training.json with inputs/public-keys.json, or
# a seed in secrets/; secrets/ may hold only *.seed files and state/ only objects/ (so the TEST
# stack's own secrets/ and state/ are refused). It prints directory names only.
# HT_D20_GEOMETRY=dense-1b (default): the dense-1b-bf16 qualification geometry
# (scripts/mfu_profiles.json mfu-dense-1b, H=8 J=4 micro_batch=2 warmup=2 lr=3e-3, dense-int8),
# layout pp=1 ep=1 dp=2 n_gpus=2, a synthetic 4096-sample u32 fixture (no quality claim).
# HT_D20_GEOMETRY=dense-0.1b: the intermediate TEST model of D68 (ADR 0015), 100,676,352 parameters,
# read from the row mfu-dense-0.1b of scripts/mfu_profiles.json (8 layers, d_model 768, seq 2048,
# bf16), layout n_gpus=1, H=J=4 micro_batch=2 (two checkpoints per result), the dense-1b fixture
# of 4096 samples and work_screen_epoch_rounds 20000 (the other geometries keep 6000). The build
# refuses a row whose shape-only parameter count leaves the band 100,000,000..300,000,000.
# HT_D20_GEOMETRY=tiny-cpu: the CPU mechanism model of tests/ledger/test_escrow_v2.py setup()
# (1 layer, d_model 8, vocab 16, seq 4, H=1 J=1, 16 samples), layout n_gpus=1, for a cpu-backend
# TEST run whose references and audits the challenge replays on CPU.
# HT_D20_COLDKEYS=N (default 2) seeds cold-0..cold-N-1, each with 20000 genesis units.
# HT_D20_RELAY_PUBKEY (required): SS58 of the relay key k1 that relay-joinbase signs with; the
# current value is the `pubkey` of /data/relay/config/relay.json on s100 (k1).
# inputs/genesis-allocation.json and inputs/proofs.json feed the run's genesis and dataset intake.
# Only the model geometry and inner settings come from S3's dense_qualify.py; the dp2 layout
# and the codec error-feedback setting were not part of S3's qualification. The dry run checks
# expiry at round 1, and it cannot show that POSTing this run before its CUDA qualification
# record exists freezes the run's backend to cpu. HT_D20_RELAY_URL overrides are not
# supported by the dry run, which expects https://relay.joinbase.ai.
set -euo pipefail

if [ "${1:-}" = --clean ]; then
    [ "$#" -eq 2 ] || { echo "usage: $0 --clean OUT_DIR" >&2; exit 2; }
    OUT=$2
    case $OUT in /*) ;; *) echo "OUT_DIR must be an absolute path" >&2; exit 2 ;; esac
    # A trailing slash (shell completion adds one to a symlinked directory) would make -L false.
    while [ "$OUT" != / ] && [ "${OUT%/}" != "$OUT" ]; do OUT=${OUT%/}; done
    [ "$OUT" != / ] || { echo "OUT_DIR must not be /" >&2; exit 2; }
    for p in "$OUT" "$OUT/secrets" "$OUT/inputs" "$OUT/out" "$OUT/state"; do
        if [ -L "$p" ]; then echo "$p is a symlink; nothing removed" >&2; exit 2; fi
    done
    # rm -rf as root: remove only what is provably a builder output, never another stack's
    # secrets/ or state/ (the TEST stack has both, with coord.key and challenge.db in them).
    not_ours() { echo "$OUT is not a builder output ($1); nothing removed" >&2; exit 2; }
    proof=0
    if [ -f "$OUT/inputs/training.json" ] && [ -f "$OUT/inputs/public-keys.json" ]; then proof=1; fi
    if [ -d "$OUT/secrets" ]; then
        for f in "$OUT/secrets"/* "$OUT/secrets"/.[!.]*; do
            { [ -e "$f" ] || [ -L "$f" ]; } || continue
            case ${f##*/} in
                *.seed) { [ -f "$f" ] && [ ! -L "$f" ]; } || not_ours "secrets/${f##*/}"; proof=1 ;;
                *) not_ours "secrets/${f##*/}" ;;
            esac
        done
    fi
    if [ -d "$OUT/state" ]; then
        for f in "$OUT/state"/* "$OUT/state"/.[!.]*; do
            { [ -e "$f" ] || [ -L "$f" ]; } || continue
            [ "${f##*/}" = objects ] || not_ours "state/${f##*/}"
        done
    fi
    [ "$proof" -eq 1 ] || not_ours "no inputs/training.json with public-keys.json, no seed"
    removed=()
    for d in secrets inputs out state; do
        if [ -e "$OUT/$d" ]; then rm -rf -- "${OUT:?}/$d"; removed+=("$d"); fi
    done
    echo "removed under $OUT: ${removed[*]:-nothing}"
    if rmdir -- "$OUT" 2>/dev/null; then echo "removed $OUT"; else echo "kept $OUT (not empty)"; fi
    exit 0
fi

if [ "$#" -lt 4 ] || [ "$#" -gt 5 ]; then
    echo "usage: $0 IMAGE_DIGEST DRIVER SM_COUNT OUT_DIR [EXP_ROUND]" >&2
    echo "       $0 --clean OUT_DIR" >&2
    exit 2
fi
IMAGE=$1
DRIVER=$2
SM_COUNT=$3
OUT=$4
EXP_ROUND=${5:-}
RELAY_URL=${HT_D20_RELAY_URL:-https://relay.joinbase.ai}
RELAY_PUBKEY=${HT_D20_RELAY_PUBKEY:?set HT_D20_RELAY_PUBKEY to the SS58 of the TEST relay key k1}
GEOMETRY=${HT_D20_GEOMETRY:-dense-1b}
COLDKEYS=${HT_D20_COLDKEYS:-2}
HT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)

case $GEOMETRY in dense-1b|dense-0.1b|tiny-cpu) ;; *) echo "HT_D20_GEOMETRY: dense-1b, dense-0.1b or tiny-cpu" >&2; exit 2 ;; esac
case $COLDKEYS in [1-9]) ;; *) echo "HT_D20_COLDKEYS must be 1..9" >&2; exit 2 ;; esac
[[ $IMAGE =~ ^sha256:[0-9a-f]{64}$ ]] || { echo "IMAGE_DIGEST must be sha256:<64 lowercase hex>" >&2; exit 2; }
[[ $DRIVER =~ ^([0-9][0-9.]*|cpu-test)$ ]] || { echo "DRIVER must be digits and dots (or cpu-test)" >&2; exit 2; }
[[ $SM_COUNT =~ ^[0-9]+$ ]] || { echo "SM_COUNT must be digits" >&2; exit 2; }
[ -z "$EXP_ROUND" ] || [[ $EXP_ROUND =~ ^[0-9]+$ ]] || { echo "EXP_ROUND must be digits" >&2; exit 2; }
# The Python stages run from $HT, so a relative OUT_DIR would point somewhere else there.
case $OUT in /*) ;; *) echo "OUT_DIR must be an absolute path" >&2; exit 2 ;; esac
while [ "$OUT" != / ] && [ "${OUT%/}" != "$OUT" ]; do OUT=${OUT%/}; done
[ "$OUT" != / ] || { echo "OUT_DIR must not be /" >&2; exit 2; }
if [ -L "$OUT" ] || [ -L "$OUT/secrets" ]; then echo "OUT_DIR or its secrets/ is a symlink" >&2; exit 2; fi
for d in inputs out state; do
    if [ -e "$OUT/$d" ] || [ -L "$OUT/$d" ]; then echo "$OUT/$d already exists; use a fresh OUT_DIR" >&2; exit 2; fi
done

# From here on a failed run removes what it created and nothing else. inputs, out and state did
# not exist above, so whatever is there then is this run's; of secrets/ only the seeds that were not
# there before are removed, and the directories only when this run made them and they are empty.
new_root=
if [ ! -e "$OUT" ]; then
    new_root=$OUT
    while [ ! -e "$(dirname -- "$new_root")" ]; do new_root=$(dirname -- "$new_root"); done
fi
made_seed_dir=0
declare -A prestaged=()
if [ -d "$OUT/secrets" ]; then
    for f in "$OUT/secrets"/*; do [ ! -e "$f" ] || prestaged[${f##*/}]=1; done
else
    made_seed_dir=1
fi
finished=0
clean_failed_run() {
    local rc=$? f d
    trap - EXIT
    # A script killed by SIGTERM or SIGHUP (an ssh drop) runs this trap with $? still 0, so a build
    # that did not reach its last line counts as failed whatever $? says.
    if [ "$rc" -ne 0 ] || [ "$finished" -ne 1 ]; then
        set +e
        rm -rf -- "$OUT/inputs" "$OUT/out" "$OUT/state"
        for f in "$OUT/secrets"/*; do
            [ ! -e "$f" ] || [ -n "${prestaged[${f##*/}]:-}" ] || rm -f -- "$f"
        done
        [ "$made_seed_dir" -eq 0 ] || rmdir -- "$OUT/secrets" 2>/dev/null
        if [ -n "$new_root" ]; then
            d=$OUT
            while rmdir -- "$d" 2>/dev/null && [ "$d" != "$new_root" ]; do d=$(dirname -- "$d"); done
        fi
        echo "build failed or interrupted (exit $rc): removed what this run created under $OUT" >&2
    fi
    exit "$rc"
}
trap clean_failed_run EXIT

mkdir -p "$OUT"
if [ ! -d "$OUT/secrets" ]; then mkdir -m 0700 "$OUT/secrets"; fi
chmod 0700 "$OUT/secrets"
mkdir "$OUT/inputs"

cd "$HT"
uv run --no-sync python -I - "$IMAGE" "$DRIVER" "$SM_COUNT" "$OUT" "$EXP_ROUND" \
    "$RELAY_URL" "$RELAY_PUBKEY" "$GEOMETRY" "$COLDKEYS" "$HT" <<'PY'
import json
import math
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

image, driver, sm_count, out, exp_round, relay_url, relay_pubkey, geometry, coldkeys, ht_root = (
    sys.argv[1:]
)
out_dir = Path(out)
secret_dir, inputs = out_dir / "secrets", out_dir / "inputs"
COLD = tuple(f"cold-{i}" for i in range(int(coldkeys)))
ROLES = ("owner", "coord", "auditor-0", "auditor-1", "referee", *COLD)
ss58 = {}
for role in ROLES:
    path = secret_dir / f"{role}.seed"
    if not path.exists():
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, "w") as handle:
            handle.write(secrets.token_bytes(32).hex())
    ss58[role] = load_keyfile(path).ss58
ss58["relay-k1"] = relay_pubkey

body = json.loads(example_manifest().model_dump_json())
if geometry == "dense-1b":
    SEQ, VOCAB, N = 2048, 32768, 4096
    rows = np.empty((N, SEQ + 1), dtype="<u4")
    for i in range(N):
        u = uniform_f64(SEQ + 1, rng_ctr("d20-dense-test-data", 0, i, 0))
        rows[i] = np.floor(u * VOCAB).astype("<u4")
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
    body["model"].update(
        arch="decoder", n_layers=16, d_model=2048, n_heads=16, n_kv_heads=8, d_ff=5632,
        n_experts=1, top_k_experts=1, vocab=VOCAB, seq_len=SEQ, compute_dtype="bf16",
        param_count=889_260_032,
    )
    body["inner"].update(
        H=8, J=4, micro_batch=2, grad_accum=1, state_policy="reset", rewarmup_steps=0
    )
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
    layout = {"pp": 1, "n_gpus": 2, "dp_size": 2, "ep_size": 1, "zero1": False}
elif geometry == "dense-0.1b":
    from hypertrain.trainer.config import TrainConfig
    from hypertrain.trainer.model import param_shapes

    # ADR 0015: the model block is the registry row; the fixture, the inner loop and the band are here.
    profiles = json.loads((Path(ht_root) / "scripts/mfu_profiles.json").read_bytes())["profiles"]
    row = profiles["mfu-dense-0.1b"]["model"]
    SEQ, VOCAB, N = row["seq_len"], row["vocab"], 4096
    BAND = (100_000_000, 300_000_000)
    body["model"].update(arch="decoder", **row)
    count = sum(math.prod(s) for s in param_shapes(TrainConfig.from_manifest(body).model).values())
    if not BAND[0] <= count <= BAND[1]:
        sys.exit(f"mfu-dense-0.1b has {count} parameters, outside the band {BAND[0]}..{BAND[1]}")
    rows = np.empty((N, SEQ + 1), dtype="<u4")
    for i in range(N):
        u = uniform_f64(SEQ + 1, rng_ctr("d20-dense-test-data", 0, i, 0))
        rows[i] = np.floor(u * VOCAB).astype("<u4")
    tree = MerkleTree([r.tobytes() for r in rows])
    reference = canonicalize(
        {
            "purpose": "S100_TEST_CUDA_RUN_NOT_PUBLIC_QUALITY",
            "qualification_profile": "dense-0.1b-bf16",
            "geometry_source": "scripts/mfu_profiles.json mfu-dense-0.1b; ADR 0015",
            "image_digest": image,
            "driver_allowlist": [driver],
            "sm_count": int(sm_count),
            "dataset": f"synthetic philox u32 ids, label d20-dense-test-data, {N} x {SEQ + 1}",
        }
    )
    body["model"]["param_count"] = count
    # J = H: the executor result keeps two checkpoints (12 P bytes each), the fewest the artifact
    # validation accepts; H = 4 stays under the 30 steps of the 100-round audit lease.
    body["inner"].update(
        H=4, J=4, micro_batch=2, grad_accum=1, state_policy="reset", rewarmup_steps=0
    )
    body["inner"]["lr_schedule"].update(peak_lr=f32hex(3e-3), warmup=2, stable=100000, decay=0)
    body["tokenizer"] = {
        "name": f"ht-synthetic-ids-{VOCAB}",
        "sha256": sha256_hex(f"d20 synthetic token ids: no tokenizer, vocab {VOCAB}".encode()),
    }
    body["dataset"].update(
        merkle_root=tree.root.hex(),
        depth=(N - 1).bit_length(),
        n_samples=N,
        sample_format="u32[seq_len+1] token ids",
        shard_uri_template="samples.u32",
        shard_sha256_root=MerkleTree([bytes.fromhex(sha256_hex(rows.tobytes()))]).root.hex(),
        holdout_commit=sha256_hex(b"D20 synthetic B300 test fixture: no quality holdout"),
    )
    layout = {"pp": 1, "n_gpus": 1, "dp_size": 1, "ep_size": 1, "zero1": False}
else:
    from hypertrain.trainer.config import TrainConfig
    from hypertrain.trainer.model import param_shapes

    # Same model, inner loop and samples as tests/ledger/test_escrow_v2.py setup(), which the
    # CPU network E2E (scripts/mine_multi_round_v2.py) mines through audits to get_weights.
    rows = (np.arange(16 * 5).reshape(16, 5) % 16).astype("<u4")
    tree = MerkleTree([row.tobytes() for row in rows])
    reference = canonicalize(
        {
            "purpose": "S100_TEST_CPU_RUN_NOT_PUBLIC_QUALITY",
            "geometry_source": "tests/ledger/test_escrow_v2.py setup() mechanism model",
            "image_digest": image,
            "driver_allowlist": [driver],
            "sm_count": int(sm_count),
            "dataset": "16 x 5 u32 ids, arange % 16",
        }
    )
    body["model"].update(
        n_layers=1, d_model=8, n_heads=2, n_kv_heads=2, d_ff=12, vocab=16, seq_len=4,
        n_experts=1, top_k_experts=1,
    )
    body["inner"].update(H=1, J=1, micro_batch=1, grad_accum=1, state_policy="reset")
    body["inner"]["lr_schedule"].update(warmup=0, stable=100, peak_lr=f32hex(0.001))
    body["model"]["param_count"] = sum(
        int(np.prod(s)) for s in param_shapes(TrainConfig.from_manifest(body).model).values()
    )
    body["dataset"].update(merkle_root=tree.root.hex(), n_samples=16, depth=4)
    layout = {"pp": 1, "n_gpus": 1, "dp_size": 1, "ep_size": 1, "zero1": False}
(inputs / "samples.u32").write_bytes(rows.tobytes())
(inputs / "proofs.json").write_bytes(
    canonicalize([[p.hex() for p in tree.proof(i)] for i in range(len(rows))])
)
(inputs / "reference-spec.json").write_bytes(reference)
body["coord_pubkey"] = ss58["coord"]
body["auditors"] = [ss58["auditor-0"], ss58["auditor-1"]]
body["reference_spec"].update(
    image_digest=image,
    driver_allowlist=[driver],
    sm_count=int(sm_count),
    spec_doc_sha256=sha256_hex(reference),
    layout=layout,
)
del body["init_state_hash"]
(inputs / "training.json").write_bytes(canonicalize(body, allow_float=False))

outer = body["outer"]
allocation = [(ss58[c], 20000, 1) for c in COLD]
(inputs / "genesis-allocation.json").write_bytes(canonicalize(allocation))
# docs/network-v2.md (admission): the first screen must cover the whole serial campaign. For the
# 100 M model that is 52 trials of 83..130 rounds (250..390 s) plus about 470 rounds to the
# d_final of round 0, i.e. 4800..7200 rounds: 6000 holds only on the fastest link.
screen_rounds = 20000 if geometry == "dense-0.1b" else 6000
policies = {
    "admission": {
        "q_base": "0000803e", "E": 4, "clean_finalizations": 12,
        "work_screen_epoch_rounds": screen_rounds, "max_pending": 32, "max_trial_replays": 2,
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
        "genesis_allocation_hash": genesis_allocation_hash(allocation),
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

uv run --no-sync python -I - "$OUT" "$IMAGE" "$DRIVER" "$SM_COUNT" "$GEOMETRY" <<'PY'
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
n_gpus = 2 if sys.argv[5] == "dense-1b" else 1
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
        "pp": 1, "n_gpus": n_gpus, "dp_size": n_gpus, "ep_size": 1, "zero1": False
    },
    image_digest=ref.image_digest == image,
    driver_allowlist=ref.driver_allowlist == [driver],
    sm_count=ref.sm_count == sm_count,
    ledger_mode_test=economics.ledger_mode == "test",
    work_screen_epoch_rounds=admission.work_screen_epoch_rounds
    == (20000 if sys.argv[5] == "dense-0.1b" else 6000),
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
finished=1
