#!/usr/bin/env python3
"""Patch filtered-copy Dockerfiles so Harbor verifiers have pytest on PATH.

Retained n15 x0017: biped and cad scored reward 0.0 because verifier stdout
was ``pytest: command not found``. Harbor execs pytest inside the task
environment / verifier container, not the guest host. A guest-image pytest
does not fix that hole.

Rewrites only the destination tree (never the pack). Last-stage ``FROM scratch``
/ distroless images are skipped (pip cannot run there). Missing pytest on a
Python-capable image fails the image build rather than scoring 0.
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
PYTHONISH = re.compile(
    r"(?i)\b(python|pip|debian|ubuntu|alpine|fedora|almalinux|rocky|"
    r"bookworm|bullseye|jammy|noble)\b"
)
SKIP_BASE = re.compile(r"(?i)^(scratch|gcr\.io/distroless/|distroless)")
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
    return bool(INSTALL_PYTEST.search(text))


def looks_python_capable(text: str) -> bool:
    return bool(PYTHONISH.search(text))


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


def patch_dockerfile(path: Path) -> str:
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
    if not looks_python_capable(text):
        return "skip-nonpython"
    path.write_text(text.rstrip() + "\n" + PYTEST_LAYER, encoding="utf-8")
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
    for path in iter_dockerfiles(root):
        result = patch_dockerfile(path)
        if result == "patched":
            stats["dockerfiles_patched"] += 1
        elif result == "already":
            stats["dockerfiles_already"] += 1
        else:
            stats["dockerfiles_skipped"] += 1
    for task_dir in sorted(p for p in root.iterdir() if p.is_dir()):
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
