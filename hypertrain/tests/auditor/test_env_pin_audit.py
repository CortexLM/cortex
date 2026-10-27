"""ADR 0011 part 3 (DET-01b): the production audit comparator, over a claim minted by the real
launcher in one ambient and replayed by the real launcher in another.

Without a pin, an honest AVX-512 miner audited on AVX2, or the reverse, is a MISMATCH at leaf 1
(the Lens reproducer: tests/auditor/test_island_replay_v2.py fixture, real torchrun). With the
manifest's pin both sides run the same environment whatever their own ambient is."""

from __future__ import annotations

import json
import time
from pathlib import Path
from typing import Any

import pytest
from test_island_replay_v2 import fixture

from env_pin_fixtures import CPU_FP32, HOSTILE, ambient, needs_avx2
from hypertrain.auditor.replay import AnchorCache, Outcome
from hypertrain.auditor.worker import LeaseGuard, execute_island_audit
from hypertrain.miner.island_launch import launch_island
from hypertrain.protocol.envelope_v2 import seal
from hypertrain.protocol.hashing import MerkleTree, sha256_hex
from hypertrain.protocol.keys import Keypair
from hypertrain.protocol.messages_v2 import AuditJobV2, IslandJobV1, RunManifestV2
from hypertrain.trainer.config import TrainConfig
from hypertrain.trainer.model import init_params


def audit_of_a_minted_claim(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, pinned: dict[str, Any] | None
) -> Outcome:
    """Mint under a clean ambient with launch_island, audit under HOSTILE with the same code."""
    job, _, blobs, get, _, key = fixture(state_policy="carry")
    rows = [get(i).astype("<u4").tobytes() for i in job.sample_ids]
    tree = MerkleTree(rows)
    document = job.manifest.body()
    document["training"]["dataset"].update(
        merkle_root=tree.root.hex(), n_samples=len(rows), depth=2
    )
    if pinned is not None:
        document["training"]["reference_spec"]["env"]["pinned"] = pinned
    wrapper = RunManifestV2.model_validate(document)
    theta = init_params(TrainConfig.from_manifest_v2(wrapper).model)
    cache = AnchorCache()
    prior = cache.genesis(wrapper, key.ss58, theta)
    staged = {
        "start_state": blobs[0],
        "ef_in": blobs[1],
        "v0": blobs[2],
        "samples": b"".join(rows),
        "sample_proofs": json.dumps(
            [[p.hex() for p in tree.proof(i)] for i in range(len(rows))]
        ).encode(),
    }

    def stage(directory: Path) -> None:
        directory.mkdir(parents=True)
        for name, data in staged.items():
            (directory / name).write_bytes(data)

    mint = tmp_path / "mint"
    stage(mint)
    island = IslandJobV1(
        job_version=1,
        run_id=wrapper.run_id(),
        w=0,
        manifest=wrapper,
        sample_ids=list(job.sample_ids),
        global_step0=0,
        start_state_sha256=sha256_hex(blobs[0]),
        ef_in_sha256=sha256_hex(blobs[1]),
        v0_sha256=sha256_hex(blobs[2]),
        object_paths={name: name for name in staged},
        deadline=int(time.time()) + 300,
    )
    ambient(monkeypatch)
    artifacts = launch_island(island, mint, backend="cpu")
    claim = json.loads((artifacts.directory / "rank-0/summary.json").read_bytes())["commitments"]
    start = job.start_state.model_copy(
        update={
            "run_id": wrapper.run_id(),
            "parent_anchor_hash": prior.anchor_hash,
            "anchor_verdict_hash": prior.proof_hash,
        }
    )
    challenge = dict(job.challenge_envelope["body"])
    challenge["anchor_hash"] = start.digest()
    commit = dict(job.commit_envelope["body"])
    commit.update({k: claim[k] for k in ("leaves_root", "delta_hash", "final_theta_hash")})
    commit["ef_out_hash"] = claim["ef_out_hash"]
    audit = AuditJobV2.model_validate(
        {
            **job.model_dump(mode="json"),
            "run_id": wrapper.run_id(),
            "manifest": wrapper.body(),
            "start_state": start.body(),
            "preimages": json.loads(artifacts.leaves.read_bytes()),
            "commit_envelope": seal(key, "CommitV2", wrapper.run_id(), commit, 100),
            "challenge_envelope": seal(
                Keypair(bytes([45]) * 32), "AuditChallengeV2", wrapper.run_id(), challenge, 100
            ),
        }
    )
    stage(tmp_path / "audit")
    ambient(monkeypatch, **HOSTILE)
    with LeaseGuard(50, 100, time.time() + 300) as guard:
        outcome, _ = execute_island_audit(
            audit,
            tmp_path / "audit",
            cache,
            guard,
            now_round=2,
            deadline_unix=int(time.time()) + 300,
            backend="cpu",
        )
    return outcome


@needs_avx2
def test_a_pinned_claim_audits_to_a_match_in_another_ambient(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    unpinned = audit_of_a_minted_claim(tmp_path / "unpinned", monkeypatch, None)
    if unpinned.result == "MATCH":
        pytest.skip("this host's roots do not depend on the ambient: nothing to pin")
    pinned = audit_of_a_minted_claim(tmp_path / "pinned", monkeypatch, CPU_FP32)
    assert (pinned.result, pinned.first_bad_leaf) == ("MATCH", None)
