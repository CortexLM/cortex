#!/usr/bin/env python3
"""Force agent-reachable networking on a filtered task copy.

Harbor's Docker environment rejects ``network_mode=no-network`` when the
daemon kernel cannot run the egress-control sidecar (Firecracker guest
kernels typically lack ``CONFIG_NFT_FIB_INET``). That aborts the trial
before the agent can call the pinned model. Agents in this eval path need
the internet (OpenRouter / BYOK). Host nftables on the TAP remain the
isolation boundary.

Rewrites only the destination tree (never the pack):

* ``task.toml`` / ``*.toml``: ``network_mode = "public"`` on environment,
  agent, and verifier tables.
* Compose / YAML: drop ``network_mode: none|no-network`` so Docker uses
  the default bridge rather than a Harbor policy the daemon cannot honour.
* JSON: rewrite ``"network_mode": "no-network"`` (Harbor env configs).
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

TOML_MODE = re.compile(
    r'(?im)^(?P<indent>\s*)network_mode\s*=\s*(?P<q>["\']?)'
    r'(?:no-network|no_network|none|isolated|allowlist)(?P=q)\s*$'
)
YAML_MODE = re.compile(
    r'(?im)^(?P<indent>\s*)network_mode\s*:\s*(?P<q>["\']?)'
    r'(?:no-network|no_network|none|isolated)(?P=q)\s*$'
)
TOML_SUFFIX = {".toml"}
YAML_SUFFIX = {".yml", ".yaml"}
JSON_SUFFIX = {".json"}
MAX_FILE_BYTES = 1024 * 1024
JSON_MODE = re.compile(
    r'(?i)("network_mode"\s*:\s*")(?:no-network|no_network|none|isolated|allowlist)(")'
)


def _fail(msg: str, code: int = 2) -> None:
    print(msg, file=sys.stderr)
    raise SystemExit(code)


def rewrite_toml(text: str, mode: str) -> str:
    return TOML_MODE.sub(rf'\g<indent>network_mode = "{mode}"', text)


def rewrite_yaml(text: str) -> str:
    # Drop the isolation pin so Compose uses bridge. Do not invent Harbor
    # network_mode keys that Docker Compose would treat as a Docker mode.
    return YAML_MODE.sub("", text)


def rewrite_json(text: str, mode: str) -> str:
    return JSON_MODE.sub(rf'\1{mode}\2', text)


def rewrite_tree(root: Path, mode: str) -> dict[str, int]:
    if not root.is_dir():
        _fail(f"not a directory: {root}")
    n_toml = 0
    n_yaml = 0
    n_json = 0
    for path in root.rglob("*"):
        if not path.is_file():
            continue
        if ".." in path.parts:
            continue
        suffix = path.suffix.lower()
        if suffix not in TOML_SUFFIX | YAML_SUFFIX | JSON_SUFFIX:
            continue
        try:
            if path.stat().st_size > MAX_FILE_BYTES:
                continue
            original = path.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        if suffix in TOML_SUFFIX:
            updated = rewrite_toml(original, mode)
            if updated != original:
                path.write_text(updated, encoding="utf-8")
                n_toml += 1
        elif suffix in YAML_SUFFIX:
            updated = rewrite_yaml(original)
            if updated != original:
                path.write_text(updated, encoding="utf-8")
                n_yaml += 1
        else:
            updated = rewrite_json(original, mode)
            if updated != original:
                path.write_text(updated, encoding="utf-8")
                n_json += 1
    return {"toml": n_toml, "yaml": n_yaml, "json": n_json}


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tasks-dir", required=True)
    parser.add_argument(
        "--mode",
        default="public",
        help="Harbor network_mode written into task.toml (default: public)",
    )
    args = parser.parse_args(argv)
    mode = args.mode.strip().lower()
    if mode != "public":
        _fail(
            f"agent network_mode must be public in the guest eval path, got {mode!r} "
            "(Docker no-network breaks agent OpenRouter calls on this runtime)"
        )
    stats = rewrite_tree(Path(args.tasks_dir), mode)
    print(
        f"rewrite_network: set Harbor network_mode={mode} in {stats['toml']} toml, "
        f"cleared compose isolation in {stats['yaml']} yaml, "
        f"rewrote {stats['json']} json",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
