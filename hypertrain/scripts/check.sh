#!/usr/bin/env bash
# Quality gate: format, lint, types, tests. Exits nonzero on first failure.
set -euo pipefail
cd "$(dirname "$0")/.."
uv run --frozen ruff format --check
uv run --frozen ruff check
MYPYPATH=src:scripts:. uv run --frozen mypy --explicit-package-bases \
  --package hypertrain --module experiments.gpu_network_v2.orchestrate \
  --module scripts.network_service_proof --module scripts.network_gpu_operation
# Extra args go to pytest (CI splits the slow sim/e2e tests into their own job).
uv run --frozen pytest -q --durations=25 "$@"
