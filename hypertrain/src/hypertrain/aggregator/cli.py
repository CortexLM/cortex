"""`hypertrain` console entry point (subcommand: verify-checkpoint)."""

from __future__ import annotations

import argparse
import json
import sys
from collections.abc import Sequence
from pathlib import Path

from hypertrain.aggregator.checkpoint import read_manifest, verify_checkpoint


def main(argv: Sequence[str] | None = None) -> int:
    ap = argparse.ArgumentParser(prog="hypertrain")
    sub = ap.add_subparsers(dest="cmd", required=True)
    vc = sub.add_parser("verify-checkpoint", help="verify a final checkpoint directory")
    vc.add_argument("dir", type=Path)
    vc.add_argument("--signer", help="expected coordinator ss58 (default: manifest signer)")
    args = ap.parse_args(argv)
    if not args.dir.is_dir():
        print(f"FAIL: {args.dir} is not a directory", file=sys.stderr)
        return 2
    errors = verify_checkpoint(args.dir, signer=args.signer)
    if errors:
        for e in errors:
            print(f"FAIL: {e}", file=sys.stderr)
        return 1
    b = read_manifest(args.dir)["body"]
    summary = {
        k: b[k] for k in ("run_id", "rounds", "theta_hash", "included", "license", "dataset")
    }
    print(json.dumps({"result": "VERIFIED", **summary}, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
