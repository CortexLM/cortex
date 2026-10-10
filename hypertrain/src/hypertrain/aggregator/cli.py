"""`hypertrain` console entry point (subcommand: verify-checkpoint)."""

from __future__ import annotations

import argparse
import json
import sys
from collections.abc import Sequence
from pathlib import Path

from hypertrain.aggregator.checkpoint import (
    network_inputs,
    read_manifest,
    require_network_roster,
    verify_checkpoint,
    verify_network_checkpoint,
    write_network_checkpoint,
)


def main(argv: Sequence[str] | None = None) -> int:
    ap = argparse.ArgumentParser(prog="hypertrain")
    sub = ap.add_subparsers(dest="cmd", required=True)
    vc = sub.add_parser("verify-checkpoint", help="verify a final checkpoint directory")
    vc.add_argument("dir", type=Path)
    vc.add_argument("--signer", help="expected coordinator ss58 (default: manifest signer)")
    vn = sub.add_parser("verify-checkpoint-v2")
    vn.add_argument("dir", type=Path)
    vn.add_argument("--signer", required=True)
    nc = sub.add_parser("checkpoint-v2")
    nc.add_argument("bundle", type=Path)
    nc.add_argument("dir", type=Path)
    nc.add_argument("--objects", type=Path, required=True)
    nc.add_argument("--keyfile", type=Path, required=True)
    ar = sub.add_parser("aggregate-v2")
    ar.add_argument("bundle", type=Path)
    ar.add_argument("--objects", type=Path, required=True)
    ar.add_argument("--keyfile", type=Path, required=True)
    rs = sub.add_parser("relay-serve")
    rs.add_argument("--host", default="127.0.0.1")
    rs.add_argument("--port", type=int, default=8080)
    args = ap.parse_args(argv)
    if args.cmd == "relay-serve":
        import uvicorn

        uvicorn.run("hypertrain.relay.app:app", host=args.host, port=args.port)
        return 0
    if args.cmd in ("checkpoint-v2", "aggregate-v2"):
        from hypertrain.aggregator.tape_v2 import make_tape, replay_tape
        from hypertrain.data.store import LocalFSStore
        from hypertrain.miner.core import load_keyfile
        from hypertrain.protocol.messages_v2 import AggregationPolicyV2, RoundOpenV2, RunManifestV2

        bundle = json.loads(args.bundle.read_bytes())
        rounds = bundle["rounds"]
        for item in rounds:
            require_network_roster(RoundOpenV2.model_validate(item["round_open"]["body"]).roster)
        manifest = RunManifestV2.model_validate(bundle["manifest"])
        store = LocalFSStore(args.objects)
        key = load_keyfile(args.keyfile)
        if args.cmd == "checkpoint-v2":
            result = write_network_checkpoint(args.dir, key, store, manifest, rounds)
        else:
            item = rounds[-1]
            policy = AggregationPolicyV2.model_validate_json(
                store.get(manifest.network.aggregation_policy_hash)
            )
            economics = store.get(manifest.network.economics_policy_hash)
            require_network_roster(item["round_open"]["body"]["roster"])
            inputs = network_inputs(item["inputs"])
            tape = make_tape(
                store,
                manifest,
                policy,
                economics,
                key,
                w=item["round_open"]["body"]["w"],
                prev_state=item["prev_state"],
                predecessor_tape_hash=item["predecessor_tape_hash"],
                inputs=inputs,
                reference_reward_units=item["reference_reward_units"],
            )
            require_network_roster(item["round_open"]["body"]["roster"])
            replay_tape(
                store,
                tape,
                manifest,
                policy,
                economics,
                signer=key.ss58,
                w=tape.body.w,
                prev_state=item["prev_state"],
                predecessor_tape_hash=item["predecessor_tape_hash"],
                inputs=inputs,
                reference_reward_units=item["reference_reward_units"],
            )
            result = {"tape_hash": store.put(tape.to_bytes())}
        print(json.dumps(result, sort_keys=True))
        return 0
    if not args.dir.is_dir():
        print(f"FAIL: {args.dir} is not a directory", file=sys.stderr)
        return 2
    errors = (
        verify_network_checkpoint(args.dir, args.signer)
        if args.cmd == "verify-checkpoint-v2"
        else verify_checkpoint(args.dir, signer=args.signer)
    )
    if errors:
        for e in errors:
            print(f"FAIL: {e}", file=sys.stderr)
        return 1
    b = read_manifest(args.dir)["body"]
    summary = {
        k: b[k]
        for k in ("run_id", "rounds", "theta_hash", "included", "license", "dataset")
        if k in b
    }
    print(json.dumps({"result": "VERIFIED", **summary}, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
