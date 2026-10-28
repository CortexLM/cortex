"""Build the od-large 8x RTX 5090 RunManifestV2 + profile + genesis seed job (DiLoCo H=512).

Port of .e2e/manifest-prep/od_large_e2e_manifest.py. The dataset is a built shards16 set
(datasets.build, mix od-a-v1); training.dataset is taken from its manifest.json, never packed
here. Miners fetch their assigned units by HTTP Range (HYPERTRAIN_DATA_BASES); only the w=0 seed
job's samples are sliced locally from the shards.

Run with the snapshot venv, PYTHONPATH=<tree>/src:<opendecision>/src. --dry-run writes every
artifact under --out-root (fresh dirs) using throwaway role/owner/escrow keys and never touches
--secrets; the real run takes the approved seeds. Never prints seed material.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import math
import os
import stat
import time
from pathlib import Path

import numpy as np

import hypertrain.trainer  # noqa: F401  (determinism pins before torch)
from hypertrain.auditor.replay import AnchorCache, pack_state
from hypertrain.datasets.shards16 import (
    SAMPLE_FORMAT_U16,
    ShardSet16Manifest,
    U16ShardSamples,
    shard_name16,
)
from hypertrain.gpu_ops.network_qualification import sources
from hypertrain.ledger.escrow_v2 import genesis_allocation_hash
from hypertrain.protocol import envelope_v2
from hypertrain.protocol.env_pins import class_pin
from hypertrain.protocol.example import example_manifest
from hypertrain.protocol.hashing import MerkleTree, sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import f32hex
from hypertrain.protocol.messages_v2 import (
    AdmissionPolicyV2,
    AggregationPolicyV2,
    ArtifactLimits,
    AuditPolicyV2,
    DisputePolicyV2,
    EconomicsPolicyV2,
    IslandJobV1,
    RunManifestV2,
    audit_lease_rounds,
)
from hypertrain.protocol.relay_messages import RelayRegistryV1
from hypertrain.trainer.compress import state_hash
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.model import init_params

D, LAYERS, HEADS, HEAD_LAYERS, VOCAB, SEQ = 1024, 24, 16, 4, 50432, 1024
N_GPUS, GPU_NAME, SM_COUNT = 8, "RTX 5090", 170
# 170 SMs is sm_120. The OD encoder calls torch SDPA itself and never reads trainer.attention,
# so HT_ATTN_BACKEND is pinned to the neutral value.
ENV_PIN = class_pin("avx2", "sm_120", "math")
PARAMS = 353861632
# Inner shape: H=512 (owner asked for 500; 512 * mb1 * ga1 * 8 GPUs = 4096 = 8 units of 512).
H, J, MICRO_BATCH, GRAD_ACCUM = 512, 64, 1, 1
ASSIGN_UNIT = 512
STEP_SECONDS = 4  # untraced inner-step bound (est. 1-3 s measured; trace hashing excluded)
EPOCH_SECONDS = 4320  # HYPERTRAIN_EPOCH_SECONDS default (challenge/app.py)
BEACON_PERIOD = 3
OUTER_LR, OUTER_MOMENTUM, CLIP = 0.7, 0.9, 256.0
PER_HOST, HOSTS, ROSTER = 7, 2, 4  # 2 qualification rounds + 5 workload executions per host
CLEANUP_MARGIN = 1800  # = schedule hard_margin_seconds
EXP_MARGIN_ROUNDS = 100  # 300 s at the 3 s drand period
ROLES = ("auditor-0", "auditor-1", "referee")
DATA_BASE = "http://185.8.107.43:18980/"


def private(path: Path) -> bytes:
    mode = path.lstat().st_mode
    if not stat.S_ISREG(mode) or mode & 0o077:
        raise SystemExit(f"{path.name}: regular 0600 file required")
    raw = path.read_bytes()
    if len(raw) != 32:
        raise SystemExit(f"{path.name}: 32-byte seed required")
    return raw


def seed_of(secrets: Path, role: str) -> Keypair:
    path = secrets / f"{role}.seed"
    if not path.exists():
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, "wb") as f:
            f.write(os.urandom(32))
            f.flush()
            os.fsync(f.fileno())
    return Keypair(private(path))


def fresh(path: Path) -> Path:
    if path.exists():
        raise SystemExit(f"{path}: must not exist (no stale artifacts)")
    path.mkdir(parents=True, mode=0o700)
    return path


def round_seconds() -> int:
    """One round's wall time: open to final; d_commit already holds 1.5 * H * STEP_SECONDS."""
    return offsets()[-1] * BEACON_PERIOD


def offsets() -> list[int]:
    """live_operator d_open..d_final beacon offsets for one H=512 round (3 s rounds).

    open 150 / assign 300 (CPU start-state build, as before); commit after the execution bound
    H*STEP_SECONDS (+50% slack); audit +20; upload +600; final = audit + 200 (live_operator caps
    d_final - d_audit at 200)."""
    commit = 300 + math.ceil(1.5 * H * STEP_SECONDS / BEACON_PERIOD)
    return [150, 300, commit, commit + 20, commit + 200, commit + 220]


def dataset_spec(manifest_path: Path, holdout_path: Path) -> tuple[dict, ShardSet16Manifest]:
    m = ShardSet16Manifest.from_json(manifest_path.read_text())
    hold = ShardSet16Manifest.from_json(holdout_path.read_text())
    if (
        m.sample_format != SAMPLE_FORMAT_U16
        or m.seq_len != SEQ
        or m.unit != ASSIGN_UNIT
        or m.n_samples % ASSIGN_UNIT
        or len(m.unit_sha256s) * m.unit != m.n_samples
        or MerkleTree([bytes.fromhex(h) for h in m.unit_sha256s]).root.hex() != m.unit_sha256_root
        or MerkleTree([bytes.fromhex(h) for h in m.shard_sha256s]).root.hex() != m.shard_sha256_root
        or hold.seq_len != SEQ
    ):
        raise SystemExit("dataset manifest is not a consistent u16 shards16 set at unit 512")
    batch = H * MICRO_BATCH * GRAD_ACCUM * N_GPUS
    if ROSTER * batch > m.n_samples:
        raise SystemExit("roster x batch exceeds dataset n_samples")
    spec = {
        "merkle_root": m.merkle_root,
        "depth": m.depth,
        "n_samples": m.n_samples,
        "sample_format": m.sample_format,
        "shard_uri_template": m.shard_uri_template,
        "shard_sha256_root": m.shard_sha256_root,
        "holdout_commit": hold.merkle_root,  # public holdout set root (no hidden salt in e2e)
        "assign_unit": ASSIGN_UNIT,
        "unit_sha256_root": m.unit_sha256_root,
    }
    return spec, m


def relay_registry(staging: Path) -> RelayRegistryV1:
    relays = json.loads(staging.read_bytes())["relays"]
    if sorted(r["id"] for r in relays) != ["relay-de", "relay-sg", "relay-us"]:
        raise SystemExit("exactly the DE/US/SG relay pins required")
    return RelayRegistryV1.model_validate(
        {
            "registry_version": 1,
            "epoch": 0,
            "previous_registry_hash": None,
            "specs": [
                {
                    "id": r["id"],
                    "region": r["id"].removeprefix("relay-"),
                    "https_url": r["https_url"],
                    "pubkeys": [
                        {
                            "key_id": r["key_id"],
                            "pubkey": r["pubkey"],
                            "valid_from_round": 0,
                            "valid_until_round": 2**40,
                        }
                    ],
                    "max_object_bytes": 2 << 30,
                    "max_inflight_bytes": 8 << 30,
                    "codecs": ["ht-sparse-v1", "ht-dense-int8-v1"],
                    "mode": "transport",
                }
                for r in sorted(relays, key=lambda r: r["id"])
            ],
        }
    )


def orchestrate_module(tree: Path):
    spec = importlib.util.spec_from_file_location(
        "orchestrate", tree / "experiments/gpu_network_v2/orchestrate.py"
    )
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def profile_for(
    tree: Path,
    body: dict,
    image: str,
    drivers: list[str],
    disk: int,
    custody: int,
    *,
    dry_run: bool,
) -> dict:
    """Exact od-large/8-GPU profile; qualification compares model/inner/outer/layout bytes."""
    profile = json.loads((tree / "experiments/gpu_network_v2/profile.json").read_bytes())
    profile["model"] = body["model"]
    profile["inner"] = body["inner"]
    profile["outer"] = body["outer"]
    profile["layout"] = body["reference_spec"]["layout"]
    execution = round_seconds()  # 4632 s: open..final at H=512, 4 s/step
    # Qualification = 2 full rounds per host plus probe; staging/rescue windows as before.
    probe = 2 * execution + 1200
    rescue_delete = 3600
    qualification = 1800 + probe + rescue_delete + 600
    hard_deadline = 1800 + 600 + rescue_delete + 1800 + PER_HOST * execution
    hard_deadline = max(hard_deadline, qualification + 1800)
    profile["schedule"] |= {
        "hosts": HOSTS,
        "executions_per_host": PER_HOST,
        "executions_total": HOSTS * PER_HOST,
        "logical_identities": 1,
        "shadow_participations_per_identity": 0,
        "shadow_executions": 0,
        "live_executions": HOSTS * (PER_HOST - 2),
        "probes": 0,
        "fault_trace_reserve": 0,
        "staging_seconds": 1800,
        "beacon_service_seconds": 600,
        "rescue_delete_seconds": rescue_delete,
        "execution_transfer_seconds": execution,
        "hard_margin_seconds": 1800,
        "hard_deadline_seconds": hard_deadline,
    }
    profile["microqualification"] |= {
        "staging_seconds": 1800,
        "probe_and_transfer_seconds": probe,
        "rescue_delete_seconds": rescue_delete,
        "failure_margin_seconds": 600,
        "qualification_deadline_seconds": qualification,
        "max_full_rounds_per_host": 2,
    }
    profile["admission"] |= {
        "ceiling_usd": "500",
        "cleanup_reserve_usd": "5",
        "max_host_hourly_usd": "10",
        "gpu_name": GPU_NAME,
        "sm_count": SM_COUNT,
        "max_ancillary_usd": "50",
        "gpus_per_host": N_GPUS,
        "min_disk_gib": disk >> 30,
    }
    profile["artifacts"] |= {
        "disk_bytes": disk,
        "remote_retention": "prune_rescued",
        "local_custody_bytes": custody,
        "rescue_transfer_seconds": 3300,
        "delete_absence_seconds": 300,
    }
    profile["runtime"] |= {"image_digest": image, "driver_allowlist": drivers, "qualified": False}
    orchestrate = orchestrate_module(tree)
    bounds = orchestrate.artifact_budget(profile)
    orchestrate.execution_quota(profile)
    orchestrate.admission_limits(profile)
    timing = orchestrate.lifecycle_timing(profile)
    if timing["execution_cap"] < execution:
        raise SystemExit(f"execution cap {timing['execution_cap']} s < round {execution} s")
    # A dry run reports a misfit instead of refusing; the real run refuses.
    misfit = []
    if bounds["local_custody_bytes_required"] > custody:
        misfit.append("local custody free bytes below prune_rescued requirement")
    if bounds["disk_artifacts_peak_bytes"] >= disk:
        misfit.append("remote artifact peak exceeds host disk")
    if misfit and not dry_run:
        raise SystemExit("; ".join(misfit))
    profile["budget_bounds"] = bounds
    profile["budget_misfit"] = misfit
    return profile


def main() -> None:
    p = argparse.ArgumentParser()
    p.add_argument(
        "--dataset-manifest",
        type=Path,
        required=True,
        help="built shards16 manifest.json (e.g. .../od-a-v1-2m/manifest.json)",
    )
    p.add_argument(
        "--holdout-manifest", type=Path, help="default: <dataset dir>/holdout/manifest.json"
    )
    p.add_argument("--tokenizer", type=Path, required=True)
    p.add_argument("--tree", type=Path, required=True)
    p.add_argument("--registry-manifest", type=Path, required=True)
    p.add_argument("--relays", type=Path, required=True)
    p.add_argument("--custody-dir", type=Path, required=True)
    p.add_argument(
        "--out-root", type=Path, required=True, help="fresh out/ and job/ are created beneath it"
    )
    p.add_argument(
        "--dry-run",
        action="store_true",
        help="ephemeral keys, no --secrets writes; budget misfit reported not fatal",
    )
    for name in ("owner-seed", "escrow-seed", "secrets"):
        p.add_argument(f"--{name}", type=Path)
    p.add_argument("--coord-pubkey", required=True)
    p.add_argument("--hotkey", required=True)
    p.add_argument("--driver", action="append", required=True)
    p.add_argument("--host-disk-bytes", type=int, required=True)
    p.add_argument("--exp-round", type=int)
    p.add_argument("--job-deadline", type=int)
    a = p.parse_args()
    now = int(time.time())
    h_end = now + 43200
    job_deadline = a.job_deadline or now + 4 * 3600
    if job_deadline > h_end - CLEANUP_MARGIN:
        raise SystemExit("job deadline exceeds H end minus cleanup margin")
    if a.dry_run:
        keys = {r: Keypair(os.urandom(32)) for r in ROLES}
        owner, escrow = Keypair(os.urandom(32)), Keypair(os.urandom(32))
    else:
        if not (a.owner_seed and a.escrow_seed and a.secrets):
            raise SystemExit("--owner-seed, --escrow-seed and --secrets required (or --dry-run)")
        a.secrets.mkdir(parents=True, exist_ok=True, mode=0o700)
        keys = {r: seed_of(a.secrets, r) for r in ROLES}
        escrow, owner = Keypair(private(a.escrow_seed)), Keypair(private(a.owner_seed))
    image = "sha256:" + sha256_hex(a.registry_manifest.read_bytes())
    tree_sources = sources(a.tree)
    out, job_dir = fresh(a.out_root / "out"), fresh(a.out_root / "job")
    vfs = os.statvfs(a.custody_dir)
    custody = vfs.f_bavail * vfs.f_frsize
    holdout = a.holdout_manifest or a.dataset_manifest.parent / "holdout/manifest.json"
    ds, shards = dataset_spec(a.dataset_manifest, holdout)
    allocations = [(escrow.ss58, 10000, 1)]
    # epochs_per_round: whole HYPERTRAIN_EPOCH_SECONDS epochs per round wall time, >= 1.
    epochs = max(1, round(round_seconds() / EPOCH_SECONDS))
    lease = audit_lease_rounds(H, float(STEP_SECONDS))
    policies = {
        "admission_policy_hash": AdmissionPolicyV2(
            q_base="0000803e",
            E=4,
            clean_finalizations=12,
            work_screen_epoch_rounds=6000,
            max_pending=32,
            max_trial_replays=2,
            max_trials_per_beacon=2,
            join_per_beacon=2,
            join_burst=4,
            ip_prefix_per_beacon=32,
            ip_prefix_burst=64,
            challenge_rounds=100,
            artifact_limits=ArtifactLimits(
                max_object_bytes=2 << 30, max_body_bytes=65536, max_chunk_manifest_bytes=1048576
            ),
        ),
        "economics_policy_hash": EconomicsPolicyV2(
            ledger_mode="test",
            G_max_units=100,
            R_collectible_units=100,
            S_min_units=1000,
            beta_ppm=1000000,
            gammaV_units=0,
            s_lower_ppm=1000000,
            q_floor=1000000,
            genesis_allocation_hash=genesis_allocation_hash(allocations),
            reward_authority=a.coord_pubkey,
            round_reward_units=epochs * 1000000,
            max_total_issuance=20000000 * epochs,
            shadow_bootstrap_rounds=12,
        ),
        "aggregation_policy_hash": AggregationPolicyV2(
            arithmetic="flat-cclip-cap-v1",
            order="utf8",
            center="prev_outer_update",
            trust_mode="uniform-verified",
            weight_quantum=16777216,
            miner_cap_units=4194304,
            probation_cap_units=4194304,
            owner_group_caps={},
            preclip_norm=f32hex(CLIP),
            cclip_tau=f32hex(CLIP),
            cclip_iters=1,
        ),
        "audit_policy_hash": AuditPolicyV2(
            lease_rounds=lease,
            step_seconds_bound=f32hex(float(STEP_SECONDS)),
            max_attempts=2,
            max_concurrent_per_auditor=1,
            max_running_jobs=2,
            max_queued_jobs=32,
            max_anchor_age_rounds=1,
            max_steps_per_attempt=2 * H,
        ),
        "dispute_policy_hash": DisputePolicyV2(
            referees=[keys["referee"].ss58],
            max_open=32,
            max_transcript_entries=256,
            max_entry_bytes=65536,
            fanout=2,
            max_referee_reassignments=1,
            max_referee_cost_units=100,
        ),
        "relay_registry_hash": relay_registry(a.relays),
    }
    body = json.loads(example_manifest().model_dump_json())
    count = VOCAB * D + LAYERS * (12 * D * D + 9 * D + 4 * (D // HEADS)) + 2 * D
    assert count == PARAMS
    body["model"].update(
        arch="od-encoder",
        n_layers=LAYERS,
        d_model=D,
        n_heads=HEADS,
        n_kv_heads=HEADS,
        d_ff=4 * D,
        vocab=VOCAB,
        seq_len=SEQ,
        param_count=count,
        compute_dtype="bf16",
        capacity_factor=f32hex(1.0),
        aux_loss_coef=f32hex(0.0),
        od={
            "preset": "od-large",
            "head_layers": HEAD_LAYERS,
            "objective": "mlm",
            "mask_ratio": f32hex(0.3),
            "mask_seed": 7,
            "tokenizer_offset": 3,
            "warm_start": False,
            "record": None,
            "decision_rule": "log",
            "rps_weight": f32hex(1.0),
            "distill_temp": f32hex(1.0),
        },
    )
    tok_sha = sha256_hex(a.tokenizer.read_bytes())
    if shards.tokenizer.get("sha256") not in (None, tok_sha):
        raise SystemExit("tokenizer bytes differ from the dataset's tokenizer sha256")
    body["tokenizer"] = {"name": "answerdotai/ModernBERT-base", "sha256": tok_sha}
    body["dataset"] = ds
    body["budget"] = {"epochs_per_round": epochs}
    body["inner"].update(
        H=H,
        J=J,
        micro_batch=MICRO_BATCH,
        grad_accum=GRAD_ACCUM,
        state_policy="carry",
        rewarmup_steps=0,
    )
    body["inner"]["lr_schedule"].update(peak_lr=f32hex(3e-4), warmup=0, stable=100000, decay=0)
    body["outer"].update(
        opt="nesterov",
        lr=f32hex(OUTER_LR),
        momentum=f32hex(OUTER_MOMENTUM),
        preclip_norm=f32hex(CLIP),
        cclip_tau=f32hex(CLIP),
        cclip_iters=1,
    )
    body["coord_pubkey"] = a.coord_pubkey
    body["auditors"] = [keys["auditor-0"].ss58, keys["auditor-1"].ss58]
    ref = body["reference_spec"]
    ref.update(
        image_digest=image,
        driver_allowlist=a.driver,
        profile="od-bf16-det-eager-v1",
        sm_count=SM_COUNT,
    )
    ref["env"]["pinned"] = ENV_PIN
    ref["layout"] = {"pp": 1, "n_gpus": N_GPUS, "dp_size": N_GPUS, "ep_size": 1, "zero1": False}
    profile = profile_for(
        a.tree, body, image, a.driver, a.host_disk_bytes, custody, dry_run=a.dry_run
    )
    profile_raw = canonicalize(profile)
    data_meta = {
        "manifest_sha256": sha256_hex(a.dataset_manifest.read_bytes()),
        "holdout_manifest_sha256": sha256_hex(holdout.read_bytes()),
        "mix_id": shards.extra.get("mix_id"),
        "base_urls": [DATA_BASE],
    }
    ref_doc = canonicalize(
        {
            "purpose": "od-large stage A MLM DiLoCo H=512 CUDA run",
            "dataset": data_meta,
            "tokenizer_sha256": tok_sha,
            "profile_sha256": sha256_hex(profile_raw),
            "sources_sha256": sha256_hex(canonicalize(tree_sources)),
            "registry_manifest_sha256": image.removeprefix("sha256:"),
        }
    )
    ref["spec_doc_sha256"] = sha256_hex(ref_doc)
    network = {
        **{k: v.digest() for k, v in policies.items()},
        "audit_mode": "anchored-full",
        "full_anchor_version": 1,
        "capabilities": ["island-replay", "all-level-disputes", "transport-receipts"],
    }
    draft = RunManifestV2.model_validate(
        {"manifest_version": 2, "training": body, "network": network}
    )
    theta = init_params(TrainConfig.from_manifest_v2(draft).model)
    assert sum(t.numel() for t in theta.values()) == count
    body["init_state_hash"] = state_hash(theta)
    manifest = RunManifestV2.model_validate(
        {"manifest_version": 2, "training": body, "network": network}
    )
    policies["audit_policy_hash"].validate_training(manifest.training)
    run_id = manifest.run_id()
    # Genesis carry seed job (no training step): deterministic init + zero optimizer/EF; its
    # samples are the first n rows (units 0..7), proved against the dataset merkle root.
    anchor = AnchorCache().genesis(manifest, a.hotkey, theta)
    n = manifest.training.batch_samples()
    rows = U16ShardSamples(
        a.dataset_manifest.parent, shards.n_shards, shards.samples_per_shard, shards.seq_len
    )
    if not (a.dataset_manifest.parent / shard_name16(0)).is_file():
        raise SystemExit("seed job needs the local shards next to the dataset manifest")
    # ponytail: the full-depth tree over 2^21 rows is built in memory (~3 min, ~200 MB) only to
    # emit the n seed proofs; add a cached proof file once round inputs stream per unit.
    tree = MerkleTree(rows)
    if tree.root.hex() != ds["merkle_root"]:
        raise SystemExit("local shard rows differ from the dataset merkle_root")
    seed_rows = np.frombuffer(b"".join(rows[i] for i in range(n)), dtype="<u2")
    inputs = {
        "start_state": pack_state(theta, anchor.state),
        "ef_in": pack_state(anchor.ef),
        "v0": pack_state({}),
        "samples": seed_rows.tobytes(),
        "sample_proofs": canonicalize([[p.hex() for p in tree.proof(i)] for i in range(n)]),
    }
    del theta, anchor, tree
    job = IslandJobV1(
        job_version=1,
        run_id=run_id,
        w=0,
        manifest=manifest,
        sample_ids=list(range(n)),
        global_step0=0,
        start_state_sha256=sha256_hex(inputs["start_state"]),
        ef_in_sha256=sha256_hex(inputs["ef_in"]),
        v0_sha256=sha256_hex(inputs["v0"]),
        object_paths={k: k for k in inputs},
        deadline=job_deadline,
    )
    for name, raw in inputs.items():
        (job_dir / name).write_bytes(raw)
    (job_dir / "job.json").write_bytes(job.model_dump_json().encode())
    for name, value in policies.items():
        (out / f"policy-{name}.json").write_bytes(canonicalize(value.body()))
    (out / "reference.json").write_bytes(ref_doc)
    (out / "profile.json").write_bytes(profile_raw)
    (out / "sources.json").write_bytes(canonicalize(tree_sources))
    (out / "manifest.json").write_bytes(manifest.model_dump_json().encode())
    beacon = body["beacon"]
    h_end_round = (h_end - beacon["genesis_time"]) // beacon["period"] + 1
    exp = a.exp_round or h_end_round
    if exp > h_end_round + EXP_MARGIN_ROUNDS:
        raise SystemExit("manifest expiry exceeds H window + margin")
    env = envelope_v2.seal(owner, "RunManifestV2", run_id, manifest, exp)
    (out / "manifest-envelope.json").write_bytes(canonicalize(env))
    RunManifestV2.model_validate_json((out / "manifest.json").read_bytes()).validate_run_id(run_id)
    roles = {r: k.ss58 for r, k in keys.items()} | {"owner": owner.ss58, "escrow-cold": escrow.ss58}
    t = manifest.training
    index = {
        "run_id": run_id,
        "dry_run": a.dry_run,
        "param_count": count,
        "image_digest": image,
        "roles": roles,
        "hotkey": a.hotkey,
        "custody_free_bytes": custody,
        "dataset": data_meta,
        "summary": {
            "inner": {k: getattr(t.inner, k) for k in ("H", "J", "micro_batch", "grad_accum")},
            "batch_samples": n,
            "units_per_slot": n // ASSIGN_UNIT,
            "outer": t.outer.model_dump(mode="json"),
            "dataset": {k: ds[k] for k in ("n_samples", "depth", "assign_unit", "merkle_root")},
            "epochs_per_round": epochs,
            "round_seconds": round_seconds(),
            "offsets": ",".join(map(str, offsets())),
            "audit": {
                "lease_rounds": lease,
                "absolute_rounds": 2 * lease,
                "step_seconds_bound": STEP_SECONDS,
            },
            "schedule": {
                k: profile["microqualification"][k]
                for k in ("probe_and_transfer_seconds", "qualification_deadline_seconds")
            }
            | {
                "hard_deadline_seconds": profile["schedule"]["hard_deadline_seconds"],
                "execution_cap": orchestrate_module(a.tree).lifecycle_timing(profile)[
                    "execution_cap"
                ],
            },
            "trace_bytes_per_rank_budget": profile["budget_bounds"]["trace_bytes_per_rank"],
            "budget_misfit": profile["budget_misfit"],
        },
        "files": {
            f"{d.name}/{f.name}": sha256_hex(f.read_bytes())
            for d in (out, job_dir)
            for f in sorted(d.iterdir())
        },
    }
    (out / "index.json").write_bytes(canonicalize(index))
    print(json.dumps({k: v for k, v in index.items() if k != "files"}, indent=1))


if __name__ == "__main__":
    main()
