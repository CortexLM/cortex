"""`proof-eval` — image entrypoint.

    proof-eval score    --request request.json --out metrics.json
    proof-eval baseline --request request.json --out baseline.json
    proof-eval selftest
    proof-eval --help

`score` is what harvest-pod runs. It writes a one-line sidecar, then prints
PROOF_METRICS=<document> and PROOF_EVAL_OK. Failures exit non-zero with no
marker and no sidecar.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path

from .baked import baked_proxies, require_baked
from .contract import (
    ADAMW_SCRIPT,
    ContractError,
    OK_MARKER,
    PROOF_METRICS_SCHEMA,
    encode_document,
    marker_line,
)
from .fabric import selftest as fabric_selftest
from .judge import require_judge
from .request import read_request
from .agent import inspect
from .harness import measure, require_runtime

EXIT_REFUSED = 2
EXIT_ERROR = 1


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="proof-eval")
    sub = parser.add_subparsers(dest="cmd", required=True)
    score = sub.add_parser("score", help="score one harvest request")
    score.add_argument("--request", required=True, type=Path)
    score.add_argument("--out", required=True, type=Path)
    base = sub.add_parser("baseline", help="measure the sealed AdamW/comms reference")
    base.add_argument("--request", required=True, type=Path)
    base.add_argument("--out", required=True, type=Path)
    sub.add_parser("selftest", help="prove PATH + fabric + baked proxy (no holdout)")
    args = parser.parse_args(argv)
    try:
        if args.cmd == "selftest":
            return _selftest()
        if args.cmd == "score":
            return _score(args.request, args.out, baseline=False)
        if args.cmd == "baseline":
            return _score(args.request, args.out, baseline=True)
        return EXIT_ERROR
    except ContractError as exc:
        print(f"refused: {exc}", file=sys.stderr)
        return EXIT_REFUSED
    except Exception as exc:  # noqa: BLE001
        print(f"error: {exc}", file=sys.stderr)
        return EXIT_ERROR


def _selftest() -> int:
    proxies = baked_proxies()
    if not proxies:
        raise ContractError("no baked proxies")
    require_baked(proxies[0])
    fabric_selftest()
    try:
        require_runtime()
        runtime = "ok"
    except ContractError as exc:
        # Contract-only builds are allowed to fail runtime; scoring images
        # must not. The publish job runs selftest on the digest it just
        # pushed and refuses a pin if this path fails there.
        if os.environ.get("PROOF_SELFTEST_REQUIRE_RUNTIME", "").strip() in (
            "1",
            "true",
            "yes",
        ):
            raise
        print(f"selftest: runtime skipped ({exc})", file=sys.stderr)
        runtime = "skipped"
    print(
        json.dumps(
            {
                "ok": True,
                "baked_proxies": proxies,
                "fabric": "ok",
                "runtime": runtime,
                "adamw_script": ADAMW_SCRIPT,
            },
            separators=(",", ":"),
        )
    )
    return 0


def _score(request_path: Path, out: Path, *, baseline: bool) -> int:
    request = read_request(request_path)
    require_judge(request)
    from .fabric import enforce

    enforce(request.constraints)
    recipe = request.claim
    if Path(ADAMW_SCRIPT).is_file():
        recipe = f"{recipe}\n{Path(ADAMW_SCRIPT).read_text(encoding='utf-8')}"
    agent = inspect(request, recipe)
    artifact_dir = os.environ.get("PROOF_ARTIFACT_DIR") or os.environ.get("PROOF_PROXY_MODEL_DIR")
    if baseline:
        artifact_dir = os.environ.get("PROOF_PROXY_MODEL_DIR") or artifact_dir
    harness = measure(request, artifact_dir)
    if "artifact_fingerprint" in harness:
        harness = {k: v for k, v in harness.items() if k != "artifact_fingerprint"}
    document = {
        "schema_version": PROOF_METRICS_SCHEMA,
        "submission_digest": request.submission_digest,
        "artifact_digest": request.artifact_digest,
        "topic_id": request.topic_id,
        "eval_image_digest": request.eval_image_digest,
        "holdout_commitment": request.holdout_commitment,
        "agent": agent,
        "harness": harness,
    }
    body = encode_document(document)
    out.parent.mkdir(parents=True, exist_ok=True)
    tmp = out.with_suffix(out.suffix + ".partial")
    tmp.write_text(body, encoding="utf-8")
    tmp.replace(out)
    sys.stdout.write(marker_line(document) + "\n")
    sys.stdout.write(OK_MARKER + "\n")
    sys.stdout.flush()
    return 0
