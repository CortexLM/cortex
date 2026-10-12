"""Sign an UpdateManifest for a built wheel with the release key file (hex sr25519 seed)."""

from __future__ import annotations

import argparse
import hashlib
import sys
import time
from pathlib import Path

from hypertrain.miner.update import MIN_WINDOW_S, UpdateManifest, sign_manifest, verify_manifest
from hypertrain.protocol.keys import Keypair


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--key-file", type=Path, required=True)
    p.add_argument("--tag", required=True)
    p.add_argument("--commit", required=True)
    p.add_argument("--wheel", type=Path, required=True)
    p.add_argument("--wheel-url", required=True)
    p.add_argument("--announce-at", type=int, default=None)
    p.add_argument("--window", type=int, default=MIN_WINDOW_S * 2)
    p.add_argument("--out", type=Path, required=True)
    a = p.parse_args(argv)
    key = Keypair(bytes.fromhex(a.key_file.read_text().strip()))
    announce_at = a.announce_at if a.announce_at is not None else int(time.time())
    manifest = UpdateManifest(
        version=a.tag.removeprefix("v"),
        tag=a.tag,
        commit=a.commit,
        wheel_url=a.wheel_url,
        wheel_sha256=hashlib.sha256(a.wheel.read_bytes()).hexdigest(),
        announce_at=announce_at,
        changeover_at=announce_at + a.window,
    )
    signed = sign_manifest(manifest, key)
    verify_manifest(signed, key.public)
    a.out.write_bytes(signed)
    print(f"signed {manifest.tag} by {key.public.hex()} -> {a.out}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
