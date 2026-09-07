"""TEST-ONLY scoring image that behaves like hostile miner code.

The real image loads the artifact with `trust_remote_code=True`, so scoring can
run anything it likes. This fixture stands in for that: it calls the judge
through the rewritten base_url AND tries to exfiltrate to several off-network
destinations, then reports every outcome.

It emits a valid PROOF_METRICS document so the observer admits it and the test
can read the findings from the returned observation. The findings ride in
`rationale`, which is a free-text field, so no schema is bent to carry them.
"""

from __future__ import annotations

import argparse
import json
import os
import socket
import sys
import urllib.error
import urllib.request

SPLITS = ["web_ood", "code_ood", "math_ood", "longctx", "multilingual_ood"]


def _probe_tcp(host: str, port: int) -> str:
    try:
        with socket.create_connection((host, port), timeout=4):
            return "reachable"
    except Exception as exc:  # noqa: BLE001
        return f"blocked:{type(exc).__name__}"


def _probe_http(url: str) -> str:
    try:
        with urllib.request.urlopen(url, timeout=4) as resp:
            return f"reachable:{getattr(resp, 'status', 0)}"
    except urllib.error.HTTPError as exc:
        return f"reachable:{exc.code}"
    except Exception as exc:  # noqa: BLE001
        return f"blocked:{type(exc).__name__}"


parser = argparse.ArgumentParser()
sub = parser.add_subparsers(dest="cmd", required=True)
score = sub.add_parser("score")
score.add_argument("--request", required=True)
score.add_argument("--out", required=True)
args = parser.parse_args()

with open(args.request, encoding="utf-8") as handle:
    request = json.load(handle)

# 1. The judge call the real image makes, through the proxy.
base = str(request.get("base_url", "")).rstrip("/")
body = json.dumps(
    {"model": request.get("model_ref"), "messages": [{"role": "user", "content": "ping"}]}
).encode()
judge_req = urllib.request.Request(base + "/chat/completions", data=body, method="POST")
judge_req.add_header("Content-Type", "application/json")
judge_reply = ""
try:
    with urllib.request.urlopen(judge_req, timeout=15) as resp:
        judge = f"reachable:{getattr(resp, 'status', 0)}"
        # Keep a little of the body so the test can prove a real model answered
        # rather than a proxy-generated status.
        judge_reply = resp.read(400).decode("utf-8", "replace")
except urllib.error.HTTPError as exc:
    # An auth/quota error still proves the request left and came back.
    judge = f"reachable:{exc.code}"
except Exception as exc:  # noqa: BLE001
    judge = f"blocked:{type(exc).__name__}"

# 2. Exfiltration attempts that must all fail on an internal network.
escapes = {
    "public_dns": _probe_tcp("8.8.8.8", 53),
    "public_http": _probe_http("http://93.184.216.34/"),
    "direct_upstream": _probe_http("https://api.openai.com/v1/models"),
    "docker_gateway": _probe_tcp("172.17.0.1", 2375),
}

# 3. Prove the judge credential never entered this container. The base image
# ships an unrelated GPG_KEY, so match the secret's own value and the proxy's
# variable names rather than any name containing "KEY".
_expected = os.environ.get("PROOF_TEST_EXPECTED_SECRET", "")
leaked = sorted(
    k
    for k, v in os.environ.items()
    if (k.startswith("PROOF_JUDGE_") and k != "PROOF_JUDGE_PROXY")
    or str(v).startswith("sk-")
    or (_expected and _expected in str(v))
)

# 4. The proxy's staged key must not be visible through the filesystem either.
try:
    with open("/run/proof/judge.json", encoding="utf-8") as handle:
        key_file = f"readable:{len(handle.read())}"
except Exception as exc:  # noqa: BLE001
    key_file = f"blocked:{type(exc).__name__}"

# 5. Everything the workload can actually read about where it is calling.
visible = {"env": dict(os.environ), "base_url": base}

findings = {
    "judge": judge,
    "escapes": escapes,
    "leaked_env": leaked,
    "key_file": key_file,
    "visible": visible,
    "judge_reply": judge_reply,
}

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
        "rationale": json.dumps(findings, separators=(",", ":")),
        "topic_id": request["topic_id"],
        "family": request.get("family") or "nll",
    },
    "harness": {
        "holdout_nll": 1.0,
        "split_nll": {s: 1.0 for s in SPLITS},
        "public_nll": None,
        "tokens_per_sec": None,
        "step_latency_ms": None,
        "wall_s": None,
        "custom_value": None,
        "canary_nll": None,
    },
}
body = json.dumps(document, separators=(",", ":"))
with open(args.out, "w", encoding="utf-8") as handle:
    handle.write(body)
print("PROOF_METRICS=" + body)
print("PROOF_EVAL_OK")
sys.stdout.flush()
