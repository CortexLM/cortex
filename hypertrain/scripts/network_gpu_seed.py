"""Synthetic functional D2 decoder seed; no public-quality or CUDA observation claim."""

from __future__ import annotations

import argparse
import json
from pathlib import Path

import numpy as np
from pydantic import BaseModel, ConfigDict, Field
from tokenizers import Tokenizer, models, pre_tokenizers

import hypertrain.trainer  # noqa: F401
from hypertrain.auditor.replay import AnchorCache, optimizer_hash, pack_state
from hypertrain.data.store import LocalFSStore
from hypertrain.data.tokenizer import HFTokenizer
from hypertrain.ledger.escrow_v2 import (
    authority_message,
    genesis_allocation_hash,
    genesis_origin_id,
)
from hypertrain.protocol import envelope_v2
from hypertrain.protocol.example import example_manifest
from hypertrain.protocol.hashing import MerkleTree, sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages import Receipt
from hypertrain.protocol.messages_v2 import (
    AdmissionPolicyV2,
    AggregationPolicyV2,
    ArtifactLimits,
    AuditPolicyV2,
    DisputePolicyV2,
    EconomicsPolicyV2,
    IslandJobV1,
    OriginAllocation,
    RunManifestV2,
    StartStateV2,
    TestGenesis,
)
from hypertrain.protocol.relay_messages import RelayRegistryV1
from hypertrain.trainer.compress import state_hash
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.model import init_params

TREE = Path(__file__).resolve().parents[1]
ROLES = ("owner", "coord", "auditor-0", "auditor-1", "referee", "relay") + tuple(
    f"{role}-{i}" for role in ("hot", "cold") for i in range(4)
)


class SeedIndex(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True)
    run_id: str = Field(pattern=r"^[0-9a-f]{64}$")
    hotkey: str
    roles: dict[str, str]
    claim: str
    files: dict[str, str] = Field(min_length=1, max_length=256)


def generate(
    destination: Path,
    secrets: Path,
    image: str,
    drivers: list[str],
    genesis_time: int,
    deadline: int,
) -> str:
    """Create pinned real inputs from explicit candidate runtime fields, never qualify them."""
    if destination.exists() or secrets.exists() or destination.resolve() == secrets.resolve():
        raise ValueError("new disjoint output/secret directories required")
    if secrets.resolve().is_relative_to(
        destination.resolve()
    ) or destination.resolve().is_relative_to(secrets.resolve()):
        raise ValueError("secrets must be separate from public output")
    if genesis_time < 1 or deadline <= genesis_time or not drivers:
        raise ValueError("explicit runtime clock and driver candidates required")
    profile_raw = (TREE / "experiments/gpu_network_v2/profile.json").read_bytes()
    profile = json.loads(profile_raw)
    keys = {
        role: Keypair(bytes.fromhex(sha256_hex(b"ht-d2-functional-seed-v1|" + role.encode())))
        for role in ROLES
    }
    allocations = [(keys[f"cold-{i}"].ss58, 10000, 1) for i in range(4)]
    admission = AdmissionPolicyV2(
        q_base="0000803e",
        E=4,
        clean_finalizations=12,
        work_screen_epoch_rounds=1000,
        max_pending=32,
        max_trial_replays=2,
        max_trials_per_beacon=2,
        join_per_beacon=2,
        join_burst=4,
        ip_prefix_per_beacon=32,
        ip_prefix_burst=64,
        challenge_rounds=100,
        artifact_limits=ArtifactLimits(
            max_object_bytes=8000000, max_body_bytes=65536, max_chunk_manifest_bytes=1048576
        ),
    )
    economics = EconomicsPolicyV2(
        ledger_mode="test",
        G_max_units=100,
        R_collectible_units=100,
        S_min_units=1000,
        beta_ppm=1000000,
        gammaV_units=0,
        s_lower_ppm=1000000,
        q_floor=1000000,
        genesis_allocation_hash=genesis_allocation_hash(allocations),
        reward_authority=keys["coord"].ss58,
        round_reward_units=1000000,
        max_total_issuance=20000000,
        shadow_bootstrap_rounds=12,
    )
    aggregation = AggregationPolicyV2(
        arithmetic="flat-cclip-cap-v1",
        order="utf8",
        center="prev_outer_update",
        trust_mode="uniform-verified",
        weight_quantum=16777216,
        miner_cap_units=4194304,
        probation_cap_units=4194304,
        owner_group_caps={},
        preclip_norm=profile["outer"].get("preclip_norm", "0000803f"),
        cclip_tau="0000803f",
        cclip_iters=1,
    )
    audit = AuditPolicyV2(
        lease_rounds=100,
        max_attempts=2,
        max_concurrent_per_auditor=1,
        max_running_jobs=2,
        max_queued_jobs=32,
        max_anchor_age_rounds=1,
        max_steps_per_attempt=60,
    )
    dispute = DisputePolicyV2(
        referees=[keys["referee"].ss58],
        max_open=32,
        max_transcript_entries=256,
        max_entry_bytes=65536,
        fanout=2,
        max_referee_reassignments=1,
        max_referee_cost_units=100,
    )
    registry = RelayRegistryV1.model_validate(
        {
            "registry_version": 1,
            "epoch": 0,
            "previous_registry_hash": None,
            "specs": [
                {
                    "id": "local",
                    "region": "local",
                    "https_url": "https://relay.test",
                    "pubkeys": [
                        {
                            "key_id": "k1",
                            "pubkey": keys["relay"].ss58,
                            "valid_from_round": 0,
                            "valid_until_round": 10000,
                        }
                    ],
                    "max_object_bytes": 8000000,
                    "max_inflight_bytes": 32000000,
                    "codecs": ["ht-sparse-v1", "ht-dense-int8-v1"],
                    "mode": "transport",
                }
            ],
        }
    )
    eos = "<|endoftext|>"
    tokenizer = Tokenizer(
        models.WordLevel({**{f"t{i}": i for i in range(63)}, eos: 63}, unk_token=eos)
    )
    tokenizer.pre_tokenizer = pre_tokenizers.WhitespaceSplit()
    tokenizer_raw = tokenizer.to_str().encode()
    rows = np.asarray([[(i * 17 + j) % 63 for j in range(17)] for i in range(4096)], dtype="<u4")
    tree = MerkleTree([row.tobytes() for row in rows])
    body = json.loads(example_manifest().model_dump_json())
    body["model"].update(profile["model"])
    body["inner"].update({k: v for k, v in profile["inner"].items() if k != "lr_schedule"})
    body["inner"]["lr_schedule"].update(profile["inner"]["lr_schedule"])
    body["outer"].update(profile["outer"])
    body["tokenizer"] = {"name": "ht-synthetic-wordlevel-64", "sha256": sha256_hex(tokenizer_raw)}
    body["dataset"].update(
        merkle_root=tree.root.hex(),
        depth=12,
        n_samples=4096,
        sample_format="u32[seq_len+1] token ids",
        shard_uri_template="samples.u32",
        shard_sha256_root=MerkleTree([bytes.fromhex(sha256_hex(rows.tobytes()))]).root.hex(),
        holdout_commit=sha256_hex(b"D2 synthetic functional fixture: no quality holdout"),
    )
    body["beacon"]["genesis_time"] = genesis_time
    body["coord_pubkey"] = keys["coord"].ss58
    body["auditors"] = [keys[f"auditor-{i}"].ss58 for i in range(2)]
    body["reference_spec"].update(image_digest=image, driver_allowlist=drivers, sm_count=170)
    body["reference_spec"]["layout"] = profile["layout"]
    reference_raw = canonicalize(
        {
            "profile_sha256": sha256_hex(profile_raw),
            "purpose": "SYNTHETIC_FUNCTIONAL_QUALIFICATION_NOT_PUBLIC_QUALITY",
            "image_digest_candidate": image,
            "driver_allowlist_candidates": drivers,
            "cuda_observed": False,
            "sources": {
                p: sha256_hex((TREE / p).read_bytes())
                for p in (
                    "src/hypertrain/trainer/model.py",
                    "src/hypertrain/auditor/replay.py",
                    "src/hypertrain/protocol/example.py",
                    "scripts/network_gpu_seed.py",
                )
            },
        }
    )
    body["reference_spec"]["spec_doc_sha256"] = sha256_hex(reference_raw)
    theta = init_params(TrainConfig.from_manifest(body).model)
    body["init_state_hash"] = state_hash(theta)
    policies = dict(
        admission_policy_hash=admission,
        economics_policy_hash=economics,
        aggregation_policy_hash=aggregation,
        audit_policy_hash=audit,
        dispute_policy_hash=dispute,
        relay_registry_hash=registry,
    )
    manifest = RunManifestV2.model_validate(
        {
            "manifest_version": 2,
            "training": body,
            "network": {
                **{k: v.digest() for k, v in policies.items()},
                "audit_mode": "anchored-full",
                "full_anchor_version": 1,
                "capabilities": ["island-replay", "all-level-disputes", "transport-receipts"],
            },
        }
    )
    if sum(x.numel() for x in theta.values()) != 61856:
        raise ValueError("profile parameter count differs")
    anchor = AnchorCache().genesis(manifest, keys["hot-0"].ss58, theta)
    inputs = {
        "start_state": pack_state(theta, anchor.state),
        "ef_in": pack_state(anchor.ef),
        "v0": pack_state({}),
        "samples": rows[:60].tobytes(),
        "sample_proofs": canonicalize([[p.hex() for p in tree.proof(i)] for i in range(60)]),
    }
    job = IslandJobV1(
        job_version=1,
        run_id=manifest.run_id(),
        w=0,
        manifest=manifest,
        sample_ids=list(range(60)),
        global_step0=0,
        start_state_sha256=sha256_hex(inputs["start_state"]),
        ef_in_sha256=sha256_hex(inputs["ef_in"]),
        v0_sha256=sha256_hex(inputs["v0"]),
        object_paths={k: k for k in inputs},
        deadline=deadline,
    )
    start = StartStateV2(
        run_id=manifest.run_id(),
        w=0,
        hotkey=keys["hot-0"].ss58,
        theta_hash=state_hash(theta),
        state_object_sha256=job.start_state_sha256,
        opt_state_hash=optimizer_hash(anchor.state),
        ef_object_sha256=job.ef_in_sha256,
        ef_hash=state_hash(anchor.ef),
        parent_anchor_hash=anchor.anchor_hash,
        global_step0=0,
        anchor_verdict_hash=anchor.proof_hash,
    )
    allocation_hash = genesis_allocation_hash(allocations)
    genesis = TestGenesis(
        run_id=manifest.run_id(),
        allocation_hash=allocation_hash,
        total_units=40000,
        origins=[
            OriginAllocation(
                origin_id=genesis_origin_id(manifest.run_id(), allocation_hash, i),
                owner=owner,
                units=units,
                mature_at=mature,
            )
            for i, (owner, units, mature) in enumerate(allocations)
        ],
        authority_sig="0" * 128,
    )
    genesis = genesis.model_copy(
        update={"authority_sig": keys["coord"].sign(authority_message(genesis)).hex()}
    )
    destination.mkdir(parents=True, mode=0o700)
    secrets.mkdir(parents=True, mode=0o700)
    for role in ROLES:
        path = secrets / (role + ".seed")
        path.write_bytes(bytes.fromhex(sha256_hex(b"ht-d2-functional-seed-v1|" + role.encode())))
        path.chmod(0o600)
    objects = LocalFSStore(destination / "objects")
    for value in policies.values():
        objects.put(canonicalize(value.body()))
    files = {
        **inputs,
        "samples.u32": rows.tobytes(),
        "tokenizer.json": tokenizer_raw,
        "profile.json": profile_raw,
        "reference.json": reference_raw,
        "job.json": job.model_dump_json().encode(),
        "start.json": start.model_dump_json().encode(),
        "manifest.json": manifest.model_dump_json().encode(),
    }
    for name, signer, kind, signed_value in (
        ("manifest-envelope.json", "owner", "RunManifestV2", manifest),
        ("genesis-envelope.json", "coord", "TestGenesis", genesis),
        (
            "reference-envelope.json",
            "coord",
            "Receipt",
            Receipt(w=0, commit_hash=sha256_hex(reference_raw), received_round=1),
        ),
        (
            "job-envelope.json",
            "coord",
            "Receipt",
            Receipt(w=0, commit_hash=job.digest(), received_round=1),
        ),
    ):
        files[name] = canonicalize(
            envelope_v2.seal(keys[signer], kind, manifest.run_id(), signed_value, 10000)
        )
    for name, raw in files.items():
        (destination / name).write_bytes(raw)
        objects.put(raw)
    index = canonicalize(
        {
            "run_id": manifest.run_id(),
            "hotkey": keys["hot-0"].ss58,
            "roles": {role: key.ss58 for role, key in keys.items()},
            "claim": "SYNTHETIC_FUNCTIONAL_SEED_NOT_CUDA_QUALIFIED",
            "files": {
                str(p.relative_to(destination)): sha256_hex(p.read_bytes())
                for p in sorted(destination.rglob("*"))
                if p.is_file()
            },
        }
    )
    (destination / "seed.json").write_bytes(index)
    validate(destination, sha256_hex(index))
    return sha256_hex(index)


def validate(directory: Path, admitted_hash: str) -> IslandJobV1:
    """Check pinned inputs and original signatures; never run CUDA or train."""
    raw = (directory / "seed.json").read_bytes()
    if len(raw) > 1 << 20 or sha256_hex(raw) != admitted_hash:
        raise ValueError("seed index digest differs")
    index = SeedIndex.model_validate_json(raw)
    for name, digest in index.files.items():
        path = directory / name
        if (
            Path(name).is_absolute()
            or ".." in Path(name).parts
            or path.is_symlink()
            or path.stat().st_size > 4 << 20
            or sha256_hex(path.read_bytes()) != digest
        ):
            raise ValueError("seed file binding differs")
    job = IslandJobV1.model_validate_json((directory / "job.json").read_bytes())
    profile = json.loads((directory / "profile.json").read_bytes())
    training = job.manifest.training.model_dump(mode="json")
    if (
        profile["profile_version"] != 2
        or job.global_step0 != 0
        or training["reference_spec"]["layout"] != profile["layout"]
        or not all(
            all(
                training[section].get(k) == v
                for k, v in profile[section].items()
                if k != "lr_schedule"
            )
            for section in ("model", "inner", "outer")
        )
        or not all(
            training["inner"]["lr_schedule"].get(k) == v
            for k, v in profile["inner"]["lr_schedule"].items()
        )
    ):
        raise ValueError("exact decoder profile differs")
    if (
        index.run_id != job.run_id
        or RunManifestV2.model_validate_json((directory / "manifest.json").read_bytes())
        != job.manifest
    ):
        raise ValueError("manifest/job binding differs")
    for name, digest in (
        ("start_state", job.start_state_sha256),
        ("ef_in", job.ef_in_sha256),
        ("v0", job.v0_sha256),
    ):
        if sha256_hex((directory / name).read_bytes()) != digest:
            raise ValueError("job input digest differs")
    tokenizer = HFTokenizer(
        directory / "tokenizer.json",
        job.manifest.training.tokenizer.sha256,
        job.manifest.training.tokenizer.name,
    )
    if tokenizer.vocab_size != 64 or tokenizer.encode("t0 t62") != [0, 62]:
        raise ValueError("synthetic tokenizer differs")
    theta = init_params(TrainConfig.from_manifest_v2(job.manifest).model)
    anchor = AnchorCache().genesis(job.manifest, index.hotkey, theta)
    start = StartStateV2.model_validate_json((directory / "start.json").read_bytes())
    if (
        start.run_id != job.run_id
        or start.hotkey != index.hotkey
        or start.w != job.w
        or start.theta_hash != state_hash(theta)
        or start.opt_state_hash != optimizer_hash(anchor.state)
        or start.state_object_sha256 != job.start_state_sha256
        or start.ef_object_sha256 != job.ef_in_sha256
        or start.parent_anchor_hash != anchor.anchor_hash
        or start.anchor_verdict_hash != anchor.proof_hash
        or start.global_step0 != 0
        or start.ef_hash != state_hash(anchor.ef)
    ):
        raise ValueError("full genesis descriptor differs")
    reference = json.loads((directory / "reference.json").read_bytes())
    if reference["profile_sha256"] != sha256_hex(
        (directory / "profile.json").read_bytes()
    ) or job.manifest.training.reference_spec.spec_doc_sha256 != sha256_hex(
        (directory / "reference.json").read_bytes()
    ):
        raise ValueError("reference/profile pin differs")
    expected = {
        "start_state": pack_state(theta, anchor.state),
        "ef_in": pack_state(anchor.ef),
        "v0": pack_state({}),
    }
    for name, value in expected.items():
        if (directory / name).read_bytes() != value:
            raise ValueError("genesis optimizer/EF/v0 differs")
    for name, signer in (
        ("manifest-envelope.json", "owner"),
        ("genesis-envelope.json", "coord"),
        ("reference-envelope.json", "coord"),
        ("job-envelope.json", "coord"),
    ):
        envelope = envelope_v2.parse_envelope((directory / name).read_bytes())
        if (
            envelope.run_id != job.run_id
            or envelope.signer != index.roles[signer]
            or not envelope_v2.verify_envelope(envelope.model_dump())
        ):
            raise ValueError("seed authority signature differs")
        if name == "manifest-envelope.json" and envelope.body != job.manifest.body():
            raise ValueError("signed manifest body differs")
        if name in ("reference-envelope.json", "job-envelope.json"):
            expected_digest = (
                sha256_hex((directory / "reference.json").read_bytes())
                if name.startswith("reference")
                else job.digest()
            )
            if envelope.type != "Receipt" or envelope.body["commit_hash"] != expected_digest:
                raise ValueError("signed descriptor digest differs")
        if name == "genesis-envelope.json":
            genesis = TestGenesis.model_validate(envelope.body)
            from hypertrain.protocol.keys import decode_hotkey, verify

            if not verify(
                decode_hotkey(envelope.signer),
                authority_message(genesis),
                bytes.fromhex(genesis.authority_sig),
            ):
                raise ValueError("signed genesis authority differs")
    proofs = json.loads((directory / "sample_proofs").read_bytes())
    all_rows = np.frombuffer((directory / "samples.u32").read_bytes(), "<u4").reshape(4096, 17)
    dataset_tree = MerkleTree([row.tobytes() for row in all_rows])
    if dataset_tree.root.hex() != job.manifest.training.dataset.merkle_root or bool(
        np.any(all_rows >= 64)
    ):
        raise ValueError("full dataset root/range differs")
    rows = np.frombuffer((directory / "samples").read_bytes(), "<u4").reshape(60, 17)
    for i, row in enumerate(rows):
        if not np.array_equal(row, all_rows[job.sample_ids[i]]):
            raise ValueError("job sample assignment differs")
        if not MerkleTree.verify(
            row.tobytes(),
            job.sample_ids[i],
            [bytes.fromhex(p) for p in proofs[i]],
            bytes.fromhex(job.manifest.training.dataset.merkle_root),
            4096,
        ):
            raise ValueError("sample membership differs")
    return job


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--destination", type=Path, required=True)
    parser.add_argument("--secrets", type=Path, required=True)
    parser.add_argument("--image-digest", required=True)
    parser.add_argument("--driver", action="append", required=True)
    parser.add_argument("--genesis-unix", type=int, required=True)
    parser.add_argument("--deadline-unix", type=int, required=True)
    args = parser.parse_args()
    print(
        generate(
            args.destination,
            args.secrets,
            args.image_digest,
            args.driver,
            args.genesis_unix,
            args.deadline_unix,
        )
    )


if __name__ == "__main__":
    main()
