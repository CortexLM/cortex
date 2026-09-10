#!/usr/bin/env python3
"""Copy pack tasks that typically finish in under one hour.

Duration is pack metadata, not a compiled task catalog. A task is excluded
when its declared timeout / duration is ≥ ``max_duration_s`` (default 3600),
when a pack ``filter.json`` deny-list names it, or when a non-empty allow-list
omits it. Unknown duration is kept unless the pack or topic asks to drop it.

The copy is written under the work directory; the pack tree is never mutated.
Zero surviving tasks fails closed.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import sys
from pathlib import Path
from typing import Any

DEFAULT_MAX_S = 3600
TASK_MARKERS = (
    "task.toml",
    "instruction.md",
    "instruction.txt",
    "docker-compose.yml",
    "docker-compose.yaml",
    "compose.yml",
    "compose.yaml",
    "environment.yaml",
    "environment.yml",
)
DURATION_KEYS = frozenset(
    {
        "estimated_duration_s",
        "typical_duration_s",
        "duration_s",
        "timeout_sec",
        "timeout_s",
        "max_time_sec",
        "time_limit_s",
        "agent_timeout_sec",
        "max_agent_timeout_sec",
    }
)
HOURS_KEYS = frozenset(
    {
        "estimated_duration_hours",
        "typical_duration_hours",
        "timeout_hours",
        "time_limit_hours",
    }
)


def _fail(msg: str, code: int = 2) -> None:
    print(msg, file=sys.stderr)
    raise SystemExit(code)


def _read_json(path: Path) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError, UnicodeError) as e:
        _fail(f"cannot read {path}: {e}")
    return None


def _load_toml(path: Path) -> dict[str, Any] | None:
    try:
        import tomllib
    except ImportError:  # pragma: no cover - py<3.11 guest images
        return None
    try:
        with path.open("rb") as fh:
            obj = tomllib.load(fh)
    except (OSError, tomllib.TOMLDecodeError):
        return None
    return obj if isinstance(obj, dict) else None


def _collect_durations(obj: Any) -> list[int]:
    found: list[int] = []
    if isinstance(obj, dict):
        for key, value in obj.items():
            lk = str(key).lower()
            if lk in DURATION_KEYS and isinstance(value, (int, float)) and not isinstance(
                value, bool
            ):
                found.append(int(value))
            elif lk in HOURS_KEYS and isinstance(value, (int, float)) and not isinstance(
                value, bool
            ):
                found.append(int(float(value) * 3600))
            else:
                found.extend(_collect_durations(value))
    elif isinstance(obj, list):
        for item in obj:
            found.extend(_collect_durations(item))
    return found


def _is_task_dir(path: Path) -> bool:
    if not path.is_dir():
        return False
    return any((path / marker).is_file() for marker in TASK_MARKERS)


def list_task_dirs(tasks_dir: Path) -> list[Path]:
    children = sorted(p for p in tasks_dir.iterdir() if p.is_dir() and _is_task_dir(p))
    if children:
        return children
    if _is_task_dir(tasks_dir):
        return [tasks_dir]
    # Harbor --path of loose task folders without markers: keep immediate dirs.
    loose = sorted(p for p in tasks_dir.iterdir() if p.is_dir())
    return loose


def task_duration_s(task_dir: Path, durations_map: dict[str, int]) -> int | None:
    name = task_dir.name
    if name in durations_map:
        return durations_map[name]
    for rel in ("task.toml", "config.toml", "harbor.toml"):
        parsed = _load_toml(task_dir / rel)
        if parsed:
            found = _collect_durations(parsed)
            if found:
                return max(found)
    for rel in ("duration.json", "meta.json"):
        path = task_dir / rel
        if path.is_file():
            obj = _read_json(path)
            found = _collect_durations(obj)
            if found:
                return max(found)
    return None


def load_pack_filter(pack_dir: Path, rel: str | None) -> dict[str, Any]:
    candidates: list[Path] = []
    if rel:
        if ".." in Path(rel).parts or rel.startswith("/"):
            _fail(f"task filter path must be a plain relative pack path: {rel}")
        candidates.append(pack_dir / rel)
    candidates.extend(
        [
            pack_dir / "filter.json",
            pack_dir / "task_filter.json",
            pack_dir / "short_tasks.json",
        ]
    )
    for path in candidates:
        if path.is_file():
            obj = _read_json(path)
            if isinstance(obj, dict):
                obj["_path"] = str(path)
                return obj
            if isinstance(obj, list) and all(isinstance(x, str) for x in obj):
                return {"allow": obj, "_path": str(path)}
            _fail(f"{path} must be a JSON object or an array of task names")
    allow_txt = pack_dir / "short_tasks.txt"
    if allow_txt.is_file():
        names = [
            line.strip()
            for line in allow_txt.read_text(encoding="utf-8", errors="replace").splitlines()
            if line.strip() and not line.strip().startswith("#")
        ]
        return {"allow": names, "_path": str(allow_txt)}
    return {}


def load_durations_map(pack_dir: Path, obj: dict[str, Any]) -> dict[str, int]:
    out: dict[str, int] = {}
    raw = obj.get("durations") if isinstance(obj.get("durations"), dict) else None
    paths = [pack_dir / "task_durations.json", pack_dir / "durations.json"]
    for path in paths:
        if path.is_file():
            loaded = _read_json(path)
            if isinstance(loaded, dict):
                raw = {**(loaded.get("durations") if isinstance(loaded.get("durations"), dict) else loaded), **(raw or {})}
            break
    if not raw:
        return out
    for key, value in raw.items():
        if key.startswith("_"):
            continue
        if isinstance(value, (int, float)) and not isinstance(value, bool):
            out[str(key)] = int(value)
    return out


def decide(
    task_dir: Path,
    *,
    max_s: int,
    allow: set[str],
    deny: set[str],
    durations_map: dict[str, int],
    drop_unknown: bool,
) -> tuple[bool, str]:
    name = task_dir.name
    if name in deny:
        return False, "deny-list"
    if allow and name not in allow:
        return False, "not on allow-list"
    duration = task_duration_s(task_dir, durations_map)
    if duration is None:
        if drop_unknown:
            return False, "unknown duration"
        return True, "unknown duration kept"
    if duration >= max_s:
        return False, f"duration_s={duration} >= {max_s}"
    return True, f"duration_s={duration}"


def filter_tasks(
    tasks_dir: Path,
    dest_dir: Path,
    *,
    pack_dir: Path,
    max_s: int,
    filter_rel: str | None,
    drop_unknown: bool,
) -> dict[str, Any]:
    if not tasks_dir.is_dir():
        _fail(f"no tasks directory {tasks_dir}")
    spec = load_pack_filter(pack_dir, filter_rel)
    if isinstance(spec.get("max_duration_s"), (int, float)):
        packed_max = int(spec["max_duration_s"])
        if packed_max > 0:
            max_s = min(max_s, packed_max)
    allow = {str(x) for x in spec.get("allow", []) if isinstance(x, str) and x}
    deny = {str(x) for x in spec.get("deny", []) if isinstance(x, str) and x}
    durations_map = load_durations_map(pack_dir, spec)
    if spec.get("exclude_unknown_duration") is True:
        drop_unknown = True

    dest_dir.parent.mkdir(parents=True, exist_ok=True)
    if dest_dir.exists():
        shutil.rmtree(dest_dir)
    dest_dir.mkdir(parents=True)

    kept: list[dict[str, str]] = []
    dropped: list[dict[str, str]] = []
    for task_dir in list_task_dirs(tasks_dir):
        ok, reason = decide(
            task_dir,
            max_s=max_s,
            allow=allow,
            deny=deny,
            durations_map=durations_map,
            drop_unknown=drop_unknown,
        )
        row = {"name": task_dir.name, "reason": reason}
        if not ok:
            dropped.append(row)
            continue
        shutil.copytree(task_dir, dest_dir / task_dir.name, dirs_exist_ok=True)
        kept.append(row)

    if not kept:
        _fail(
            f"task duration filter left 0 tasks under {tasks_dir} "
            f"(max_duration_s={max_s}, dropped={len(dropped)}); refusing to score an empty pack"
        )
    summary = {
        "max_duration_s": max_s,
        "filter": spec.get("_path", ""),
        "n_kept": len(kept),
        "n_dropped": len(dropped),
        "kept": kept,
        "dropped": dropped,
    }
    (dest_dir / ".proof-task-filter.json").write_text(
        json.dumps(summary, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    return summary


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tasks-dir", required=True)
    parser.add_argument("--dest-dir", required=True)
    parser.add_argument("--pack-dir", default=os.environ.get("PROOF_PACK_DIR", ""))
    parser.add_argument(
        "--max-duration-s",
        type=int,
        default=int(os.environ.get("PROOF_PARAM_MAX_TASK_DURATION_S") or DEFAULT_MAX_S),
    )
    parser.add_argument(
        "--filter-rel",
        default=os.environ.get("PROOF_PARAM_TASK_FILTER", ""),
    )
    parser.add_argument(
        "--drop-unknown",
        action="store_true",
        default=os.environ.get("PROOF_PARAM_EXCLUDE_UNKNOWN_DURATION", "") == "true",
    )
    args = parser.parse_args(argv)
    pack_dir = Path(args.pack_dir) if args.pack_dir else Path(args.tasks_dir).parent
    max_s = args.max_duration_s
    if max_s <= 0:
        _fail(f"max_duration_s must be a positive integer, got {max_s}")
    summary = filter_tasks(
        Path(args.tasks_dir),
        Path(args.dest_dir),
        pack_dir=pack_dir,
        max_s=max_s,
        filter_rel=args.filter_rel.strip() or None,
        drop_unknown=args.drop_unknown,
    )
    print(
        f"filter_tasks: kept {summary['n_kept']} dropped {summary['n_dropped']} "
        f"max_duration_s={summary['max_duration_s']}",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
