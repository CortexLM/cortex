"""Mutation check of the ledger invariants (D39): each mutant must fail the tests that name it.

A table `tests/ledger_invariants/mutants_<package>.py` exposes `MUTANTS`, a list of dicts:

    id          unique string, selected by --only
    invariants  Lens ids of the invariants the change breaks
    path        file to patch, relative to hypertrain/, under src/hypertrain/
    old, new    the snippet to replace (`old` occurs exactly once in `path`) and its replacement
    tests       pytest node ids, relative to hypertrain/, that must FAIL under the mutant
    why         one sentence: how the change breaks the invariant

Unlike scripts/check_elastic_mutants.py, the working tree is never edited. src/ is copied into a
private directory outside the worktree, the copy is patched, and its src/ goes first on
PYTHONPATH (the editable install only appends the shared src/ through a .pth file). A probe
proves that the import resolves to the copy before any test runs. pytest runs from hypertrain/,
so conftest and configuration are the real ones. Only exit status 1 with a failed test is a kill;
a collection or usage error, a timeout and a setup error are reported as ERROR. Run from
hypertrain/:

    uv run --frozen python scripts/check_ledger_invariant_mutants.py [--list] [--only ID ...]
        [--jobs N] [--json PATH]

Exit status: 0 every mutant killed, 1 a mutant survived or errored or the unmutated copy fails,
2 a malformed table, a refused work directory or a probe that disagrees.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time
from collections.abc import Callable, Sequence
from concurrent.futures import ThreadPoolExecutor
from dataclasses import asdict, dataclass
from pathlib import Path
from queue import SimpleQueue
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
TABLES = "tests/ledger_invariants"
KEYS = frozenset({"id", "invariants", "path", "old", "new", "tests", "why"})


class RunnerError(Exception):
    """A condition that makes every verdict meaningless: the run stops."""


@dataclass(frozen=True, slots=True)
class Project:
    root: Path = ROOT
    package: str = "hypertrain"


@dataclass(frozen=True, slots=True)
class Mutant:
    id: str
    invariants: tuple[str, ...]
    path: str
    old: str
    new: str
    tests: tuple[str, ...]
    why: str
    table: str


@dataclass(slots=True)
class Outcome:
    id: str
    status: str  # KILLED, SURVIVED or ERROR
    exit_code: int | None
    failed: list[str]
    seconds: float
    detail: str


@dataclass(slots=True)
class Report:
    baseline_exit: int | None
    baseline_seconds: float
    baseline_tail: str
    outcomes: list[Outcome]


def field_problems(item: dict[str, Any], project: Project) -> list[str]:
    problems = []
    if not (
        isinstance(item["id"], str) and re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]*", item["id"])
    ):
        problems.append("id must be a non-empty word of letters, digits, dot, dash or underscore")
    invariants = item["invariants"]
    if not (
        isinstance(invariants, list)
        and invariants
        and all(isinstance(i, str) and i for i in invariants)
    ):
        problems.append("invariants must be a non-empty list of non-empty strings")
    path = item["path"]
    prefix = f"src/{project.package}/"
    if not (
        isinstance(path, str)
        and path.startswith(prefix)
        and path.endswith(".py")
        and ".." not in Path(path).parts
    ):
        problems.append(f"path must be a .py file under {prefix}")
    old, new = item["old"], item["new"]
    if not (isinstance(old, str) and old and isinstance(new, str)):
        problems.append("old must be a non-empty string and new a string")
    elif old == new:
        problems.append("old and new are the same")
    tests = item["tests"]
    if not (
        isinstance(tests, list)
        and tests
        and len(set(map(str, tests))) == len(tests)
        and all(isinstance(t, str) and "::" in t for t in tests)
    ):
        problems.append("tests must be a non-empty list of distinct node ids (file::name)")
    why = item["why"]
    if not (isinstance(why, str) and why.strip() and "\n" not in why):
        problems.append("why must be one non-empty line")
    return problems


def parse_table(raw: object, table: str, project: Project) -> tuple[list[Mutant], list[str]]:
    if not isinstance(raw, list):
        return [], [f"{table}: MUTANTS must be a list of dicts"]
    mutants: list[Mutant] = []
    problems: list[str] = []
    for index, item in enumerate(raw):
        if not isinstance(item, dict):
            problems.append(f"{table}[{index}]: not a dict")
            continue
        label = f"{table}: {item.get('id', index)}"
        if set(item) != KEYS:
            missing, extra = sorted(KEYS - set(item)), sorted(set(item) - KEYS)
            problems.append(f"{label}: keys differ (missing {missing}, unexpected {extra})")
            continue
        found = field_problems(item, project)
        problems.extend(f"{label}: {problem}" for problem in found)
        if not found:
            mutants.append(
                Mutant(
                    item["id"],
                    tuple(item["invariants"]),
                    item["path"],
                    item["old"],
                    item["new"],
                    tuple(item["tests"]),
                    item["why"],
                    table,
                )
            )
    return mutants, problems


def load_tables(project: Project) -> tuple[list[Mutant], list[str]]:
    mutants: list[Mutant] = []
    problems: list[str] = []
    for table in sorted((project.root / TABLES).glob("mutants_*.py")):
        spec = importlib.util.spec_from_file_location(f"li_mutants_{table.stem}", table)
        assert spec is not None and spec.loader is not None
        module = importlib.util.module_from_spec(spec)
        try:
            spec.loader.exec_module(module)
        except Exception as error:
            problems.append(f"{table.name}: cannot be loaded: {error!r}")
            continue
        found, bad = parse_table(getattr(module, "MUTANTS", None), table.name, project)
        mutants.extend(found)
        problems.extend(bad)
    return mutants, problems


def check_project(project: Project, mutants: Sequence[Mutant]) -> list[str]:
    """Problems of well-formed mutants against the current tree."""
    problems: list[str] = []
    seen: set[str] = set()
    for m in mutants:
        label = f"{m.table}: {m.id}"
        if m.id in seen:
            problems.append(f"{label}: duplicate id")
        seen.add(m.id)
        target = project.root / m.path
        if not target.is_file():
            problems.append(f"{label}: {m.path} does not exist")
        else:
            text = target.read_text(encoding="utf-8")
            if text.count(m.old) != 1:
                problems.append(f"{label}: snippet found {text.count(m.old)} times in {m.path}")
            else:
                try:
                    compile(text.replace(m.old, m.new), m.path, "exec")
                except SyntaxError as error:
                    problems.append(f"{label}: the mutated source does not compile: {error}")
        problems.extend(
            f"{label}: {t.split('::')[0]} does not exist"
            for t in m.tests
            if not (project.root / t.split("::")[0]).is_file()
        )
    return problems


def worktree_of(root: Path) -> Path:
    for candidate in (root, *root.parents):
        if (candidate / ".git").exists():
            return candidate
    return root


def check_workdir(project: Project, parent: Path) -> None:
    top = worktree_of(project.root.resolve())
    if parent.resolve().is_relative_to(top):
        raise RunnerError(f"refusing to work inside the worktree {top}: {parent}")


def make_copy(project: Project, destination: Path) -> None:
    shutil.copytree(
        project.root / "src",
        destination / "src",
        ignore=shutil.ignore_patterns("__pycache__", "*.pyc"),
        symlinks=True,
    )
    # Tests find repository files relative to the package (hypertrain.__file__.parents[2]).
    for entry in project.root.iterdir():
        if entry.name != "src" and not entry.name.startswith("."):
            (destination / entry.name).symlink_to(entry)


def child_env(copy: Path) -> dict[str, str]:
    # TMPDIR is inherited on purpose: the store binds a unix socket under it (sun_path < 108).
    inherited = os.environ.get("PYTHONPATH")
    return {
        **os.environ,
        "PYTHONPATH": str(copy / "src") + (os.pathsep + inherited if inherited else ""),
        "PYTHONDONTWRITEBYTECODE": "1",
    }


_active: set[subprocess.Popen[str]] = set()
_active_lock = threading.Lock()


def kill(process: subprocess.Popen[str]) -> None:
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass


def kill_active() -> None:
    with _active_lock:
        for process in _active:
            kill(process)


def run(
    command: Sequence[str], *, cwd: Path, env: dict[str, str], timeout: float
) -> tuple[int | None, str]:
    """Exit code (None after a timeout) and the combined output; the process group dies with it."""
    process = subprocess.Popen(  # noqa: S603  (argv is this interpreter and fixed arguments)
        list(command),
        cwd=cwd,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        errors="replace",
        start_new_session=True,
    )
    with _active_lock:
        _active.add(process)
    try:
        try:
            output, _ = process.communicate(timeout=timeout)
        except subprocess.TimeoutExpired:
            kill(process)
            output, _ = process.communicate()
            return None, output
        return process.returncode, output
    finally:
        with _active_lock:
            _active.discard(process)


def probe(project: Project, copy: Path, code: str, expected: Path) -> None:
    returncode, output = run(
        [sys.executable, "-B", "-c", code], cwd=project.root, env=child_env(copy), timeout=120
    )
    reported = output.strip().splitlines()[-1] if output.strip() else ""
    if returncode != 0 or Path(reported).resolve() != expected.resolve():
        raise RunnerError(
            f"probe: the import resolves to {reported or output.strip()!r}, "
            f"expected the private copy {expected}"
        )


def probe_package(project: Project, copy: Path) -> None:
    package = project.package
    probe(
        project,
        copy,
        f"import {package}; print({package}.__file__)",
        copy / "src" / package / "__init__.py",
    )


def probe_module(project: Project, copy: Path, mutant: Mutant) -> None:
    parts = Path(mutant.path).relative_to("src").with_suffix("").parts
    name = ".".join(parts[:-1] if parts[-1] == "__init__" else parts)
    probe(
        project,
        copy,
        f"import importlib.util; print(importlib.util.find_spec({name!r}).origin)",
        copy / mutant.path,
    )


def pytest_command(tests: Sequence[str], copy: Path) -> list[str]:
    return [
        sys.executable,
        "-B",
        "-m",
        "pytest",
        "-x",
        "-q",
        "-p",
        "no:cacheprovider",
        "-rfE",
        f"--basetemp={copy / 'bt'}",
        *tests,
    ]


def failed_tests(output: str) -> list[str]:
    return [
        line.removeprefix("FAILED ").split(" - ")[0].strip()
        for line in output.splitlines()
        if line.startswith("FAILED ")
    ]


def tail(output: str, lines: int = 25) -> str:
    return "\n".join(output.splitlines()[-lines:])


def classify(code: int | None, output: str, timeout: float) -> tuple[str, list[str], str]:
    if code is None:
        return "ERROR", [], f"timeout after {timeout:g} s"
    if code == 0:
        return "SURVIVED", [], ""
    failed = failed_tests(output)
    if code == 1 and failed:
        return "KILLED", failed, ""
    reason = "pytest exit 1 without a failed test (setup or teardown error)"
    return "ERROR", [], f"{reason if code == 1 else f'pytest exit {code}'}\n{tail(output)}"


def evaluate(project: Project, copy: Path, mutant: Mutant, timeout: float) -> Outcome:
    started = time.monotonic()
    target = copy / mutant.path
    original = target.read_bytes()
    text = original.decode()
    if text.count(mutant.old) != 1:
        raise RunnerError(f"{mutant.id}: snippet found {text.count(mutant.old)} times in the copy")
    try:
        target.write_bytes(text.replace(mutant.old, mutant.new).encode())
        probe_module(project, copy, mutant)
        code, output = run(
            pytest_command(mutant.tests, copy),
            cwd=project.root,
            env=child_env(copy),
            timeout=timeout,
        )
    finally:
        target.write_bytes(original)
    status, failed, detail = classify(code, output, timeout)
    return Outcome(mutant.id, status, code, failed, round(time.monotonic() - started, 1), detail)


def describe(outcome: Outcome) -> str:
    line = f"{outcome.status} {outcome.id}"
    if outcome.failed:
        line += " " + " ".join(outcome.failed)
    if outcome.detail:
        line += " - " + outcome.detail.splitlines()[0]
    return line


def run_mutants(
    project: Project,
    mutants: Sequence[Mutant],
    *,
    parent: Path,
    jobs: int,
    timeout: float,
    keep: bool,
    echo: Callable[[str], None],
) -> Report:
    check_workdir(project, parent)
    parent.mkdir(parents=True, exist_ok=True)
    work = Path(tempfile.mkdtemp(prefix="li-mutants-", dir=parent))
    try:
        copies = [work / f"c{index}" for index in range(max(1, min(jobs, len(mutants))))]
        for copy in copies:
            make_copy(project, copy)
            probe_package(project, copy)
        tests = list(dict.fromkeys(t for m in mutants for t in m.tests))
        started = time.monotonic()
        code, output = run(
            pytest_command(tests, copies[0]),
            cwd=project.root,
            env=child_env(copies[0]),
            timeout=timeout,
        )
        seconds = round(time.monotonic() - started, 1)
        if code != 0:
            return Report(code, seconds, tail(output), [])
        slots: SimpleQueue[Path] = SimpleQueue()
        for copy in copies:
            slots.put(copy)
        echo_lock = threading.Lock()

        def task(mutant: Mutant) -> Outcome:
            copy = slots.get()
            try:
                outcome = evaluate(project, copy, mutant, timeout)
            finally:
                slots.put(copy)
            with echo_lock:
                echo(describe(outcome))
            return outcome

        pool = ThreadPoolExecutor(max_workers=len(copies))
        try:
            outcomes = list(pool.map(task, mutants))
        except BaseException:
            kill_active()
            pool.shutdown(wait=True, cancel_futures=True)
            raise
        pool.shutdown()
        return Report(0, seconds, "", outcomes)
    finally:
        if not keep:
            shutil.rmtree(work)


def summary(report: Report) -> dict[str, Any]:
    count = {
        s: sum(o.status == s for o in report.outcomes) for s in ("KILLED", "SURVIVED", "ERROR")
    }
    return {
        "baseline": {
            "exit_code": report.baseline_exit,
            "seconds": report.baseline_seconds,
            "tail": report.baseline_tail,
        },
        "killed": count["KILLED"],
        "survived": count["SURVIVED"],
        "error": count["ERROR"],
        "mutants": [asdict(o) for o in report.outcomes],
    }


def print_list(mutants: Sequence[Mutant], echo: Callable[[str], None]) -> None:
    for m in mutants:
        echo(m.id)
        echo("  invariants: " + ", ".join(m.invariants))
        echo("  path: " + m.path)
        for test in m.tests:
            echo("  test: " + test)
        echo("  why: " + m.why)
    echo(f"{len(mutants)} mutants")


def main(argv: Sequence[str] | None = None, project: Project | None = None) -> int:
    project = project or Project()
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0] if __doc__ else None)
    parser.add_argument("--list", action="store_true", help="print the mutants and stop")
    parser.add_argument("--only", nargs="+", metavar="ID", help="run only these mutants")
    parser.add_argument("--jobs", type=int, default=1, help="mutants in parallel, one copy each")
    parser.add_argument("--json", type=Path, metavar="PATH", help="write the report here")
    parser.add_argument("--timeout", type=float, default=900, help="seconds per pytest run")
    parser.add_argument(
        "--workdir", type=Path, help="parent of the private copy (default: the temp directory)"
    )
    parser.add_argument("--keep", action="store_true", help="keep the private copy")
    args = parser.parse_args(argv)

    mutants, problems = load_tables(project)
    problems += check_project(project, mutants)
    if problems:
        print("\n".join(problems), file=sys.stderr)
        return 2
    if args.only:
        unknown = sorted(set(args.only) - {m.id for m in mutants})
        if unknown:
            print(f"unknown mutant ids: {', '.join(unknown)}", file=sys.stderr)
            return 2
        mutants = [m for m in mutants if m.id in args.only]
    if args.list:
        print_list(mutants, print)
        return 0
    if not mutants:
        print("no mutant to run", file=sys.stderr)
        return 2

    try:
        report = run_mutants(
            project,
            mutants,
            parent=args.workdir or Path(tempfile.gettempdir()),
            jobs=args.jobs,
            timeout=args.timeout,
            keep=args.keep,
            echo=print,
        )
    except RunnerError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2
    if args.json:
        args.json.write_text(json.dumps(summary(report), indent=2) + "\n")
    if report.baseline_exit != 0:
        how = "timeout" if report.baseline_exit is None else f"pytest exit {report.baseline_exit}"
        print(f"the unmutated copy fails ({how}):")
        print(report.baseline_tail)
        return 1
    result = summary(report)
    print(f"{result['killed']}/{len(mutants)} mutants killed")
    return 0 if result["killed"] == len(mutants) else 1


if __name__ == "__main__":
    for number in (signal.SIGTERM, signal.SIGHUP):
        signal.signal(number, lambda signum, _frame: sys.exit(128 + signum))
    raise SystemExit(main())
