#!/usr/bin/env python3
"""Patch filtered-copy Dockerfiles so Harbor verifiers have pytest on PATH.

A retained metal run scored two tasks reward 0.0 because verifier stdout was
``pytest: command not found``. Harbor execs pytest inside the task
environment / verifier container, not the guest host. A guest-image pytest
does not fix that hole. The topic may sign ``ensure_verifier_pytest=false``
to leave its pack's images untouched.

Environment, verifier, and tests Dockerfiles are patched even when the FROM
line does not look like Python (CUDA / MuJoCo / FreeCAD images). Last-stage
``FROM scratch`` / distroless images are skipped. Missing pytest on a
patchable image fails the image build rather than scoring 0.

Rewrites only the destination tree (never the pack). After the install
layer's ``USER root``, the previous final-stage ``USER`` is restored so
patched verifier images do not run as root.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

MAX_FILE_BYTES = 1024 * 1024
INSTALL_PYTEST = re.compile(
    r"(?is)(?:pip\d*\s+install[^\n]*\bpytest\b|"
    r"python3?\s+-m\s+pip\s+install[^\n]*\bpytest\b|"
    r"apt-get\s+install[^\n]*\bpython3-pytest\b|"
    r"apk\s+add[^\n]*\bpy3-pytest\b)"
)
LAST_FROM = re.compile(r"(?im)^FROM\s+(\S+)")
FROM_INSTR = re.compile(r"(?i)^FROM\b")
USER_INSTR = re.compile(r"(?i)^USER\s+(\S+)")
PYTHONISH = re.compile(
    r"(?i)\b(python|pip|debian|ubuntu|alpine|fedora|almalinux|rocky|"
    r"bookworm|bullseye|jammy|noble)\b"
)
SKIP_BASE = re.compile(r"(?i)^(scratch|gcr\.io/distroless/|distroless)")
ENV_DIR_NAMES = frozenset({"environment", "verifier", "tests"})
PYTEST_MARKERS = ("pytest.ini", "conftest.py", "pyproject.toml")
REQ_REL = (
    "environment/requirements.txt",
    "verifier/requirements.txt",
    "tests/requirements.txt",
    "requirements.txt",
)

PYTEST_LAYER = """
# proof adaptor: Harbor verifier execs pytest in this image (must be on PATH)
USER root
RUN set -e; \\
  if command -v pytest >/dev/null 2>&1; then exit 0; fi; \\
  if command -v python3 >/dev/null 2>&1 && python3 -m pip install --no-cache-dir pytest; then exit 0; fi; \\
  if command -v pip3 >/dev/null 2>&1 && pip3 install --no-cache-dir pytest; then exit 0; fi; \\
  if command -v pip >/dev/null 2>&1 && pip install --no-cache-dir pytest; then exit 0; fi; \\
  if command -v apt-get >/dev/null 2>&1; then \\
    apt-get update; \\
    DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends python3 python3-pip python3-pytest || true; \\
    python3 -m pip install --no-cache-dir pytest; \\
    exit 0; \\
  fi; \\
  if command -v apk >/dev/null 2>&1; then \\
    apk add --no-cache python3 py3-pip py3-pytest || true; \\
    python3 -m pip install --no-cache-dir pytest; \\
    exit 0; \\
  fi; \\
  echo "proof adaptor: cannot install pytest in this image" >&2; \\
  exit 1
"""


def _fail(msg: str, code: int = 2) -> None:
    print(msg, file=sys.stderr)
    raise SystemExit(code)


def last_from_base(text: str) -> str:
    found = LAST_FROM.findall(text)
    if not found:
        return ""
    return found[-1].split(":")[0]


def should_skip_image(text: str) -> bool:
    base = last_from_base(text)
    if not base:
        return False
    return bool(SKIP_BASE.search(base))


def already_installs_pytest(text: str) -> bool:
    """True only if the **final** stage already installs pytest.

    A builder-stage ``pip install pytest`` does not put pytest on PATH in
    the runtime image Harbor execs. Checking the whole file would skip
    the injection layer and score a false zero.
    """
    return bool(INSTALL_PYTEST.search(final_stage_text(text)))


def looks_python_capable(text: str) -> bool:
    return bool(PYTHONISH.search(text))


def final_stage_text(text: str) -> str:
    """Return the last Dockerfile stage (last ``FROM`` through EOF)."""
    lines = text.splitlines()
    start = 0
    for i, raw in enumerate(lines):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        if FROM_INSTR.match(line):
            start = i
    return "\n".join(lines[start:])


def last_user_of_final_stage(text: str) -> str | None:
    """Return the last ``USER`` token after the last ``FROM`` (final stage).

    ``USER app:group`` is preserved as ``app:group``. Comments and earlier
    stages are ignored. No USER in the final stage → ``None`` (image default).
    """
    user: str | None = None
    for raw in text.splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        if FROM_INSTR.match(line):
            user = None
            continue
        match = USER_INSTR.match(line)
        if match:
            user = match.group(1)
    return user


def render_pytest_layer(restore_user: str | None) -> str:
    layer = PYTEST_LAYER.rstrip("\n")
    if restore_user:
        layer = f"{layer}\nUSER {restore_user}"
    return layer + "\n"


def is_dockerfile(path: Path) -> bool:
    name = path.name.lower()
    return name == "dockerfile" or name.startswith("dockerfile.")


def iter_dockerfiles(root: Path) -> list[Path]:
    found: list[Path] = []
    for path in root.rglob("*"):
        if not path.is_file():
            continue
        if ".." in path.parts:
            continue
        if is_dockerfile(path):
            found.append(path)
    return sorted(found)


def is_env_dockerfile(path: Path, task_dir: Path) -> bool:
    try:
        rel = path.relative_to(task_dir)
    except ValueError:
        return False
    parts = {p.lower() for p in rel.parts[:-1]}
    if parts & ENV_DIR_NAMES:
        return True
    return path.name.lower() == "dockerfile" and len(rel.parts) == 1


def task_has_pytest_tests(task_dir: Path) -> bool:
    for folder in ("tests", "verifier"):
        d = task_dir / folder
        if not d.is_dir():
            continue
        for marker in PYTEST_MARKERS:
            if (d / marker).is_file():
                return True
        try:
            for py in d.rglob("*.py"):
                name = py.name.lower()
                if name.startswith("test_") or name.endswith("_test.py"):
                    return True
        except OSError:
            continue
    return False


def patch_dockerfile(path: Path, *, force: bool = False) -> str:
    try:
        if path.stat().st_size > MAX_FILE_BYTES:
            return "too-large"
        text = path.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return "unreadable"
    if already_installs_pytest(text):
        return "already"
    if should_skip_image(text):
        return "skip-base"
    if not force and not looks_python_capable(text):
        return "skip-nonpython"
    restore_user = last_user_of_final_stage(text)
    path.write_text(
        text.rstrip() + "\n" + render_pytest_layer(restore_user),
        encoding="utf-8",
    )
    return "patched"


def patch_requirements(path: Path) -> str:
    try:
        if path.stat().st_size > MAX_FILE_BYTES:
            return "too-large"
        text = path.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return "unreadable"
    lines = [ln.strip() for ln in text.splitlines() if ln.strip() and not ln.strip().startswith("#")]
    for line in lines:
        pkg = re.split(r"[=<>!~\[]", line, maxsplit=1)[0].strip().lower()
        if pkg == "pytest":
            return "already"
    addition = "pytest\n" if text.endswith("\n") or not text else "\npytest\n"
    path.write_text(text + addition, encoding="utf-8")
    return "patched"


def iter_task_dirs(root: Path) -> list[Path]:
    children = sorted(
        p for p in root.iterdir() if p.is_dir() and not p.name.startswith(".")
    )
    return children if children else [root]


def ensure_tree(root: Path) -> dict[str, int]:
    if not root.is_dir():
        _fail(f"not a directory: {root}")
    stats = {
        "dockerfiles_patched": 0,
        "dockerfiles_already": 0,
        "dockerfiles_skipped": 0,
        "requirements_patched": 0,
        "requirements_already": 0,
    }
    for task_dir in iter_task_dirs(root):
        has_tests = task_has_pytest_tests(task_dir)
        for path in iter_dockerfiles(task_dir):
            force = is_env_dockerfile(path, task_dir) or has_tests
            result = patch_dockerfile(path, force=force)
            if result == "patched":
                stats["dockerfiles_patched"] += 1
            elif result == "already":
                stats["dockerfiles_already"] += 1
            else:
                stats["dockerfiles_skipped"] += 1
        for rel in REQ_REL:
            path = task_dir / rel
            if not path.is_file():
                continue
            result = patch_requirements(path)
            if result == "patched":
                stats["requirements_patched"] += 1
            elif result == "already":
                stats["requirements_already"] += 1
    return stats


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tasks-dir", required=True)
    args = parser.parse_args(argv)
    stats = ensure_tree(Path(args.tasks_dir))
    print(
        "ensure_verifier: dockerfiles patched={dockerfiles_patched} "
        "already={dockerfiles_already} skipped={dockerfiles_skipped}; "
        "requirements patched={requirements_patched} already={requirements_already}".format(
            **stats
        ),
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
