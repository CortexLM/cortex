#!/usr/bin/env bash
# Remote half of scripts/vast_e2e.py --profile miner-cycle: one CPU-safe miner cycle, no paid or
# external service. vast_e2e.py rescues /workspace/out before deleting the instance.
set -euo pipefail
out=${HT_E2E_OUT:-/workspace/out}
mkdir -p "$out"
cd "${HT_E2E_SRC:-/workspace/ht}"
command -v uv >/dev/null || curl -LsSf https://astral.sh/uv/0.12.17/install.sh | sh
export PATH="$HOME/.local/bin:$PATH"
uv sync --locked --all-extras --group dev >"$out/sync.log" 2>&1
if [ -f scripts/mine-smoke.sh ]; then
  MINE_SMOKE_EVIDENCE="$out/mine-smoke.json" bash scripts/mine-smoke.sh 2>&1 \
    | tee "$out/mine-smoke.log"
else
  MINER_FLOW_EVIDENCE="$out/miner-flow.json" uv run --frozen python scripts/miner_flow_e2e.py \
    2>&1 | tee "$out/miner-flow.log"
fi
