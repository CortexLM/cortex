"""Repository contracts, frozen specifications and version-controlled file hygiene.

Run --final only after the Rust migration cleanup. Local ignored build output is
excluded, while an accidentally tracked ignored file remains subject to checks.
"""

from __future__ import annotations

import argparse
import ast
import hashlib
import json
import re
import subprocess
import sys
import tomllib
from collections.abc import Iterable
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
FROZEN_SPECS = {
    "docs/BUNDLE_SPEC.md": "c7a43fca324a2f44e7c40bca5acde1bd5ea30cabde1e8c0a0011aeedbb4330a3",
    "docs/DESIGN_CHALLENGE.md": "c93051da4e08f16390dbbef33747aee8ccc7451a2cb4660acc105a8fff231617",
    "docs/PRISM.md": "bce5789ac64cbc75e62ac78daa8452b8ae3a5eaff0b0134afb6f9ff9dd372925",
}
SHARES = {"bounty": 2000, "proof": 8000}
PROPORTIONAL_SHARES = {"bounty": 3000, "proof": 7000}
DOC_CONTRACTS = {
    "docs/external-miner/README.md": ("bounty", "proof", "3000", "7000"),
    "docs/external-miner/bounty.md": (
        "/v1/pair",
        "/v1/reports",
        "terms_accepted",
        "severity",
        "503",
        "CortexLM/backend",
    ),
    "docs/external-miner/proof.md": (
        "/v1/proof/topics",
        "/v1/submissions",
        "/v1/submissions/lookup",
        "topic_id",
        "hotkey_signature",
        "submit_nonce",
        "base-proof-submit-v1",
        "artifact_digest",
        "eval_image_digest",
        "holdout",
        "baseline",
        "wta",
        "discovery",
        "503",
    ),
    "docs/external-miner/validators.md": (
        "/v1/weights/latest",
        "sealed",
        "burn_outcome",
        "3000",
        "7000",
    ),
}
PUBLIC_ROUTES = {
    "src/cortex/bounty/api.py": {
        ("post", "/v1/pair"),
        ("post", "/v1/reports"),
        ("get", "/v1/status"),
    },
    "src/cortex/proof/api.py": {
        ("get", "/v1/proof/topics"),
        ("post", "/v1/submissions"),
        ("post", "/v1/submissions/lookup"),
        ("get", "/v1/status"),
    },
    "src/cortex/gateway/api.py": {("get", "/v1/weights/latest")},
}
ARTIFACT_DIRS = {
    "__pycache__",
    ".pytest_cache",
    ".mypy_cache",
    ".ruff_cache",
    ".venv",
    "venv",
    "node_modules",
    "target",
    "dist",
    "build",
    "coverage",
    "test-results",
    ".terraform",
}
ARTIFACT_SUFFIXES = {
    ".pyc",
    ".pyo",
    ".o",
    ".so",
    ".whl",
    ".sqlite",
    ".sqlite3",
    ".db",
    ".bak",
    ".tmp",
    ".swp",
}
SECRET_SUFFIXES = {".pem", ".key", ".age", ".p12", ".pfx", ".sk", ".token", ".secret"}
SECRET_PATTERNS = (
    re.compile(rb"sk-or-v1-[A-Za-z0-9]{32,}"),
    re.compile(rb"sk-(?:proj-|ant-api\d+-)?[A-Za-z0-9_-]{40,}"),
    re.compile(rb"gh[pousr]_[A-Za-z0-9]{30,}"),
    re.compile(rb"github_pat_[A-Za-z0-9_]{40,}"),
    re.compile(rb"AKIA[A-Z0-9]{16}"),
    re.compile(rb"AGE-SECRET-KEY-[A-Z0-9]{40,}"),
    re.compile(rb"-----BEGIN (?:[A-Z0-9]+ )?PRIVATE KEY-----"),
)
REMOVED = re.compile(
    r"(?<![a-z0-9])(?:relearn(?:[-_](?:image|agent|mm))?|prism|design[-_]challenge)(?![a-z0-9])",
    re.I,
)
ACTIVE_DIRS = {"src", "config", "deploy", "bins", "crates"}
REMOVED_DESIGN_VALUE = re.compile(
    r"(?:^|[\s{,])(?:id|challenge_id|challenge|service|name)\s*[:=]\s*[\"']?design(?:[\"'\s,}]|$)"
    r"|^\s*[\"']?design[\"']?\s*:",
    re.I,
)
TEXT_SUFFIXES = {".py", ".toml", ".json", ".yml", ".yaml", ".sh", ".rs", ".example", ".txt"}


def repository_files(root: Path) -> list[Path]:
    result = subprocess.run(
        ["git", "-C", str(root), "ls-files", "-z", "--cached", "--others", "--exclude-standard"],
        check=True,
        capture_output=True,
    )
    return sorted({Path(name.decode("utf-8")) for name in result.stdout.split(b"\0") if name})


def check_specs(root: Path) -> list[str]:
    failures = []
    for name, expected in FROZEN_SPECS.items():
        path = root / name
        if not path.is_file() or path.is_symlink():
            failures.append(f"{name}: frozen specification missing or symlinked")
        elif hashlib.sha256(path.read_bytes()).hexdigest() != expected:
            failures.append(f"{name}: frozen specification SHA-256 changed")
    return failures


def check_trust_roots(root: Path) -> list[str]:
    failures = []
    paths = set((root / "config").glob("challenges*.toml"))
    paths.add(root / "config/challenges.toml")
    for path in sorted(paths):
        name = path.relative_to(root)
        try:
            if path.is_symlink():
                raise ValueError("symlinked trust root")
            document = tomllib.loads(path.read_text())
            rows = document["challenges"]
            if not isinstance(rows, list) or len(rows) != 2:
                raise ValueError("exactly two challenges required")
            actual = {}
            for row in rows:
                identifier, share = row["id"], row["emission_share_bps"]
                if identifier in actual or type(share) is not int:
                    raise ValueError("duplicate id or invalid share")
                if not re.fullmatch(r"[0-9a-f]{64}", row["public_key"]):
                    raise ValueError("invalid challenge public key")
                actual[identifier] = share
            version = document["version"]
            if type(version) is not int or version < 1:
                raise ValueError("invalid trust document version")
            if actual != SHARES and not (actual == PROPORTIONAL_SHARES and version >= 2):
                raise ValueError("unsupported shares or activation version")
        except (OSError, UnicodeError, ValueError, KeyError, TypeError):
            failures.append(
                f"{name}: expected bounty/proof=2000/8000 or version >=2 with 3000/7000"
            )
    return failures


def declared_routes(source: str) -> set[tuple[str, str]]:
    routes = set()
    for function in ast.walk(ast.parse(source)):
        if not isinstance(function, ast.FunctionDef | ast.AsyncFunctionDef):
            continue
        for node in function.decorator_list:
            if not isinstance(node, ast.Call) or not isinstance(node.func, ast.Attribute):
                continue
            if node.func.attr not in {"get", "post", "put", "patch", "delete", "head", "options"}:
                continue
            if (
                node.args
                and isinstance(node.args[0], ast.Constant)
                and isinstance(node.args[0].value, str)
            ):
                routes.add((node.func.attr, node.args[0].value))
    return routes


def check_public_contracts(root: Path) -> list[str]:
    failures = []
    for name, markers in DOC_CONTRACTS.items():
        path = root / name
        try:
            body = path.read_text() if not path.is_symlink() else ""
        except (OSError, UnicodeError):
            body = ""
        for marker in markers:
            if marker.casefold() not in body.casefold():
                failures.append(f"{name}: essential public contract missing: {marker}")
        if not re.search(r"<!--\s*protocol_version:\s*1\s*-->", body):
            failures.append(f"{name}: bundle protocol_version: 1 badge required")
    for name, required in PUBLIC_ROUTES.items():
        try:
            path = root / name
            if path.is_symlink():
                raise ValueError("symlinked API source")
            routes = declared_routes(path.read_text())
            for method, route in sorted(required - routes):
                failures.append(f"{name}: public API removed: {method.upper()} {route}")
            if "bounty" in name and any("/public/" in route for _, route in routes):
                failures.append(f"{name}: Bounty public feed must remain an external backend API")
        except (OSError, UnicodeError, ValueError, SyntaxError):
            failures.append(f"{name}: public API source missing or invalid")
    return failures


def _secret_path(path: Path) -> bool:
    name = path.name.lower()
    if name.endswith((".example", ".sample")):
        return False
    return (
        path.suffix.lower() in SECRET_SUFFIXES
        or name == ".env"
        or name.startswith(".env.")
        or name.endswith(".env")
        or name in {"id_rsa", "id_ed25519", "terraform.tfvars"}
        or ".tfstate" in name
        or (path.parts[:2] == ("deploy", "secrets") and name != "readme.md")
    )


def _artifact_path(path: Path) -> bool:
    fixture_log = "fixtures" in path.parts and "tests" in path.parts
    return (
        bool(set(path.parts) & ARTIFACT_DIRS)
        or any(part.endswith(".egg-info") for part in path.parts)
        or path.suffix.lower() in ARTIFACT_SUFFIXES
        or (path.suffix.lower() == ".log" and not fixture_log)
        or path.name in {".DS_Store", "Thumbs.db", ".coverage"}
    )


def check_hygiene(root: Path, files: Iterable[Path]) -> list[str]:
    failures = []
    for name in files:
        path = root / name
        if name.is_absolute() or ".." in name.parts:
            failures.append("repository inventory contains an unsafe path")
            continue
        if not path.exists() and not path.is_symlink():
            continue  # A tracked deletion is already absent from the proposed tree.
        if _secret_path(name):
            failures.append(f"{name}: secret or runtime credential file must not be versioned")
        if _artifact_path(name):
            failures.append(f"{name}: generated artifact or cache must not be versioned")
        if path.is_symlink():
            if not path.resolve().is_relative_to(root.resolve()):
                failures.append(f"{name}: symlink leaves the repository")
            continue
        if path.is_file():
            try:
                content = path.read_bytes()
            except OSError:
                failures.append(f"{name}: cannot inspect repository file")
                continue
            if any(pattern.search(content) for pattern in SECRET_PATTERNS):
                failures.append(f"{name}: possible embedded credential (value redacted)")
    return failures


def _strings(value: object) -> Iterable[str]:
    if isinstance(value, str):
        yield value
    elif isinstance(value, dict):
        for key, item in value.items():
            yield from _strings(key)
            yield from _strings(item)
    elif isinstance(value, list):
        for item in value:
            yield from _strings(item)


def check_final(root: Path, files: Iterable[Path]) -> list[str]:
    failures = []
    for name in files:
        path = root / name
        if not path.is_file() or path.is_symlink():
            continue
        if (
            name.suffix == ".rs"
            or name.name
            in {
                "Cargo.toml",
                "Cargo.lock",
                "rust-toolchain",
                "rust-toolchain.toml",
                "clippy.toml",
                "rustfmt.toml",
            }
            or ".cargo" in name.parts
        ):
            failures.append(f"{name}: Rust/Cargo file remains after Python migration")
        if name.parts[0] not in ACTIVE_DIRS:
            continue
        if name.suffix == ".md":
            continue  # Historical documentation is not a live product registration.
        if REMOVED.search(str(name)) or any(Path(part).stem == "design" for part in name.parts):
            failures.append(f"{name}: removed product path remains in active tree")
        if name.suffix not in TEXT_SUFFIXES and name.name != "Dockerfile":
            continue
        try:
            body = path.read_text()
            if name.suffix == ".toml":
                values = list(_strings(tomllib.loads(body)))
            elif name.suffix == ".json":
                values = list(_strings(json.loads(body)))
            elif name.suffix == ".py":
                values = [
                    node.value
                    for node in ast.walk(ast.parse(body))
                    if isinstance(node, ast.Constant) and isinstance(node.value, str)
                ]
            else:
                values = [line.split("#", 1)[0] for line in body.splitlines()]
            if any(
                REMOVED.search(value)
                or value.strip().lower() == "design"
                or REMOVED_DESIGN_VALUE.search(value)
                for value in values
            ):
                failures.append(
                    f"{name}: removed challenge referenced by active code/configuration"
                )
        except (OSError, UnicodeError, ValueError, SyntaxError):
            failures.append(f"{name}: cannot parse active source/configuration")
    return failures


def check_repository(root: Path, *, final: bool = False) -> list[str]:
    files = repository_files(root)
    failures = check_specs(root) + check_trust_roots(root) + check_public_contracts(root)
    failures += check_hygiene(root, files)
    if final:
        failures += check_final(root, files)
    return sorted(set(failures))


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=ROOT)
    parser.add_argument(
        "--final", action="store_true", help="require completed Python-only migration"
    )
    args = parser.parse_args(argv)
    try:
        failures = check_repository(args.root.resolve(), final=args.final)
    except (OSError, ValueError, subprocess.SubprocessError):
        print("Repository check could not inspect the Git worktree.", file=sys.stderr)
        return 1
    if failures:
        print("Repository contract failures:", file=sys.stderr)
        for failure in failures:
            print(f"  - {failure}", file=sys.stderr)
        return 1
    print("Repository contracts passed" + (" (final Python-only gate)." if args.final else "."))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
