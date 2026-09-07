"""Independent FLOP recomputation. A tiny matmul does not attest every recipe."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from proof_eval.compute_trace import collect_trace, verify_trace
from proof_eval.contract import ContractError
from proof_eval.training_evidence import verify_training_evidence


def test_independent_recompute_matches_collector() -> None:
    torch = pytest.importorskip("torch")
    a = torch.randn(2, 3)
    b = torch.randn(3, 4)
    with collect_trace() as trace:
        _ = a @ b
    body = trace.to_dict()
    assert verify_trace(body) == 48


def test_unlisted_compute_refuses() -> None:
    with pytest.raises(ContractError, match="unlisted"):
        verify_trace(
            {
                "schema_version": 1,
                "ops": [
                    {
                        "op": "aten::convolution",
                        "shapes": [[1, 1, 3, 3], [1, 1, 1, 1]],
                        "dtype": "float32",
                        "count": 1,
                    }
                ],
            }
        )


def test_training_evidence_must_match_the_trace() -> None:
    trace = {
        "schema_version": 1,
        "ops": [
            {
                "op": "aten::mm",
                "shapes": [[2, 3], [3, 4]],
                "dtype": "float32",
                "count": 1,
            }
        ],
    }
    ok = verify_training_evidence(
        {
            "schema_version": 1,
            "script_digest": "ab" * 32,
            "log_digest": "cd" * 32,
            "seed": 1,
            "flops_used": 48,
            "trace": trace,
        }
    )
    assert ok["flops_used"] == 48
    with pytest.raises(ContractError, match="do not match"):
        verify_training_evidence(
            {
                "schema_version": 1,
                "script_digest": "ab" * 32,
                "log_digest": "cd" * 32,
                "seed": 1,
                "flops_used": 99,
                "trace": trace,
            }
        )
    with pytest.raises(ContractError, match="trace"):
        verify_training_evidence(
            {
                "schema_version": 1,
                "script_digest": "ab" * 32,
                "log_digest": "cd" * 32,
                "seed": 1,
                "flops_used": 48,
            }
        )


def test_missing_training_evidence_file_refuses(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    from proof_eval.training_evidence import load_training_evidence

    monkeypatch.delenv("PROOF_TRAINING_EVIDENCE_FILE", raising=False)
    with pytest.raises(ContractError, match="missing"):
        load_training_evidence()
    missing = tmp_path / "nope.json"
    with pytest.raises(ContractError, match="regular file"):
        load_training_evidence(missing)
