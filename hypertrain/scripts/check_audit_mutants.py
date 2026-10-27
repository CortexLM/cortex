"""Mutation check of the audit, intake, sanction and ledger defences (ADR 0009).

A mutant removes or weakens one exact snippet of one source file; the tests named for it must then
fail. The check fails when a named test still passes under its mutant (pytest exit 1 is the only
kill: a collection error, a skip or a timeout is not), when a snippet is not found exactly once,
when a named test does not exist, or when the named tests fail without a mutant.

Every mutant runs in a private copy of the tree (src, tests, scripts), never in the working tree,
so the check can run beside other work and a killed run leaves nothing to restore. The service-level
tests need the opt-in flag and a graduated snapshot, which the first run builds (about 3 minutes)
and the later runs reuse. Run from hypertrain/:

    AUDIT_CHEATERS_FULL=1 AUDIT_CHEATERS_SNAPSHOT_DIR=<dir> \\
        uv run --frozen python scripts/check_audit_mutants.py [--list] [--only NAME ...]

`--list` validates the table without running a test. A mutant is a frozen `Mutant(name, path, old,
new, tests)`; `path` is relative to hypertrain/ and `tests` are pytest node ids relative to it.
"""

from __future__ import annotations

import argparse
import ast
import importlib.util
import os
import shutil
import subprocess
import sys
import tempfile
import time
from collections.abc import Iterable, Sequence
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
COPIED = ("src", "tests", "scripts", "pyproject.toml")
TIMEOUT_SECONDS = 1200


@dataclass(frozen=True, slots=True)
class Mutant:
    name: str
    path: str
    old: str
    new: str
    tests: tuple[str, ...]


def mutants() -> tuple[Mutant, ...]:
    """The table lives in scripts/audit_mutants_table.py so this runner stays readable."""
    spec = importlib.util.spec_from_file_location(
        "audit_mutants_table", Path(__file__).with_name("audit_mutants_table.py")
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return tuple(Mutant(*row) for row in module.TABLE)


def test_exists(root: Path, node_id: str) -> bool:
    """True when the file of `node_id` defines a function named like it (parameters ignored)."""
    file, _, name = node_id.partition("::")
    path = root / file
    if not path.is_file() or not name:
        return False
    wanted = name.split("[", 1)[0].split("::")[-1]
    tree = ast.parse(path.read_text())
    return any(
        isinstance(node, ast.FunctionDef | ast.AsyncFunctionDef) and node.name == wanted
        for node in ast.walk(tree)
    )


def problems(table: Iterable[Mutant], root: Path) -> list[str]:
    """Everything wrong with the table itself: stale snippets, missing tests, duplicates."""
    found: list[str] = []
    names: set[str] = set()
    for m in table:
        if m.name in names:
            found.append(f"{m.name}: duplicate mutant name")
        names.add(m.name)
        path = root / m.path
        if not path.is_file():
            found.append(f"{m.name}: {m.path} does not exist")
            continue
        count = path.read_text().count(m.old)
        if count != 1:
            found.append(f"{m.name}: snippet found {count} times in {m.path}")
        if m.old == m.new:
            found.append(f"{m.name}: the mutation changes nothing")
        if not m.tests:
            found.append(f"{m.name}: no test is named for it")
        found.extend(
            f"{m.name}: test {t} does not exist" for t in m.tests if not test_exists(root, t)
        )
    return found


def tracked(root: Path) -> list[Path]:
    """Files of the copied directories that git tracks or would track (new files included)."""
    out = subprocess.run(  # noqa: S603 - fixed argv, no shell
        ["git", "ls-files", "--cached", "--others", "--exclude-standard", "--", *COPIED],  # noqa: S607
        cwd=root,
        capture_output=True,
        text=True,
        check=True,
    )
    return [root / line for line in out.stdout.splitlines() if (root / line).is_file()]


def make_copy(root: Path, parent: Path) -> Path:
    copy = parent / "hypertrain"
    for file in tracked(root):
        target = copy / file.relative_to(root)
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(file, target)
    return copy


def run_tests(copy: Path, targets: Sequence[str], scratch: Path) -> tuple[int, str]:
    env = {
        **os.environ,
        "PYTHONDONTWRITEBYTECODE": "1",
        "PYTHONPATH": str(copy / "src"),
        "PET_LOG_DIR": str(scratch / "pet"),
    }
    cmd = [
        sys.executable,
        "-B",
        "-m",
        "pytest",
        "-q",
        "-x",
        "-p",
        "no:cacheprovider",
        f"--basetemp={scratch / 'bt'}",
        *targets,
    ]
    try:
        run = subprocess.run(  # noqa: S603 - this interpreter and test ids from the table
            cmd,
            cwd=copy,
            env=env,
            capture_output=True,
            text=True,
            timeout=TIMEOUT_SECONDS,
            check=False,
        )
    except subprocess.TimeoutExpired:
        return -1, "timeout"
    shutil.rmtree(scratch / "bt", ignore_errors=True)
    tail = run.stdout.strip().splitlines()[-1:] or [""]
    return run.returncode, tail[0]


def verdict(code: int, tail: str) -> str:
    """pytest exit 1 (a test failed) is the only kill; a pass is a survivor, and a run in which
    nothing passed (all skipped, nothing collected) proves nothing."""
    if code == 1:
        return "killed"
    if code == 0:
        return f"SURVIVED ({tail})" if "passed" in tail else f"NOT RUN ({tail})"
    return f"ERROR (pytest exit {code}: {tail})"


def check(copy: Path, m: Mutant, scratch: Path) -> tuple[str, float]:
    path = copy / m.path
    original = path.read_text()
    started = time.monotonic()
    try:
        path.write_text(original.replace(m.old, m.new))
        code, tail = run_tests(copy, m.tests, scratch)
    finally:
        path.write_text(original)
    return verdict(code, tail), time.monotonic() - started


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--list", action="store_true", help="validate the table, run nothing")
    parser.add_argument("--only", nargs="+", metavar="NAME", help="run only these mutants")
    args = parser.parse_args(argv)
    table = mutants()
    if args.only:
        unknown = sorted(set(args.only) - {m.name for m in table})
        if unknown:
            print(f"unknown mutants: {', '.join(unknown)}")
            return 2
        table = tuple(m for m in table if m.name in set(args.only))
    bad = problems(table, ROOT)
    for line in bad:
        print(line)
    if bad or args.list:
        print(f"{len(table)} mutants, {len(bad)} problems")
        return 1 if bad else 0
    with tempfile.TemporaryDirectory(prefix="audit-mutants-") as parent:
        scratch = Path(parent) / "run"
        scratch.mkdir()
        copy = make_copy(ROOT, Path(parent))
        targets = sorted({t for m in table for t in m.tests})
        code, tail = run_tests(copy, targets, scratch)
        if code != 0:
            print(f"the named tests fail without a mutant (exit {code}): {tail}")
            return 1
        rows = [(m, *check(copy, m, scratch)) for m in table]
    print("| mutant | file | caught by | result | seconds |\n|---|---|---|---|---|")
    for m, result, seconds in rows:
        tests = ", ".join(f"`{t.split('::', 1)[1]}`" for t in m.tests)
        print(f"| {m.name} | {Path(m.path).name} | {tests} | {result} | {seconds:.0f} |")
    survivors = [m.name for m, result, _ in rows if result != "killed"]
    print(f"\n{len(rows) - len(survivors)}/{len(rows)} mutants killed")
    return 1 if survivors else 0


if __name__ == "__main__":
    raise SystemExit(main())
