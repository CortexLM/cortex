"""Build owner-signed network-v2 manifests offline; emit tiny public example inputs."""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Any, Never

from pydantic import JsonValue, ValidationError

from hypertrain.data.store import CorruptObjectError, LocalFSStore
from hypertrain.ledger.escrow_v2 import genesis_allocation_hash
from hypertrain.protocol.envelope_v2 import Intake, load_json, parse_envelope, seal
from hypertrain.protocol.example import example_manifest
from hypertrain.protocol.hashing import MerkleTree, sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.keys import encode_hotkey
from hypertrain.protocol.messages import f32hex
from hypertrain.protocol.messages_v2 import (
    AdmissionPolicyV2,
    AggregationPolicyV2,
    ArtifactLimits,
    AuditPolicyV2,
    DisputePolicyV2,
    EconomicsPolicyV2,
    RunManifestV2,
    TrainingManifestV2,
    WireModel,
)
from hypertrain.protocol.relay_messages import RelayRegistryV1

POLICIES: dict[str, type[WireModel]] = {
    "admission": AdmissionPolicyV2,
    "economics": EconomicsPolicyV2,
    "aggregation": AggregationPolicyV2,
    "audit": AuditPolicyV2,
    "dispute": DisputePolicyV2,
}


class _Parser(argparse.ArgumentParser):
    def error(self, message: str) -> Never:
        raise ValueError("invalid command arguments; use --help")


def _read(path: Path) -> dict[str, JsonValue]:
    raw = path.read_bytes()
    try:
        return load_json(raw)
    except (ValueError, RecursionError):
        raise ValueError(
            "invalid JSON input: expected a bounded object without duplicates or floats"
        ) from None


def _check_out(out: Path) -> None:
    if out.is_symlink() or (out.exists() and (not out.is_dir() or any(out.iterdir()))):
        raise ValueError("--out must not exist or be empty")


def _write(out: Path, files: dict[str, bytes]) -> None:
    _check_out(out)
    out.mkdir(parents=True, exist_ok=True)
    for name, raw in files.items():
        with (out / name).open("xb") as target:
            target.write(raw)


def _build(args: argparse.Namespace) -> dict[str, JsonValue]:
    if args.exp_round <= 0:
        raise ValueError("--exp-round must be positive")
    _check_out(args.out)
    raw_policies = _read(args.policies)
    if raw_policies.keys() != POLICIES.keys():
        raise ValueError(
            "--policies requires exactly admission, economics, aggregation, audit, dispute"
        )
    objects = {name: model.model_validate(raw_policies[name]) for name, model in POLICIES.items()}
    objects["relay_registry"] = RelayRegistryV1.model_validate(_read(args.relays))
    training = _read(args.training)
    supplied_hash = training.get("init_state_hash")
    training.setdefault("init_state_hash", "0" * 64)
    validated = TrainingManifestV2.model_validate(training)
    audit = objects["audit"]
    assert isinstance(audit, AuditPolicyV2)
    audit.validate_training(validated)
    if len(set(validated.auditors)) < 2:
        raise ValueError("two distinct audit reference identities required")
    if validated.inner.state_policy == "derived":
        raise ValueError("v2 derived-state influence requires authenticated v0 integration")
    economics = objects["economics"]
    assert isinstance(economics, EconomicsPolicyV2)
    if economics.round_reward_units != validated.budget.epochs_per_round * 1_000_000:
        raise ValueError("MANIFEST_REWARD_BUDGET")
    if economics.reward_authority != validated.coord_pubkey:
        raise ValueError("PINNED_AUTHORITIES")
    hashes = {name: model.digest() for name, model in objects.items()}
    network = {
        **{f"{name}_policy_hash": hashes[name] for name in POLICIES},
        "relay_registry_hash": hashes["relay_registry"],
        "audit_mode": "anchored-full",
        "full_anchor_version": 1,
        "capabilities": ["island-replay", "all-level-disputes", "transport-receipts"],
    }
    draft = RunManifestV2.model_validate(
        {"manifest_version": 2, "training": validated.body(), "network": network}
    )
    from hypertrain.miner.core import KeyfileError, load_keyfile
    from hypertrain.trainer.compress import state_hash
    from hypertrain.trainer.config import TrainConfig
    from hypertrain.trainer.model import init_params

    try:
        owner = load_keyfile(args.owner_seed)
    except KeyfileError as error:
        raise ValueError(str(error)) from None
    initial_hash = state_hash(init_params(TrainConfig.from_manifest_v2(draft).model))
    if supplied_hash is not None and supplied_hash != initial_hash:
        raise ValueError("init_state_hash does not match computed initial state")
    final_training = validated.body()
    final_training["init_state_hash"] = initial_hash
    manifest = RunManifestV2.model_validate(
        {"manifest_version": 2, "training": final_training, "network": network}
    )
    run_id = manifest.run_id()
    envelope = canonicalize(seal(owner, "RunManifestV2", run_id, manifest, args.exp_round))
    parsed = parse_envelope(envelope)
    RunManifestV2.model_validate(parsed.body).validate_run_id(parsed.run_id)
    Intake(run_id, {"RunManifestV2": lambda signer: signer == owner.ss58}).accept(envelope, 1)
    files = {
        "manifest.json": canonicalize(manifest.body(), allow_float=False),
        "manifest-envelope.json": envelope,
        **{
            f"policy-{name}.json": canonicalize(objects[name].body(), allow_float=False)
            for name in POLICIES
        },
        "relay-registry.json": canonicalize(objects["relay_registry"].body(), allow_float=False),
    }
    index: dict[str, JsonValue] = {
        "run_id": run_id,
        "owner_ss58": owner.ss58,
        "objects": dict(hashes),
        "files": {name: sha256_hex(raw) for name, raw in files.items()},
    }
    files["index.json"] = canonicalize(index, allow_float=False)
    if args.state_dir is not None:
        store = LocalFSStore(args.state_dir / "objects")
        for model in objects.values():
            store.get(store.put(canonicalize(model.body(), allow_float=False)))
    _write(args.out, files)
    return index


def _example(ledger_mode: str = "test") -> dict[str, bytes]:
    coord, auditor0, auditor1, referee, relay, cold, other = (
        encode_hotkey(bytes([i]) * 32) for i in range(1, 8)
    )
    training: dict[str, Any] = example_manifest().body()
    training["model"].update(
        n_layers=1,
        d_model=8,
        n_heads=2,
        n_kv_heads=2,
        d_ff=12,
        vocab=16,
        seq_len=4,
        n_experts=1,
        top_k_experts=1,
        param_count=824,
    )
    training["inner"].update(H=1, J=1, micro_batch=1, grad_accum=1, state_policy="reset")
    training["inner"]["lr_schedule"].update(warmup=0, stable=100, peak_lr=f32hex(0.001))
    rows = [b"".join(((i * 5 + j) % 16).to_bytes(4, "little") for j in range(5)) for i in range(16)]
    training["dataset"].update(merkle_root=MerkleTree(rows).root.hex(), n_samples=16, depth=4)
    training["reference_spec"]["driver_allowlist"] = ["cpu-test"]
    training["coord_pubkey"] = coord
    training["auditors"] = [auditor0, auditor1]
    training["init_state_hash"] = "0" * 64
    training = TrainingManifestV2.model_validate(training).body()
    del training["init_state_hash"]
    admission = AdmissionPolicyV2(
        q_base="0000803e",
        E=4,
        clean_finalizations=12,
        # ~5 h of 3 s rounds: the oldest screen of a 40-trial admission campaign (3 x 12 + 4,
        # each <= challenge_rounds + 1) must outlast round 0. 1000 (~50 min) cannot.
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
            max_object_bytes=1_000_000, max_body_bytes=65536, max_chunk_manifest_bytes=1048576
        ),
    )
    allocation = genesis_allocation_hash([(cold, 20000, 1), (other, 10000, 1)])
    production = ledger_mode == "production"
    economics = EconomicsPolicyV2(
        ledger_mode="production" if production else "test",
        G_max_units=100,
        R_collectible_units=100,
        S_min_units=1000,
        beta_ppm=1_000_000,
        gammaV_units=0,
        s_lower_ppm=1_000_000,
        q_floor=1_000_000,
        genesis_allocation_hash=None if production else allocation,
        reward_authority=coord,
        round_reward_units=1_000_000,
        max_total_issuance=20_000_000,
        shadow_bootstrap_rounds=12,
        bootstrap_allocation_hash=allocation if production else None,
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
        preclip_norm=f32hex(1),
        cclip_tau=f32hex(1),
        cclip_iters=2,
    )
    audit = AuditPolicyV2(
        lease_rounds=100,
        max_attempts=2,
        max_concurrent_per_auditor=1,
        max_running_jobs=2,
        max_queued_jobs=32,
        max_anchor_age_rounds=1,
        max_steps_per_attempt=2,
    )
    dispute = DisputePolicyV2(
        referees=[referee],
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
                            "pubkey": relay,
                            "valid_from_round": 0,
                            "valid_until_round": 10000,
                        }
                    ],
                    "max_object_bytes": 1000000,
                    "max_inflight_bytes": 1000000,
                    "codecs": ["ht-sparse-v1", "ht-dense-int8-v1"],
                    "mode": "transport",
                }
            ],
        }
    )
    policies = dict(
        admission=admission,
        economics=economics,
        aggregation=aggregation,
        audit=audit,
        dispute=dispute,
    )
    return {
        "training.json": canonicalize(training, allow_float=False),
        "policies.json": canonicalize(
            {k: v.body() for k, v in policies.items()}, allow_float=False
        ),
        "relays.json": canonicalize(registry.body(), allow_float=False),
    }


def main(argv: list[str] | None = None) -> int:
    parser = _Parser(prog="python -m hypertrain.ops.manifest_v2")
    sub = parser.add_subparsers(dest="cmd", required=True)
    build = sub.add_parser("build", help="validate, initialize, sign, and write a v2 run")
    for name in ("training", "policies", "relays", "owner-seed", "out"):
        build.add_argument(f"--{name}", type=Path, required=True)
    build.add_argument("--exp-round", type=int, required=True)
    build.add_argument("--state-dir", type=Path)
    example = sub.add_parser("example", help="write tiny CPU inputs with public placeholders")
    example.add_argument("--out", type=Path, required=True)
    example.add_argument("--ledger-mode", choices=("test", "production"), default="test")
    try:
        args = parser.parse_args(argv)
        if args.cmd == "build":
            result = _build(args)
        else:
            _check_out(args.out)
            files = _example(args.ledger_mode)
            _write(args.out, files)
            result = {"files": {name: sha256_hex(raw) for name, raw in files.items()}}
    except ValidationError as error:
        detail = error.errors(include_input=False, include_context=False, include_url=False)[0]
        location = " ".join(".".join(map(str, detail["loc"])).splitlines()) or "input"
        print(f"error: {location}: {detail['type']}", file=sys.stderr)
        return 2
    except ValueError as error:
        print("error: " + " ".join(str(error).splitlines()), file=sys.stderr)
        return 2
    except CorruptObjectError:
        print("error: object store contains corrupt content", file=sys.stderr)
        return 2
    except OSError:
        print("error: file access failed", file=sys.stderr)
        return 2
    except Exception:
        print("error: unexpected failure", file=sys.stderr)
        return 1
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main())
