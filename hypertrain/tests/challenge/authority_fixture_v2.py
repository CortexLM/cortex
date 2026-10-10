"""Hermetic signed admission metadata oracle, NOT independent training evidence."""

from __future__ import annotations

import importlib.util
import json
import sqlite3
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest

import hypertrain.trainer  # noqa: F401
from hypertrain.auditor.replay import AnchorCache, pack_state, unpack_state
from hypertrain.data.trial_assignment import trial_samples
from hypertrain.miner.island_launch import IslandArtifacts
from hypertrain.protocol.hashing import MerkleTree, sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages import Finalize
from hypertrain.protocol.messages_v2 import (
    EscrowLock,
    RankWorkResult,
    RunManifestV2,
    WorkProof,
    WorkScreenV2,
)
from hypertrain.trainer.compress import state_hash
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.model import init_params

spec = importlib.util.spec_from_file_location(
    "hermetic_authority_service", Path(__file__).parent / "test_service_network_v2.py"
)
assert spec is not None and spec.loader is not None
service = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = service
spec.loader.exec_module(service)


def metadata_artifacts(job, directory):
    """Independent byte/job oracle; no manufactured optimizer trajectory claim."""
    record = json.loads((directory / "metadata-oracle.json").read_bytes())
    if record["job_hash"] != job.digest():
        raise ValueError("metadata oracle job binding differs")
    for name, digest in record["files"].items():
        if sha256_hex((directory / "rank-0" / name).read_bytes()) != digest:
            raise ValueError("metadata oracle artifact differs")
    source = directory.parent
    theta, optimizer = unpack_state((source / "start_state").read_bytes())
    expected = init_params(TrainConfig.from_manifest_v2(job.manifest).model)
    anchor = AnchorCache().genesis(job.manifest, service.HOT[0].ss58, expected)
    if (
        optimizer is None
        or optimizer.step != 0
        or pack_state(theta, optimizer) != pack_state(anchor.theta, anchor.state)
    ):
        raise ValueError("metadata oracle genesis differs")
    if sha256_hex((source / "start_state").read_bytes()) != job.start_state_sha256:
        raise ValueError("metadata oracle source digest differs")
    width = (job.manifest.training.model.seq_len + 1) * 2
    samples = (source / "samples").read_bytes()
    proofs = json.loads((source / "sample_proofs").read_bytes())
    for i, sample_id in enumerate(job.sample_ids):
        if not MerkleTree.verify(
            samples[i * width : (i + 1) * width],
            sample_id,
            [bytes.fromhex(p) for p in proofs[i]],
            bytes.fromhex(job.manifest.training.dataset.merkle_root),
            job.manifest.training.dataset.n_samples,
        ):
            raise ValueError("metadata oracle sample membership differs")
    return IslandArtifacts(
        directory,
        *(
            directory / "rank-0" / name
            for name in ("state.safetensors", "ef.safetensors", "delta.bin", "leaves.json")
        ),
        tuple(RankWorkResult.model_validate(r) for r in record["ranks"]),
    )


def _screen(job, directory, challenge, *, now_beacon, backend):
    """Replace only expensive execution with explicit signed metadata reference."""
    assert backend == "cpu"
    published = directory / "published"
    rank = published / "rank-0"
    rank.mkdir(parents=True)
    theta, state = unpack_state((directory / "start_state").read_bytes())
    assert state is not None
    # No optimizer update is claimed: metadata-only fixture, not work qualification.
    values = {
        "state.safetensors": pack_state(theta, state),
        "ef.safetensors": (directory / "ef_in").read_bytes(),
        "delta.bin": b"hermetic-authority-metadata-not-training",
        "leaves.json": canonicalize({"job_hash": job.digest(), "fixture": "metadata-only"}),
    }
    refs = []
    for name, raw in values.items():
        (rank / name).write_bytes(raw)
        refs.append({"sha256": sha256_hex(raw), "size": len(raw)})
    ranks = [
        RankWorkResult(
            rank=i,
            fingerprint=sha256_hex(canonicalize([job.digest(), i])),
            elapsed_ns=0,
            allocated_bytes=0,
            reserved_bytes=0,
            total_bytes=1,
        )
        for i in range(job.manifest.training.reference_spec.layout.n_gpus)
    ]
    (published / "metadata-oracle.json").write_bytes(
        canonicalize(
            {
                "job_hash": job.digest(),
                "files": {name: sha256_hex(raw) for name, raw in values.items()},
                "ranks": [r.body() for r in ranks],
            }
        )
    )
    metadata_artifacts(job, published)
    proof = WorkProof(
        admission_id=challenge.admission_id,
        challenge_hash=challenge.digest(),
        leaves_root=sha256_hex(values["leaves.json"]),
        delta_hash=sha256_hex(values["delta.bin"]),
        artifact_refs=refs,
    )
    screen = WorkScreenV2(
        evidence_kind="work-proof-v1",
        admission_id=challenge.admission_id,
        nonce=challenge.nonce,
        policy_hash=challenge.policy_hash,
        image_digest=job.manifest.training.reference_spec.image_digest,
        layout=challenge.layout,
        challenge_hash=challenge.digest(),
        rank_results=ranks,
        artifact_hashes=[r["sha256"] for r in refs],
    )
    return screen, proof


def build_authority(source: Path) -> tuple[str, dict[str, bytes]]:
    """Use real service predicates for 4x12 finalized signatures, no cached state."""
    from hypertrain.challenge import admission as admission_module

    with pytest.MonkeyPatch.context() as patch:
        setup_type = service.fixture.Setup

        def carry(manifest, policy, admission_policy, rows, tree):
            body = manifest.body()
            body["training"]["inner"].update(state_policy="carry", rewarmup_steps=0)
            return setup_type(
                RunManifestV2.model_validate(body), policy, admission_policy, rows, tree
            )

        patch.setattr(service.fixture, "Setup", carry)
        patch.setattr(service.time, "time", lambda: 1700000000.0)
        serial = iter(range(1, 1000))
        patch.setattr(admission_module.secrets, "token_hex", lambda n: f"{next(serial):0{2 * n}x}")
        patch.setattr(admission_module, "screen_work", _screen)
        (source.parent / "live").mkdir()
        generator = service.network.__wrapped__(
            source.parent / "live", SimpleNamespace(param="mlm")
        )
        network = next(generator)
        try:
            store = network.store
            _, admission, _ = store._services(network.manifest.run_id())
            for i in range(4):
                if i:
                    network.push(network.now + 101)
                identity = network.join(i).json()["admission_id"]
                lock = EscrowLock(
                    operation_id=f"{i + 1:064x}",
                    owner=service.COLD[i].ss58,
                    units=1000,
                    origin_ids=[network.origin_ids[i]],
                    admission_id=identity,
                    dispute_id=None,
                    kind="LOCK_ADMISSION",
                )
                admission.lock(
                    canonicalize(network.signed(service.COLD[i], "EscrowLock", lock)),
                    now=network.now,
                )
                for _ in range(12):
                    network.push(network.now + 1)
                    raw = admission.challenge(identity, now=network.now)
                    admission.prepare_reference(identity, now=network.now)
                    row = store._db.execute(
                        "SELECT * FROM admissions_v2 WHERE admission_id=?", (identity,)
                    ).fetchone()
                    proof = WorkProof.model_validate_json(row["reference_json"])
                    from hypertrain.protocol.messages_v2 import JoinChallenge

                    c = JoinChallenge.model_validate(raw["body"])
                    job, directory = store.stage_trial_v2(
                        network.manifest.run_id(),
                        c,
                        trial_samples(
                            network.manifest, identity, c.nonce, store._beacon_v2(c.seed_beacon)
                        ),
                        row["trial_epoch"],
                    )
                    artifacts = metadata_artifacts(job, directory / "published")
                    screen = WorkScreenV2(
                        evidence_kind="work-proof-v1",
                        admission_id=identity,
                        nonce=c.nonce,
                        policy_hash=c.policy_hash,
                        image_digest=network.manifest.training.reference_spec.image_digest,
                        layout=c.layout,
                        challenge_hash=c.digest(),
                        rank_results=list(artifacts.ranks),
                        artifact_hashes=[a.sha256 for a in proof.artifact_refs],
                    )
                    admission.proof(
                        canonicalize(network.signed(service.HOT[i], "WorkProof", proof)),
                        canonicalize(network.signed(service.HOT[i], "WorkScreenV2", screen)),
                        now=network.now,
                    )
                    final = Finalize(
                        w=row["trial_epoch"],
                        included=[service.HOT[i].ss58],
                        entitlements_root=proof.digest(),
                        final_theta_hash_w1=state_hash(
                            unpack_state(artifacts.state.read_bytes())[0]
                        ),
                    )
                    status = admission.finalize_trial(
                        identity,
                        canonicalize(network.signed(service.COORD, "Finalize", final)),
                        now=network.now,
                    )
                assert (
                    status.record.state == "ACTIVE"
                    and status.eligible
                    and status.record.clean_count == 12
                )
            source.mkdir()
            with sqlite3.connect(source / "challenge.db") as saved:
                store._db.backup(saved)
            import shutil

            for name in ("objects", "trials-v2", "ledger"):
                shutil.copytree(store.state_dir / name, source / name)
            seeds = [bytes(range(32)), b"\x71" * 32, bytes(31) + b"\x01"]
            seeds += [bytes([i]) * 32 for i in (0x72, 0x73, 0x74, 80, 81, 82, 83, 90, 91, 92, 93)]
            from hypertrain.protocol.keys import Keypair

            return network.manifest.run_id(), {Keypair(seed).ss58: seed for seed in seeds}
        finally:
            generator.close()
            store._db.close()
