#!/usr/bin/env bash
# Fast gate: the `fast`-marked CPU mining-path subset (see tests/conftest.py) in under 2 minutes.
# Paths are passed explicitly so pytest only collects those files (collecting the whole tree
# under -m costs ~45 s on a loaded host). Prints the measured wall time; exits 1 when tests
# fail or the budget is exceeded. Extra arguments go to pytest.
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.local/bin:$PATH"

budget="${CHECK_FAST_BUDGET_SECONDS:-120}"
mapfile -t paths < <(uv run --frozen python - <<'EOF'
import sys
sys.path.insert(0, "tests")
import conftest  # noqa: E402
print("\n".join([*conftest.FAST_DIRS, *conftest.FAST_FILES]))
EOF
)
start=$(date +%s%N)
rc=0
timeout -k 5 "$budget" uv run --frozen python -m pytest -q -p no:cacheprovider \
  -m "fast and not od" --durations=10 "${paths[@]}" "$@" || rc=$?
end=$(date +%s%N)
elapsed_ms=$(( (end - start) / 1000000 ))
printf 'check-fast: exit=%s elapsed=%sms budget=%ss\n' "$rc" "$elapsed_ms" "$budget"

if [ "$rc" -eq 124 ]; then
  echo "check-fast: FAIL (budget exceeded)" >&2
  exit 1
fi
[ "$rc" -eq 0 ] || exit "$rc"
if [ "$elapsed_ms" -gt $(( budget * 1000 )) ]; then
  echo "check-fast: FAIL (${elapsed_ms}ms > ${budget}s)" >&2
  exit 1
fi
echo "check-fast: PASS"
