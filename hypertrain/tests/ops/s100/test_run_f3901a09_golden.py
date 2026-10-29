"""The S17d run manifest (f3901a09, s100 TEST) loads unchanged under the audit freshness hotfix.

The bytes are the run's own `manifest.json` and `policy-audit.json` (S17c out dir). The hotfix
must not move the run id, the audit policy hash or the policy it names: the TEST stack keeps
serving that run after the server image swap.
"""

from __future__ import annotations

from pathlib import Path

from hypertrain.protocol.hashing import sha256_hex
from hypertrain.protocol.jcs import canonicalize
from hypertrain.protocol.messages_v2 import AuditPolicyV2, RunManifestV2

RUN = Path(__file__).parent / "fixtures/f3901a09"
RUN_ID = "f3901a090b584fc3cb920b1970448c9d781e5308bb4384dbe27b57226fa3019e"
AUDIT_POLICY = "1dd433fe0192026a8e830f1522b66a3ce88bcc924376e47960bca4df6d9fbe28"


def test_the_s17d_manifest_keeps_its_run_id_and_audit_policy_hash() -> None:
    raw = (RUN / "manifest.json").read_bytes()
    manifest = RunManifestV2.model_validate_json(raw)
    assert sha256_hex(raw) == manifest.run_id() == RUN_ID
    assert canonicalize(manifest.body()) == raw
    assert manifest.network.audit_policy_hash == AUDIT_POLICY
    policy_raw = (RUN / "policy-audit.json").read_bytes()
    policy = AuditPolicyV2.model_validate_json(policy_raw)
    assert sha256_hex(policy_raw) == policy.digest() == AUDIT_POLICY
    assert canonicalize(policy.body()) == policy_raw
    assert (policy.lease_rounds, policy.max_running_jobs) == (100, 2)
