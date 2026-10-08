from __future__ import annotations

import argparse
import json
import sys

from hypertrain.beacon.core import BeaconError, BeaconUnavailable
from hypertrain.beacon.drand import RELAYS, DrandQuicknet


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(prog="python -m hypertrain.beacon")
    sub = p.add_subparsers(dest="cmd", required=True)
    c = sub.add_parser("check", help="fetch + verify the latest quicknet round")
    c.add_argument("--live", action="store_true", required=True)
    c.add_argument("--relay", action="append", default=None)
    args = p.parse_args(argv)

    relays = tuple(args.relay or RELAYS)
    beacon = DrandQuicknet(relays)
    try:
        info_ok = beacon.info_matches()
        br = beacon.latest()
    except BeaconUnavailable as e:
        print(json.dumps({"live": "CENSORED(network)", "relays": relays, "error": str(e)}))
        return 3
    except BeaconError as e:
        print(json.dumps({"live": "FAIL", "relays": relays, "error": str(e)}))
        return 1
    out = {
        "live": "PASS" if info_ok else "FAIL",
        "relays": relays,
        "chain_info_matches_pinned": info_ok,
        "round": br.round,
        "randomness": br.randomness,
        "signature": br.signature,
        "verified": br.bls_verified,
    }
    print(json.dumps(out))
    return 0 if info_ok and br.bls_verified else 1


if __name__ == "__main__":
    sys.exit(main())
