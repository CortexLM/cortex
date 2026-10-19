#!/usr/bin/env bash
# Bitwise proof gate: golden replay, codec v2, tests/parity, trace parity and island replay v2,
# the suites scripts/check-fast.sh leaves out. One pytest run per suite so each prints its own
# wall time; all suites share one budget (default 600 s). `od` tests are excluded as in PR CI
# (they need the opendecision extra). Exits 1 when a suite fails or the budget is exceeded.
# Extra arguments go to every pytest run.
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.local/bin:$PATH"

budget="${CHECK_PROOF_BUDGET_SECONDS:-600}"
suites=(
  tests/trainer/test_golden_replay.py
  tests/trainer/test_codec_v2.py
  tests/parity/test_parity.py
  tests/auditor/test_trace_parity.py
  tests/auditor/test_island_replay_v2.py
)
start=$(date +%s%N)
failed=()
for suite in "${suites[@]}"; do
  left_ms=$(( budget * 1000 - ($(date +%s%N) - start) / 1000000 ))
  if [ "$left_ms" -le 0 ]; then
    echo "check-proof: FAIL (budget exceeded before ${suite})" >&2
    echo "check-proof: FAIL"
    exit 1
  fi
  s=$(date +%s%N)
  rc=0
  timeout -k 5 "$(( (left_ms + 999) / 1000 ))" uv run --frozen python -m pytest -q \
    -p no:cacheprovider -m "not od" --durations=5 "$suite" "$@" || rc=$?
  printf 'check-proof: suite=%s exit=%s elapsed=%sms\n' \
    "$suite" "$rc" "$(( ($(date +%s%N) - s) / 1000000 ))"
  [ "$rc" -eq 0 ] || failed+=("$suite")
done
elapsed_ms=$(( ($(date +%s%N) - start) / 1000000 ))
printf 'check-proof: elapsed=%sms budget=%ss\n' "$elapsed_ms" "$budget"

if [ "${#failed[@]}" -gt 0 ]; then
  echo "check-proof: failed suites: ${failed[*]}" >&2
  echo "check-proof: FAIL"
  exit 1
fi
if [ "$elapsed_ms" -gt $(( budget * 1000 )) ]; then
  echo "check-proof: FAIL (${elapsed_ms}ms > ${budget}s)" >&2
  echo "check-proof: FAIL"
  exit 1
fi
echo "check-proof: PASS"
