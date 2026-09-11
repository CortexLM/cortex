#!/usr/bin/env python3
"""Copy pack tasks for Harbor evaluate / baseline.

Two owner-selected modes (``PROOF_TASK_FILTER`` / ``--mode``, also topic
``constraints.params.task_filter_mode`` → ``PROOF_PARAM_TASK_FILTER_MODE``):

* **first15** (default, measured TB4 first-15 baseline): keep the pack's
  first-15 set. Drop **INFRA-only** excludes (known broken cls / ctr /
  batched, plus the other broken-until-fixed Harbor ids). No shortpack
  allow-list. No duration / hour-plus wall gate — those would collapse
  first-15 to the 6-task Dev shortpack.
* **shortpack**: Dev n15 x0017 6-task allow-list plus hour-plus and broken
  excludes. Pack ``filter.json`` ``allow`` may only **intersect** that
  allow-list (further restrict). Pack ``max_duration_s`` may only lower
  the ceiling.

A task is always excluded when a pack ``filter.json`` deny-list names it
(or an alias / ``key-`` prefix). Empty filtered set fails closed.

Shortpack duration gate (not used in first15): a task named **exactly** on
the allow-list was measured under an hour, so only a measured wall
(``walls_sec`` / adaptor hint) may drop it; a pack-declared ``agent_timeout``
is the harness ceiling rather than a duration and is ignored for it.

Adaptor ``duration_hints.json`` (operator measurement, not a compiled Proof
catalog):

* **allow** (shortpack only, must be <1h): ``cargo-flight-dispatch``,
  ``embedding-drift-monitor``, ``bun-sourcemap-leak``, ``fin-saccr-rwa``,
  ``foodstuff-beta-activity``, ``atrx-vep-crispr``.
* **exclude >1h** (shortpack): ``biped-contact-dynamics`` (~5.2h),
  ``formal-crypto`` (~2.1h), ``cad-model`` (~1.2h), ``data-anonymization``
  (~1.1h).
* **infra / broken until fixed** (both modes): ``batched-eval-parity``
  (no-network), ``ctr-optimization`` and ``cumulative-layout-shift``
  (EnvStartTimeout), ``distributed-dedup`` (tmux), ``coq-block-bound``
  (wall cut). ``biped-contact-dynamics`` / ``cad-model`` also stay out of
  **shortpack** until verifier pytest is proven on metal; first15 keeps
  them (hour-plus is not INFRA).
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
# What an allow-list entry asserts: the adaptor measured that exact task under
# an hour. A tighter ceiling is outside that assertion.
ALLOW_VETTED_UNDER_S = DEFAULT_MAX_S
DEFAULT_HINTS = Path(__file__).resolve().parent / "duration_hints.json"
MODE_FIRST15 = "first15"
MODE_SHORTPACK = "shortpack"
FILTER_MODES = frozenset({MODE_FIRST15, MODE_SHORTPACK})
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
        "expert_time_estimate_hours",
        "time_estimate_hours",
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


def alias_match(name: str, key: str) -> bool:
    """Exact directory name, or Harbor id prefix (``biped`` → ``biped-…``)."""
    if not name or not key:
        return False
    if name == key:
        return True
    if name.startswith(key + "-"):
        return True
    if key.startswith(name + "-"):
        return True
    return False


def lookup_named(name: str, mapping: dict[str, int]) -> int | None:
    if name in mapping:
        return mapping[name]
    best: int | None = None
    for key, value in mapping.items():
        if alias_match(name, key):
            best = value if best is None else max(best, value)
    return best


# Dev default short-task filter from retained n15 x0017.
X0017_ALLOW = (
    "cargo-flight-dispatch",
    "embedding-drift-monitor",
    "bun-sourcemap-leak",
    "fin-saccr-rwa",
    "foodstuff-beta-activity",
    "atrx-vep-crispr",
)
X0017_EXCLUDE_LONG = (
    "biped-contact-dynamics",
    "formal-crypto",
    "cad-model",
    "data-anonymization",
)
X0017_EXCLUDE_BROKEN = (
    "batched-eval-parity",
    "ctr-optimization",
    "cumulative-layout-shift",
    "distributed-dedup",
    "coq-block-bound",
)
X0017_EXCLUDE = X0017_EXCLUDE_LONG + X0017_EXCLUDE_BROKEN


def parse_mode(raw: str | None) -> str:
    text = (raw or "").strip().lower()
    if not text:
        return MODE_FIRST15
    if text not in FILTER_MODES:
        _fail(f"task filter mode must be first15 or shortpack, got {raw!r}")
    return text


def load_adaptor_spec(
    path: Path | None = None,
) -> tuple[dict[str, int], set[str], set[str], set[str]]:
    hints_path = path or DEFAULT_HINTS
    if not hints_path.is_file():
        return {}, set(), set(), set(X0017_EXCLUDE_BROKEN)
    obj = _read_json(hints_path)
    if not isinstance(obj, dict):
        _fail(f"{hints_path} must be a JSON object")
    raw = obj.get("walls_sec")
    if raw is None:
        raw = obj.get("durations")
    if raw is None:
        raw = {}
    if not isinstance(raw, dict):
        _fail(f"{hints_path} walls_sec/durations must be an object")
    walls: dict[str, int] = {}
    for key, value in raw.items():
        if str(key).startswith("_"):
            continue
        if isinstance(value, (int, float)) and not isinstance(value, bool):
            walls[str(key)] = int(value)
    exclude: set[str] = set()
    for field in ("exclude", "deny"):
        listed_names = obj.get(field)
        if isinstance(listed_names, list):
            exclude.update(str(x) for x in listed_names if isinstance(x, str) and x.strip())
    allow: set[str] = set()
    raw_allow = obj.get("allow")
    if isinstance(raw_allow, list):
        allow.update(str(x) for x in raw_allow if isinstance(x, str) and x.strip())
    infra: set[str] = set()
    raw_infra = obj.get("infra_exclude")
    if isinstance(raw_infra, list):
        infra.update(str(x) for x in raw_infra if isinstance(x, str) and x.strip())
    if not infra:
        infra = set(X0017_EXCLUDE_BROKEN)
    return walls, exclude, allow, infra


def load_adaptor_hints(path: Path | None = None) -> dict[str, int]:
    walls, _exclude, _allow, _infra = load_adaptor_spec(path)
    return walls


def list_task_dirs(tasks_dir: Path) -> list[Path]:
    children = sorted(p for p in tasks_dir.iterdir() if p.is_dir() and _is_task_dir(p))
    if children:
        return children
    if _is_task_dir(tasks_dir):
        return [tasks_dir]
    # Harbor --path of loose task folders without markers: keep immediate dirs.
    loose = sorted(p for p in tasks_dir.iterdir() if p.is_dir())
    return loose


def task_duration_s(
    task_dir: Path,
    durations_map: dict[str, int],
    adaptor_hints: dict[str, int] | None = None,
) -> int | None:
    name = task_dir.name
    declared: int | None = None
    for rel in ("task.toml", "config.toml", "harbor.toml"):
        parsed = _load_toml(task_dir / rel)
        if parsed:
            found = _collect_durations(parsed)
            if found:
                declared = max(found)
                break
    if declared is None:
        for rel in ("duration.json", "meta.json"):
            path = task_dir / rel
            if path.is_file():
                obj = _read_json(path)
                found = _collect_durations(obj)
                if found:
                    declared = max(found)
                    break
    pack_hint = lookup_named(name, durations_map)
    adaptor_hint = lookup_named(name, adaptor_hints or {})
    candidates = [x for x in (declared, pack_hint, adaptor_hint) if x is not None]
    if not candidates:
        return None
    return max(candidates)


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


def listed(name: str, names: set[str]) -> bool:
    if name in names:
        return True
    return any(alias_match(name, key) for key in names)


def decide(
    task_dir: Path,
    *,
    max_s: int,
    allow: set[str],
    deny: set[str],
    durations_map: dict[str, int],
    adaptor_hints: dict[str, int],
    drop_unknown: bool,
    skip_duration: bool = False,
    keep_reason: str = "",
) -> tuple[bool, str]:
    name = task_dir.name
    if listed(name, deny):
        return False, "deny-list"
    if skip_duration:
        return True, keep_reason or "first15"
    if allow:
        if not listed(name, allow):
            return False, "not on allow-list"
        # An exact allow-list entry is a task measured under
        # ``ALLOW_VETTED_UNDER_S``. A pack ``task.toml`` ``agent_timeout`` is
        # the harness ceiling, not a duration, so it may not drop one (n15
        # attempt1: all six declared 28800 and the 3600 default emptied the
        # pack). A measured wall still may, at any ceiling. An alias hit is a
        # different task, and a ceiling tighter than the vetting bound is not
        # covered by the allow-list, so both fall through to the metadata gate.
        wall = lookup_named(name, adaptor_hints) if name in allow else None
        if wall is not None:
            if wall >= max_s:
                return False, f"allow-list wall_s={wall} >= {max_s}"
            return True, f"allow-list wall_s={wall}"
        if name in allow and max_s >= ALLOW_VETTED_UNDER_S:
            return True, "allow-list"
    duration = task_duration_s(task_dir, durations_map, adaptor_hints)
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
    hints_path: Path | None = None,
    mode: str | None = None,
) -> dict[str, Any]:
    if not tasks_dir.is_dir():
        _fail(f"no tasks directory {tasks_dir}")
    mode = parse_mode(mode)
    spec = load_pack_filter(pack_dir, filter_rel)
    if mode == MODE_SHORTPACK and isinstance(spec.get("max_duration_s"), (int, float)):
        packed_max = int(spec["max_duration_s"])
        if packed_max > 0:
            max_s = min(max_s, packed_max)
    allow = {str(x) for x in spec.get("allow", []) if isinstance(x, str) and x}
    deny = {str(x) for x in spec.get("deny", []) if isinstance(x, str) and x}
    durations_map = load_durations_map(pack_dir, spec)
    adaptor_hints, adaptor_exclude, adaptor_allow, adaptor_infra = load_adaptor_spec(
        hints_path
    )
    skip_duration = mode == MODE_FIRST15
    if mode == MODE_FIRST15:
        # Measured first-15: INFRA excludes only. Pack deny still applies.
        # Never the shortpack allow-list (adaptor or pack).
        allow = set()
        deny = deny | adaptor_infra
    else:
        deny = deny | adaptor_exclude
        if adaptor_allow:
            allow = (allow & adaptor_allow) if allow else set(adaptor_allow)
    if spec.get("exclude_unknown_duration") is True and not skip_duration:
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
            adaptor_hints=adaptor_hints,
            drop_unknown=drop_unknown,
            skip_duration=skip_duration,
            keep_reason=mode,
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
        "mode": mode,
        "filter": spec.get("_path", ""),
        "n_kept": len(kept),
        "n_dropped": len(dropped),
        "n_adaptor_hints": len(adaptor_hints),
        "n_adaptor_exclude": len(adaptor_exclude),
        "n_adaptor_allow": 0 if skip_duration else len(adaptor_allow),
        "n_adaptor_infra": len(adaptor_infra),
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
    parser.add_argument(
        "--hints",
        default="",
        help="optional duration_hints.json (default: adaptor-local n15 walls)",
    )
    parser.add_argument(
        "--mode",
        default=os.environ.get(
            "PROOF_TASK_FILTER",
            os.environ.get("PROOF_PARAM_TASK_FILTER_MODE", MODE_FIRST15),
        ),
        help="first15 (measured baseline, default) or shortpack (Dev n15 allow-list)",
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
        hints_path=Path(args.hints) if args.hints.strip() else None,
        mode=args.mode,
    )
    print(
        f"filter_tasks: mode={summary['mode']} kept {summary['n_kept']} "
        f"dropped {summary['n_dropped']} max_duration_s={summary['max_duration_s']}",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
