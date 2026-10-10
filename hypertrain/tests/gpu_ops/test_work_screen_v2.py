"""Actual complete tiny island work; negative bindings, no hardware-count bounty."""

from __future__ import annotations

import importlib
import sys
from pathlib import Path

import pytest
from pydantic import ValidationError

from hypertrain.data.trial_assignment import trial_assignment_hash
from hypertrain.gpu_ops.work_screen import WorkScreenError, screen_work
from hypertrain.protocol.messages_v2 import JoinChallenge, RankWorkResult, WorkScreenV2

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "ledger"))
fixtures = importlib.import_module("test_escrow_v2")
H, digest, setup, stage = fixtures.H, fixtures.digest, fixtures.setup, fixtures.stage


def challenge(job) -> JoinChallenge:
    s = setup()
    return JoinChallenge(
        admission_id=H,
        nonce=H,
        seed_beacon=2,
        deadline_beacon=102,
        manifest_hash=job.run_id,
        theta_hash=job.manifest.training.init_state_hash,
        assignment_hash=trial_assignment_hash(job.manifest, job.w, tuple(job.sample_ids)),
        layout=job.manifest.training.reference_spec.layout,
        artifact_limits=s.admission_policy.artifact_limits,
        policy_hash=digest(s.admission_policy),
    )


def test_full_round_work_screen_when_real_launch(tmp_path: Path) -> None:
    # Given
    s = setup()
    job = stage(s, tmp_path, (0,), 0)
    c = challenge(job)
    # When
    screen, proof = screen_work(job, tmp_path, c, now_beacon=2, backend="cpu")
    # Then: real publication, every logical rank, bounded full state/delta/EF/leaves.
    assert screen.evidence_kind == "work-proof-v1" and len(screen.rank_results) == 1
    assert proof.delta_hash == proof.artifact_refs[2].sha256
    assert screen.artifact_hashes == [r.sha256 for r in proof.artifact_refs]
    assert (tmp_path / "published/rank-0/state.safetensors").is_file()
    assert screen.rank_results[0].elapsed_ns > 0


@pytest.mark.parametrize("fault", ["late", "assignment", "manifest", "theta"])
def test_work_screen_rejects_when_challenge_wrong(tmp_path: Path, fault: str) -> None:
    # Given
    job = stage(setup(), tmp_path, (0,), 0)
    c = challenge(job)
    fields = {"assignment": "assignment_hash", "manifest": "manifest_hash", "theta": "theta_hash"}
    if fault in fields:
        c = JoinChallenge.model_validate({**c.body(), fields[fault]: "ff" * 32})
    # When / Then
    with pytest.raises(WorkScreenError):
        screen_work(job, tmp_path, c, now_beacon=103 if fault == "late" else 2, backend="cpu")
    assert not (tmp_path / "published").exists()


@pytest.mark.parametrize("allocated,reserved", [(86, 10), (10, 86)])
def test_peak_memory_rejects_when_full_round_exceeds_budget(allocated: int, reserved: int) -> None:
    # Given / When / Then
    with pytest.raises(ValidationError):
        RankWorkResult(
            rank=0,
            fingerprint=H,
            elapsed_ns=1,
            allocated_bytes=allocated,
            reserved_bytes=reserved,
            total_bytes=100,
        )


@pytest.mark.parametrize("kind", ["nvidia-cc", "tpm", "snp", "tdx"])
def test_remote_hardware_evidence_rejects_when_unsupported(kind: str) -> None:
    # Given: hint is never an admission substitute.
    # When / Then
    with pytest.raises(ValidationError):
        WorkScreenV2.model_validate(dict(evidence_kind=kind))
