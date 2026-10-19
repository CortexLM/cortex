from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

import httpx

from hypertrain.beacon.core import BeaconError, BeaconUnavailable
from hypertrain.beacon.drand import RELAYS, DrandQuicknet
from hypertrain.beacon.push import Pusher, read_token


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(prog="python -m hypertrain.beacon")
    sub = p.add_subparsers(dest="cmd", required=True)
    c = sub.add_parser("check", help="fetch + verify the latest quicknet round")
    c.add_argument("--live", action="store_true", required=True)
    c.add_argument("--relay", action="append", default=None)
    u = sub.add_parser("push", help="keep a challenge service fed with verified rounds")
    u.add_argument("--api", required=True, help="challenge service base URL")
    u.add_argument("--admin-token-file", type=Path, required=True)
    u.add_argument("--relay", action="append", default=None, help="repeatable, >= 2")
    u.add_argument("--interval", type=float, default=3.0, help="seconds between ticks")
    u.add_argument("--catch-up", type=int, default=20, help="push at most the last N rounds")
    u.add_argument("--once", action="store_true", help="one tick, then exit")
    u.add_argument("--max-failures", type=int, default=10, help="consecutive, then exit 1")
    args = p.parse_args(argv)
    if args.cmd == "push":
        return _push(u, args)

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


def _http_url(p: argparse.ArgumentParser, flag: str, value: str) -> httpx.URL:
    try:
        url = httpx.URL(value)
    except httpx.InvalidURL as e:
        p.error(f"{flag}: {e}")
    if url.scheme not in ("http", "https") or not url.host:
        p.error(f"{flag} must be an http(s) URL")
    return url


def _push(p: argparse.ArgumentParser, args: argparse.Namespace) -> int:
    # Distinct relays, or "all >= 2 relays agree" degrades to trusting one.
    relays = tuple(dict.fromkeys(r.rstrip("/") for r in args.relay or RELAYS))
    for relay in relays:
        _http_url(p, "--relay", relay)
    if len(relays) < 2:
        p.error("need >= 2 distinct relays")
    # NaN fails every comparison; the upper bound keeps time.sleep() in range.
    if not 0 < args.interval <= 3600 or args.catch_up < 1 or args.max_failures < 1:
        p.error("--interval must be in (0, 3600], --catch-up and --max-failures >= 1")
    api = _http_url(p, "--api", args.api)
    try:
        token = read_token(args.admin_token_file)
    except (OSError, ValueError) as e:
        print(f"admin token: {e}", file=sys.stderr)
        return 2
    with httpx.Client(base_url=api, timeout=10.0) as service:
        pusher = Pusher(service, DrandQuicknet(relays), token, args.catch_up)
        return pusher.run(args.interval, args.once, args.max_failures)


if __name__ == "__main__":
    sys.exit(main())
