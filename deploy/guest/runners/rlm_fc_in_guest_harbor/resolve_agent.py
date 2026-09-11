#!/usr/bin/env python3
"""Resolve a Harbor ``--agent`` import path from a miner agent directory.

Harbor's ``-a`` / ``--agent`` accepts a built-in name (``terminus-2``), a
Python import path (``module.path:ClassName``), or an ACP registry shorthand.
It does **not** accept a filesystem path — verified against Harbor CLI help
and ``AgentFactory.get_agent_class_from_config`` (name with ``:`` is treated
as ``import_path``; a directory is not).

This helper turns an artefact directory into ``module:Class`` so evaluate can
pass ``-a`` without silently falling back to the topic's built-in agent.
Custom Python (``Agent`` / ``ProofAgent``) is the primary path; Harbor
``BaseAgent`` subclasses still resolve. Miner code is **not** executed:
class discovery is AST-only. A named ``import_path`` is resolved in the
evaluate import env (artefact parent only) and rejected when the origin is
outside the staged artefact.
"""

from __future__ import annotations

import argparse
import ast
import importlib.machinery
import os
import sys
from pathlib import Path

AGENT_BASES = frozenset({"BaseAgent", "BaseInstalledAgent"})
# Custom Python primary path: miners need not subclass Harbor BaseAgent.
AGENT_CLASS_NAMES = frozenset({"Agent", "ProofAgent", "CustomAgent"})
MAX_PY_BYTES = 256 * 1024
MAX_PY_FILES = 64
_NON_FILE_ORIGINS = frozenset({"built-in", "frozen"})


def _fail(msg: str, code: int = 2) -> None:
    print(msg, file=sys.stderr)
    raise SystemExit(code)


def _read_import_path_file(agent_dir: Path) -> str | None:
    path = agent_dir / "import_path"
    if not path.is_file():
        return None
    try:
        text = path.read_text(encoding="utf-8", errors="replace")
    except OSError as e:
        _fail(f"cannot read {path}: {e}")
    for raw in text.splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        if ":" not in line or line.startswith("acp:"):
            _fail(
                f"{path} must name a Python import path module.path:ClassName; "
                f"got {line!r} (a built-in Harbor name is not miner code)"
            )
        if ".." in line or "/" in line or "\\" in line:
            _fail(f"{path} is not a Python import path: {line!r}")
        _assert_import_origin(line, agent_dir)
        return line
    _fail(f"{path} is empty")
    return None


def _containment_root(agent_dir: Path) -> Path:
    raw = os.environ.get("PROOF_ARTIFACT_DIR", "").strip()
    if raw:
        art = Path(raw)
        if art.is_dir():
            return art.resolve()
    return agent_dir.resolve()


def _is_inside(path: Path, root: Path) -> bool:
    try:
        path.resolve().relative_to(root.resolve())
        return True
    except (OSError, ValueError):
        return False


def _module_file_candidates(search_root: Path, module_name: str) -> list[Path]:
    parts = [p for p in module_name.split(".") if p]
    if not parts or any(p in {".", ".."} or not p.isidentifier() for p in parts):
        return []
    base = search_root.joinpath(*parts)
    return [Path(str(base) + ".py"), base / "__init__.py"]


def _resolve_module_origin(module_name: str, search_root: Path) -> Path | None:
    """Resolve ``module_name`` using only ``search_root`` (no inherited PYTHONPATH).

    Miner code is not executed. PathFinder + on-disk candidates only.
    """
    try:
        spec = importlib.machinery.PathFinder.find_spec(module_name, [str(search_root)])
    except KeyError:
        # Parent package is not on sys.modules (implicit namespace / first look).
        spec = None
    if spec is not None and isinstance(spec.origin, str) and spec.origin not in _NON_FILE_ORIGINS:
        origin = Path(spec.origin)
        if origin.is_file():
            return origin
    for candidate in _module_file_candidates(search_root, module_name):
        if candidate.is_file():
            return candidate
    return None


def _assert_import_origin(import_path: str, agent_dir: Path) -> None:
    """Reject ``module:Class`` whose origin is outside the staged artefact."""
    module_name, sep, class_name = import_path.partition(":")
    if not sep or not module_name or not class_name:
        _fail(f"{import_path!r} is not a Python import path module.path:ClassName")
    if not class_name.isidentifier():
        _fail(f"{import_path!r} class name is not a Python identifier")
    search_root = agent_dir.resolve().parent
    origin = _resolve_module_origin(module_name, search_root)
    if origin is None:
        _fail(
            f"{import_path} does not resolve to a module under {search_root} "
            "(inherited PYTHONPATH / stdlib / Harbor installs are not miner code)"
        )
    bound = _containment_root(agent_dir)
    if not _is_inside(origin, bound):
        _fail(
            f"{import_path} origin {origin.resolve()} is outside staged artefact {bound}"
        )
    if not _is_inside(origin, agent_dir) and not _is_inside(origin, search_root):
        _fail(
            f"{import_path} origin {origin.resolve()} is outside the evaluate import root"
        )


def _base_attr(node: ast.expr) -> str | None:
    if isinstance(node, ast.Name):
        return node.id
    if isinstance(node, ast.Attribute):
        return node.attr
    if isinstance(node, ast.Subscript):
        return _base_attr(node.value)
    return None


def _agent_classes(tree: ast.AST) -> list[tuple[str, str]]:
    """Return ``(class_name, kind)`` where kind is ``harbor`` or ``python``."""
    found: list[tuple[str, str]] = []
    seen: set[str] = set()
    for node in ast.walk(tree):
        if not isinstance(node, ast.ClassDef):
            continue
        kind = None
        for base in node.bases:
            attr = _base_attr(base)
            if attr in AGENT_BASES:
                kind = "harbor"
                break
        if kind is None and node.name in AGENT_CLASS_NAMES:
            kind = "python"
        if kind is None:
            continue
        if node.name in seen:
            continue
        seen.add(node.name)
        found.append((node.name, kind))
    return found


def _module_name(agent_dir: Path, py_file: Path) -> str:
    rel = py_file.relative_to(agent_dir)
    parts = list(rel.with_suffix("").parts)
    if parts[-1] == "__init__":
        parts = parts[:-1]
    pkg = agent_dir.name
    if not parts:
        return pkg
    return pkg + "." + ".".join(parts)


def inspect_agent(agent_dir: Path) -> dict[str, str]:
    """Return ``import_path``, ``pythonpath``, and ``kind`` (``harbor`` / ``python``)."""
    if not agent_dir.is_dir():
        _fail(f"not a directory: {agent_dir}")
    parent = str(agent_dir.parent.resolve())
    named = _read_import_path_file(agent_dir)
    if named is not None:
        kind = _kind_for_import_path(agent_dir, named)
        return {"import_path": named, "pythonpath": parent, "kind": kind}

    py_files = sorted(
        p
        for p in agent_dir.rglob("*.py")
        if p.is_file() and ".." not in p.parts
    )[:MAX_PY_FILES]
    if not py_files:
        _fail(
            f"{agent_dir} is not a Python agent directory "
            "(no import_path file and no .py). Pass module:Class, not a path."
        )

    discovered: list[tuple[str, str, str]] = []
    for py_file in py_files:
        try:
            size = py_file.stat().st_size
        except OSError:
            continue
        if size > MAX_PY_BYTES:
            continue
        try:
            src = py_file.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        try:
            tree = ast.parse(src, filename=str(py_file))
        except SyntaxError:
            continue
        for cls, kind in _agent_classes(tree):
            discovered.append(
                (_module_name(agent_dir, py_file) + ":" + cls, parent, kind)
            )

    if not discovered:
        _fail(
            f"{agent_dir} has Python files but no custom Agent / ProofAgent class "
            "and no Harbor BaseAgent subclass, and no import_path file."
        )
    unique_paths = sorted({item[0] for item in discovered})
    if len(unique_paths) > 1:
        _fail(
            f"{agent_dir} has multiple agent classes ({', '.join(unique_paths)}); "
            "write a one-line import_path file (module.path:ClassName) to choose"
        )
    import_path, pythonpath, kind = discovered[0]
    _assert_import_origin(import_path, agent_dir)
    return {"import_path": import_path, "pythonpath": pythonpath, "kind": kind}


def _kind_for_import_path(agent_dir: Path, import_path: str) -> str:
    module_name, _, class_name = import_path.partition(":")
    parts = [p for p in module_name.split(".") if p]
    if not parts:
        return "python"
    # Prefer a file inside the agent dir matching the last module part.
    candidates = [
        agent_dir / (parts[-1] + ".py"),
        agent_dir / parts[-1] / "__init__.py",
        agent_dir / "agent.py",
    ]
    if len(parts) >= 2:
        candidates.insert(0, agent_dir / (parts[-1] + ".py"))
    for py_file in candidates:
        if not py_file.is_file():
            continue
        try:
            tree = ast.parse(
                py_file.read_text(encoding="utf-8", errors="replace"),
                filename=str(py_file),
            )
        except (OSError, SyntaxError):
            continue
        for cls, kind in _agent_classes(tree):
            if cls == class_name:
                return kind
    return "python"


def discover(agent_dir: Path) -> tuple[str, str]:
    """Return ``(import_path, pythonpath_parent)``."""
    info = inspect_agent(agent_dir)
    return info["import_path"], info["pythonpath"]


def is_agent_dir(agent_dir: Path) -> bool:
    try:
        discover(agent_dir)
    except SystemExit:
        return False
    return True


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dir", required=True, help="candidate miner agent directory")
    parser.add_argument(
        "--check",
        action="store_true",
        help="exit 0 if it is a Harbor agent dir, 1 otherwise (no import path printed)",
    )
    args = parser.parse_args(argv)
    agent_dir = Path(args.dir)
    if args.check:
        return 0 if is_agent_dir(agent_dir) else 1
    import_path, pythonpath = discover(agent_dir)
    print(f"import_path={import_path}")
    print(f"pythonpath={pythonpath}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
