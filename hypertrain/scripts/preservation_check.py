"""Hash every file protected by the research-quality-multiregion-20261004 freeze.

Default: print a JSON snapshot (sha256 per path, relative to the workspace root).
--out PATH: write the snapshot there.  --compare PATH: exit 1 if any hash differs
from that earlier snapshot (missing-before stays allowed only if still missing).
Paths under IGNORED_PREFIXES are orchestrator state that legitimately mutates during
a run; their changes are reported as ignored, never as failures.
"""

import argparse
import hashlib
import json
import sys
from pathlib import Path

ROOT = Path("/root/distributed-decision-training")
FREEZE_DIR = ROOT / "research-quality-multiregion-20261004"
IGNORED_PREFIXES = (
    ".omo/ulw-execute/",
    ".omo/boulder.json",
    ".omo/plans/",
    ".omo/drafts/",
    ".omo/evidence/",
)


def sha256(path: Path) -> str | None:
    if not path.is_file():
        return None
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def snapshot() -> dict[str, object]:
    inventory = json.loads((FREEZE_DIR / "inventory.json").read_text())
    rels = sorted(
        set(inventory)
        | {
            "research-quality-multiregion-20261004/inventory.json",
            "research-quality-multiregion-20261004/freeze.json",
        }
    )
    files = {rel: sha256(ROOT / rel) for rel in rels}
    return {
        "root": str(ROOT),
        "count": len(files),
        "hashed": sum(v is not None for v in files.values()),
        "missing": sorted(k for k, v in files.items() if v is None),
        "differs_from_inventory": sorted(
            k for k, v in files.items() if k in inventory and v is not None and v != inventory[k]
        ),
        "files": files,
    }


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser()
    g = ap.add_mutually_exclusive_group()
    g.add_argument("--out", type=Path)
    g.add_argument("--compare", type=Path)
    args = ap.parse_args(argv)
    snap = snapshot()
    if args.compare:
        before: dict[str, str | None] = json.loads(args.compare.read_text())["files"]
        now: dict[str, str | None] = snap["files"]  # type: ignore[assignment]
        diff = sorted(k for k in before.keys() | now.keys() if before.get(k) != now.get(k))
        ignored = [k for k in diff if k.startswith(IGNORED_PREFIXES)]
        changed = [k for k in diff if not k.startswith(IGNORED_PREFIXES)]
        report = {
            "compared": len(before),
            "ignored_prefixes": list(IGNORED_PREFIXES),
            "ignored_changed_count": len(ignored),
            "ignored_changed": ignored,
            "changed": changed,
        }
        print(json.dumps(report))
        return 1 if changed else 0
    text = json.dumps(snap, indent=1, sort_keys=True)
    if args.out:
        args.out.write_text(text + "\n")
        print(json.dumps({k: snap[k] for k in ("count", "hashed", "missing")}))
    else:
        print(text)
    return 0


if __name__ == "__main__":
    sys.exit(main())
