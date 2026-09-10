#!/usr/bin/env python3
"""Resolve a Harbor ``--agent`` import path from a miner agent directory.

Harbor's ``-a`` / ``--agent`` accepts a built-in name (``terminus-2``), a
Python import path (``module.path:ClassName``), or an ACP registry shorthand.
It does **not** accept a filesystem path — verified against Harbor CLI help
and ``AgentFactory.get_agent_class_from_config`` (name with ``:`` is treated
as ``import_path``; a directory is not).

This helper turns an artefact directory into ``module:Class`` so evaluate can
pass ``-a`` without silently falling back to the topic's built-in agent.
Miner code is **not** executed: class discovery is AST-only.
"""

from __future__ import annotations

import argparse
import ast
import sys
from pathlib import Path

AGENT_BASES = frozenset({"BaseAgent", "BaseInstalledAgent"})
MAX_PY_BYTES = 256 * 1024
MAX_PY_FILES = 64


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
        return line
    _fail(f"{path} is empty")
    return None


def _base_attr(node: ast.expr) -> str | None:
    if isinstance(node, ast.Name):
        return node.id
    if isinstance(node, ast.Attribute):
        return node.attr
    if isinstance(node, ast.Subscript):
        return _base_attr(node.value)
    return None


def _agent_classes(tree: ast.AST) -> list[str]:
    found: list[str] = []
    for node in ast.walk(tree):
        if not isinstance(node, ast.ClassDef):
            continue
        for base in node.bases:
            attr = _base_attr(base)
            if attr in AGENT_BASES:
                found.append(node.name)
                break
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


def discover(agent_dir: Path) -> tuple[str, str]:
    """Return ``(import_path, pythonpath_parent)``."""
    if not agent_dir.is_dir():
        _fail(f"not a directory: {agent_dir}")
    parent = str(agent_dir.parent.resolve())
    named = _read_import_path_file(agent_dir)
    if named is not None:
        return named, parent

    py_files = sorted(
        p
        for p in agent_dir.rglob("*.py")
        if p.is_file() and ".." not in p.parts
    )[:MAX_PY_FILES]
    if not py_files:
        _fail(
            f"{agent_dir} is not a Harbor agent directory "
            "(no import_path file and no .py). Harbor -a does not take a path."
        )

    discovered: list[tuple[str, str]] = []
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
        for cls in _agent_classes(tree):
            discovered.append((_module_name(agent_dir, py_file) + ":" + cls, parent))

    if not discovered:
        _fail(
            f"{agent_dir} has Python files but no BaseAgent / BaseInstalledAgent "
            "subclass and no import_path file. Harbor -a needs module:Class."
        )
    unique_paths = sorted({item[0] for item in discovered})
    if len(unique_paths) > 1:
        _fail(
            f"{agent_dir} has multiple Harbor agent classes ({', '.join(unique_paths)}); "
            "write a one-line import_path file (module.path:ClassName) to choose"
        )
    return discovered[0]


def is_agent_dir(agent_dir: Path) -> bool:
    try:
        discover(agent_dir)
    except SystemExit:
        return False
    return True


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dir", required=True, help="candidate Harbor agent directory")
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
