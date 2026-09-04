"""Unit tests for the image contract (no torch required)."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from proof_eval.baked import baked_proxies, require_baked
from proof_eval.contract import DEFAULT_PROXY, METRICS_MARKER, OK_MARKER
from proof_eval.fabric import DT_NO_IB_GBPS, enforce
from proof_eval.request import Constraints, HarvestRequest, canonical_json


def test_markers_match_harvest_pod() -> None:
    assert METRICS_MARKER == "PROOF_METRICS="
    assert OK_MARKER == "PROOF_EVAL_OK"


def test_default_proxy_is_baked() -> None:
    assert DEFAULT_PROXY == "Qwen/Qwen3.8-0.6B"
    assert DEFAULT_PROXY in baked_proxies()
    require_baked(DEFAULT_PROXY)


def test_unknown_proxy_is_refused() -> None:
    with pytest.raises(Exception, match="not baked"):
        require_baked("Qwen/Qwen3.8-27B")


def test_fabric_applies_the_dt_no_ib_cap(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr("proof_eval.fabric._data_ifaces", lambda: [])
    applied = enforce(
        Constraints(
            no_infiniband=True,
            no_nvlink=True,
            no_nccl_fast_fabric=True,
            max_inter_node_gbps=DT_NO_IB_GBPS,
        )
    )
    assert applied["NCCL_IB_DISABLE"] == "1"
    assert applied["NCCL_P2P_DISABLE"] == "1"
    assert applied["NCCL_NET"] == "Socket"
    assert applied["PROOF_MAX_INTER_NODE_GBPS"] == "12.5"


def test_loosening_the_gbps_cap_is_refused() -> None:
    with pytest.raises(Exception, match="loosens"):
        enforce(Constraints(max_inter_node_gbps=25.0))


def test_unknown_constraint_key_is_refused() -> None:
    with pytest.raises(Exception, match="allow_secret_fabric"):
        Constraints.from_dict({"no_infiniband": True, "allow_secret_fabric": True})


def test_canonical_json_sorts_keys() -> None:
    assert canonical_json({"b": 1, "a": {"d": [1, 2], "c": "x"}}) == '{"a":{"c":"x","d":[1,2]},"b":1}'


def test_request_refuses_empty_claim() -> None:
    raw = {
        "schema_version": 1,
        "challenge_id": "proof",
        "submission_digest": "d",
        "artifact_digest": "a",
        "topic_id": "dt-no-ib-v0",
        "family": "throughput",
        "proxy_model": DEFAULT_PROXY,
        "eval_image_digest": "sha256:" + "ab" * 32,
        "holdout_commitment": "00" * 32,
        "constraints": {},
        "flops_budget": 1,
        "wall_budget_s": 1,
        "claim": "",
        "holdout": [{
            "id": 1,
            "split": "web_ood",
            "dataset_id": "synthetic-dev",
            "content_sha256": "aa" * 32,
            "token_count": 2048,
        }],
    }
    with pytest.raises(Exception, match="claim"):
        HarvestRequest.from_dict(raw)
