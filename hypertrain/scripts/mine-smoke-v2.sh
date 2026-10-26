#!/usr/bin/env bash
# CPU network-v2 mining smoke: challenge service + TLS relay on 127.0.0.1, one miner driving
# join-v2 -> probe-v2 -> proof-v2 -> run-v2 through the real CLI (see scripts/mine_smoke_v2.py).
# PASS = every stage exits 0 and run-v2 uploads round 0 inside the budget
# (default 300 s: measured ~132 s on CPU, plus margin for a loaded host).
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.local/bin:$PATH"

budget="${MINE_SMOKE_V2_BUDGET_SECONDS:-300}"
evidence="${MINE_SMOKE_V2_EVIDENCE:-$(mktemp -d)/mine-smoke-v2-evidence.json}"
export MINE_SMOKE_V2_EVIDENCE="$evidence"

start=$(date +%s%N)
rc=0
timeout -k 5 "$budget" uv run --frozen python scripts/mine_smoke_v2.py || rc=$?
end=$(date +%s%N)
elapsed_ms=$(( (end - start) / 1000000 ))

printf 'mine-smoke-v2: exit=%s elapsed=%sms budget=%ss evidence=%s\n' \
  "$rc" "$elapsed_ms" "$budget" "$evidence"

if [ "$rc" -eq 124 ]; then
  echo "mine-smoke-v2: FAIL (budget exceeded)" >&2
  exit 1
fi
[ "$rc" -eq 0 ] || exit "$rc"

budget_ms=$(( budget * 1000 ))
if [ "$elapsed_ms" -gt "$budget_ms" ]; then
  echo "mine-smoke-v2: FAIL (${elapsed_ms}ms > ${budget_ms}ms)" >&2
  exit 1
fi
echo "mine-smoke-v2: PASS"
