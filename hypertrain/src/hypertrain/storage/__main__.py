"""python -m hypertrain.storage doctor [--env-file F] [--cors-origin URL]."""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

from hypertrain.storage.doctor import doctor
from hypertrain.storage.r2 import ConfigError, load_config


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(prog="python -m hypertrain.storage")
    sub = p.add_subparsers(dest="cmd", required=True)
    d = sub.add_parser("doctor")
    d.add_argument("--env-file", type=Path)
    d.add_argument("--cors-origin", default="", help="also require CORS for this Origin")
    a = p.parse_args(argv)
    try:
        cfg = load_config(env_file=a.env_file)
    except (ConfigError, OSError) as e:
        print(f"FAIL config: {e}", file=sys.stderr)
        return 2
    res = doctor(cfg, cors_origin=a.cors_origin)
    for name, ok, detail in res:
        print(f"{'ok  ' if ok else 'FAIL'} {name} {detail}".rstrip())
    return 0 if all(ok for _, ok, _ in res) else 1


if __name__ == "__main__":
    sys.exit(main())
