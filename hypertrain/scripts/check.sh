#!/usr/bin/env bash
# Quality gate: format, lint, types, tests. Exits nonzero on first failure.
set -euo pipefail
cd "$(dirname "$0")/.."
uv run --frozen ruff format --check
uv run --frozen ruff check
uv run --frozen mypy src
uv run --frozen pytest -q
