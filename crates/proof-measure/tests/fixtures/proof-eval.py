"""TEST-ONLY stand-in for `proof-eval score`. Not the scoring image.

Emits the PROOF_METRICS/PROOF_EVAL_OK contract with deterministic values:
the NLL is derived from the artifact bytes the controller staged, FLOPs are a
fixed positive count, and every holdout shard must be present in the mounted
store or the run refuses (mirrors the real image's fail-closed behaviour).
"""
import argparse
import hashlib
import json
import os
import sys

SPLITS = ["web_ood", "code_ood", "math_ood", "longctx", "multilingual_ood"]

parser = argparse.ArgumentParser()
sub = parser.add_subparsers(dest="cmd", required=True)
score = sub.add_parser("score")
score.add_argument("--request", required=True)
score.add_argument("--out", required=True)
args = parser.parse_args()

with open(args.request, encoding="utf-8") as fh:
    request = json.load(fh)
if request.get("challenge_id") != "proof" or request.get("schema_version") != 1:
    print("refused: bad request", file=sys.stderr)
    sys.exit(2)
store = os.environ.get("PROOF_HOLDOUT_STORE", "")
for rec in request["holdout"]:
    if not os.path.isfile(os.path.join(store, rec["content_sha256"].lower())):
        print("refused: shard missing from PROOF_HOLDOUT_STORE", file=sys.stderr)
        sys.exit(2)
artifact_dir = os.environ.get("PROOF_ARTIFACT_DIR", "")
digest = hashlib.sha256()
for root, _dirs, files in os.walk(artifact_dir):
    for name in sorted(files):
        with open(os.path.join(root, name), "rb") as fh:
            digest.update(fh.read())
if not artifact_dir or digest.hexdigest() == hashlib.sha256(b"").hexdigest():
    print("refused: empty artifact", file=sys.stderr)
    sys.exit(2)
# Artifact content controls the NLL so tests can steer baseline vs candidate.
nll = 1.0 + (digest.digest()[0] % 8) / 100.0
try:
    with open(os.path.join(artifact_dir, "nll.txt"), encoding="utf-8") as fh:
        nll = float(fh.read().strip())
except (OSError, ValueError):
    pass
document = {
    "schema_version": 1,
    "submission_digest": request["submission_digest"],
    "artifact_digest": request["artifact_digest"],
    "topic_id": request["topic_id"],
    "eval_image_digest": request["eval_image_digest"],
    "holdout_commitment": request["holdout_commitment"],
    "agent": {
        "verdict": "clean",
        "reproduced": True,
        "claim_holds_public": True,
        "contamination": False,
        "canary_hit": False,
        "flops_used": 1_000_000,
        "flops_budget": request["flops_budget"],
        "cheat_codes": [],
        "rationale": "test-only deterministic observer",
        "topic_id": request["topic_id"],
        "family": request.get("family") or "nll",
    },
    "harness": {
        "holdout_nll": nll,
        "split_nll": {s: nll for s in SPLITS},
        "public_nll": None,
        "tokens_per_sec": None,
        "step_latency_ms": None,
        "wall_s": None,
        "custom_value": None,
        "canary_nll": None,
    },
}
body = json.dumps(document, separators=(",", ":"))
with open(args.out, "w", encoding="utf-8") as fh:
    fh.write(body)
print("PROOF_METRICS=" + body)
print("PROOF_EVAL_OK")
