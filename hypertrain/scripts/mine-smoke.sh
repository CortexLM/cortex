#!/usr/bin/env bash
# CPU mining smoke: challenge service + one miner + one auditor, one complete test-mode round.
# PASS = the round reaches FINAL with a MATCH verdict inside the budget (default 120 s).
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.local/bin:$PATH"

budget="${MINE_SMOKE_BUDGET_SECONDS:-120}"
evidence="${MINE_SMOKE_EVIDENCE:-$(mktemp -d)/mine-smoke-evidence.json}"
export MINE_SMOKE_EVIDENCE="$evidence"

start=$(date +%s%N)
rc=0
timeout -k 5 "$budget" uv run --frozen python scripts/mine_smoke.py || rc=$?
end=$(date +%s%N)
elapsed_ms=$(( (end - start) / 1000000 ))

printf 'mine-smoke: exit=%s elapsed=%sms budget=%ss evidence=%s\n' \
  "$rc" "$elapsed_ms" "$budget" "$evidence"

if [ "$rc" -eq 124 ]; then
  echo "mine-smoke: FAIL (budget exceeded)" >&2
  exit 1
fi
[ "$rc" -eq 0 ] || exit "$rc"

budget_ms=$(( budget * 1000 ))
if [ "$elapsed_ms" -gt "$budget_ms" ]; then
  echo "mine-smoke: FAIL (${elapsed_ms}ms > ${budget_ms}ms)" >&2
  exit 1
fi
echo "mine-smoke: PASS"
