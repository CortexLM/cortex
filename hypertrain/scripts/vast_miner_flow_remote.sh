#!/usr/bin/env bash
# Runs on the Vast host (vastai/pytorch image): seedless install + key + synchronized update flow.
# Evidence lands in /workspace/out, which vast_e2e.py rescues before it deletes the instance.
set -euo pipefail
out=/workspace/out
mkdir -p "$out"
cd /workspace/ht
command -v uv >/dev/null || curl -LsSf https://astral.sh/uv/0.12.17/install.sh | sh
export PATH="$HOME/.local/bin:$PATH"
nvidia-smi --query-gpu=name,driver_version --format=csv,noheader >"$out/gpu.txt"
MINER_FLOW_EVIDENCE="$out/miner-flow.json" bash scripts/miner_flow_e2e.sh 2>&1 | tee "$out/miner-flow.log"
# The host must never have seen a coldkey: the e2e script asserts it; record the scan too.
grep -rl -- "coldkey" /root/.hypertrain 2>/dev/null >"$out/coldkey-scan.txt" || true
