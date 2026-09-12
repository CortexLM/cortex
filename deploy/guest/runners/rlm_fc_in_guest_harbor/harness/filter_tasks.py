#!/usr/bin/env python3
"""Materialise the scored task set for one paid job — from topic data only.

Nothing in this file names a benchmark, a task, or a slice. The set is a
pure function of the signed topic (``constraints.params`` /
``constraints.task_slice``, exported by the guest as ``PROOF_PARAM_*`` /
``PROOF_TASK_SLICE``) and of the topic-pinned pack (``filter.json``,
``slices/``, ``task_durations.json``). Selection, in order:

1. ``tasks`` (``PROOF_PARAM_TASKS``): the exact ordered names to score. A
   name the pack does not hold **fails closed** — a topic that names a task
   is never scored on a smaller set. One name is the single-task smoke.
2. else ``task_slice`` (``PROOF_TASK_SLICE``) when the pack defines it:
   ``slices/<label>.json`` / ``slices/<label>.txt`` or
   ``filter.json`` → ``slices.<label>``. A label the pack does not define
   fails closed **when the pack defines any slice at all**; a pack with no
   slices treats the label as informational (recorded in the summary).
3. else ``filter.json`` → ``allow`` (the pack's own default set).
4. else every task directory under ``tasks_dir``.

Then, always: ``task_exclude`` (``PROOF_PARAM_TASK_EXCLUDE``) and the pack's
``filter.json`` → ``deny`` are removed (exact names, no aliases); an optional
duration gate (``max_task_duration_s`` — the pack's ``max_duration_s`` may
only lower it; no gate when neither is set) drops tasks whose **known**
duration is at or over the ceiling (pack ``durations`` first, then duration
keys in task metadata — a declared *timeout* is a ceiling, not a duration,
and never counts); ``exclude_unknown_duration = "true"`` drops tasks with no
duration under a gate; ``n_tasks`` keeps the first N of what is left. An
empty result fails closed. The kept tasks are **copied** to ``--dest-dir``
(never the pack itself) and ``.proof-task-filter.json`` records what was
kept, what was dropped, and why.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import sys
from pathlib import Path
from typing import Any

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
# Duration metadata a task may declare. Timeouts (``timeout_sec``,
# ``agent_timeout_sec``, …) are deliberately absent: a harness ceiling says
# nothing about how long a task takes.
DURATION_KEYS = frozenset(
    {
        "estimated_duration_s",
        "typical_duration_s",
        "duration_s",
        "measured_duration_s",
        "wall_s",
    }
)
HOURS_KEYS = frozenset(
    {
        "estimated_duration_hours",
        "typical_duration_hours",
        "expert_time_estimate_hours",
        "time_estimate_hours",
    }
)
TASK_ID = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_.-]{0,63}$")
FILTER_FILES = ("filter.json", "task_filter.json")
DURATION_FILES = ("task_durations.json", "durations.json")
SLICES_DIR = "slices"


def _fail(msg: str, code: int = 2) -> None:
    print(f"filter_tasks: {msg}", file=sys.stderr)
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


def is_task_id(name: str) -> bool:
    return bool(TASK_ID.match(name)) and name not in {".", ".."}


def parse_names(raw: str | None, what: str) -> list[str]:
    """Comma / whitespace separated names; ordered, unique, well-formed."""
    if not raw or not raw.strip():
        return []
    out: list[str] = []
    for name in re.split(r"[,\s]+", raw.strip()):
        if not name:
            continue
        if not is_task_id(name):
            _fail(f"{what}: {name!r} is not a task name ([A-Za-z0-9][A-Za-z0-9_.-]{{0,63}})")
        if name in out:
            _fail(f"{what}: {name!r} is named twice")
        out.append(name)
    return out


def _names_from(obj: Any, what: str) -> list[str]:
    if isinstance(obj, dict):
        obj = obj.get("tasks", obj.get("allow"))
    if isinstance(obj, str):
        return parse_names(obj, what)
    if not isinstance(obj, list) or not all(isinstance(x, str) for x in obj):
        _fail(f"{what} must be a list of task names")
    return parse_names(",".join(obj), what)


def _collect_durations(obj: Any) -> list[int]:
    found: list[int] = []
    if isinstance(obj, dict):
        for key, value in obj.items():
            lk = str(key).lower()
            is_num = isinstance(value, (int, float)) and not isinstance(value, bool)
            if lk in DURATION_KEYS and is_num:
                found.append(int(value))
            elif lk in HOURS_KEYS and is_num:
                found.append(int(float(value) * 3600))
            else:
                found.extend(_collect_durations(value))
    elif isinstance(obj, list):
        for item in obj:
            found.extend(_collect_durations(item))
    return found


def _is_task_dir(path: Path) -> bool:
    return path.is_dir() and any((path / m).is_file() for m in TASK_MARKERS)


def list_task_dirs(tasks_dir: Path) -> list[Path]:
    children = sorted(p for p in tasks_dir.iterdir() if p.is_dir() and _is_task_dir(p))
    if children:
        return children
    if _is_task_dir(tasks_dir):
        return [tasks_dir]
    # Loose task folders without markers: keep immediate dirs.
    return sorted(p for p in tasks_dir.iterdir() if p.is_dir())


def load_pack_filter(pack_dir: Path, rel: str | None) -> dict[str, Any]:
    candidates: list[Path] = []
    if rel:
        if ".." in Path(rel).parts or rel.startswith("/"):
            _fail(f"task filter path must be a plain relative pack path: {rel}")
        candidates.append(pack_dir / rel)
    candidates.extend(pack_dir / name for name in FILTER_FILES)
    for path in candidates:
        if path.is_file():
            obj = _read_json(path)
            if isinstance(obj, dict):
                obj["_path"] = str(path)
                return obj
            if isinstance(obj, list) and all(isinstance(x, str) for x in obj):
                return {"allow": obj, "_path": str(path)}
            _fail(f"{path} must be a JSON object or an array of task names")
    if rel:
        _fail(f"task_filter names {rel}, which is not a file in the staged pack")
    return {}


def pack_defines_slices(pack_dir: Path, spec: dict[str, Any]) -> bool:
    if isinstance(spec.get("slices"), dict) and spec["slices"]:
        return True
    slices = pack_dir / SLICES_DIR
    return slices.is_dir() and any(p.is_file() for p in slices.iterdir())


def resolve_slice(pack_dir: Path, spec: dict[str, Any], label: str) -> list[str] | None:
    """Names the pack defines for ``label``; ``None`` when it defines none."""
    label = label.strip()
    if not label:
        return None
    if is_task_id(label):
        for suffix in (".json", ".txt"):
            path = pack_dir / SLICES_DIR / f"{label}{suffix}"
            if path.is_file():
                if suffix == ".json":
                    return _names_from(_read_json(path), f"slice {label}")
                lines = [
                    ln.strip()
                    for ln in path.read_text(encoding="utf-8", errors="replace").splitlines()
                    if ln.strip() and not ln.strip().startswith("#")
                ]
                return parse_names(",".join(lines), f"slice {label}")
    slices = spec.get("slices")
    if isinstance(slices, dict) and label in slices:
        return _names_from(slices[label], f"slice {label}")
    return None


def load_durations_map(pack_dir: Path, spec: dict[str, Any]) -> dict[str, int]:
    raw: dict[str, Any] = {}
    for name in DURATION_FILES:
        path = pack_dir / name
        if path.is_file():
            loaded = _read_json(path)
            if isinstance(loaded, dict):
                inner = loaded.get("durations")
                raw.update(inner if isinstance(inner, dict) else loaded)
            break
    if isinstance(spec.get("durations"), dict):
        raw.update(spec["durations"])
    out: dict[str, int] = {}
    for key, value in raw.items():
        if str(key).startswith("_"):
            continue
        if isinstance(value, (int, float)) and not isinstance(value, bool):
            out[str(key)] = int(value)
    return out


def task_duration_s(task_dir: Path, durations_map: dict[str, int]) -> int | None:
    """Known duration: the pack's measured map first, then task metadata."""
    name = task_dir.name
    if name in durations_map:
        return durations_map[name]
    for rel in ("task.toml", "config.toml"):
        parsed = _load_toml(task_dir / rel)
        if parsed:
            found = _collect_durations(parsed)
            if found:
                return max(found)
    for rel in ("duration.json", "meta.json"):
        path = task_dir / rel
        if path.is_file():
            found = _collect_durations(_read_json(path))
            if found:
                return max(found)
    return None


def select_base(
    available: list[str],
    *,
    tasks: list[str],
    task_slice: str | None,
    pack_dir: Path,
    spec: dict[str, Any],
) -> tuple[list[str], str, bool]:
    """The ordered base set and where it came from.

    Returns ``(names, source, slice_resolved)``.
    """
    have = set(available)

    def must_exist(names: list[str], what: str) -> list[str]:
        missing = [n for n in names if n not in have]
        if missing:
            _fail(
                f"{what} names {', '.join(missing)}, not present under the staged tasks_dir; "
                "refusing to score a smaller set than the topic named"
            )
        return names

    if tasks:
        return must_exist(tasks, "constraints.params.tasks"), "params.tasks", False
    label = (task_slice or "").strip()
    if label:
        names = resolve_slice(pack_dir, spec, label)
        if names is not None:
            if not names:
                _fail(f"pack slice {label!r} names no task")
            return must_exist(names, f"pack slice {label!r}"), f"pack slice {label}", True
        if pack_defines_slices(pack_dir, spec):
            _fail(
                f"task_slice {label!r} is not a slice this pack defines "
                f"({SLICES_DIR}/ or filter.json slices); refusing to guess a task set"
            )
    allow = spec.get("allow")
    if isinstance(allow, list) and allow:
        names = _names_from(allow, "filter.json allow")
        return must_exist(names, "pack filter.json allow"), "pack allow", False
    return list(available), "tasks_dir", False


def filter_tasks(
    tasks_dir: Path,
    dest_dir: Path,
    *,
    pack_dir: Path,
    tasks: list[str],
    exclude: list[str],
    n_tasks: int | None,
    task_slice: str | None,
    max_s: int | None,
    drop_unknown: bool,
    filter_rel: str | None,
) -> dict[str, Any]:
    if not tasks_dir.is_dir():
        _fail(f"no tasks directory {tasks_dir}")
    if n_tasks is not None and n_tasks <= 0:
        _fail(f"n_tasks must be a positive integer, got {n_tasks}")
    if max_s is not None and max_s <= 0:
        _fail(f"max_task_duration_s must be a positive integer, got {max_s}")
    spec = load_pack_filter(pack_dir, filter_rel)
    dirs = {p.name: p for p in list_task_dirs(tasks_dir)}
    available = sorted(dirs)
    base, source, slice_resolved = select_base(
        available, tasks=tasks, task_slice=task_slice, pack_dir=pack_dir, spec=spec
    )
    deny = set(exclude)
    pack_deny = spec.get("deny")
    if isinstance(pack_deny, list):
        deny.update(_names_from(pack_deny, "filter.json deny"))
    if overlap := [n for n in tasks if n in deny]:
        _fail(f"tasks and task_exclude/deny both name {', '.join(overlap)}")
    ceiling = max_s
    packed_max = spec.get("max_duration_s")
    if isinstance(packed_max, (int, float)) and not isinstance(packed_max, bool):
        packed_max = int(packed_max)
        if packed_max > 0:
            ceiling = packed_max if ceiling is None else min(ceiling, packed_max)
    if spec.get("exclude_unknown_duration") is True:
        drop_unknown = True
    durations_map = load_durations_map(pack_dir, spec)

    kept: list[dict[str, Any]] = []
    dropped: list[dict[str, Any]] = []
    for name in base:
        task_dir = dirs[name]
        if name in deny:
            dropped.append({"name": name, "reason": "excluded"})
            continue
        duration = task_duration_s(task_dir, durations_map)
        if ceiling is not None:
            if duration is None:
                if drop_unknown:
                    dropped.append({"name": name, "reason": "unknown duration"})
                    continue
            elif duration >= ceiling:
                dropped.append(
                    {"name": name, "reason": f"duration_s={duration} >= {ceiling}"}
                )
                continue
        if n_tasks is not None and len(kept) >= n_tasks:
            dropped.append({"name": name, "reason": f"beyond n_tasks={n_tasks}"})
            continue
        row: dict[str, Any] = {"name": name, "reason": source}
        if duration is not None:
            row["duration_s"] = duration
        kept.append(row)

    if not kept:
        _fail(
            f"selection left 0 tasks under {tasks_dir} (source={source}, "
            f"dropped={len(dropped)}); refusing to score an empty set"
        )
    dest_dir.parent.mkdir(parents=True, exist_ok=True)
    if dest_dir.exists():
        shutil.rmtree(dest_dir)
    dest_dir.mkdir(parents=True)
    for row in kept:
        shutil.copytree(dirs[row["name"]], dest_dir / row["name"], dirs_exist_ok=True)
    summary = {
        "source": source,
        "task_slice": (task_slice or "").strip(),
        "task_slice_resolved": slice_resolved,
        "filter": spec.get("_path", ""),
        "n_available": len(available),
        "n_kept": len(kept),
        "n_dropped": len(dropped),
        "n_tasks": n_tasks,
        "max_duration_s": ceiling,
        "exclude_unknown_duration": drop_unknown,
        "excluded": sorted(deny),
        "kept": kept,
        "dropped": dropped,
    }
    (dest_dir / ".proof-task-filter.json").write_text(
        json.dumps(summary, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    return summary


def _int_or_none(raw: str | None, what: str) -> int | None:
    if raw is None or not raw.strip():
        return None
    try:
        return int(raw.strip())
    except ValueError:
        _fail(f"{what} must be an integer, got {raw!r}")
    return None


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tasks-dir", required=True)
    parser.add_argument("--dest-dir", required=True)
    parser.add_argument("--pack-dir", default=os.environ.get("PROOF_PACK_DIR", ""))
    parser.add_argument("--tasks", default=os.environ.get("PROOF_PARAM_TASKS", ""))
    parser.add_argument("--exclude", default=os.environ.get("PROOF_PARAM_TASK_EXCLUDE", ""))
    parser.add_argument("--n-tasks", default=os.environ.get("PROOF_PARAM_N_TASKS", ""))
    parser.add_argument("--task-slice", default=os.environ.get("PROOF_TASK_SLICE", ""))
    parser.add_argument(
        "--max-duration-s",
        default=os.environ.get("PROOF_PARAM_MAX_TASK_DURATION_S", ""),
        help="duration ceiling in seconds; unset = no gate",
    )
    parser.add_argument(
        "--drop-unknown",
        action="store_true",
        default=os.environ.get("PROOF_PARAM_EXCLUDE_UNKNOWN_DURATION", "").strip().lower()
        == "true",
    )
    parser.add_argument(
        "--filter-rel",
        default=os.environ.get("PROOF_PARAM_TASK_FILTER", ""),
        help="relative pack path of filter.json (default: filter.json / task_filter.json)",
    )
    args = parser.parse_args(argv)
    pack_dir = Path(args.pack_dir) if args.pack_dir else Path(args.tasks_dir).parent
    summary = filter_tasks(
        Path(args.tasks_dir),
        Path(args.dest_dir),
        pack_dir=pack_dir,
        tasks=parse_names(args.tasks, "constraints.params.tasks"),
        exclude=parse_names(args.exclude, "constraints.params.task_exclude"),
        n_tasks=_int_or_none(args.n_tasks, "n_tasks"),
        task_slice=args.task_slice,
        max_s=_int_or_none(args.max_duration_s, "max_task_duration_s"),
        drop_unknown=args.drop_unknown,
        filter_rel=args.filter_rel.strip() or None,
    )
    print(
        f"filter_tasks: source={summary['source']} kept {summary['n_kept']} of "
        f"{summary['n_available']} (dropped {summary['n_dropped']}"
        + (f", max_duration_s={summary['max_duration_s']}" if summary["max_duration_s"] else "")
        + (f", n_tasks={summary['n_tasks']}" if summary["n_tasks"] else "")
        + ")",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
