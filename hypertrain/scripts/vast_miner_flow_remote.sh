#!/usr/bin/env bash
# Runs on the Vast host; vast_e2e.py rescues /workspace/out before deleting the instance.
set -euo pipefail
out=/workspace/out
mkdir -p "$out"
cd /workspace/ht
command -v uv >/dev/null || curl -LsSf https://astral.sh/uv/0.12.17/install.sh | sh
export PATH="$HOME/.local/bin:$PATH"
nvidia-smi --query-gpu=name,driver_version --format=csv,noheader >"$out/gpu.txt"
uv sync --locked --all-extras --group dev 2>&1 | tail -5 >"$out/sync.log"
uv run --frozen hypertrain-miner --help >"$out/cli-help.txt"
MINER_FLOW_EVIDENCE="$out/miner-flow.json" uv run --frozen python scripts/miner_flow_e2e.py \
  2>&1 | tee "$out/miner-flow.log"
