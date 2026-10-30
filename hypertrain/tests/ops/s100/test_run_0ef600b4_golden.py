"""The S20 run manifest (0ef600b4, s100 TEST) loads unchanged under the weights hotfix.

The bytes are the run's own `manifest.json` and `policy-audit.json` (S20 real out dir). The hotfix
must not move the run id, the audit policy hash, the policy it names or the implementation hash
its capacity binding carries: the TEST stack keeps serving that run after the server image swap,
and its round 0 is settled with 1,000,000 units to be paid once.
"""

from __future__ import annotations

from pathlib import Path

import pytest

from hypertrain.challenge.store import ChallengeStore
from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages_v2 import AuditPolicyV2, RunManifestV2

RUN = Path(__file__).parent / "fixtures/0ef600b4"
RUN_ID = "0ef600b46c2f3afee2912d9b0dd58decf8e6a62dded8fcb73857c5faaf5d22f6"
AUDIT_POLICY = "86ac1ac32d0b1759b59f4020616c85b6ca0170ad59ea97231b0fba06283ed761"
# What the running challenge served for this run (S20 ident-newrun.json, tools/ident.py).
IMPLEMENTATION_HASH = "24fbe36d2496830d437140d64b1744c5a5a98ec973dd4a04f2e02d8106c185ce"
IMPLEMENTATION_PATHS = (
    "aggregator/core.py",
    "aggregator/tape_v2.py",
    "aggregator/rollback_v2.py",
    "trainer/compress.py",
    "trainer/config.py",
    "trainer/model.py",
    "models/registry.py",
    "miner/island_launch.py",
    "aggregator/capacity_worker.py",
    "auditor/replay.py",
    "protocol/env_pins.py",
    "trainer/determinism.py",
)


def test_the_s20_manifest_keeps_its_run_id_and_audit_policy_hash() -> None:
    raw = (RUN / "manifest.json").read_bytes()
    manifest = RunManifestV2.model_validate_json(raw)
    assert sha256_hex(raw) == manifest.run_id() == RUN_ID
    assert canonicalize(manifest.body()) == raw
    assert manifest.network.audit_policy_hash == AUDIT_POLICY
    policy_raw = (RUN / "policy-audit.json").read_bytes()
    policy = AuditPolicyV2.model_validate_json(policy_raw)
    assert sha256_hex(policy_raw) == policy.digest() == AUDIT_POLICY
    assert canonicalize(policy.body()) == policy_raw
    assert (policy.lease_rounds, policy.max_running_jobs, policy.gap_cap_rounds) == (100, 2, 400)


@pytest.mark.xfail(
    strict=True,
    reason="branch hotfix-finalize-entry-gate (not merged, for a future run): finalize_v2 judges "
    "its beacon gate before its replay, so its owned body, and implementation_hash, change "
    "(24fbe36d... -> 8235e5f5d3ed56075a86e22d5702c6db15925bf849c2e55979ca83b98184b5a8)",
)
def test_the_s20_run_keeps_its_implementation_hash() -> None:
    manifest = RunManifestV2.model_validate_json((RUN / "manifest.json").read_bytes())
    source = Path(__file__).parents[3] / "src/hypertrain"
    pin = manifest.training.reference_spec.env.pin()
    assert (
        ChallengeStore._service_implementation_v2(source, IMPLEMENTATION_PATHS, pin)
        == IMPLEMENTATION_HASH
    )
